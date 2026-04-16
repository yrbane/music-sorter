use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct FingerprintResult {
    pub duration: u32,
    pub fingerprint: String,
}

pub fn is_fpcalc_available() -> bool {
    Command::new("fpcalc")
        .arg("-version")
        .output()
        .is_ok()
}

pub fn generate_fingerprint(path: &Path) -> Result<FingerprintResult> {
    let output = Command::new("fpcalc")
        .arg("-json")
        .arg(path.as_os_str())
        .output()
        .context("Impossible de lancer fpcalc")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("fpcalc a échoué : {}", stderr);
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Impossible de parser la sortie JSON de fpcalc")?;

    let duration = json["duration"]
        .as_f64()
        .map(|d| d as u32)
        .context("Durée manquante dans la sortie fpcalc")?;

    let fingerprint = json["fingerprint"]
        .as_str()
        .context("Fingerprint manquant dans la sortie fpcalc")?
        .to_string();

    Ok(FingerprintResult { duration, fingerprint })
}

pub fn lookup_acoustid(
    api_key: &str,
    fingerprint: &FingerprintResult,
) -> Result<Option<String>> {
    let url = format!(
        "https://api.acoustid.org/v2/lookup?client={}&duration={}&fingerprint={}&meta=recordings",
        api_key, fingerprint.duration, fingerprint.fingerprint
    );

    let resp: serde_json::Value = reqwest::blocking::get(&url)
        .context("Erreur réseau AcoustID")?
        .json()
        .context("Erreur parsing réponse AcoustID")?;

    let recording_id = parse_acoustid_response(&resp);
    Ok(recording_id)
}

fn parse_acoustid_response(resp: &serde_json::Value) -> Option<String> {
    resp["results"]
        .as_array()?
        .iter()
        .filter(|r| r["score"].as_f64().unwrap_or(0.0) > 0.5)
        .filter_map(|r| r["recordings"].as_array())
        .flatten()
        .filter_map(|rec| rec["id"].as_str())
        .next()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_acoustid_response_with_match() {
        let json = serde_json::json!({
            "status": "ok",
            "results": [{"score": 0.95, "recordings": [{"id": "abc-123-def"}]}]
        });
        let result = parse_acoustid_response(&json);
        assert_eq!(result, Some("abc-123-def".into()));
    }

    #[test]
    fn test_parse_acoustid_response_low_score() {
        let json = serde_json::json!({
            "status": "ok",
            "results": [{"score": 0.2, "recordings": [{"id": "abc-123"}]}]
        });
        let result = parse_acoustid_response(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_acoustid_response_empty() {
        let json = serde_json::json!({"status": "ok", "results": []});
        let result = parse_acoustid_response(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_acoustid_response_no_recordings() {
        let json = serde_json::json!({"status": "ok", "results": [{"score": 0.9}]});
        let result = parse_acoustid_response(&json);
        assert_eq!(result, None);
    }
}
