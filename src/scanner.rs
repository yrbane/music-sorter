use crate::models::SUPPORTED_EXTENSIONS;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Scanne récursivement un dossier et retourne les fichiers audio supportés
pub fn scan(source: &Path) -> Vec<PathBuf> {
    WalkDir::new(source)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !is_apple_double(e.path()))
        .filter(|e| is_supported_audio(e.path()))
        .map(|e| e.into_path())
        .collect()
}

fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Détecte les fichiers AppleDouble (`._foo.mp3`) créés par macOS sur exFAT/NTFS.
/// Ce ne sont pas des fichiers audio mais des sidecars binaires de métadonnées.
fn is_apple_double(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.starts_with("._"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_finds_audio_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("song.mp3"), b"fake").unwrap();
        fs::write(dir.path().join("album.flac"), b"fake").unwrap();
        fs::write(dir.path().join("readme.txt"), b"text").unwrap();
        fs::write(dir.path().join("image.jpg"), b"img").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("album");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.path().join("root.mp3"), b"fake").unwrap();
        fs::write(sub.join("track.flac"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_all_supported_extensions() {
        let dir = TempDir::new().unwrap();
        for ext in SUPPORTED_EXTENSIONS {
            fs::write(dir.path().join(format!("file.{}", ext)), b"fake").unwrap();
        }
        // Fichier WAV ne doit pas être détecté
        fs::write(dir.path().join("file.wav"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), SUPPORTED_EXTENSIONS.len());
    }

    #[test]
    fn test_scan_case_insensitive_extension() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("song.MP3"), b"fake").unwrap();
        fs::write(dir.path().join("song.Flac"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir = TempDir::new().unwrap();
        let files = scan(dir.path());
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_skips_apple_double_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("song.mp3"), b"fake").unwrap();
        fs::write(dir.path().join("._song.mp3"), b"appledouble").unwrap();
        fs::write(dir.path().join("._track.flac"), b"appledouble").unwrap();
        fs::write(dir.path().join("real.flac"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
        for f in &files {
            let name = f.file_name().unwrap().to_str().unwrap();
            assert!(!name.starts_with("._"), "AppleDouble non filtré : {}", name);
        }
    }
}
