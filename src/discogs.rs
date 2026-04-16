use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::blocking::Client;

use crate::models::TrackInfo;
use crate::rate_limiter::RateLimiter;

/// URL de base de l'API Discogs
const BASE_URL: &str = "https://api.discogs.com";

/// Détails complets d'une release Discogs
pub struct ReleaseDetails {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    #[allow(dead_code)]
    pub tracks: Vec<DiscogsTrack>,
    pub cover_url: Option<String>,
}

/// Piste issue de la tracklist Discogs
#[allow(dead_code)]
pub struct DiscogsTrack {
    pub position: String,
    pub title: String,
}

/// Client HTTP pour interroger l'API Discogs
pub struct DiscogsClient {
    client: Client,
    token: String,
    rate_limiter: Arc<RateLimiter>,
}

impl DiscogsClient {
    /// Crée un nouveau client Discogs avec le token et le limiteur de débit fournis
    pub fn new(token: String, rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("music-sorter/0.1.0")
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self { client, token, rate_limiter })
    }

    /// Recherche une release par artiste et album
    /// Retourne (infos de base, resource_url) si trouvé
    pub fn search_release(
        &self,
        artist: &str,
        album: &str,
    ) -> Result<Option<(TrackInfo, Option<String>)>> {
        let artist_enc = url_encode(artist);
        let album_enc = url_encode(album);
        let url = format!(
            "{}/database/search?artist={}&release_title={}&type=release&token={}&per_page=5",
            BASE_URL, artist_enc, album_enc, self.token
        );

        self.rate_limiter.wait();
        let response = self.client.get(&url).send()?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = response.json()?;
        Ok(parse_search_response(&json))
    }

    /// Récupère les détails complets d'une release via sa resource_url Discogs
    pub fn get_release_details(&self, resource_url: &str) -> Result<Option<ReleaseDetails>> {
        self.rate_limiter.wait();
        let response = self.client.get(resource_url).send()?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = response.json()?;
        Ok(parse_release_details(&json))
    }

    /// Télécharge une image depuis son URL avec authentification Discogs
    pub fn fetch_image(&self, image_url: &str) -> Result<Vec<u8>> {
        self.rate_limiter.wait();
        let response = self.client
            .get(image_url)
            .header(
                "Authorization",
                format!("Discogs token={}", self.token),
            )
            .send()?;

        let bytes = response.bytes()?;
        Ok(bytes.to_vec())
    }
}

/// Encode les caractères spéciaux pour les paramètres d'URL
fn url_encode(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('?', "%3F")
}

/// Parse la réponse de recherche Discogs
/// Le format du titre est "Artiste - Album"
fn parse_search_response(resp: &serde_json::Value) -> Option<(TrackInfo, Option<String>)> {
    let results = resp["results"].as_array()?;
    let first = results.first()?;

    // Le titre Discogs est au format "Artiste - Album"
    let title_raw = first["title"].as_str().unwrap_or("");
    let (artist, album) = if let Some(idx) = title_raw.find(" - ") {
        let a = &title_raw[..idx];
        let b = &title_raw[idx + 3..];
        (Some(a.to_string()), Some(b.to_string()))
    } else {
        (None, Some(title_raw.to_string()))
    };

    // L'année peut être une chaîne ou un nombre dans le JSON
    let year = first["year"]
        .as_str()
        .and_then(|y| y.parse::<u32>().ok())
        .or_else(|| first["year"].as_u64().map(|y| y as u32));

    // Le genre est dans le tableau genre[0]
    let genre = first["genre"]
        .as_array()
        .and_then(|g| g.first())
        .and_then(|g| g.as_str())
        .map(|g| g.to_string());

    let resource_url = first["resource_url"]
        .as_str()
        .map(|u| u.to_string());

    let info = TrackInfo {
        artist,
        album,
        year,
        genre,
        title: None,
        track_number: None,
        total_tracks: None,
        cover_art: None,
    };

    Some((info, resource_url))
}

/// Parse les détails complets d'une release Discogs
fn parse_release_details(resp: &serde_json::Value) -> Option<ReleaseDetails> {
    // L'artiste est dans artists[0].name
    let artist = resp["artists"][0]["name"]
        .as_str()
        .map(|s| s.to_string());

    let album = resp["title"].as_str().map(|s| s.to_string());

    // L'année peut être une chaîne ou un nombre
    let year = resp["year"]
        .as_u64()
        .map(|y| y as u32)
        .or_else(|| {
            resp["year"]
                .as_str()
                .and_then(|y| y.parse::<u32>().ok())
        });

    // Le genre est dans genres[0]
    let genre = resp["genres"]
        .as_array()
        .and_then(|g| g.first())
        .and_then(|g| g.as_str())
        .map(|g| g.to_string());

    // Tracklist : on extrait position et titre
    let tracks = resp["tracklist"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|t| {
                    let position = t["position"].as_str().unwrap_or("").to_string();
                    let title = t["title"].as_str()?.to_string();
                    Some(DiscogsTrack { position, title })
                })
                .collect()
        })
        .unwrap_or_default();

    // Image de couverture : préférer type "primary", sinon la première image
    let cover_url = resp["images"].as_array().and_then(|images| {
        images
            .iter()
            .find(|img| img["type"].as_str() == Some("primary"))
            .or_else(|| images.first())
            .and_then(|img| img["uri"].as_str())
            .map(|u| u.to_string())
    });

    Some(ReleaseDetails { artist, album, year, genre, tracks, cover_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_search_response() {
        // Réponse simulée avec "Boards of Canada - Geogaddi"
        let json = serde_json::json!({
            "results": [{
                "title": "Boards of Canada - Geogaddi",
                "year": "2002",
                "genre": ["Electronic"],
                "resource_url": "https://api.discogs.com/releases/12345"
            }]
        });

        let (info, resource_url) = parse_search_response(&json).unwrap();
        assert_eq!(info.artist, Some("Boards of Canada".into()));
        assert_eq!(info.album, Some("Geogaddi".into()));
        assert_eq!(info.year, Some(2002));
        assert_eq!(info.genre, Some("Electronic".into()));
        assert_eq!(
            resource_url,
            Some("https://api.discogs.com/releases/12345".into())
        );
    }

    #[test]
    fn test_parse_search_response_empty() {
        // Résultats vides : doit retourner None
        let json = serde_json::json!({ "results": [] });
        let result = parse_search_response(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_release_details() {
        // Réponse simulée avec tracklist et image primaire
        let json = serde_json::json!({
            "title": "Geogaddi",
            "year": 2002,
            "artists": [{ "name": "Boards of Canada" }],
            "genres": ["Electronic"],
            "tracklist": [
                { "position": "A1", "title": "Ready Let's Go" },
                { "position": "A2", "title": "Music Is Math" }
            ],
            "images": [
                { "type": "secondary", "uri": "https://img.discogs.com/secondary.jpg" },
                { "type": "primary", "uri": "https://img.discogs.com/primary.jpg" }
            ]
        });

        let details = parse_release_details(&json).unwrap();
        assert_eq!(details.artist, Some("Boards of Canada".into()));
        assert_eq!(details.album, Some("Geogaddi".into()));
        assert_eq!(details.year, Some(2002));
        assert_eq!(details.genre, Some("Electronic".into()));
        assert_eq!(details.tracks.len(), 2);
        assert_eq!(details.tracks[0].position, "A1");
        assert_eq!(details.tracks[1].title, "Music Is Math");
        // L'image primaire doit être préférée
        assert_eq!(
            details.cover_url,
            Some("https://img.discogs.com/primary.jpg".into())
        );
    }

    #[test]
    fn test_parse_release_details_fallback_image() {
        // Pas d'image primaire : utiliser la première image disponible
        let json = serde_json::json!({
            "title": "Geogaddi",
            "year": 2002,
            "artists": [{ "name": "Boards of Canada" }],
            "genres": ["Electronic"],
            "tracklist": [],
            "images": [
                { "type": "secondary", "uri": "https://img.discogs.com/first.jpg" },
                { "type": "secondary", "uri": "https://img.discogs.com/second.jpg" }
            ]
        });

        let details = parse_release_details(&json).unwrap();
        // Doit utiliser la première image en l'absence d'image primaire
        assert_eq!(
            details.cover_url,
            Some("https://img.discogs.com/first.jpg".into())
        );
    }
}
