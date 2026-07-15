/// Informations sur une piste audio
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub total_tracks: Option<u32>,
    pub genre: Option<String>,
    pub cover_art: Option<Vec<u8>>,
}

/// Niveau de confiance d'un enrichissement, pour router les matchs faibles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Confirmé par une API (AcoustID/MusicBrainz).
    High,
    /// Tags embarqués complets, sans confirmation API.
    Medium,
    /// Organisable uniquement grâce aux heuristiques (nom de fichier/dossier).
    Low,
}

/// Résultat du traitement d'un fichier
#[derive(Debug)]
#[allow(dead_code)]
pub enum ProcessResult {
    Organized { from: std::path::PathBuf, to: std::path::PathBuf },
    /// Skip total via le cache : aucun appel API, aucune copie
    CachedSkip { from: std::path::PathBuf, to: std::path::PathBuf },
    ConflictResolved { path: std::path::PathBuf, kept_bitrate: u32 },
    Unsorted { from: std::path::PathBuf, to: std::path::PathBuf },
    /// Doublon de contenu : un fichier au contenu identique a déjà été rangé.
    Duplicate { from: std::path::PathBuf, of: std::path::PathBuf },
    /// Doublon acoustique : même enregistrement (empreinte+durée) déjà rangé en
    /// meilleure qualité. Le fichier courant est envoyé à la corbeille.
    AcousticDuplicate { from: std::path::PathBuf, of: std::path::PathBuf },
    /// Traitement abandonné suite à une interruption (Ctrl-C) : non traité, non enregistré.
    Interrupted { path: std::path::PathBuf },
    Error { path: std::path::PathBuf, reason: String },
}

/// Extensions audio supportées
pub const SUPPORTED_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "m4a", "aac", "opus", "wma"];

impl TrackInfo {
    /// Fusionne les champs manquants de self avec ceux de other
    pub fn merge(&mut self, other: &TrackInfo) {
        if self.artist.is_none() {
            self.artist.clone_from(&other.artist);
        }
        if self.album.is_none() {
            self.album.clone_from(&other.album);
        }
        if self.title.is_none() {
            self.title.clone_from(&other.title);
        }
        if self.year.is_none() {
            self.year = other.year;
        }
        if self.track_number.is_none() {
            self.track_number = other.track_number;
        }
        if self.total_tracks.is_none() {
            self.total_tracks = other.total_tracks;
        }
        if self.genre.is_none() {
            self.genre.clone_from(&other.genre);
        }
        if self.cover_art.is_none() {
            self.cover_art.clone_from(&other.cover_art);
        }
    }

    /// Vérifie si on a assez d'info pour chercher dans les APIs
    pub fn has_minimum_for_search(&self) -> bool {
        self.artist.is_some() && self.title.is_some()
    }

    /// Vérifie si tous les champs essentiels + la pochette sont présents
    pub fn has_full_metadata(&self) -> bool {
        self.artist.is_some() && self.album.is_some() && self.title.is_some()
            && self.year.is_some() && self.track_number.is_some() && self.cover_art.is_some()
    }

    /// Vérifie si on a assez d'info pour organiser le fichier
    pub fn has_minimum_for_organization(&self) -> bool {
        self.artist.is_some() && self.album.is_some() && self.title.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_fills_missing_fields() {
        let mut base = TrackInfo {
            artist: Some("Boards of Canada".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        let other = TrackInfo {
            artist: Some("Autre artiste".into()),
            album: Some("Geogaddi".into()),
            year: Some(2002),
            track_number: Some(2),
            genre: Some("Electronic".into()),
            ..Default::default()
        };

        base.merge(&other);

        assert_eq!(base.artist, Some("Boards of Canada".into()));
        assert_eq!(base.album, Some("Geogaddi".into()));
        assert_eq!(base.title, Some("Music Is Math".into()));
        assert_eq!(base.year, Some(2002));
        assert_eq!(base.track_number, Some(2));
        assert_eq!(base.genre, Some("Electronic".into()));
    }

    #[test]
    fn test_merge_does_not_overwrite_existing() {
        let mut base = TrackInfo {
            artist: Some("BoC".into()),
            year: Some(2002),
            ..Default::default()
        };
        let other = TrackInfo {
            artist: Some("Autre".into()),
            year: Some(1999),
            ..Default::default()
        };

        base.merge(&other);

        assert_eq!(base.artist, Some("BoC".into()));
        assert_eq!(base.year, Some(2002));
    }

    #[test]
    fn test_has_minimum_for_search() {
        let empty = TrackInfo::default();
        assert!(!empty.has_minimum_for_search());

        let partial = TrackInfo {
            artist: Some("BoC".into()),
            ..Default::default()
        };
        assert!(!partial.has_minimum_for_search());

        let enough = TrackInfo {
            artist: Some("BoC".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        assert!(enough.has_minimum_for_search());
    }

    #[test]
    fn test_has_full_metadata_requires_all_fields() {
        let mut info = TrackInfo {
            artist: Some("a".into()),
            album: Some("b".into()),
            title: Some("c".into()),
            year: Some(2020),
            track_number: Some(1),
            genre: Some("g".into()),
            cover_art: Some(vec![1]),
            ..Default::default()
        };
        assert!(info.has_full_metadata());
        info.cover_art = None;
        assert!(!info.has_full_metadata());
    }

    #[test]
    fn test_has_minimum_for_organization() {
        let partial = TrackInfo {
            artist: Some("BoC".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        assert!(!partial.has_minimum_for_organization());

        let enough = TrackInfo {
            artist: Some("BoC".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        assert!(enough.has_minimum_for_organization());
    }
}
