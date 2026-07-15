use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Config {
    pub discogs_token: Option<String>,
    pub acoustid_api_key: Option<String>,
    pub source: Option<String>,
    pub target: Option<String>,
    pub workers: Option<usize>,
    pub r#move: Option<bool>,
    pub cache_enabled: Option<bool>,         // défaut true
    pub api_cache_ttl_days: Option<u32>,     // défaut 30
    pub naming_template: Option<String>,     // défaut organizer::DEFAULT_TEMPLATE
    pub unsorted_ttl_days: Option<i64>,      // défaut 30 — re-tente _unsorted après ce délai
    pub quarantine_enabled: Option<bool>,    // défaut true — route les matchs faibles vers _review/
    pub dedup_enabled: Option<bool>,         // défaut true — détecte les doublons par hash de contenu
    pub audio_dedup: Option<bool>,           // défaut true — dédup acoustique (empreinte+durée → corbeille)
    pub fix_tags: Option<bool>,              // défaut false — réécrit les tags canoniques sur match sûr
}

impl Config {
    /// Charge la config depuis ~/.config/music-sorter/config.toml
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path();

        if !config_path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Impossible de lire {}", config_path.display()))?;

        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Erreur de parsing dans {}", config_path.display()))?;

        Ok(config)
    }

    /// Parse une config depuis une chaîne TOML (pour les tests)
    pub fn from_str(content: &str) -> Result<Self> {
        let config: Config = toml::from_str(content)?;
        Ok(config)
    }

    fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("/home"))
            .join("music-sorter")
            .join("config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
            discogs_token = "abc123"
            acoustid_api_key = "def456"
        "#;

        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.discogs_token, Some("abc123".into()));
        assert_eq!(config.acoustid_api_key, Some("def456".into()));
    }

    #[test]
    fn test_parse_partial_config() {
        let toml = r#"discogs_token = "abc123""#;
        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.discogs_token, Some("abc123".into()));
        assert_eq!(config.acoustid_api_key, None);
    }

    #[test]
    fn test_parse_empty_config() {
        let config = Config::from_str("").unwrap();
        assert_eq!(config.discogs_token, None);
        assert_eq!(config.acoustid_api_key, None);
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.discogs_token, None);
    }

    #[test]
    fn test_parse_cache_options() {
        let toml = r#"
cache_enabled = false
api_cache_ttl_days = 7
"#;
        let c = Config::from_str(toml).unwrap();
        assert_eq!(c.cache_enabled, Some(false));
        assert_eq!(c.api_cache_ttl_days, Some(7));
    }

    #[test]
    fn test_parse_organization_options() {
        let toml = r#"
naming_template = "{artist}/{year} - {album}/{track} - {title}"
unsorted_ttl_days = 7
quarantine_enabled = false
dedup_enabled = true
fix_tags = true
"#;
        let c = Config::from_str(toml).unwrap();
        assert_eq!(
            c.naming_template,
            Some("{artist}/{year} - {album}/{track} - {title}".into())
        );
        assert_eq!(c.unsorted_ttl_days, Some(7));
        assert_eq!(c.quarantine_enabled, Some(false));
        assert_eq!(c.dedup_enabled, Some(true));
        assert_eq!(c.fix_tags, Some(true));
    }
}
