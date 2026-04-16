use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "music-sorter", version, about = "Organise ta musique automatiquement")]
pub struct Args {
    /// Dossier source à scanner
    #[arg(long, default_value_t = default_source())]
    pub source: String,

    /// Dossier destination
    #[arg(long, default_value_t = default_target())]
    pub target: String,

    /// Nombre de workers parallèles
    #[arg(long, default_value_t = 1)]
    pub workers: usize,

    /// Déplacer les fichiers au lieu de les copier
    #[arg(long, default_value_t = false)]
    pub r#move: bool,
}

impl Args {
    pub fn source_path(&self) -> PathBuf {
        expand_tilde(&self.source)
    }

    pub fn target_path(&self) -> PathBuf {
        expand_tilde(&self.target)
    }
}

fn default_source() -> String {
    dirs::home_dir()
        .map(|h| h.join("Téléchargements").to_string_lossy().into_owned())
        .unwrap_or_else(|| "~/Téléchargements".into())
}

fn default_target() -> String {
    dirs::home_dir()
        .map(|h| h.join("Music").to_string_lossy().into_owned())
        .unwrap_or_else(|| "~/Music".into())
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/Music");
        assert!(expanded.to_string_lossy().contains("Music"));
        assert!(!expanded.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_expand_no_tilde() {
        let path = expand_tilde("/tmp/music");
        assert_eq!(path, PathBuf::from("/tmp/music"));
    }

    #[test]
    fn test_default_source_not_empty() {
        let src = default_source();
        assert!(!src.is_empty());
        assert!(src.contains("chargements"));
    }

    #[test]
    fn test_default_target_not_empty() {
        let tgt = default_target();
        assert!(!tgt.is_empty());
        assert!(tgt.contains("Music"));
    }
}
