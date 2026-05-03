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
    /// existing_album : album dans les tags locaux, pour choisir le bon release
    pub fn lookup_by_recording_id(
        &self,
        recording_id: &str,
        existing_album: Option<&str>,
    ) -> Result<Option<(TrackInfo, Option<String>)>> {
        let url = format!(
            "{}/recording/{}?inc=releases+artists+genres+release-groups&fmt=json",
            BASE_URL, recording_id
        );

        self.rate_limiter.wait();
        let response = self.client.get(&url).send()?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let json: serde_json::Value = response.json()?;
        let release_id = extract_release_id(&json);

        match parse_recording_response(&json, existing_album) {
            Some(info) => Ok(Some((info, release_id))),
            None => Ok(None),
        }
    }

    /// Recherche une piste par texte (artiste + titre)
    /// existing_album : album dans les tags locaux, pour choisir le bon release
    pub fn search_by_text(
        &self,
        artist: &str,
        title: &str,
        existing_album: Option<&str>,
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

        match parse_search_response(&json, existing_album) {
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

/// Extrait le release_id du meilleur release
pub fn extract_release_id(resp: &serde_json::Value) -> Option<String> {
    let releases = resp["releases"].as_array()?;
    let best = pick_best_release(releases, None)?;
    best["id"].as_str().map(|s| s.to_string())
}

/// Score un release pour choisir le meilleur parmi les résultats MusicBrainz
/// Préfère : Album officiel > EP > Single > tout le reste. Évite compilations et lives.
fn score_release(release: &serde_json::Value, existing_album: Option<&str>) -> i32 {
    let mut score: i32 = 0;

    // Correspondance avec l'album existant dans les tags → priorité absolue
    if let Some(existing) = existing_album {
        if let Some(title) = release["title"].as_str() {
            if title.to_lowercase() == existing.to_lowercase() {
                score += 100;
            }
        }
    }

    // Type du release-group
    let primary_type = release["release-group"]["primary-type"]
        .as_str()
        .unwrap_or("");

    match primary_type {
        "Album" => score += 10,
        "EP" => score += 5,
        "Single" => score += 2,
        _ => {}
    }

    // Pénaliser les compilations et les lives (secondary-types)
    if let Some(secondary) = release["release-group"]["secondary-types"].as_array() {
        for st in secondary {
            match st.as_str().unwrap_or("") {
                "Compilation" => score -= 20,
                "Live" => score -= 20,
                "DJ-mix" => score -= 15,
                "Remix" => score -= 10,
                _ => {}
            }
        }
    }

    // Aussi pénaliser si le primary-type est directement Compilation
    if primary_type == "Compilation" || primary_type == "Broadcast" {
        score -= 20;
    }

    // Préférer les releases officiels
    if release["status"].as_str() == Some("Official") {
        score += 3;
    }

    // Bonus si une date est présente
    if release["date"].as_str().is_some() {
        score += 1;
    }

    score
}

/// Choisit le meilleur release parmi une liste, en tenant compte de l'album existant
fn pick_best_release<'a>(
    releases: &'a [serde_json::Value],
    existing_album: Option<&str>,
) -> Option<&'a serde_json::Value> {
    releases
        .iter()
        .max_by_key(|r| score_release(r, existing_album))
}

/// Extrait les infos d'un release dans un TrackInfo partiel
fn extract_from_release(release: &serde_json::Value, title: Option<String>, artist: Option<String>, genre: Option<String>) -> TrackInfo {
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

    TrackInfo {
        artist,
        album,
        title,
        year,
        track_number,
        total_tracks,
        genre,
        cover_art: None,
    }
}

/// Parse une réponse de type recording (lookup par ID)
/// existing_album permet de préférer le release correspondant à l'album déjà dans les tags
fn parse_recording_response(json: &serde_json::Value, existing_album: Option<&str>) -> Option<TrackInfo> {
    let title = json["title"].as_str().map(|s| s.to_string());
    let artist = json["artist-credit"][0]["name"]
        .as_str()
        .map(|s| s.to_string());
    let genre = json["genres"][0]["name"]
        .as_str()
        .map(|s| s.to_string());

    let releases = json["releases"].as_array()?;
    let release = pick_best_release(releases, existing_album)?;

    Some(extract_from_release(release, title, artist, genre))
}

/// Parse une réponse de type search (recherche textuelle)
/// Ne retourne un résultat que si le score est >= 80
fn parse_search_response(json: &serde_json::Value, existing_album: Option<&str>) -> Option<(TrackInfo, String)> {
    let recordings = json["recordings"].as_array()?;
    let first = recordings.first()?;

    let score = first["score"].as_u64()?;
    if score < 80 {
        return None;
    }

    let title = first["title"].as_str().map(|s| s.to_string());
    let artist = first["artist-credit"][0]["name"]
        .as_str()
        .map(|s| s.to_string());

    let releases = first["releases"].as_array().filter(|a| !a.is_empty())?;
    let release = pick_best_release(releases, existing_album)?;
    let release_id = release["id"].as_str()?.to_string();

    let info = extract_from_release(release, title, artist, None);
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
                "release-group": {"primary-type": "Album"},
                "status": "Official",
                "media": [{"track-offset": 1, "track-count": 23}]
            }],
            "genres": [{"name": "electronic"}]
        });

        let info = parse_recording_response(&json, None).unwrap();
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
    fn test_pick_best_release_prefers_album_over_compilation() {
        let json = serde_json::json!({
            "title": "Roygbiv",
            "artist-credit": [{"name": "Boards of Canada"}],
            "releases": [
                {
                    "title": "Trip-Hop Classics",
                    "id": "comp-1",
                    "date": "2010-01-01",
                    "release-group": {"primary-type": "Album", "secondary-types": ["Compilation"]},
                    "status": "Official",
                    "media": [{"track-offset": 5, "track-count": 40}]
                },
                {
                    "title": "Music Has the Right to Children",
                    "id": "album-1",
                    "date": "1998-04-20",
                    "release-group": {"primary-type": "Album"},
                    "status": "Official",
                    "media": [{"track-offset": 9, "track-count": 17}]
                }
            ],
            "genres": [{"name": "electronic"}]
        });

        let info = parse_recording_response(&json, None).unwrap();
        assert_eq!(info.album, Some("Music Has the Right to Children".into()));
        assert_eq!(info.track_number, Some(10));
    }

    #[test]
    fn test_pick_best_release_prefers_existing_album() {
        let json = serde_json::json!({
            "title": "Roygbiv",
            "artist-credit": [{"name": "Boards of Canada"}],
            "releases": [
                {
                    "title": "Music Has the Right to Children",
                    "id": "album-1",
                    "date": "1998-04-20",
                    "release-group": {"primary-type": "Album"},
                    "status": "Official",
                    "media": [{"track-offset": 9, "track-count": 17}]
                },
                {
                    "title": "Warp20 (Chosen)",
                    "id": "comp-2",
                    "date": "2009-01-01",
                    "release-group": {"primary-type": "Album", "secondary-types": ["Compilation"]},
                    "status": "Official",
                    "media": [{"track-offset": 2, "track-count": 20}]
                }
            ],
            "genres": []
        });

        // Sans album existant → préfère l'album
        let info = parse_recording_response(&json, None).unwrap();
        assert_eq!(info.album, Some("Music Has the Right to Children".into()));

        // Avec album existant qui matche → le choisit même si c'est une compilation
        let info2 = parse_recording_response(&json, Some("Warp20 (Chosen)")).unwrap();
        assert_eq!(info2.album, Some("Warp20 (Chosen)".into()));
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
                    "release-group": {"primary-type": "Album"},
                    "status": "Official",
                    "media": [{"track-count": 18}]
                }]
            }]
        });

        let (info, release_id) = parse_search_response(&json, None).unwrap();
        assert_eq!(info.title, Some("Roygbiv".into()));
        assert_eq!(info.artist, Some("Boards of Canada".into()));
        assert_eq!(release_id, "rel-456");
    }

    #[test]
    fn test_parse_search_response_low_score() {
        let json = serde_json::json!({
            "recordings": [{"score": 30, "title": "Something", "artist-credit": [{"name": "Unknown"}], "releases": []}]
        });
        let result = parse_search_response(&json, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_search_response_empty() {
        let json = serde_json::json!({"recordings": []});
        let result = parse_search_response(&json, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_release_id() {
        let json = serde_json::json!({"releases": [{"id": "abc-123", "release-group": {"primary-type": "Album"}, "status": "Official"}]});
        assert_eq!(extract_release_id(&json), Some("abc-123".into()));
    }
}
