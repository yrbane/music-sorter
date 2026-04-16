use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::blocking::Client;

use crate::models::TrackInfo;
use crate::rate_limiter::RateLimiter;

/// URL de base de l'API MusicBrainz
const BASE_URL: &str = "https://musicbrainz.org/ws/2";

/// Client HTTP pour interroger l'API MusicBrainz
pub struct MusicBrainzClient {
    client: Client,
    rate_limiter: Arc<RateLimiter>,
}

impl MusicBrainzClient {
    /// Crée un nouveau client MusicBrainz avec le limiteur de débit fourni
    pub fn new(rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("music-sorter/0.1.0 (https://github.com/music-sorter)")
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self { client, rate_limiter })
    }

    /// Recherche une piste par son identifiant MusicBrainz Recording ID
    /// Retourne (TrackInfo, release_id) si trouvé
    pub fn lookup_by_recording_id(
        &self,
        recording_id: &str,
    ) -> Result<Option<(TrackInfo, Option<String>)>> {
        let url = format!(
            "{}/recording/{}?inc=releases+artists+genres&fmt=json",
            BASE_URL, recording_id
        );

        self.rate_limiter.wait();
        let response = self.client.get(&url).send()?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = response.json()?;
        let release_id = extract_release_id(&json);

        match parse_recording_response(&json) {
            Some(info) => Ok(Some((info, release_id))),
            None => Ok(None),
        }
    }

    /// Recherche une piste par texte (artiste + titre)
    /// Retourne (TrackInfo, release_id) si un résultat avec score >= 80 est trouvé
    pub fn search_by_text(
        &self,
        artist: &str,
        title: &str,
    ) -> Result<Option<(TrackInfo, String)>> {
        let query = format!(
            "artist:\"{}\" AND recording:\"{}\"",
            artist, title
        );
        let encoded = url_encode(&query);
        let url = format!(
            "{}/recording/?query={}&fmt=json&limit=5",
            BASE_URL, encoded
        );

        self.rate_limiter.wait();
        let response = self.client.get(&url).send()?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = response.json()?;

        match parse_search_response(&json) {
            Some((info, release_id)) => Ok(Some((info, release_id))),
            None => Ok(None),
        }
    }
}

/// Encode une URL en remplaçant les caractères spéciaux
fn url_encode(s: &str) -> String {
    s.replace(' ', "%20")
        .replace('"', "%22")
        .replace(':', "%3A")
}

/// Extrait le release_id depuis une réponse JSON MusicBrainz
pub fn extract_release_id(resp: &serde_json::Value) -> Option<String> {
    resp["releases"][0]["id"].as_str().map(|s| s.to_string())
}

/// Parse une réponse de type recording (lookup par ID)
fn parse_recording_response(json: &serde_json::Value) -> Option<TrackInfo> {
    // Le titre est à la racine de la réponse
    let title = json["title"].as_str().map(|s| s.to_string());

    // L'artiste est dans artist-credit[0].name
    let artist = json["artist-credit"][0]["name"]
        .as_str()
        .map(|s| s.to_string());

    let release = &json["releases"][0];

    // L'album est dans releases[0].title
    let album = release["title"].as_str().map(|s| s.to_string());

    // L'année est dans les 4 premiers caractères de releases[0].date
    let year = release["date"]
        .as_str()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<u32>().ok());

    let media = &release["media"][0];

    // Le numéro de piste : track-offset + 1
    let track_number = media["track-offset"]
        .as_u64()
        .map(|n| n as u32 + 1);

    // Le total des pistes
    let total_tracks = media["track-count"]
        .as_u64()
        .map(|n| n as u32);

    // Le genre est dans genres[0].name
    let genre = json["genres"][0]["name"]
        .as_str()
        .map(|s| s.to_string());

    Some(TrackInfo {
        artist,
        album,
        title,
        year,
        track_number,
        total_tracks,
        genre,
        cover_art: None,
    })
}

/// Parse une réponse de type search (recherche textuelle)
/// Ne retourne un résultat que si le score est >= 80
fn parse_search_response(json: &serde_json::Value) -> Option<(TrackInfo, String)> {
    let recordings = json["recordings"].as_array()?;
    let first = recordings.first()?;

    // Filtre les résultats avec un score insuffisant
    let score = first["score"].as_u64()?;
    if score < 80 {
        return None;
    }

    let title = first["title"].as_str().map(|s| s.to_string());

    let artist = first["artist-credit"][0]["name"]
        .as_str()
        .map(|s| s.to_string());

    let release = &first["releases"][0];

    let release_id = release["id"].as_str()?.to_string();

    let album = release["title"].as_str().map(|s| s.to_string());

    let year = release["date"]
        .as_str()
        .and_then(|d| d.get(..4))
        .and_then(|y| y.parse::<u32>().ok());

    let media = &release["media"][0];

    let track_number = media["track-offset"]
        .as_u64()
        .map(|n| n as u32 + 1);

    let total_tracks = media["track-count"]
        .as_u64()
        .map(|n| n as u32);

    let info = TrackInfo {
        artist,
        album,
        title,
        year,
        track_number,
        total_tracks,
        genre: None,
        cover_art: None,
    };

    Some((info, release_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_recording_response() {
        let json = serde_json::json!({
            "title": "Music Is Math",
            "artist-credit": [{"name": "Boards of Canada"}],
            "releases": [{
                "title": "Geogaddi",
                "date": "2002-02-04",
                "id": "release-123",
                "media": [{"track-offset": 1, "track-count": 23}]
            }],
            "genres": [{"name": "electronic"}]
        });

        let info = parse_recording_response(&json).unwrap();
        assert_eq!(info.title, Some("Music Is Math".into()));
        assert_eq!(info.artist, Some("Boards of Canada".into()));
        assert_eq!(info.album, Some("Geogaddi".into()));
        assert_eq!(info.year, Some(2002));
        assert_eq!(info.track_number, Some(2));
        assert_eq!(info.total_tracks, Some(23));
        assert_eq!(info.genre, Some("electronic".into()));

        let release_id = extract_release_id(&json);
        assert_eq!(release_id, Some("release-123".into()));
    }

    #[test]
    fn test_parse_search_response_high_score() {
        let json = serde_json::json!({
            "recordings": [{
                "score": 95,
                "title": "Roygbiv",
                "artist-credit": [{"name": "Boards of Canada"}],
                "releases": [{
                    "id": "rel-456",
                    "title": "Music Has the Right to Children",
                    "date": "1998-04-20",
                    "media": [{"track-count": 18}]
                }]
            }]
        });

        let (info, release_id) = parse_search_response(&json).unwrap();
        assert_eq!(info.title, Some("Roygbiv".into()));
        assert_eq!(info.artist, Some("Boards of Canada".into()));
        assert_eq!(release_id, "rel-456");
    }

    #[test]
    fn test_parse_search_response_low_score() {
        let json = serde_json::json!({
            "recordings": [{"score": 30, "title": "Something", "artist-credit": [{"name": "Unknown"}], "releases": []}]
        });
        let result = parse_search_response(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_search_response_empty() {
        let json = serde_json::json!({"recordings": []});
        let result = parse_search_response(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_release_id() {
        let json = serde_json::json!({"releases": [{"id": "abc-123"}]});
        assert_eq!(extract_release_id(&json), Some("abc-123".into()));
    }
}
