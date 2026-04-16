use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Config {
    pub discogs_token: Option<String>,
    pub acoustid_api_key: Option<String>,
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
}
