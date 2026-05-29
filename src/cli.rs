use crate::config::Config;
use clap::Parser;
use std::path::PathBuf;

/// Bannière ASCII art colorée affichée avant le help.
/// Codes ANSI : 36=cyan, 1;35=bold magenta, 2=dim, 0=reset.
const BANNER: &str = "\
\x1b[36m  ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫\x1b[0m
\x1b[1;35m   __  __           _        ____             _
  |  \\/  |_   _ ___(_) ___  / ___|  ___  _ __| |_ ___ _ __
  | |\\/| | | | / __| |/ __| \\___ \\ / _ \\| '__| __/ _ \\ '__|
  | |  | | |_| \\__ \\ | (__   ___) | (_) | |  | ||  __/ |
  |_|  |_|\\__,_|___/_|\\___| |____/ \\___/|_|   \\__\\___|_|\x1b[0m
\x1b[36m  ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪ ♫ ♪\x1b[0m
\x1b[2m  Organise ta musique automatiquement avec MusicBrainz, AcoustID et Discogs\x1b[0m
";

/// Exemples affichés après le help.
const EXAMPLES: &str = "\
\x1b[1;33mExemples :\x1b[0m
  \x1b[36mmusic-sorter --source ~/Téléchargements --target ~/Music\x1b[0m
  \x1b[36mmusic-sorter --workers 4 --move\x1b[0m
  \x1b[36mmusic-sorter --dry-run\x1b[0m                                  \x1b[2m# simule, ne touche rien\x1b[0m
  \x1b[36mmusic-sorter --target ~/Music --list-unsorted\x1b[0m         \x1b[2m# fichiers non rangés + raison\x1b[0m
  \x1b[36mmusic-sorter --target ~/Music --list-processed\x1b[0m         \x1b[2m# audit du cache\x1b[0m
  \x1b[36mmusic-sorter --target ~/Music --rollback\x1b[0m                \x1b[2m# dry-run\x1b[0m
  \x1b[36mmusic-sorter --target ~/Music --rollback --apply\x1b[0m         \x1b[2m# exécute\x1b[0m

\x1b[2mLes valeurs par défaut peuvent être stockées dans ~/.config/music-sorter/config.toml\x1b[0m
";

#[derive(Parser, Debug, Clone)]
#[command(
    name = "music-sorter",
    version,
    about = "Organise ta musique automatiquement",
    before_help = BANNER,
    after_help = EXAMPLES,
)]
pub struct Args {
    /// Dossier source à scanner
    #[arg(long)]
    pub source: Option<String>,

    /// Dossier destination
    #[arg(long)]
    pub target: Option<String>,

    /// Nombre de workers parallèles
    #[arg(long)]
    pub workers: Option<usize>,

    /// Déplacer les fichiers au lieu de les copier
    #[arg(long, default_value_t = false)]
    pub r#move: bool,

    /// Liste les entrées du cache processed_files (source → dest) puis quitte
    #[arg(long, default_value_t = false)]
    pub list_processed: bool,

    /// Liste les fichiers non rangés (unsorted/error) avec leur raison puis quitte
    #[arg(long, default_value_t = false)]
    pub list_unsorted: bool,

    /// Simule le tri sans rien copier/déplacer (rapport des destinations prévues)
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Reprend après interruption : skip aussi les _unsorted déjà vus (ignore le TTL)
    #[arg(long, default_value_t = false)]
    pub resume: bool,

    /// Défait les opérations passées en se basant sur le cache (dry-run par défaut)
    #[arg(long, default_value_t = false)]
    pub rollback: bool,

    /// Avec --rollback : exécute réellement (sans ce flag, simple aperçu)
    #[arg(long, default_value_t = false)]
    pub apply: bool,
}

/// Arguments résolus (config + CLI + défauts)
pub struct ResolvedArgs {
    pub source: PathBuf,
    pub target: PathBuf,
    pub workers: usize,
    pub do_move: bool,
    pub list_processed: bool,
    pub list_unsorted: bool,
    pub dry_run: bool,
    pub resume: bool,
    pub rollback: bool,
    pub apply: bool,
}

impl Args {
    /// Résout les arguments : CLI > config > défauts
    pub fn resolve(self, config: &Config) -> ResolvedArgs {
        let source_str = self
            .source
            .or_else(|| config.source.clone())
            .unwrap_or_else(default_source);

        let target_str = self
            .target
            .or_else(|| config.target.clone())
            .unwrap_or_else(default_target);

        let workers = self
            .workers
            .or(config.workers)
            .unwrap_or(1);

        let do_move = if self.r#move {
            true
        } else {
            config.r#move.unwrap_or(false)
        };

        ResolvedArgs {
            source: expand_tilde(&source_str),
            target: expand_tilde(&target_str),
            workers,
            do_move,
            list_processed: self.list_processed,
            list_unsorted: self.list_unsorted,
            dry_run: self.dry_run,
            resume: self.resume,
            rollback: self.rollback,
            apply: self.apply,
        }
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

    #[test]
    fn test_resolve_cli_overrides_config() {
        let args = Args {
            source: Some("/tmp/src".into()),
            target: Some("/tmp/dst".into()),
            workers: Some(4),
            r#move: true,
            list_processed: false,
            list_unsorted: false,
            dry_run: false,
            resume: false,
            rollback: false,
            apply: false,
        };
        let config = Config {
            source: Some("~/Downloads".into()),
            target: Some("~/Musique".into()),
            workers: Some(2),
            r#move: Some(false),
            ..Default::default()
        };

        let resolved = args.resolve(&config);
        assert_eq!(resolved.source, PathBuf::from("/tmp/src"));
        assert_eq!(resolved.target, PathBuf::from("/tmp/dst"));
        assert_eq!(resolved.workers, 4);
        assert!(resolved.do_move);
    }

    #[test]
    fn test_resolve_falls_back_to_config() {
        let args = Args {
            source: None,
            target: None,
            workers: None,
            r#move: false,
            list_processed: false,
            list_unsorted: false,
            dry_run: false,
            resume: false,
            rollback: false,
            apply: false,
        };
        let config = Config {
            source: Some("/data/music-in".into()),
            target: Some("/data/music-out".into()),
            workers: Some(2),
            r#move: Some(true),
            ..Default::default()
        };

        let resolved = args.resolve(&config);
        assert_eq!(resolved.source, PathBuf::from("/data/music-in"));
        assert_eq!(resolved.target, PathBuf::from("/data/music-out"));
        assert_eq!(resolved.workers, 2);
        assert!(resolved.do_move);
    }

    #[test]
    fn test_resolve_falls_back_to_defaults() {
        let args = Args {
            source: None,
            target: None,
            workers: None,
            r#move: false,
            list_processed: false,
            list_unsorted: false,
            dry_run: false,
            resume: false,
            rollback: false,
            apply: false,
        };
        let config = Config::default();

        let resolved = args.resolve(&config);
        assert!(resolved.source.to_string_lossy().contains("chargements"));
        assert!(resolved.target.to_string_lossy().contains("Music"));
        assert_eq!(resolved.workers, 1);
        assert!(!resolved.do_move);
    }
}
