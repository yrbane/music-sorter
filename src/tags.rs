use crate::models::TrackInfo;
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, Tag};
use std::path::Path;

/// Lit les tags audio d'un fichier et retourne un TrackInfo
pub fn read_tags(path: &Path) -> Result<TrackInfo> {
    // Ouvre le fichier avec lofty en utilisant read_from_path
    let tagged_file =
        lofty::read_from_path(path).with_context(|| format!("Impossible de lire : {}", path.display()))?;

    // Récupère le tag principal ou le premier tag disponible
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    let info = match tag {
        Some(tag) => {
            let cover_art = tag.pictures().first().map(|pic| pic.data().to_vec());
            TrackInfo {
                artist: tag.artist().map(|v| v.into_owned()),
                album: tag.album().map(|v| v.into_owned()),
                title: tag.title().map(|v| v.into_owned()),
                year: tag.year(),
                track_number: tag.track(),
                total_tracks: tag.track_total(),
                genre: tag.genre().map(|v| v.into_owned()),
                cover_art,
            }
        }
        None => TrackInfo::default(),
    };

    Ok(info)
}

/// Retourne le bitrate audio en kbps
pub fn get_bitrate(path: &Path) -> Result<u32> {
    // Ouvre le fichier avec lofty
    let tagged_file =
        lofty::read_from_path(path).with_context(|| format!("Impossible de lire : {}", path.display()))?;

    // Tente de récupérer le bitrate audio, puis le bitrate global
    let bitrate = tagged_file
        .properties()
        .audio_bitrate()
        .or_else(|| tagged_file.properties().overall_bitrate())
        .unwrap_or(0);

    Ok(bitrate)
}

/// Écrit les tags enrichis dans un fichier (remplit seulement les champs manquants)
pub fn write_tags(path: &Path, info: &TrackInfo) -> Result<()> {
    // Ouvre le fichier de façon mutable via BoundTaggedFile
    let mut tagged_file = lofty::read_from_path(path)
        .with_context(|| format!("Impossible de lire : {}", path.display()))?;

    // Récupère ou crée le tag principal
    let tag_type = tagged_file.primary_tag_type();
    if tagged_file.primary_tag().is_none() {
        let new_tag = Tag::new(tag_type);
        tagged_file.insert_tag(new_tag);
    }

    {
        let tag = tagged_file
            .primary_tag_mut()
            .expect("Le tag vient d'être créé");

        // Ne remplit que les champs manquants dans le tag existant
        if tag.artist().is_none() {
            if let Some(artist) = &info.artist {
                tag.set_artist(artist.clone());
            }
        }
        if tag.album().is_none() {
            if let Some(album) = &info.album {
                tag.set_album(album.clone());
            }
        }
        if tag.title().is_none() {
            if let Some(title) = &info.title {
                tag.set_title(title.clone());
            }
        }
        if tag.year().is_none() {
            if let Some(year) = info.year {
                tag.set_year(year);
            }
        }
        if tag.track().is_none() {
            if let Some(track) = info.track_number {
                tag.set_track(track);
            }
        }
        if tag.track_total().is_none() {
            if let Some(total) = info.total_tracks {
                tag.set_track_total(total);
            }
        }
        if tag.genre().is_none() {
            if let Some(genre) = &info.genre {
                tag.set_genre(genre.clone());
            }
        }

        // Intègre la pochette si le tag n'en a pas et que info en a une
        if tag.pictures().is_empty() {
            if let Some(cover_data) = &info.cover_art {
                // Détecte JPEG (0xFF 0xD8) ou PNG par défaut
                let mime_type = if cover_data.starts_with(&[0xFF, 0xD8]) {
                    MimeType::Jpeg
                } else {
                    MimeType::Png
                };

                let picture = Picture::new_unchecked(
                    PictureType::CoverFront,
                    Some(mime_type),
                    None,
                    cover_data.clone(),
                );
                tag.push_picture(picture);
            }
        }
    }

    // Sauvegarde le tag dans le fichier
    tagged_file
        .save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Impossible de sauvegarder les tags dans : {}", path.display()))?;

    Ok(())
}

/// Heuristique : extrait artiste et titre du nom de fichier quand les tags manquent.
/// Strip un préfixe optionnel de numéro de piste ("01 - ", "00- ", "1.", etc.)
/// puis split sur le premier " - " pour séparer artiste et titre.
/// Si pas de séparateur, retourne (None, Some(stem)) — on a au moins un titre.
pub fn parse_artist_title_from_filename(path: &Path) -> (Option<String>, Option<String>) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let cleaned = strip_track_prefix(stem).trim();
    if cleaned.is_empty() {
        return (None, None);
    }

    if let Some(idx) = cleaned.find(" - ") {
        let a = cleaned[..idx].trim().to_string();
        let t = cleaned[idx + 3..].trim().to_string();
        if !a.is_empty() && !t.is_empty() {
            return (Some(a), Some(t));
        }
    }

    (None, Some(cleaned.to_string()))
}

/// Strip un préfixe de numéro de piste comme "01 - ", "00- ", "1.", "12 ", etc.
/// Retourne la chaîne d'origine si aucun préfixe reconnaissable.
fn strip_track_prefix(s: &str) -> &str {
    let trimmed = s.trim_start();
    let bytes = trimmed.as_bytes();
    let mut digits_end = 0;
    while digits_end < 3 && digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
        digits_end += 1;
    }
    if digits_end == 0 {
        return s;
    }
    let rest = &trimmed[digits_end..];
    let after = rest.trim_start_matches(|c: char| c == '.' || c == '-' || c == ' ' || c == '_');
    if after.len() < rest.len() && !after.is_empty() {
        after
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_tags_nonexistent_file() {
        let result = read_tags(Path::new("/tmp/nonexistent_file_xyz.mp3"));
        assert!(result.is_err());
    }

    #[test]
    fn test_get_bitrate_nonexistent_file() {
        let result = get_bitrate(Path::new("/tmp/nonexistent_file_xyz.mp3"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_filename_artist_title_split() {
        let (a, t) = parse_artist_title_from_filename(Path::new("Slope - Komputa Groove.mp3"));
        assert_eq!(a, Some("Slope".into()));
        assert_eq!(t, Some("Komputa Groove".into()));
    }

    #[test]
    fn test_parse_filename_with_track_number_prefix() {
        let (a, t) = parse_artist_title_from_filename(Path::new("00- La femme - Sphynx.mp3"));
        assert_eq!(a, Some("La femme".into()));
        assert_eq!(t, Some("Sphynx".into()));
    }

    #[test]
    fn test_parse_filename_track_only_after_strip() {
        // "01 - Sphynx" → après strip, juste "Sphynx" sans séparateur → titre seul
        let (a, t) = parse_artist_title_from_filename(Path::new("01 - Sphynx.mp3"));
        assert_eq!(a, None);
        assert_eq!(t, Some("Sphynx".into()));
    }

    #[test]
    fn test_parse_filename_keeps_parentheses_in_title() {
        let (a, t) = parse_artist_title_from_filename(Path::new(
            "Jimi Hendrix - Hey Joe (Der Joe Remix).mp3",
        ));
        assert_eq!(a, Some("Jimi Hendrix".into()));
        assert_eq!(t, Some("Hey Joe (Der Joe Remix)".into()));
    }

    #[test]
    fn test_parse_filename_splits_on_first_separator() {
        // Le titre peut contenir " - " ; on split UNIQUEMENT sur la première occurrence.
        let (a, t) = parse_artist_title_from_filename(Path::new(
            "Stabfinger & K.D.S - Double Trouble - (Original Mix).mp3",
        ));
        assert_eq!(a, Some("Stabfinger & K.D.S".into()));
        assert_eq!(t, Some("Double Trouble - (Original Mix)".into()));
    }

    #[test]
    fn test_parse_filename_no_separator_returns_title_only() {
        let (a, t) = parse_artist_title_from_filename(Path::new(
            "Agent51 the return of the secret disco agent.mp3",
        ));
        assert_eq!(a, None);
        assert_eq!(
            t,
            Some("Agent51 the return of the secret disco agent".into())
        );
    }

    #[test]
    fn test_parse_filename_strips_three_digit_prefix() {
        let (a, t) = parse_artist_title_from_filename(Path::new("100 - Track Title.mp3"));
        assert_eq!(a, None);
        assert_eq!(t, Some("Track Title".into()));
    }
}
