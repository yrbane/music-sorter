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
        .or_else(|| tagged_file.first_tag())
        .with_context(|| format!("Aucun tag trouvé dans : {}", path.display()))?;

    // Extrait l'image de couverture si présente
    let cover_art = tag
        .pictures()
        .first()
        .map(|pic| pic.data().to_vec());

    Ok(TrackInfo {
        artist: tag.artist().map(|v| v.into_owned()),
        album: tag.album().map(|v| v.into_owned()),
        title: tag.title().map(|v| v.into_owned()),
        year: tag.year(),
        track_number: tag.track(),
        total_tracks: tag.track_total(),
        genre: tag.genre().map(|v| v.into_owned()),
        cover_art,
    })
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
}
