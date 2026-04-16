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

/// Orchestre l'enrichissement des métadonnées audio via MusicBrainz, AcoustID, Cover Art Archive et Discogs
/// Écrase artiste et album avec les noms canoniques de la DB,
/// puis fusionne le reste (remplit les champs manquants)
fn override_from_db(info: &mut TrackInfo, db_info: &TrackInfo) {
    if db_info.artist.is_some() {
        info.artist.clone_from(&db_info.artist);
    }
    if db_info.album.is_some() {
        info.album.clone_from(&db_info.album);
    }
    info.merge(db_info);
}

pub struct Enricher {
    musicbrainz: MusicBrainzClient,
    discogs: Option<DiscogsClient>,
    coverart: CoverArtClient,
    fpcalc_available: bool,
    acoustid_api_key: Option<String>,
}

impl Enricher {
    /// Crée un nouvel Enricher à partir de la configuration fournie
    pub fn new(config: &Config) -> Result<Self> {
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
        })
    }

    /// Enrichit les métadonnées d'un fichier audio
    pub fn enrich(&self, path: &Path) -> Result<TrackInfo> {
        // Étape 1 : lecture des tags existants
        let mut info = tags::read_tags(path).unwrap_or_default();

        let mut release_id: Option<String> = None;

        // Étape 2 : si les tags sont insuffisants, tenter le fingerprinting AcoustID
        if !info.has_minimum_for_search() && self.fpcalc_available {
            if let Some(ref api_key) = self.acoustid_api_key {
                if let Ok(fp) = fingerprint::generate_fingerprint(path) {
                    if let Ok(Some(recording_id)) = fingerprint::lookup_acoustid(api_key, &fp) {
                        if let Ok(Some((mb_info, rid))) =
                            self.musicbrainz.lookup_by_recording_id(&recording_id)
                        {
                            // Préférer le nom canonique de la DB pour artiste et album
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
            let title = info.title.as_deref().unwrap();

            if let Ok(Some((mb_info, rid))) = self.musicbrainz.search_by_text(artist, title) {
                // Préférer le nom canonique de la DB pour artiste et album
                override_from_db(&mut info, &mb_info);
                release_id = Some(rid);
            }
        }

        // Étape 4 : pochette via Cover Art Archive
        if info.cover_art.is_none() {
            if let Some(ref rid) = release_id {
                if let Ok(Some(cover)) = self.coverart.fetch_cover(rid) {
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
                        discogs.search_release(artist, album)
                    {
                        info.merge(&discogs_info);

                        if let Some(ref url) = resource_url {
                            if let Ok(Some(details)) = discogs.get_release_details(url) {
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

        Ok(info)
    }
}
