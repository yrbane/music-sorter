use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::blocking::Client;

use crate::rate_limiter::RateLimiter;

/// URL de base de l'API Cover Art Archive
const BASE_URL: &str = "https://coverartarchive.org";

/// Cherche une image de pochette dans un dossier : priorité aux noms explicites
/// (cover/front/folder/albumart), sinon la plus grande image. Renvoie ses octets.
pub fn find_local_cover(dir: &std::path::Path) -> Option<Vec<u8>> {
    const EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];
    const PREFERRED: &[&str] = &["cover", "front", "folder", "albumart"];

    let mut images: Vec<(std::path::PathBuf, u64)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?.to_lowercase();
            if EXTS.contains(&ext.as_str()) {
                Some((path, e.metadata().map(|m| m.len()).unwrap_or(0)))
            } else {
                None
            }
        })
        .collect();
    if images.is_empty() {
        return None;
    }

    // Priorité aux noms explicites (cover/front/folder/albumart).
    let named = images.iter().find(|(p, _)| {
        p.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                let low = s.to_lowercase();
                PREFERRED.iter().any(|k| low.contains(k))
            })
            .unwrap_or(false)
    });
    let chosen = match named {
        Some((p, _)) => p.clone(),
        None => {
            // Sinon la plus grande image (souvent la pochette pleine résolution).
            images.sort_by_key(|(_, sz)| *sz);
            images.last()?.0.clone()
        }
    };
    std::fs::read(&chosen).ok()
}

/// Écrit une pochette `cover.jpg` dans le dossier d'album si absente.
pub fn write_album_cover(album_dir: &std::path::Path, bytes: &[u8]) {
    let cover = album_dir.join("cover.jpg");
    if !cover.exists() {
        let _ = std::fs::write(&cover, bytes);
    }
}

/// Client HTTP pour récupérer les pochettes d'albums via Cover Art Archive
pub struct CoverArtClient {
    client: Client,
    rate_limiter: Arc<RateLimiter>,
}

impl CoverArtClient {
    /// Crée un nouveau client Cover Art Archive avec le limiteur de débit fourni
    pub fn new(rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("music-sorter/", env!("CARGO_PKG_VERSION")))
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

        let response = match crate::retry::get_with_retry(Some(&self.rate_limiter), || {
            self.client.get(&url)
        }) {
            Some(r) => r,
            None => return Ok(None),
        };

        let json: serde_json::Value = response.json()?;

        // Extrait l'URL de la pochette depuis la réponse JSON
        let image_url = match extract_front_image_url(&json) {
            Some(url) => url,
            None => return Ok(None),
        };

        // Télécharge l'image depuis l'URL extraite
        let img_response = match crate::retry::get_with_retry(Some(&self.rate_limiter), || {
            self.client.get(&image_url)
        }) {
            Some(r) => r,
            None => return Ok(None),
        };

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
    fn test_find_local_cover_prefers_named() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("random.png"), b"random-data").unwrap();
        std::fs::write(dir.path().join("cover.jpg"), b"the-cover").unwrap();
        assert_eq!(find_local_cover(dir.path()).unwrap(), b"the-cover");
    }

    #[test]
    fn test_find_local_cover_largest_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"tiny").unwrap();
        std::fs::write(dir.path().join("b.jpg"), vec![7u8; 500]).unwrap();
        assert_eq!(find_local_cover(dir.path()).unwrap().len(), 500);
    }

    #[test]
    fn test_find_local_cover_none_without_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("song.mp3"), b"audio").unwrap();
        assert!(find_local_cover(dir.path()).is_none());
    }

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
