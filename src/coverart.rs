use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::blocking::Client;

use crate::rate_limiter::RateLimiter;

/// URL de base de l'API Cover Art Archive
const BASE_URL: &str = "https://coverartarchive.org";

/// Client HTTP pour récupérer les pochettes d'albums via Cover Art Archive
pub struct CoverArtClient {
    client: Client,
    rate_limiter: Arc<RateLimiter>,
}

impl CoverArtClient {
    /// Crée un nouveau client Cover Art Archive avec le limiteur de débit fourni
    pub fn new(rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("music-sorter/0.1.0")
            .timeout(Duration::from_secs(10))
            .gzip(true)
            .pool_max_idle_per_host(4)
            .build()?;

        Ok(Self { client, rate_limiter })
    }

    /// Récupère la pochette d'album pour un release_id MusicBrainz
    /// Retourne les octets de l'image si trouvée
    pub fn fetch_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("{}/release/{}", BASE_URL, release_id);

        self.rate_limiter.wait();
        let response = self.client.get(&url).send()?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = response.json()?;

        // Extrait l'URL de la pochette depuis la réponse JSON
        let image_url = match extract_front_image_url(&json) {
            Some(url) => url,
            None => return Ok(None),
        };

        // Télécharge l'image depuis l'URL extraite
        self.rate_limiter.wait();
        let img_response = self.client.get(&image_url).send()?;

        if !img_response.status().is_success() {
            return Ok(None);
        }

        let bytes = img_response.bytes()?;
        Ok(Some(bytes.to_vec()))
    }

    /// Récupère la pochette via le cache binaire ; sinon HTTP + record
    pub fn fetch_cover_cached(
        &self,
        cache: &crate::cache::Cache,
        release_id: &str,
    ) -> Result<Option<Vec<u8>>> {
        if let Some(b) = cache.lookup_cover(release_id)? {
            return Ok(Some(b));
        }
        match self.fetch_cover(release_id)? {
            Some(bytes) => {
                cache.record_cover(release_id, &bytes)?;
                Ok(Some(bytes))
            }
            None => Ok(None),
        }
    }
}

/// Extrait l'URL de la pochette avant depuis la réponse JSON Cover Art Archive
/// Préfère l'image avec front: true, sinon utilise la première disponible
fn extract_front_image_url(resp: &serde_json::Value) -> Option<String> {
    let images = resp["images"].as_array()?;

    // Cherche d'abord une image marquée "front"
    images
        .iter()
        .find(|img| img["front"].as_bool() == Some(true))
        .or_else(|| images.first())
        .and_then(|img| img["image"].as_str())
        .map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_front_image_url() {
        // Doit préférer l'image avec front: true
        let json = serde_json::json!({
            "images": [
                { "front": false, "image": "https://coverartarchive.org/release/abc/back.jpg" },
                { "front": true, "image": "https://coverartarchive.org/release/abc/front.jpg" }
            ]
        });

        let url = extract_front_image_url(&json).unwrap();
        assert_eq!(url, "https://coverartarchive.org/release/abc/front.jpg");
    }

    #[test]
    fn test_extract_front_image_url_fallback() {
        // Aucune image front : utiliser la première disponible
        let json = serde_json::json!({
            "images": [
                { "front": false, "image": "https://coverartarchive.org/release/abc/first.jpg" },
                { "front": false, "image": "https://coverartarchive.org/release/abc/second.jpg" }
            ]
        });

        let url = extract_front_image_url(&json).unwrap();
        assert_eq!(url, "https://coverartarchive.org/release/abc/first.jpg");
    }

    #[test]
    fn test_extract_front_image_url_empty() {
        // Tableau d'images vide : doit retourner None
        let json = serde_json::json!({ "images": [] });
        let url = extract_front_image_url(&json);
        assert!(url.is_none());
    }
}
