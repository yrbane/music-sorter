use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::config::Config;
use crate::coverart::CoverArtClient;
use crate::discogs::DiscogsClient;
use crate::fingerprint;
use crate::models::TrackInfo;
use crate::musicbrainz::MusicBrainzClient;
use crate::rate_limiter::RateLimiter;
use crate::tags;

/// Écrase l'artiste avec le nom fourni par la DB (MusicBrainz/Discogs).
/// L'unification de casse est déléguée à `Enricher::finalize`, appelée sur tous
/// les chemins de sortie d'`enrich` (pas seulement les matchs API).
/// L'album n'est écrasé que si le fichier n'en a pas (évite de remplacer
/// l'album original par une compilation).
fn override_from_db(info: &mut TrackInfo, db_info: &TrackInfo) {
    if let Some(artist) = db_info.artist.as_ref() {
        info.artist = Some(artist.clone());
    }
    // Ne PAS écraser l'album si on en a déjà un dans les tags locaux
    info.merge(db_info);
}

/// Détermine la confiance d'un enrichissement.
/// - match API → High ; sinon tags embarqués complets → Medium ; sinon Low.
fn compute_confidence(had_embedded_org: bool, api_matched: bool) -> crate::models::Confidence {
    use crate::models::Confidence;
    if api_matched {
        Confidence::High
    } else if had_embedded_org {
        Confidence::Medium
    } else {
        Confidence::Low
    }
}

pub struct Enricher {
    musicbrainz: MusicBrainzClient,
    discogs: Option<DiscogsClient>,
    coverart: CoverArtClient,
    fpcalc_available: bool,
    acoustid_api_key: Option<String>,
    cache: Arc<crate::cache::Cache>,
    api_cache_ttl_secs: i64,
}

impl Enricher {
    /// Crée un nouvel Enricher à partir de la configuration fournie
    pub fn new(config: &Config, cache: Arc<crate::cache::Cache>) -> Result<Self> {
        let mb_limiter = Arc::new(RateLimiter::new(Duration::from_millis(1100)));
        let discogs_limiter = Arc::new(RateLimiter::new(Duration::from_millis(1000)));
        let coverart_limiter = Arc::new(RateLimiter::new(Duration::from_millis(1100)));

        let musicbrainz = MusicBrainzClient::new(mb_limiter)?;

        // Discogs est optionnel : uniquement si un token est configuré
        let discogs = config
            .discogs_token
            .as_ref()
            .map(|token| DiscogsClient::new(token.clone(), discogs_limiter))
            .transpose()?;

        let coverart = CoverArtClient::new(coverart_limiter)?;

        let fpcalc_available = fingerprint::is_fpcalc_available();
        if !fpcalc_available {
            eprintln!(
                "\x1b[33m⚠ fpcalc non trouvé — fingerprinting désactivé. Installer : sudo pacman -S chromaprint\x1b[0m"
            );
        }

        Ok(Self {
            musicbrainz,
            discogs,
            coverart,
            fpcalc_available,
            acoustid_api_key: config.acoustid_api_key.clone(),
            cache,
            api_cache_ttl_secs: config.api_cache_ttl_days.unwrap_or(30) as i64 * 86400,
        })
    }

    /// Enrichit les métadonnées d'un fichier audio.
    /// Retourne aussi le niveau de confiance (cf. Confidence) pour router les matchs faibles.
    pub fn enrich(&self, path: &Path) -> Result<(TrackInfo, crate::models::Confidence)> {
        // Étape 1 : lecture des tags existants
        let mut info = tags::read_tags(path).unwrap_or_default();

        // Mémorise si les tags EMBARQUÉS (avant heuristiques) suffisaient à organiser.
        let had_embedded_org = info.has_minimum_for_organization();

        // Étape 1bis : fallback nom de fichier si artist/title manquent
        // (utile pour les fichiers téléchargés sans tags ID3)
        if info.artist.is_none() || info.title.is_none() {
            let (fa, ft) = tags::parse_artist_title_from_filename(path);
            if info.artist.is_none() {
                info.artist = fa;
            }
            if info.title.is_none() {
                info.title = ft;
            }
        }

        // Étape 1ter : fallback dossier parent au format « Artist - Year - Album »
        // ou « Artist - Album » (rip d'album sans tags rangé proprement)
        if info.artist.is_none() || info.album.is_none() || info.year.is_none() {
            let (fa, fy, fal) = tags::parse_folder_metadata(path);
            if info.artist.is_none() {
                info.artist = fa;
            }
            if info.album.is_none() {
                info.album = fal;
            }
            if info.year.is_none() {
                info.year = fy;
            }
        }

        if info.has_full_metadata() {
            // Métadonnées complètes sans appel API : confiance basée sur les tags embarqués.
            return Ok(self.finalize(info, compute_confidence(had_embedded_org, false)));
        }

        let mut release_id: Option<String> = None;

        let existing_album = info.album.clone();

        // Étape 2 : si les tags sont insuffisants, tenter le fingerprinting AcoustID
        if !info.has_minimum_for_search() && self.fpcalc_available {
            if let Some(ref api_key) = self.acoustid_api_key {
                if let Ok(fp) = fingerprint::generate_or_cached(&self.cache, path) {
                    if let Ok(Some(recording_id)) = fingerprint::lookup_acoustid(api_key, &fp) {
                        if let Ok(Some((mb_info, rid))) =
                            self.musicbrainz.lookup_by_recording_id_with_cache(
                                &self.cache,
                                &recording_id,
                                existing_album.as_deref(),
                                self.api_cache_ttl_secs,
                            )
                        {
                            override_from_db(&mut info, &mb_info);
                            release_id = rid;
                        }
                    }
                }
            }
        }

        // Étape 3 : recherche MusicBrainz par texte si artiste+titre disponibles et pas encore de release_id
        if info.has_minimum_for_search() && release_id.is_none() {
            let artist = info.artist.as_deref().unwrap();
            let raw_title = info.title.as_deref().unwrap();
            // Nettoie le titre des suffixes parasites (Original Mix, OUT NOW, _soundcloud, ...)
            // pour augmenter le taux de match MusicBrainz, sans toucher au titre réel.
            let title_cleaned = crate::title_cleaner::clean_for_search(raw_title);

            if let Ok(Some((mb_info, rid))) = self.musicbrainz.search_by_text_with_cache(
                &self.cache,
                artist,
                &title_cleaned,
                existing_album.as_deref(),
                self.api_cache_ttl_secs,
            ) {
                override_from_db(&mut info, &mb_info);
                release_id = Some(rid);
            }
        }

        // Étape 4 : pochette via Cover Art Archive
        if info.cover_art.is_none() {
            if let Some(ref rid) = release_id {
                if let Ok(Some(cover)) = self.coverart.fetch_cover_cached(&self.cache, rid) {
                    info.cover_art = Some(cover);
                }
            }
        }

        // Étape 5 : fallback Discogs si les infos sont incomplètes ou la pochette manquante
        if let Some(ref discogs) = self.discogs {
            let needs_discogs = !info.has_minimum_for_organization() || info.cover_art.is_none();

            if needs_discogs {
                let artist = info.artist.as_deref().unwrap_or("");
                let album = info.album.as_deref().unwrap_or("");

                if !artist.is_empty() && !album.is_empty() {
                    if let Ok(Some((discogs_info, resource_url))) =
                        discogs.search_release_with_cache(&self.cache, artist, album, self.api_cache_ttl_secs)
                    {
                        info.merge(&discogs_info);

                        if let Some(ref url) = resource_url {
                            if let Ok(Some(details)) = discogs.get_release_details_with_cache(&self.cache, url, self.api_cache_ttl_secs) {
                                // Préférer le nom canonique Discogs pour artiste et album
                                let details_info = TrackInfo {
                                    artist: details.artist,
                                    album: details.album,
                                    year: details.year,
                                    genre: details.genre,
                                    ..Default::default()
                                };
                                override_from_db(&mut info, &details_info);

                                if info.cover_art.is_none() {
                                    if let Some(ref cover_url) = details.cover_url {
                                        if let Ok(cover_data) = discogs.fetch_image(cover_url) {
                                            info.cover_art = Some(cover_data);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let confidence = compute_confidence(had_embedded_org, release_id.is_some());
        Ok(self.finalize(info, confidence))
    }

    /// Point de sortie unique d'`enrich` : applique le registre de casse à
    /// l'artiste pour garantir un dossier unique par artiste, quelle que soit la
    /// source (tags complets, heuristiques ou match API). Idempotent.
    fn finalize(
        &self,
        mut info: TrackInfo,
        confidence: crate::models::Confidence,
    ) -> (TrackInfo, crate::models::Confidence) {
        if let Some(artist) = info.artist.as_ref() {
            let canonical = crate::artist_registry::canonicalize(&self.cache, artist)
                .unwrap_or_else(|_| artist.clone());
            info.artist = Some(canonical);
        }
        // Registre d'album : unifie la casse du nom d'album (première vue) et
        // retient l'année la plus ancienne connue (artiste déjà canonicalisé).
        if let (Some(artist), Some(album)) = (info.artist.as_ref(), info.album.as_ref()) {
            if let Ok((canon_album, canon_year)) =
                crate::album_group::canonicalize(&self.cache, artist, album, info.year)
            {
                info.album = Some(canon_album);
                info.year = canon_year;
            }
        }
        (info, confidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[test]
    fn test_confidence_high_when_api_matched() {
        use crate::models::Confidence;
        assert_eq!(compute_confidence(false, true), Confidence::High);
        assert_eq!(compute_confidence(true, true), Confidence::High);
    }

    #[test]
    fn test_confidence_medium_when_embedded_tags_no_api() {
        use crate::models::Confidence;
        assert_eq!(compute_confidence(true, false), Confidence::Medium);
    }

    #[test]
    fn test_confidence_low_when_heuristic_only() {
        use crate::models::Confidence;
        assert_eq!(compute_confidence(false, false), Confidence::Low);
    }

    #[test]
    fn test_override_from_db_sets_db_artist_and_fills_missing() {
        // override_from_db écrase l'artiste avec la valeur DB (casse brute :
        // l'unification est déléguée à finalize) et complète les champs manquants.
        let mut info = TrackInfo::default();
        let db_info = TrackInfo {
            artist: Some("Boards Of Canada".into()),
            album: Some("Geogaddi".into()),
            ..Default::default()
        };
        override_from_db(&mut info, &db_info);
        assert_eq!(info.artist, Some("Boards Of Canada".into()));
        assert_eq!(info.album, Some("Geogaddi".into()));
    }

    /// finalize() doit unifier la casse de l'artiste sur TOUS les chemins de
    /// sortie d'enrich() (y compris tags complets / heuristique, sans match API).
    #[test]
    fn test_finalize_unifies_artist_casing() {
        let dir = tempdir().unwrap();
        let cache = Arc::new(crate::cache::Cache::open(dir.path()).unwrap());
        let enricher =
            Enricher::new(&crate::config::Config::default(), cache).unwrap();

        // Premier artiste vu → enregistré tel quel.
        let (i1, _) = enricher.finalize(
            TrackInfo {
                artist: Some("Boards Of Canada".into()),
                ..Default::default()
            },
            crate::models::Confidence::Medium,
        );
        assert_eq!(i1.artist, Some("Boards Of Canada".into()));

        // Même artiste, casse différente → reprend la casse enregistrée.
        let (i2, _) = enricher.finalize(
            TrackInfo {
                artist: Some("boards of canada".into()),
                ..Default::default()
            },
            crate::models::Confidence::Medium,
        );
        assert_eq!(i2.artist, Some("Boards Of Canada".into()));
    }

    /// finalize() doit aussi unifier la casse d'album et retenir l'année la plus
    /// ancienne pour un même album (artiste déjà canonicalisé).
    #[test]
    fn test_finalize_unifies_album_casing_and_year() {
        let dir = tempdir().unwrap();
        let cache = Arc::new(crate::cache::Cache::open(dir.path()).unwrap());
        let enricher =
            Enricher::new(&crate::config::Config::default(), cache).unwrap();

        let (i1, _) = enricher.finalize(
            TrackInfo {
                artist: Some("Boards of Canada".into()),
                album: Some("Geogaddi".into()),
                year: Some(2002),
                ..Default::default()
            },
            crate::models::Confidence::Medium,
        );
        assert_eq!(i1.year, Some(2002));

        // Même album, casse différente + année plus récente → casse unifiée,
        // année la plus ancienne (2002) conservée.
        let (i2, _) = enricher.finalize(
            TrackInfo {
                artist: Some("boards of canada".into()),
                album: Some("geogaddi".into()),
                year: Some(2013),
                ..Default::default()
            },
            crate::models::Confidence::Medium,
        );
        assert_eq!(i2.album, Some("Geogaddi".into()));
        assert_eq!(i2.year, Some(2002));
    }
}
