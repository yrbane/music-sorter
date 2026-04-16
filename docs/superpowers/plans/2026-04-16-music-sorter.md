# music-sorter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Exécutable Rust CLI qui scanne un dossier source, identifie les fichiers musicaux via tags + fingerprint + APIs (MusicBrainz, Discogs), enrichit les métadonnées (dont pochette), et copie les fichiers dans une arborescence organisée.

**Architecture:** Pipeline séquentiel par défaut (parallélisable via `--workers N`). Chaque fichier passe par : scan → lecture tags → fingerprint si nécessaire → enrichissement MusicBrainz/Discogs → écriture tags enrichis → copie vers destination. Modules découplés avec structures partagées via `models.rs`.

**Tech Stack:** Rust, clap, lofty, reqwest (blocking), serde/serde_json, toml, walkdir, colored, rayon, anyhow, dirs

---

## Structure des fichiers

```
src/
├── main.rs              # Point d'entrée, parsing CLI, orchestration globale
├── models.rs            # TrackInfo, ProcessResult, constantes
├── config.rs            # Lecture config TOML (~/.config/music-sorter/config.toml)
├── scanner.rs           # Scan récursif, filtrage par extension
├── tags.rs              # Lecture/écriture tags audio via lofty
├── fingerprint.rs       # Appel fpcalc + requête AcoustID
├── rate_limiter.rs      # Rate limiter partagé pour les APIs
├── musicbrainz.rs       # Client API MusicBrainz
├── discogs.rs           # Client API Discogs + pochettes Discogs
├── coverart.rs          # Pochettes Cover Art Archive
├── enricher.rs          # Pipeline d'enrichissement (orchestration)
├── organizer.rs         # Construction chemins, copie, gestion conflits
Cargo.toml
tests/
└── integration_test.rs  # Test d'intégration end-to-end
```

---

### Task 1: Initialisation du projet et modèles

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/models.rs`

- [ ] **Step 1: Initialiser le projet Cargo**

```bash
cd /home/seb/Dev/music-sorter
cargo init
```

- [ ] **Step 2: Configurer les dépendances dans Cargo.toml**

Remplacer le contenu de `Cargo.toml` :

```toml
[package]
name = "music-sorter"
version = "0.1.0"
edition = "2024"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
colored = "3"
dirs = "6"
lofty = "0.22"
rayon = "1.10"
reqwest = { version = "0.12", features = ["blocking", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
walkdir = "2"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Écrire les tests pour TrackInfo**

Remplacer `src/models.rs` :

```rust
/// Informations sur une piste audio
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub total_tracks: Option<u32>,
    pub genre: Option<String>,
    pub cover_art: Option<Vec<u8>>,
}

/// Résultat du traitement d'un fichier
#[derive(Debug)]
pub enum ProcessResult {
    Organized { from: std::path::PathBuf, to: std::path::PathBuf },
    ConflictResolved { path: std::path::PathBuf, kept_bitrate: u32 },
    Unsorted { from: std::path::PathBuf, to: std::path::PathBuf },
    Error { path: std::path::PathBuf, reason: String },
}

/// Extensions audio supportées
pub const SUPPORTED_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "m4a", "aac", "opus", "wma"];

impl TrackInfo {
    /// Fusionne les champs manquants de self avec ceux de other
    pub fn merge(&mut self, other: &TrackInfo) {
        if self.artist.is_none() {
            self.artist.clone_from(&other.artist);
        }
        if self.album.is_none() {
            self.album.clone_from(&other.album);
        }
        if self.title.is_none() {
            self.title.clone_from(&other.title);
        }
        if self.year.is_none() {
            self.year = other.year;
        }
        if self.track_number.is_none() {
            self.track_number = other.track_number;
        }
        if self.total_tracks.is_none() {
            self.total_tracks = other.total_tracks;
        }
        if self.genre.is_none() {
            self.genre.clone_from(&other.genre);
        }
        if self.cover_art.is_none() {
            self.cover_art.clone_from(&other.cover_art);
        }
    }

    /// Vérifie si on a assez d'info pour chercher dans les APIs
    pub fn has_minimum_for_search(&self) -> bool {
        self.artist.is_some() && self.title.is_some()
    }

    /// Vérifie si on a assez d'info pour organiser le fichier
    pub fn has_minimum_for_organization(&self) -> bool {
        self.artist.is_some() && self.album.is_some() && self.title.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_fills_missing_fields() {
        let mut base = TrackInfo {
            artist: Some("Boards of Canada".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        let other = TrackInfo {
            artist: Some("Autre artiste".into()),
            album: Some("Geogaddi".into()),
            year: Some(2002),
            track_number: Some(2),
            genre: Some("Electronic".into()),
            ..Default::default()
        };

        base.merge(&other);

        assert_eq!(base.artist, Some("Boards of Canada".into()));
        assert_eq!(base.album, Some("Geogaddi".into()));
        assert_eq!(base.title, Some("Music Is Math".into()));
        assert_eq!(base.year, Some(2002));
        assert_eq!(base.track_number, Some(2));
        assert_eq!(base.genre, Some("Electronic".into()));
    }

    #[test]
    fn test_merge_does_not_overwrite_existing() {
        let mut base = TrackInfo {
            artist: Some("BoC".into()),
            year: Some(2002),
            ..Default::default()
        };
        let other = TrackInfo {
            artist: Some("Autre".into()),
            year: Some(1999),
            ..Default::default()
        };

        base.merge(&other);

        assert_eq!(base.artist, Some("BoC".into()));
        assert_eq!(base.year, Some(2002));
    }

    #[test]
    fn test_has_minimum_for_search() {
        let empty = TrackInfo::default();
        assert!(!empty.has_minimum_for_search());

        let partial = TrackInfo {
            artist: Some("BoC".into()),
            ..Default::default()
        };
        assert!(!partial.has_minimum_for_search());

        let enough = TrackInfo {
            artist: Some("BoC".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        assert!(enough.has_minimum_for_search());
    }

    #[test]
    fn test_has_minimum_for_organization() {
        let partial = TrackInfo {
            artist: Some("BoC".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        assert!(!partial.has_minimum_for_organization());

        let enough = TrackInfo {
            artist: Some("BoC".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            ..Default::default()
        };
        assert!(enough.has_minimum_for_organization());
    }
}
```

- [ ] **Step 4: Mettre à jour main.rs pour déclarer le module**

```rust
mod models;

fn main() {
    println!("music-sorter v0.1.0");
}
```

- [ ] **Step 5: Lancer les tests**

```bash
cargo test -- --nocapture
```

Expected: 4 tests passent.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/
git commit -m "Initialisation du projet et modèle TrackInfo avec tests"
```

---

### Task 2: Configuration TOML

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire les tests pour config**

Créer `src/config.rs` :

```rust
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
```

- [ ] **Step 2: Déclarer le module dans main.rs**

```rust
mod config;
mod models;

fn main() {
    println!("music-sorter v0.1.0");
}
```

- [ ] **Step 3: Lancer les tests**

```bash
cargo test config -- --nocapture
```

Expected: 4 tests passent.

- [ ] **Step 4: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "Ajout du module config avec parsing TOML"
```

---

### Task 3: Parsing CLI avec clap

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le module CLI**

Créer `src/cli.rs` :

```rust
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
```

- [ ] **Step 2: Déclarer le module dans main.rs**

```rust
mod cli;
mod config;
mod models;

use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    println!("Source: {}", args.source);
    println!("Destination: {}", args.target);
    println!("Workers: {}", args.workers);
    println!("Move: {}", args.r#move);
}
```

- [ ] **Step 3: Lancer les tests**

```bash
cargo test cli -- --nocapture
```

Expected: 4 tests passent.

- [ ] **Step 4: Tester l'exécutable**

```bash
cargo run -- --help
cargo run -- --source /tmp --target /tmp/out --workers 2 --move
```

Expected: affichage de l'aide puis des paramètres.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "Ajout du parsing CLI avec clap"
```

---

### Task 4: Scanner récursif

**Files:**
- Create: `src/scanner.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire les tests pour le scanner**

Créer `src/scanner.rs` :

```rust
use crate::models::SUPPORTED_EXTENSIONS;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Scanne récursivement un dossier et retourne les fichiers audio supportés
pub fn scan(source: &Path) -> Vec<PathBuf> {
    WalkDir::new(source)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| is_supported_audio(e.path()))
        .map(|e| e.into_path())
        .collect()
}

fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_finds_audio_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("song.mp3"), b"fake").unwrap();
        fs::write(dir.path().join("album.flac"), b"fake").unwrap();
        fs::write(dir.path().join("readme.txt"), b"text").unwrap();
        fs::write(dir.path().join("image.jpg"), b"img").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_recursive() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("album");
        fs::create_dir(&sub).unwrap();
        fs::write(dir.path().join("root.mp3"), b"fake").unwrap();
        fs::write(sub.join("track.flac"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_all_supported_extensions() {
        let dir = TempDir::new().unwrap();
        for ext in SUPPORTED_EXTENSIONS {
            fs::write(dir.path().join(format!("file.{}", ext)), b"fake").unwrap();
        }
        // Fichier WAV ne doit pas être détecté
        fs::write(dir.path().join("file.wav"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), SUPPORTED_EXTENSIONS.len());
    }

    #[test]
    fn test_scan_case_insensitive_extension() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("song.MP3"), b"fake").unwrap();
        fs::write(dir.path().join("song.Flac"), b"fake").unwrap();

        let files = scan(dir.path());
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir = TempDir::new().unwrap();
        let files = scan(dir.path());
        assert!(files.is_empty());
    }
}
```

- [ ] **Step 2: Déclarer le module dans main.rs**

```rust
mod cli;
mod config;
mod models;
mod scanner;

use clap::Parser;

fn main() {
    let args = cli::Args::parse();
    let files = scanner::scan(&args.source_path());
    println!("{} fichiers audio trouvés", files.len());
    for f in &files {
        println!("  {}", f.display());
    }
}
```

- [ ] **Step 3: Lancer les tests**

```bash
cargo test scanner -- --nocapture
```

Expected: 5 tests passent.

- [ ] **Step 4: Tester sur le vrai dossier**

```bash
cargo run
```

Expected: liste des fichiers audio trouvés dans ~/Téléchargements/.

- [ ] **Step 5: Commit**

```bash
git add src/scanner.rs src/main.rs
git commit -m "Ajout du scanner récursif de fichiers audio"
```

---

### Task 5: Lecture des tags et bitrate

**Files:**
- Create: `src/tags.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le module de lecture des tags**

Créer `src/tags.rs` :

```rust
use crate::models::TrackInfo;
use anyhow::{Context, Result};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, Tag, TagExt};
use std::path::Path;

/// Lit les tags audio d'un fichier et retourne un TrackInfo
pub fn read_tags(path: &Path) -> Result<TrackInfo> {
    let tagged_file = Probe::open(path)
        .with_context(|| format!("Impossible d'ouvrir {}", path.display()))?
        .options(ParseOptions::new())
        .read()
        .with_context(|| format!("Impossible de lire les tags de {}", path.display()))?;

    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    let info = match tag {
        Some(tag) => TrackInfo {
            artist: tag.artist().map(|s| s.into_owned()),
            album: tag.album().map(|s| s.into_owned()),
            title: tag.title().map(|s| s.into_owned()),
            year: tag.year(),
            track_number: tag.track(),
            total_tracks: tag.track_total(),
            genre: tag.genre().map(|s| s.into_owned()),
            cover_art: tag.pictures().first().map(|p| p.data().to_vec()),
        },
        None => TrackInfo::default(),
    };

    Ok(info)
}

/// Retourne le bitrate audio en kbps
pub fn get_bitrate(path: &Path) -> Result<u32> {
    let tagged_file = Probe::open(path)?
        .options(ParseOptions::new())
        .read()?;

    let properties = tagged_file.properties();
    Ok(properties.audio_bitrate().unwrap_or(0))
}

/// Écrit les tags enrichis dans le fichier destination
pub fn write_tags(path: &Path, info: &TrackInfo) -> Result<()> {
    let mut tagged_file = Probe::open(path)?
        .options(ParseOptions::new())
        .read()
        .with_context(|| format!("Impossible de lire {}", path.display()))?;

    let tag = match tagged_file.primary_tag_mut() {
        Some(t) => t,
        None => {
            // Détermine le type de tag approprié selon le format
            let tag_type = tagged_file.primary_tag_type();
            tagged_file.insert_tag(Tag::new(tag_type));
            tagged_file.primary_tag_mut().unwrap()
        }
    };

    // Ne remplace que les champs manquants ou vides
    if tag.artist().is_none() {
        if let Some(ref artist) = info.artist {
            tag.set_artist(artist.clone());
        }
    }
    if tag.album().is_none() {
        if let Some(ref album) = info.album {
            tag.set_album(album.clone());
        }
    }
    if tag.title().is_none() {
        if let Some(ref title) = info.title {
            tag.set_title(title.clone());
        }
    }
    if tag.year().is_none() {
        if let Some(year) = info.year {
            tag.set_year(year);
        }
    }
    if tag.track().is_none() {
        if let Some(track) = info.track_number {
            tag.set_track(track);
        }
    }
    if tag.track_total().is_none() {
        if let Some(total) = info.total_tracks {
            tag.set_track_total(total);
        }
    }
    if tag.genre().is_none() {
        if let Some(ref genre) = info.genre {
            tag.set_genre(genre.clone());
        }
    }

    // Ajouter la pochette si absente
    if tag.pictures().is_empty() {
        if let Some(ref cover_data) = info.cover_art {
            let mime = if cover_data.starts_with(&[0xFF, 0xD8]) {
                MimeType::Jpeg
            } else {
                MimeType::Png
            };
            let picture = Picture::new_unchecked(
                PictureType::CoverFront,
                Some(mime),
                None,
                cover_data.clone(),
            );
            tag.push_picture(picture);
        }
    }

    tag.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Impossible d'écrire les tags dans {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_tags_nonexistent_file() {
        let result = read_tags(Path::new("/tmp/nonexistent_file_xyz.mp3"));
        assert!(result.is_err());
    }

    #[test]
    fn test_get_bitrate_nonexistent_file() {
        let result = get_bitrate(Path::new("/tmp/nonexistent_file_xyz.mp3"));
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Déclarer le module dans main.rs**

Ajouter `mod tags;` dans `src/main.rs`.

- [ ] **Step 3: Lancer les tests**

```bash
cargo test tags -- --nocapture
```

Expected: 2 tests passent.

- [ ] **Step 4: Tester sur un vrai fichier**

```bash
cargo run
```

Modifier temporairement main.rs pour afficher les tags du premier fichier trouvé. Vérifier que les tags sont lus correctement.

- [ ] **Step 5: Commit**

```bash
git add src/tags.rs src/main.rs
git commit -m "Ajout lecture/écriture des tags audio via lofty"
```

---

### Task 6: Fingerprint audio (fpcalc + AcoustID)

**Files:**
- Create: `src/fingerprint.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le module fingerprint**

Créer `src/fingerprint.rs` :

```rust
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Résultat du fingerprinting
#[derive(Debug, Clone)]
pub struct FingerprintResult {
    pub duration: u32,
    pub fingerprint: String,
}

/// Vérifie si fpcalc est installé
pub fn is_fpcalc_available() -> bool {
    Command::new("fpcalc")
        .arg("-version")
        .output()
        .is_ok()
}

/// Génère l'empreinte Chromaprint d'un fichier audio
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

    Ok(FingerprintResult {
        duration,
        fingerprint,
    })
}

/// Interroge AcoustID pour identifier un fichier à partir de son empreinte
/// Retourne le MusicBrainz Recording ID si trouvé
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

/// Parse la réponse AcoustID et extrait le recording ID le plus probable
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
            "results": [{
                "score": 0.95,
                "recordings": [{
                    "id": "abc-123-def"
                }]
            }]
        });

        let result = parse_acoustid_response(&json);
        assert_eq!(result, Some("abc-123-def".into()));
    }

    #[test]
    fn test_parse_acoustid_response_low_score() {
        let json = serde_json::json!({
            "status": "ok",
            "results": [{
                "score": 0.2,
                "recordings": [{
                    "id": "abc-123"
                }]
            }]
        });

        let result = parse_acoustid_response(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_acoustid_response_empty() {
        let json = serde_json::json!({
            "status": "ok",
            "results": []
        });

        let result = parse_acoustid_response(&json);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_acoustid_response_no_recordings() {
        let json = serde_json::json!({
            "status": "ok",
            "results": [{
                "score": 0.9
            }]
        });

        let result = parse_acoustid_response(&json);
        assert_eq!(result, None);
    }
}
```

- [ ] **Step 2: Déclarer le module dans main.rs**

Ajouter `mod fingerprint;` dans `src/main.rs`.

- [ ] **Step 3: Lancer les tests**

```bash
cargo test fingerprint -- --nocapture
```

Expected: 4 tests passent.

- [ ] **Step 4: Commit**

```bash
git add src/fingerprint.rs src/main.rs
git commit -m "Ajout du fingerprinting audio via fpcalc et AcoustID"
```

---

### Task 7: Rate limiter

**Files:**
- Create: `src/rate_limiter.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le rate limiter**

Créer `src/rate_limiter.rs` :

```rust
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Rate limiter thread-safe pour respecter les limites des APIs
pub struct RateLimiter {
    min_interval: Duration,
    last_request: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last_request: Mutex::new(Instant::now() - min_interval),
        }
    }

    /// Bloque jusqu'à ce que le délai minimum soit écoulé
    pub fn wait(&self) {
        let mut last = self.last_request.lock().unwrap();
        let elapsed = last.elapsed();
        if elapsed < self.min_interval {
            std::thread::sleep(self.min_interval - elapsed);
        }
        *last = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_first_call_no_wait() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        limiter.wait();
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn test_rate_limiter_enforces_delay() {
        let limiter = RateLimiter::new(Duration::from_millis(100));
        limiter.wait(); // Premier appel
        let start = Instant::now();
        limiter.wait(); // Doit attendre ~100ms
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(80));
    }
}
```

- [ ] **Step 2: Déclarer le module**

Ajouter `mod rate_limiter;` dans `src/main.rs`.

- [ ] **Step 3: Lancer les tests**

```bash
cargo test rate_limiter -- --nocapture
```

Expected: 2 tests passent.

- [ ] **Step 4: Commit**

```bash
git add src/rate_limiter.rs src/main.rs
git commit -m "Ajout du rate limiter thread-safe pour les APIs"
```

---

### Task 8: Client MusicBrainz

**Files:**
- Create: `src/musicbrainz.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le client MusicBrainz**

Créer `src/musicbrainz.rs` :

```rust
use crate::models::TrackInfo;
use crate::rate_limiter::RateLimiter;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

const MUSICBRAINZ_BASE: &str = "https://musicbrainz.org/ws/2";
const USER_AGENT: &str = "music-sorter/0.1.0 (https://github.com/music-sorter)";

pub struct MusicBrainzClient {
    client: reqwest::blocking::Client,
    rate_limiter: Arc<RateLimiter>,
}

impl MusicBrainzClient {
    pub fn new(rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            rate_limiter,
        })
    }

    /// Recherche un enregistrement par MusicBrainz Recording ID
    /// Retourne (TrackInfo, release_id) si trouvé
    pub fn lookup_by_recording_id(&self, recording_id: &str) -> Result<Option<(TrackInfo, Option<String>)>> {
        self.rate_limiter.wait();

        let url = format!(
            "{}/recording/{}?inc=releases+artists+genres&fmt=json",
            MUSICBRAINZ_BASE, recording_id
        );

        let resp: serde_json::Value = self
            .client
            .get(&url)
            .send()
            .context("Erreur réseau MusicBrainz")?
            .json()
            .context("Erreur parsing MusicBrainz")?;

        let release_id = extract_release_id(&resp);
        Ok(parse_recording_response(&resp).map(|info| (info, release_id)))
    }

    /// Recherche par texte (artiste + titre)
    pub fn search_by_text(&self, artist: &str, title: &str) -> Result<Option<(TrackInfo, String)>> {
        self.rate_limiter.wait();

        let query = format!(
            "artist:\"{}\" AND recording:\"{}\"",
            artist, title
        );
        let url = format!(
            "{}/recording/?query={}&fmt=json&limit=5",
            MUSICBRAINZ_BASE,
            urlencod(&query)
        );

        let resp: serde_json::Value = self
            .client
            .get(&url)
            .send()
            .context("Erreur réseau MusicBrainz")?
            .json()
            .context("Erreur parsing MusicBrainz")?;

        Ok(parse_search_response(&resp))
    }
}

/// Encode basique pour URL
fn urlencod(s: &str) -> String {
    s.replace(' ', "%20")
        .replace('"', "%22")
        .replace(':', "%3A")
}

/// Parse la réponse d'un lookup recording
fn parse_recording_response(resp: &serde_json::Value) -> Option<TrackInfo> {
    let title = resp["title"].as_str()?.to_string();

    let artist = resp["artist-credit"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|a| a["name"].as_str())
        .map(|s| s.to_string());

    // Prendre le premier release
    let release = resp["releases"].as_array().and_then(|arr| arr.first());

    let album = release.and_then(|r| r["title"].as_str()).map(|s| s.to_string());

    let year = release
        .and_then(|r| r["date"].as_str())
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<u32>().ok());

    let track_number = release
        .and_then(|r| r["media"].as_array())
        .and_then(|m| m.first())
        .and_then(|m| m["track-offset"].as_u64())
        .map(|n| n as u32 + 1);

    let total_tracks = release
        .and_then(|r| r["media"].as_array())
        .and_then(|m| m.first())
        .and_then(|m| m["track-count"].as_u64())
        .map(|n| n as u32);

    let genre = resp["genres"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|g| g["name"].as_str())
        .map(|s| s.to_string());

    Some(TrackInfo {
        artist,
        album,
        title: Some(title),
        year,
        track_number,
        total_tracks,
        genre,
        cover_art: None,
    })
}

/// Parse la réponse d'une recherche texte — retourne (TrackInfo, release_id)
fn parse_search_response(resp: &serde_json::Value) -> Option<(TrackInfo, String)> {
    let recording = resp["recordings"]
        .as_array()?
        .iter()
        .max_by(|a, b| {
            let sa = a["score"].as_u64().unwrap_or(0);
            let sb = b["score"].as_u64().unwrap_or(0);
            sa.cmp(&sb)
        })?;

    let score = recording["score"].as_u64().unwrap_or(0);
    if score < 80 {
        return None;
    }

    let title = recording["title"].as_str()?.to_string();

    let artist = recording["artist-credit"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|a| a["name"].as_str())
        .map(|s| s.to_string());

    let release = recording["releases"].as_array().and_then(|arr| arr.first());

    let release_id = release
        .and_then(|r| r["id"].as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    let album = release.and_then(|r| r["title"].as_str()).map(|s| s.to_string());

    let year = release
        .and_then(|r| r["date"].as_str())
        .and_then(|d| d.split('-').next())
        .and_then(|y| y.parse::<u32>().ok());

    let track_number = release
        .and_then(|r| r["media"].as_array())
        .and_then(|m| m.first())
        .and_then(|m| m["track"][0]["number"].as_str())
        .and_then(|n| n.parse::<u32>().ok());

    let total_tracks = release
        .and_then(|r| r["media"].as_array())
        .and_then(|m| m.first())
        .and_then(|m| m["track-count"].as_u64())
        .map(|n| n as u32);

    Some((
        TrackInfo {
            artist,
            album,
            title: Some(title),
            year,
            track_number,
            total_tracks,
            genre: None,
            cover_art: None,
        },
        release_id,
    ))
}

/// Extrait le release ID depuis une réponse de lookup recording
pub fn extract_release_id(resp: &serde_json::Value) -> Option<String> {
    resp["releases"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|r| r["id"].as_str())
        .map(|s| s.to_string())
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
            "recordings": [{
                "score": 30,
                "title": "Something",
                "artist-credit": [{"name": "Unknown"}],
                "releases": []
            }]
        });

        let result = parse_search_response(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_search_response_empty() {
        let json = serde_json::json!({
            "recordings": []
        });

        let result = parse_search_response(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_release_id() {
        let json = serde_json::json!({
            "releases": [{"id": "abc-123"}]
        });
        assert_eq!(extract_release_id(&json), Some("abc-123".into()));
    }
}
```

- [ ] **Step 2: Déclarer le module**

Ajouter `mod musicbrainz;` dans `src/main.rs`.

- [ ] **Step 3: Lancer les tests**

```bash
cargo test musicbrainz -- --nocapture
```

Expected: 5 tests passent.

- [ ] **Step 4: Commit**

```bash
git add src/musicbrainz.rs src/main.rs
git commit -m "Ajout du client MusicBrainz avec parsing des réponses"
```

---

### Task 9: Client Discogs + pochettes

**Files:**
- Create: `src/discogs.rs`
- Create: `src/coverart.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le client Discogs**

Créer `src/discogs.rs` :

```rust
use crate::models::TrackInfo;
use crate::rate_limiter::RateLimiter;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

const DISCOGS_BASE: &str = "https://api.discogs.com";

pub struct DiscogsClient {
    client: reqwest::blocking::Client,
    token: String,
    rate_limiter: Arc<RateLimiter>,
}

impl DiscogsClient {
    pub fn new(token: String, rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("music-sorter/0.1.0")
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            token,
            rate_limiter,
        })
    }

    /// Recherche un release sur Discogs par artiste et album
    pub fn search_release(&self, artist: &str, album: &str) -> Result<Option<(TrackInfo, Option<String>)>> {
        self.rate_limiter.wait();

        let url = format!(
            "{}/database/search?artist={}&release_title={}&type=release&token={}&per_page=5",
            DISCOGS_BASE,
            urlencod(artist),
            urlencod(album),
            self.token
        );

        let resp: serde_json::Value = self
            .client
            .get(&url)
            .send()
            .context("Erreur réseau Discogs")?
            .json()
            .context("Erreur parsing Discogs")?;

        Ok(parse_search_response(&resp))
    }

    /// Récupère les détails d'un release (tracks, année, genre)
    pub fn get_release_details(&self, resource_url: &str) -> Result<Option<ReleaseDetails>> {
        self.rate_limiter.wait();

        let url = format!("{}?token={}", resource_url, self.token);

        let resp: serde_json::Value = self
            .client
            .get(&url)
            .send()
            .context("Erreur réseau Discogs")?
            .json()
            .context("Erreur parsing Discogs")?;

        Ok(parse_release_details(&resp))
    }

    /// Télécharge une image depuis une URL
    pub fn fetch_image(&self, image_url: &str) -> Result<Vec<u8>> {
        self.rate_limiter.wait();

        let bytes = self
            .client
            .get(image_url)
            .header("Authorization", format!("Discogs token={}", self.token))
            .send()
            .context("Erreur téléchargement image Discogs")?
            .bytes()
            .context("Erreur lecture image")?;

        Ok(bytes.to_vec())
    }
}

#[derive(Debug)]
pub struct ReleaseDetails {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub tracks: Vec<DiscogsTrack>,
    pub cover_url: Option<String>,
}

#[derive(Debug)]
pub struct DiscogsTrack {
    pub position: String,
    pub title: String,
}

fn urlencod(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('?', "%3F")
}

fn parse_search_response(resp: &serde_json::Value) -> Option<(TrackInfo, Option<String>)> {
    let result = resp["results"].as_array()?.first()?;

    let title = result["title"].as_str()?.to_string();
    let cover_url = result["cover_image"].as_str().map(|s| s.to_string());
    let resource_url = result["resource_url"].as_str().map(|s| s.to_string());
    let year_str = result["year"].as_str().or_else(|| {
        result["year"].as_u64().map(|_| "").or(None)
    });
    let year = result["year"]
        .as_str()
        .and_then(|y| y.parse::<u32>().ok())
        .or_else(|| result["year"].as_u64().map(|y| y as u32));

    // Le titre Discogs est souvent "Artist - Album"
    let (artist, album) = if let Some((a, b)) = title.split_once(" - ") {
        (Some(a.to_string()), Some(b.to_string()))
    } else {
        (None, Some(title))
    };

    let genre = result["genre"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|g| g.as_str())
        .map(|s| s.to_string());

    Some((
        TrackInfo {
            artist,
            album,
            year,
            genre,
            cover_art: None,
            ..Default::default()
        },
        resource_url,
    ))
}

fn parse_release_details(resp: &serde_json::Value) -> Option<ReleaseDetails> {
    let artist = resp["artists"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|a| a["name"].as_str())
        .map(|s| s.to_string());

    let album = resp["title"].as_str().map(|s| s.to_string());
    let year = resp["year"].as_u64().map(|y| y as u32);

    let genre = resp["genres"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|g| g.as_str())
        .map(|s| s.to_string());

    let cover_url = resp["images"]
        .as_array()
        .and_then(|arr| arr.iter().find(|img| img["type"].as_str() == Some("primary")))
        .or_else(|| resp["images"].as_array().and_then(|arr| arr.first()))
        .and_then(|img| img["uri"].as_str())
        .map(|s| s.to_string());

    let tracks = resp["tracklist"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    Some(DiscogsTrack {
                        position: t["position"].as_str()?.to_string(),
                        title: t["title"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(ReleaseDetails {
        artist,
        album,
        year,
        genre,
        tracks,
        cover_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_search_response() {
        let json = serde_json::json!({
            "results": [{
                "title": "Boards of Canada - Geogaddi",
                "year": "2002",
                "cover_image": "https://img.discogs.com/cover.jpg",
                "resource_url": "https://api.discogs.com/releases/123",
                "genre": ["Electronic"]
            }]
        });

        let (info, resource_url) = parse_search_response(&json).unwrap();
        assert_eq!(info.artist, Some("Boards of Canada".into()));
        assert_eq!(info.album, Some("Geogaddi".into()));
        assert_eq!(info.year, Some(2002));
        assert_eq!(info.genre, Some("Electronic".into()));
        assert!(resource_url.is_some());
    }

    #[test]
    fn test_parse_search_response_empty() {
        let json = serde_json::json!({ "results": [] });
        assert!(parse_search_response(&json).is_none());
    }

    #[test]
    fn test_parse_release_details() {
        let json = serde_json::json!({
            "title": "Geogaddi",
            "year": 2002,
            "artists": [{"name": "Boards of Canada"}],
            "genres": ["Electronic"],
            "images": [{"type": "primary", "uri": "https://img.discogs.com/primary.jpg"}],
            "tracklist": [
                {"position": "1", "title": "Ready Lets Go"},
                {"position": "2", "title": "Music Is Math"}
            ]
        });

        let details = parse_release_details(&json).unwrap();
        assert_eq!(details.artist, Some("Boards of Canada".into()));
        assert_eq!(details.album, Some("Geogaddi".into()));
        assert_eq!(details.year, Some(2002));
        assert_eq!(details.tracks.len(), 2);
        assert_eq!(details.tracks[0].title, "Ready Lets Go");
        assert!(details.cover_url.is_some());
    }

    #[test]
    fn test_parse_release_details_fallback_image() {
        let json = serde_json::json!({
            "title": "Album",
            "year": 2020,
            "artists": [{"name": "Artist"}],
            "genres": [],
            "images": [{"type": "secondary", "uri": "https://img.discogs.com/sec.jpg"}],
            "tracklist": []
        });

        let details = parse_release_details(&json).unwrap();
        assert_eq!(details.cover_url, Some("https://img.discogs.com/sec.jpg".into()));
    }
}
```

- [ ] **Step 2: Écrire le module Cover Art Archive**

Créer `src/coverart.rs` :

```rust
use crate::rate_limiter::RateLimiter;
use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;

const COVERART_BASE: &str = "https://coverartarchive.org";

pub struct CoverArtClient {
    client: reqwest::blocking::Client,
    rate_limiter: Arc<RateLimiter>,
}

impl CoverArtClient {
    pub fn new(rate_limiter: Arc<RateLimiter>) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("music-sorter/0.1.0")
            .timeout(Duration::from_secs(15))
            .build()?;

        Ok(Self {
            client,
            rate_limiter,
        })
    }

    /// Récupère la pochette d'un release MusicBrainz
    pub fn fetch_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>> {
        self.rate_limiter.wait();

        let url = format!("{}/release/{}", COVERART_BASE, release_id);

        let resp = self.client.get(&url).send();

        match resp {
            Ok(r) if r.status().is_success() => {
                let json: serde_json::Value = r.json()
                    .context("Erreur parsing Cover Art Archive")?;

                let image_url = extract_front_image_url(&json);

                match image_url {
                    Some(url) => {
                        self.rate_limiter.wait();
                        let img_resp = self.client.get(&url).send()
                            .context("Erreur téléchargement pochette")?;
                        if img_resp.status().is_success() {
                            let bytes = img_resp.bytes()?.to_vec();
                            Ok(Some(bytes))
                        } else {
                            Ok(None)
                        }
                    }
                    None => Ok(None),
                }
            }
            Ok(_) => Ok(None), // 404 ou autre — pas de pochette
            Err(e) => Err(e.into()),
        }
    }
}

fn extract_front_image_url(resp: &serde_json::Value) -> Option<String> {
    resp["images"]
        .as_array()?
        .iter()
        .find(|img| img["front"].as_bool() == Some(true))
        .or_else(|| resp["images"].as_array()?.first())
        .and_then(|img| img["image"].as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_front_image_url() {
        let json = serde_json::json!({
            "images": [
                {"front": false, "image": "https://back.jpg"},
                {"front": true, "image": "https://front.jpg"}
            ]
        });

        assert_eq!(
            extract_front_image_url(&json),
            Some("https://front.jpg".into())
        );
    }

    #[test]
    fn test_extract_front_image_url_fallback() {
        let json = serde_json::json!({
            "images": [
                {"front": false, "image": "https://only.jpg"}
            ]
        });

        assert_eq!(
            extract_front_image_url(&json),
            Some("https://only.jpg".into())
        );
    }

    #[test]
    fn test_extract_front_image_url_empty() {
        let json = serde_json::json!({ "images": [] });
        assert_eq!(extract_front_image_url(&json), None);
    }
}
```

- [ ] **Step 3: Déclarer les modules**

Ajouter `mod discogs;` et `mod coverart;` dans `src/main.rs`.

- [ ] **Step 4: Lancer les tests**

```bash
cargo test discogs -- --nocapture
cargo test coverart -- --nocapture
```

Expected: 4 tests Discogs + 3 tests Cover Art passent.

- [ ] **Step 5: Commit**

```bash
git add src/discogs.rs src/coverart.rs src/main.rs
git commit -m "Ajout des clients Discogs et Cover Art Archive"
```

---

### Task 10: Organisateur de fichiers (chemins + copie + conflits)

**Files:**
- Create: `src/organizer.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le module organizer**

Créer `src/organizer.rs` :

```rust
use crate::models::TrackInfo;
use crate::tags;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Construit le chemin destination à partir des métadonnées
pub fn build_destination_path(
    target: &Path,
    info: &TrackInfo,
    original_path: &Path,
) -> PathBuf {
    if !info.has_minimum_for_organization() {
        let filename = original_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        return target.join("_unsorted").join(filename.as_ref());
    }

    let artist = info.artist.as_deref().unwrap_or("Unknown Artist");
    let album = info.album.as_deref().unwrap_or("Unknown Album");
    let title = info.title.as_deref().unwrap_or("Unknown");
    let ext = original_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");

    // Dossier : [artist] - [year] - [album]
    let folder = match info.year {
        Some(year) => format!("{} - {} - {}", artist, year, album),
        None => format!("{} - {}", artist, album),
    };

    // Fichier : [trackNb] - [trackName].[ext]
    let filename = match info.track_number {
        Some(num) => format!("{:02} - {}.{}", num, title, ext),
        None => format!("{}.{}", title, ext),
    };

    target
        .join(sanitize_filename(&folder))
        .join(sanitize_filename(&filename))
}

/// Remplace les caractères invalides pour le filesystem
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Copie un fichier vers la destination, gère les conflits de bitrate
/// Retourne le chemin final du fichier
pub fn copy_to_destination(
    source: &Path,
    destination: &Path,
) -> Result<CopyResult> {
    // Créer le dossier parent si nécessaire
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Impossible de créer {}", parent.display()))?;
    }

    if destination.exists() {
        // Conflit : comparer les bitrates
        let source_bitrate = tags::get_bitrate(source).unwrap_or(0);
        let dest_bitrate = tags::get_bitrate(destination).unwrap_or(0);

        if source_bitrate > dest_bitrate {
            // Le nouveau fichier est meilleur, remplacer
            std::fs::copy(source, destination)
                .with_context(|| format!("Impossible de copier vers {}", destination.display()))?;
            return Ok(CopyResult::Replaced { bitrate: source_bitrate });
        } else {
            // Le fichier existant est meilleur ou égal, skip
            return Ok(CopyResult::Skipped { existing_bitrate: dest_bitrate });
        }
    }

    std::fs::copy(source, destination)
        .with_context(|| format!("Impossible de copier vers {}", destination.display()))?;

    Ok(CopyResult::Copied)
}

/// Supprime le fichier source (mode --move)
pub fn remove_source(source: &Path) -> Result<()> {
    std::fs::remove_file(source)
        .with_context(|| format!("Impossible de supprimer {}", source.display()))
}

#[derive(Debug)]
pub enum CopyResult {
    Copied,
    Replaced { bitrate: u32 },
    Skipped { existing_bitrate: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_build_destination_full_info() {
        let target = Path::new("/home/user/Music");
        let info = TrackInfo {
            artist: Some("Boards of Canada".into()),
            album: Some("Geogaddi".into()),
            title: Some("Music Is Math".into()),
            year: Some(2002),
            track_number: Some(2),
            ..Default::default()
        };

        let path = build_destination_path(target, &info, Path::new("song.flac"));
        assert_eq!(
            path,
            PathBuf::from("/home/user/Music/Boards of Canada - 2002 - Geogaddi/02 - Music Is Math.flac")
        );
    }

    #[test]
    fn test_build_destination_no_year() {
        let target = Path::new("/home/user/Music");
        let info = TrackInfo {
            artist: Some("BoC".into()),
            album: Some("Geogaddi".into()),
            title: Some("Track".into()),
            ..Default::default()
        };

        let path = build_destination_path(target, &info, Path::new("t.mp3"));
        assert_eq!(
            path,
            PathBuf::from("/home/user/Music/BoC - Geogaddi/Track.mp3")
        );
    }

    #[test]
    fn test_build_destination_no_track_number() {
        let target = Path::new("/tmp/music");
        let info = TrackInfo {
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            title: Some("Title".into()),
            year: Some(2020),
            ..Default::default()
        };

        let path = build_destination_path(target, &info, Path::new("f.ogg"));
        assert_eq!(
            path,
            PathBuf::from("/tmp/music/Artist - 2020 - Album/Title.ogg")
        );
    }

    #[test]
    fn test_build_destination_unsorted() {
        let target = Path::new("/tmp/music");
        let info = TrackInfo::default();

        let path = build_destination_path(target, &info, Path::new("unknown.mp3"));
        assert_eq!(
            path,
            PathBuf::from("/tmp/music/_unsorted/unknown.mp3")
        );
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("AC/DC"), "AC_DC");
        assert_eq!(sanitize_filename("a:b*c?d"), "a_b_c_d");
        assert_eq!(sanitize_filename("normal name"), "normal name");
    }

    #[test]
    fn test_copy_to_destination_new_file() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        let dest = dir.path().join("sub").join("dest.txt");
        std::fs::write(&source, b"content").unwrap();

        let result = copy_to_destination(&source, &dest).unwrap();
        assert!(matches!(result, CopyResult::Copied));
        assert!(dest.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "content");
    }
}
```

- [ ] **Step 2: Déclarer le module**

Ajouter `mod organizer;` dans `src/main.rs`.

- [ ] **Step 3: Lancer les tests**

```bash
cargo test organizer -- --nocapture
```

Expected: 6 tests passent.

- [ ] **Step 4: Commit**

```bash
git add src/organizer.rs src/main.rs
git commit -m "Ajout de l'organisateur : chemins, copie, conflits bitrate"
```

---

### Task 11: Pipeline d'enrichissement

**Files:**
- Create: `src/enricher.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le pipeline d'enrichissement**

Créer `src/enricher.rs` :

```rust
use crate::config::Config;
use crate::coverart::CoverArtClient;
use crate::discogs::DiscogsClient;
use crate::fingerprint;
use crate::models::TrackInfo;
use crate::musicbrainz::MusicBrainzClient;
use crate::rate_limiter::RateLimiter;
use crate::tags;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub struct Enricher {
    musicbrainz: MusicBrainzClient,
    discogs: Option<DiscogsClient>,
    coverart: CoverArtClient,
    fpcalc_available: bool,
    acoustid_api_key: Option<String>,
}

impl Enricher {
    pub fn new(config: &Config) -> Result<Self> {
        let mb_limiter = Arc::new(RateLimiter::new(Duration::from_millis(1100)));
        let discogs_limiter = Arc::new(RateLimiter::new(Duration::from_millis(1000)));
        let coverart_limiter = Arc::new(RateLimiter::new(Duration::from_millis(1100)));

        let musicbrainz = MusicBrainzClient::new(mb_limiter)?;

        let discogs = config.discogs_token.as_ref().map(|token| {
            DiscogsClient::new(token.clone(), discogs_limiter)
        }).transpose()?;

        let coverart = CoverArtClient::new(coverart_limiter)?;

        let fpcalc_available = fingerprint::is_fpcalc_available();
        if !fpcalc_available {
            eprintln!("\x1b[33m⚠ fpcalc non trouvé — fingerprinting désactivé. Installer : sudo pacman -S chromaprint\x1b[0m");
        }

        Ok(Self {
            musicbrainz,
            discogs,
            coverart,
            fpcalc_available,
            acoustid_api_key: config.acoustid_api_key.clone(),
        })
    }

    /// Enrichit les métadonnées d'un fichier audio
    /// Retourne le TrackInfo le plus complet possible
    pub fn enrich(&self, path: &Path) -> Result<TrackInfo> {
        // Étape 1 : Lire les tags existants
        let mut info = tags::read_tags(path).unwrap_or_default();

        // Étape 2 : Si tags insuffisants, tenter le fingerprint
        let mut release_id: Option<String> = None;

        if !info.has_minimum_for_search() && self.fpcalc_available {
            if let Some(ref api_key) = self.acoustid_api_key {
                if let Ok(fp) = fingerprint::generate_fingerprint(path) {
                    if let Ok(Some(recording_id)) = fingerprint::lookup_acoustid(api_key, &fp) {
                        if let Ok(Some((mb_info, rid))) = self.musicbrainz.lookup_by_recording_id(&recording_id) {
                            info.merge(&mb_info);
                            release_id = rid;
                        }
                    }
                }
            }
        }

        // Étape 3 : Recherche MusicBrainz par texte si on a artiste+titre et pas encore de release_id
        if info.has_minimum_for_search() && release_id.is_none() {
            let artist = info.artist.as_deref().unwrap();
            let title = info.title.as_deref().unwrap();

            if let Ok(Some((mb_info, rid))) = self.musicbrainz.search_by_text(artist, title) {
                info.merge(&mb_info);
                release_id = Some(rid);
            }
        }

        // Étape 4 : Pochette via Cover Art Archive
        if info.cover_art.is_none() {
            if let Some(ref rid) = release_id {
                if let Ok(Some(cover)) = self.coverart.fetch_cover(rid) {
                    info.cover_art = Some(cover);
                }
            }
        }

        // Étape 5 : Fallback Discogs si info incomplète ou pochette manquante
        if let Some(ref discogs) = self.discogs {
            let needs_discogs = !info.has_minimum_for_organization() || info.cover_art.is_none();

            if needs_discogs {
                let artist = info.artist.as_deref().unwrap_or("");
                let album = info.album.as_deref().unwrap_or("");

                if !artist.is_empty() && !album.is_empty() {
                    if let Ok(Some((discogs_info, resource_url))) = discogs.search_release(artist, album) {
                        info.merge(&discogs_info);

                        // Détails du release pour pochette et genre
                        if let Some(ref url) = resource_url {
                            if let Ok(Some(details)) = discogs.get_release_details(url) {
                                if info.genre.is_none() {
                                    info.genre = details.genre;
                                }
                                // Pochette Discogs si toujours manquante
                                if info.cover_art.is_none() {
                                    if let Some(ref cover_url) = details.cover_url {
                                        if let Ok(cover_data) = discogs.fetch_image(cover_url) {
                                            info.cover_art = Some(cover_data);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(info)
    }
}
```

- [ ] **Step 2: Déclarer le module**

Ajouter `mod enricher;` dans `src/main.rs`.

- [ ] **Step 3: Vérifier la compilation**

```bash
cargo build
```

Expected: compilation réussie.

- [ ] **Step 4: Commit**

```bash
git add src/enricher.rs src/main.rs
git commit -m "Ajout du pipeline d'enrichissement MusicBrainz/Discogs/AcoustID"
```

---

### Task 12: Intégration main avec logs colorés

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le main complet**

Remplacer `src/main.rs` :

```rust
mod cli;
mod config;
mod coverart;
mod discogs;
mod enricher;
mod fingerprint;
mod models;
mod musicbrainz;
mod organizer;
mod rate_limiter;
mod scanner;
mod tags;

use crate::enricher::Enricher;
use crate::models::ProcessResult;
use anyhow::Result;
use clap::Parser;
use colored::*;

fn main() -> Result<()> {
    let args = cli::Args::parse();
    let config = config::Config::load()?;

    let source = args.source_path();
    let target = args.target_path();

    println!(
        "{} {}",
        "Source:".bold(),
        source.display()
    );
    println!(
        "{} {}",
        "Destination:".bold(),
        target.display()
    );

    // Vérifier que la source existe
    if !source.exists() {
        eprintln!(
            "{} Le dossier source n'existe pas : {}",
            "✗".red().bold(),
            source.display()
        );
        std::process::exit(1);
    }

    // Scanner les fichiers
    let files = scanner::scan(&source);
    println!(
        "\n{} fichiers audio trouvés\n",
        files.len().to_string().bold()
    );

    if files.is_empty() {
        println!("Rien à faire.");
        return Ok(());
    }

    // Créer l'enricher
    let enricher = Enricher::new(&config)?;

    // Traitement
    let results: Vec<ProcessResult> = if args.workers > 1 {
        process_parallel(&files, &enricher, &target, args.r#move, args.workers)
    } else {
        process_sequential(&files, &enricher, &target, args.r#move)
    };

    // Récapitulatif
    print_summary(&results);

    Ok(())
}

fn process_sequential(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    target: &std::path::Path,
    do_move: bool,
) -> Vec<ProcessResult> {
    files
        .iter()
        .map(|file| process_file(file, enricher, target, do_move))
        .collect()
}

fn process_parallel(
    files: &[std::path::PathBuf],
    enricher: &Enricher,
    target: &std::path::Path,
    do_move: bool,
    workers: usize,
) -> Vec<ProcessResult> {
    use rayon::prelude::*;

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .expect("Impossible de créer le pool de threads");

    pool.install(|| {
        files
            .par_iter()
            .map(|file| process_file(file, enricher, target, do_move))
            .collect()
    })
}

fn process_file(
    file: &std::path::Path,
    enricher: &Enricher,
    target: &std::path::Path,
    do_move: bool,
) -> ProcessResult {
    let filename = file
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    // Enrichir les métadonnées
    let info = match enricher.enrich(file) {
        Ok(info) => info,
        Err(e) => {
            eprintln!(
                "  {} {} — {}",
                "✗".red().bold(),
                filename,
                e
            );
            return ProcessResult::Error {
                path: file.to_path_buf(),
                reason: e.to_string(),
            };
        }
    };

    // Construire le chemin destination
    let dest = organizer::build_destination_path(target, &info, file);

    // Copier le fichier
    match organizer::copy_to_destination(file, &dest) {
        Ok(organizer::CopyResult::Copied) => {
            // Écrire les tags enrichis sur la copie
            if let Err(e) = tags::write_tags(&dest, &info) {
                eprintln!(
                    "  {} {} — Copié mais erreur tags : {}",
                    "⚠".yellow().bold(),
                    filename,
                    e
                );
            }

            let is_unsorted = dest.to_string_lossy().contains("_unsorted");
            if is_unsorted {
                println!(
                    "  {} {} → _unsorted/",
                    "⚠".yellow().bold(),
                    filename
                );
                if do_move {
                    let _ = organizer::remove_source(file);
                }
                ProcessResult::Unsorted {
                    from: file.to_path_buf(),
                    to: dest,
                }
            } else {
                println!(
                    "  {} {} → {}",
                    "✓".green().bold(),
                    filename,
                    dest.display()
                );
                if do_move {
                    let _ = organizer::remove_source(file);
                }
                ProcessResult::Organized {
                    from: file.to_path_buf(),
                    to: dest,
                }
            }
        }
        Ok(organizer::CopyResult::Replaced { bitrate }) => {
            if let Err(e) = tags::write_tags(&dest, &info) {
                eprintln!("  ⚠ Erreur écriture tags après remplacement : {}", e);
            }
            println!(
                "  {} {} — remplacé ({}kbps)",
                "↑".cyan().bold(),
                filename,
                bitrate
            );
            if do_move {
                let _ = organizer::remove_source(file);
            }
            ProcessResult::ConflictResolved {
                path: dest,
                kept_bitrate: bitrate,
            }
        }
        Ok(organizer::CopyResult::Skipped { existing_bitrate }) => {
            println!(
                "  {} {} — ignoré (existant : {}kbps)",
                "—".dimmed(),
                filename,
                existing_bitrate
            );
            ProcessResult::Organized {
                from: file.to_path_buf(),
                to: dest,
            }
        }
        Err(e) => {
            eprintln!(
                "  {} {} — {}",
                "✗".red().bold(),
                filename,
                e
            );
            ProcessResult::Error {
                path: file.to_path_buf(),
                reason: e.to_string(),
            }
        }
    }
}

fn print_summary(results: &[ProcessResult]) {
    let organized = results
        .iter()
        .filter(|r| matches!(r, ProcessResult::Organized { .. }))
        .count();
    let conflicts = results
        .iter()
        .filter(|r| matches!(r, ProcessResult::ConflictResolved { .. }))
        .count();
    let unsorted = results
        .iter()
        .filter(|r| matches!(r, ProcessResult::Unsorted { .. }))
        .count();
    let errors = results
        .iter()
        .filter(|r| matches!(r, ProcessResult::Error { .. }))
        .count();

    println!("\n{}", "Traitement terminé :".bold());
    if organized > 0 {
        println!(
            "  {} {} fichiers organisés",
            "✓".green().bold(),
            organized
        );
    }
    if conflicts > 0 {
        println!(
            "  {} {} conflits résolus (meilleur bitrate conservé)",
            "↑".cyan().bold(),
            conflicts
        );
    }
    if unsorted > 0 {
        println!(
            "  {} {} fichiers non identifiés → _unsorted/",
            "⚠".yellow().bold(),
            unsorted
        );
    }
    if errors > 0 {
        println!(
            "  {} {} erreurs",
            "✗".red().bold(),
            errors
        );
    }
}
```

- [ ] **Step 2: Vérifier la compilation**

```bash
cargo build
```

Expected: compilation réussie.

- [ ] **Step 3: Test manuel**

Créer un fichier de config minimal :

```bash
mkdir -p ~/.config/music-sorter
echo 'discogs_token = ""' > ~/.config/music-sorter/config.toml
echo 'acoustid_api_key = ""' >> ~/.config/music-sorter/config.toml
```

Tester avec un dossier temporaire :

```bash
cargo run -- --source ~/Téléchargements --target /tmp/music-test
```

Expected: scan des fichiers, tentatives d'enrichissement, copie vers /tmp/music-test/.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "Intégration complète : pipeline, logs colorés, récapitulatif"
```

---

### Task 13: Test d'intégration end-to-end

**Files:**
- Create: `tests/integration_test.rs`

- [ ] **Step 1: Écrire le test d'intégration**

Créer `tests/integration_test.rs` :

```rust
use std::path::Path;
use std::process::Command;

#[test]
fn test_help_flag() {
    let output = Command::new("cargo")
        .args(["run", "--", "--help"])
        .output()
        .expect("Impossible de lancer music-sorter");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("music-sorter"));
    assert!(stdout.contains("--source"));
    assert!(stdout.contains("--target"));
    assert!(stdout.contains("--workers"));
    assert!(stdout.contains("--move"));
}

#[test]
fn test_empty_source_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = tempfile::TempDir::new().unwrap();

    let output = Command::new("cargo")
        .args([
            "run",
            "--",
            "--source",
            dir.path().to_str().unwrap(),
            "--target",
            target.path().to_str().unwrap(),
        ])
        .output()
        .expect("Impossible de lancer music-sorter");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0 fichiers audio"));
}

#[test]
fn test_nonexistent_source() {
    let output = Command::new("cargo")
        .args([
            "run",
            "--",
            "--source",
            "/tmp/nonexistent_music_sorter_test_xyz",
        ])
        .output()
        .expect("Impossible de lancer music-sorter");

    assert!(!output.status.success());
}
```

- [ ] **Step 2: Builder le projet et lancer les tests**

```bash
cargo build
cargo test --test integration_test -- --nocapture
```

Expected: 3 tests passent.

- [ ] **Step 3: Lancer tous les tests du projet**

```bash
cargo test -- --nocapture
```

Expected: tous les tests passent (unitaires + intégration).

- [ ] **Step 4: Commit**

```bash
git add tests/
git commit -m "Ajout des tests d'intégration end-to-end"
```

---

### Task 14: Gestion du retry avec backoff

**Files:**
- Create: `src/retry.rs`
- Modify: `src/musicbrainz.rs` (utiliser retry)
- Modify: `src/discogs.rs` (utiliser retry)
- Modify: `src/coverart.rs` (utiliser retry)
- Modify: `src/fingerprint.rs` (utiliser retry)
- Modify: `src/main.rs`

- [ ] **Step 1: Écrire le module retry**

Créer `src/retry.rs` :

```rust
use std::thread;
use std::time::Duration;

/// Exécute une closure avec retry et backoff exponentiel
/// 3 tentatives : 1s, 2s, 4s
pub fn with_retry<T, E, F>(operation: F) -> Result<T, E>
where
    F: Fn() -> Result<T, E>,
    E: std::fmt::Display,
{
    let delays = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
    ];

    let mut last_err = None;

    for (i, delay) in delays.iter().enumerate() {
        match operation() {
            Ok(val) => return Ok(val),
            Err(e) => {
                eprintln!(
                    "  Tentative {}/3 échouée : {}. Retry dans {}s...",
                    i + 1,
                    e,
                    delay.as_secs()
                );
                last_err = Some(e);
                thread::sleep(*delay);
            }
        }
    }

    // Dernière tentative
    match operation() {
        Ok(val) => Ok(val),
        Err(e) => {
            last_err = Some(e);
            Err(last_err.unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_retry_succeeds_first_try() {
        let result = with_retry(|| Ok::<_, String>(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_retry_succeeds_after_failures() {
        let attempts = AtomicU32::new(0);
        let result = with_retry(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(format!("tentative {}", n))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result.unwrap(), 42);
    }
}
```

- [ ] **Step 2: Déclarer le module**

Ajouter `mod retry;` dans `src/main.rs`.

- [ ] **Step 3: Lancer les tests**

```bash
cargo test retry -- --nocapture
```

Expected: 2 tests passent.

- [ ] **Step 4: Commit**

```bash
git add src/retry.rs src/main.rs
git commit -m "Ajout du retry avec backoff exponentiel pour les appels réseau"
```

---

### Task 15: Build release et test final

**Files:**
- Aucun nouveau fichier

- [ ] **Step 1: Build en mode release**

```bash
cargo build --release
```

Expected: compilation réussie, binaire dans `target/release/music-sorter`.

- [ ] **Step 2: Lancer tous les tests**

```bash
cargo test
```

Expected: tous les tests passent.

- [ ] **Step 3: Configurer les clés API**

Éditer `~/.config/music-sorter/config.toml` avec les vrais tokens :

```toml
discogs_token = "ton_vrai_token_discogs"
acoustid_api_key = "ta_vraie_clé_acoustid"
```

- [ ] **Step 4: Test réel sur ~/Téléchargements**

```bash
./target/release/music-sorter --target /tmp/music-test
```

Vérifier :
- Les fichiers sont copiés dans la bonne arborescence
- Les métadonnées sont enrichies (vérifier avec `ffprobe` ou un lecteur audio)
- Les pochettes sont embarquées
- Les fichiers non identifiés sont dans `_unsorted/`
- Le récapitulatif final est correct

- [ ] **Step 5: Commit final**

```bash
git add -A
git commit -m "Build release et tests finaux validés"
```
