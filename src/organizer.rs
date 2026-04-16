use crate::models::TrackInfo;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Résultat d'une copie de fichier
#[derive(Debug)]
pub enum CopyResult {
    Copied,
    Replaced { bitrate: u32 },
    Skipped { existing_bitrate: u32 },
}

/// Remplace les caractères invalides dans un nom de fichier par `_`
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Cherche un dossier existant dont le nom correspond (case-insensitive)
/// et retourne son nom exact. Sinon retourne le nom proposé.
fn resolve_existing_folder(target: &Path, proposed: &str) -> String {
    let proposed_lower = proposed.to_lowercase();
    if let Ok(entries) = std::fs::read_dir(target) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.to_lowercase() == proposed_lower {
                    return name.to_string();
                }
            }
        }
    }
    proposed.to_string()
}

/// Construit le chemin de destination d'un fichier audio
pub fn build_destination_path(target: &Path, info: &TrackInfo, original_path: &Path) -> PathBuf {
    // Récupère l'extension du fichier original
    let ext = original_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // Pas assez d'info → dossier _unsorted
    if !info.has_minimum_for_organization() {
        let filename = original_path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("unknown"));
        return target.join("_unsorted").join(filename);
    }

    // Les champs sont garantis Some ici
    let artist = sanitize_filename(info.artist.as_deref().unwrap_or("Unknown"));
    let album = sanitize_filename(info.album.as_deref().unwrap_or("Unknown"));
    let title = sanitize_filename(info.title.as_deref().unwrap_or("Unknown"));

    // Dossier : [artist] - [year] - [album]  ou  [artist] - [album]
    let folder_name = match info.year {
        Some(year) => format!("{} - {} - {}", artist, year, album),
        None => format!("{} - {}", artist, album),
    };

    // Réutilise un dossier existant si seule la casse diffère
    let folder_name = resolve_existing_folder(target, &folder_name);

    // Nom de fichier : [NN] - [title].[ext]  ou  [title].[ext]
    let file_name = match info.track_number {
        Some(n) => format!("{:02} - {}.{}", n, title, ext),
        None => format!("{}.{}", title, ext),
    };

    target.join(folder_name).join(file_name)
}

/// Copie un fichier vers sa destination en gérant les conflits par bitrate
pub fn copy_to_destination(source: &Path, destination: &Path) -> Result<CopyResult> {
    // Crée les dossiers parents si nécessaire
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // La destination existe déjà → comparaison de bitrate
    if destination.exists() {
        let src_bitrate = crate::tags::get_bitrate(source).unwrap_or(0);
        let dst_bitrate = crate::tags::get_bitrate(destination).unwrap_or(0);

        if src_bitrate > dst_bitrate {
            // La source est de meilleure qualité → on remplace
            std::fs::copy(source, destination)?;
            return Ok(CopyResult::Replaced { bitrate: src_bitrate });
        } else {
            // La destination est au moins aussi bonne → on garde
            return Ok(CopyResult::Skipped {
                existing_bitrate: dst_bitrate,
            });
        }
    }

    // Pas de conflit → copie simple
    std::fs::copy(source, destination)?;
    Ok(CopyResult::Copied)
}

/// Supprime le fichier source
pub fn remove_source(source: &Path) -> Result<()> {
    std::fs::remove_file(source)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TrackInfo;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_build_destination_full_info() {
        let info = TrackInfo {
            artist: Some("Boards of Canada".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            year: Some(2002),
            track_number: Some(2),
            ..Default::default()
        };
        let original = Path::new("song.flac");
        let target = Path::new("/home/user/Music");

        let result = build_destination_path(target, &info, original);

        assert_eq!(
            result,
            Path::new("/home/user/Music/Boards of Canada - 2002 - Geogaddi/02 - Music Is Math.flac")
        );
    }

    #[test]
    fn test_build_destination_no_year() {
        let info = TrackInfo {
            artist: Some("BoC".into()),
            album: Some("Geogaddi".into()),
            title: Some("Track".into()),
            year: None,
            track_number: None,
            ..Default::default()
        };
        let original = Path::new("file.mp3");
        let target = Path::new("/music");

        let result = build_destination_path(target, &info, original);

        assert_eq!(result, Path::new("/music/BoC - Geogaddi/Track.mp3"));
    }

    #[test]
    fn test_build_destination_no_track_number() {
        let info = TrackInfo {
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            title: Some("Title".into()),
            year: Some(2020),
            track_number: None,
            ..Default::default()
        };
        let original = Path::new("track.ogg");
        let target = Path::new("/music");

        let result = build_destination_path(target, &info, original);

        assert_eq!(result, Path::new("/music/Artist - 2020 - Album/Title.ogg"));
    }

    #[test]
    fn test_build_destination_unsorted() {
        let info = TrackInfo::default();
        let original = Path::new("unknown.mp3");
        let target = Path::new("/music");

        let result = build_destination_path(target, &info, original);

        assert_eq!(result, Path::new("/music/_unsorted/unknown.mp3"));
    }

    #[test]
    fn test_build_destination_reuses_existing_folder_case_insensitive() {
        let dir = tempdir().unwrap();
        let target = dir.path();

        // Créer un dossier existant avec "Boards Of Canada"
        std::fs::create_dir_all(target.join("Boards Of Canada - 2002 - Geogaddi")).unwrap();

        // Un fichier avec "Boards of Canada" (o minuscule) doit réutiliser le dossier existant
        let info = TrackInfo {
            artist: Some("Boards of Canada".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            year: Some(2002),
            track_number: Some(2),
            ..Default::default()
        };

        let result = build_destination_path(target, &info, Path::new("song.flac"));

        assert_eq!(
            result,
            target.join("Boards Of Canada - 2002 - Geogaddi/02 - Music Is Math.flac")
        );
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("AC/DC"), "AC_DC");
        assert_eq!(sanitize_filename("a:b*c?d"), "a_b_c_d");
        assert_eq!(sanitize_filename("normal name"), "normal name");
    }

    #[test]
    fn test_copy_to_destination_new_file() {
        let dir = tempdir().unwrap();

        // Crée le fichier source
        let source = dir.path().join("source.txt");
        let mut f = std::fs::File::create(&source).unwrap();
        f.write_all(b"hello world").unwrap();

        // Destination dans un sous-dossier qui n'existe pas encore
        let destination = dir.path().join("sub/dest.txt");

        let result = copy_to_destination(&source, &destination).unwrap();

        // Vérifie que le fichier existe avec le bon contenu
        assert!(destination.exists());
        let content = std::fs::read(&destination).unwrap();
        assert_eq!(content, b"hello world");

        // Vérifie que le résultat est Copied
        assert!(matches!(result, CopyResult::Copied));
    }
}
