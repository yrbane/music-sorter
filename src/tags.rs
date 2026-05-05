use crate::models::TrackInfo;
use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Accessor, Tag, TagType};
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

    // Supprime l'éventuel tag ID3v1 : son encodeur (lofty 0.22) panique
    // sur les chaînes contenant des caractères multi-octets pile à la frontière
    // de 30 bytes (limite ID3v1). De toute façon ID3v1 est obsolète et incapable
    // de stocker artiste/album/titre au-delà de 30 bytes.
    let _ = tagged_file.remove(TagType::Id3v1);

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

/// Heuristique : extrait artist / year / album du nom du dossier parent du fichier.
/// Reconnaît les patterns « Artist - Year - Album » et « Artist - Album ».
/// Retourne (None, None, None) si le dossier ne matche aucun pattern reconnaissable
/// ou s'il s'agit d'un dossier technique (commence par `_` ou `.`).
pub fn parse_folder_metadata(path: &Path) -> (Option<String>, Option<u32>, Option<String>) {
    let parent_name = match path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        Some(s) => s.trim(),
        None => return (None, None, None),
    };

    if parent_name.is_empty()
        || parent_name.starts_with('_')
        || parent_name.starts_with('.')
    {
        return (None, None, None);
    }

    // Pattern 1 : « Artist - Year - Album » (year = 4 chiffres entre 1900 et 2100)
    let triple: Vec<&str> = parent_name.splitn(3, " - ").collect();
    if triple.len() == 3 {
        if let Ok(year) = triple[1].trim().parse::<u32>() {
            if (1900..=2100).contains(&year) {
                let artist = triple[0].trim().to_string();
                let album = triple[2].trim().to_string();
                if !artist.is_empty() && !album.is_empty() {
                    return (Some(artist), Some(year), Some(album));
                }
            }
        }
    }

    // Pattern 2 : « Artist - Album » (un seul séparateur ` - `)
    let pair: Vec<&str> = parent_name.splitn(2, " - ").collect();
    if pair.len() == 2 {
        let artist = pair[0].trim().to_string();
        let album = pair[1].trim().to_string();
        if !artist.is_empty() && !album.is_empty() {
            return (Some(artist), None, Some(album));
        }
    }

    (None, None, None)
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

    #[test]
    fn test_parse_folder_artist_year_album() {
        let (a, y, al) = parse_folder_metadata(Path::new(
            "/music/Boards of Canada - 2002 - Geogaddi/01 track.mp3",
        ));
        assert_eq!(a, Some("Boards of Canada".into()));
        assert_eq!(y, Some(2002));
        assert_eq!(al, Some("Geogaddi".into()));
    }

    #[test]
    fn test_parse_folder_artist_album_no_year() {
        let (a, y, al) = parse_folder_metadata(Path::new(
            "/music/Boards of Canada - Geogaddi/01 track.mp3",
        ));
        assert_eq!(a, Some("Boards of Canada".into()));
        assert_eq!(y, None);
        assert_eq!(al, Some("Geogaddi".into()));
    }

    #[test]
    fn test_parse_folder_no_separator_returns_none() {
        let (a, y, al) = parse_folder_metadata(Path::new(
            "/music/JustAnAlbumName/track.mp3",
        ));
        assert_eq!(a, None);
        assert_eq!(y, None);
        assert_eq!(al, None);
    }

    #[test]
    fn test_parse_folder_skips_underscore_prefix() {
        let (a, _, al) = parse_folder_metadata(Path::new(
            "/music/_unsorted/Foo - Bar/track.mp3",
        ));
        // Le parent direct (« Foo - Bar ») est valide ici, mais on saurait skip
        // si c'était _unsorted lui-même qui contenait la file. Test vérifie que
        // le filtre s'applique seulement au parent direct :
        assert_eq!(a, Some("Foo".into()));
        assert_eq!(al, Some("Bar".into()));
    }

    #[test]
    fn test_parse_folder_skips_when_parent_starts_with_underscore() {
        let (a, y, al) = parse_folder_metadata(Path::new("/music/_unsorted/track.mp3"));
        assert_eq!(a, None);
        assert_eq!(y, None);
        assert_eq!(al, None);
    }

    #[test]
    fn test_parse_folder_album_with_dashes() {
        // « Album - With - Dashes » → artist=Artist, album="Album - With - Dashes"
        let (a, _, al) = parse_folder_metadata(Path::new(
            "/music/Artist - Album - With - Dashes/track.mp3",
        ));
        assert_eq!(a, Some("Artist".into()));
        assert_eq!(al, Some("Album - With - Dashes".into()));
    }

    #[test]
    fn test_parse_folder_year_out_of_range_falls_back() {
        // « 999 » n'est pas une année valide → on retombe sur le pattern Artist - Album
        let (a, y, al) = parse_folder_metadata(Path::new(
            "/music/Artist - 999 - Album/track.mp3",
        ));
        assert_eq!(a, Some("Artist".into()));
        assert_eq!(y, None);
        assert_eq!(al, Some("999 - Album".into()));
    }
}
