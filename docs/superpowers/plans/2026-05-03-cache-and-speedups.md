# Cache SQLite et accélérations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accélérer drastiquement les re-runs (de minutes à secondes) via un cache SQLite persistant et plusieurs micro-optimisations, tout en réglant le bug de casse des dossiers d'artistes.

**Architecture:** Une base SQLite unique `<target>/.music-sorter.db` agit comme cache persistant pour : index des fichiers traités, fingerprints fpcalc, réponses API (MusicBrainz/AcoustID/Discogs/CoverArt), pochettes binaires, et registre canonique des artistes. Le pipeline d'enrichissement consulte le cache avant tout appel coûteux et y écrit les résultats. Album-grouping pré-traite les fichiers d'un même dossier source pour partager une seule lookup d'album.

**Tech Stack:** Rust, `rusqlite` (bundled SQLite), schéma WAL, `sha2` pour content-hash, reqwest avec gzip/HTTP2, `reflink-copy` pour CoW.

---

## Vue d'ensemble des fichiers

**Nouveaux modules :**
- `src/cache.rs` — wrapper SQLite (ouverture, schéma, requêtes typées)
- `src/cache_keys.rs` — calcul des clés (content_hash, api_key)
- `src/artist_registry.rs` — résolution canonique des noms d'artistes
- `src/album_group.rs` — regroupement des fichiers par dossier source

**Modules modifiés :**
- `Cargo.toml` — ajout deps
- `src/enricher.rs` — branchement cache à toutes les étapes
- `src/musicbrainz.rs` — accepte `&Cache`
- `src/discogs.rs` — accepte `&Cache`
- `src/coverart.rs` — accepte `&Cache`
- `src/fingerprint.rs` — accepte `&Cache`
- `src/organizer.rs` — copie via `reflink`, registre artistes
- `src/main.rs` — instancie `Cache`, propage, album-grouping
- `src/config.rs` — option `cache_enabled` + `cache_ttl_days`

---

## Task 1 : Ajout des dépendances et squelette du module cache

**Files:**
- Modify: `Cargo.toml`
- Create: `src/cache.rs`
- Modify: `src/main.rs`

- [ ] **Step 1 : Ajouter les dépendances**

```toml
# dans [dependencies]
rusqlite = { version = "0.32", features = ["bundled"] }
sha2 = "0.10"
hex = "0.4"
reflink-copy = "0.1"
```

- [ ] **Step 2 : Créer le module cache vide**

```rust
// src/cache.rs
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub struct Cache {
    conn: Connection,
}

impl Cache {
    pub fn open(target: &Path) -> Result<Self> {
        let db_path = target.join(".music-sorter.db");
        std::fs::create_dir_all(target)?;
        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(_conn: &Connection) -> Result<()> {
        Ok(())
    }
}
```

- [ ] **Step 3 : Déclarer le module dans main.rs**

```rust
// src/main.rs (en haut, à côté des autres mod)
mod cache;
```

- [ ] **Step 4 : Compiler**

Run: `cargo build`
Expected: succès, warnings tolérés.

- [ ] **Step 5 : Commit**

```bash
git add Cargo.toml Cargo.lock src/cache.rs src/main.rs
git commit -m "Ajout dépendances et squelette du module cache SQLite"
```

---

## Task 2 : Schéma SQLite complet et tests d'init

**Files:**
- Modify: `src/cache.rs`

- [ ] **Step 1 : Écrire le test d'init**

```rust
// dans src/cache.rs, dans #[cfg(test)] mod tests
use tempfile::tempdir;

#[test]
fn test_open_creates_db_and_tables() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    let names: Vec<String> = cache
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(names.contains(&"processed_files".into()));
    assert!(names.contains(&"fingerprint_cache".into()));
    assert!(names.contains(&"api_cache".into()));
    assert!(names.contains(&"cover_cache".into()));
    assert!(names.contains(&"artists".into()));
}
```

- [ ] **Step 2 : Lancer le test (échoue)**

Run: `cargo test cache::tests::test_open_creates_db_and_tables`
Expected: FAIL — tables absentes.

- [ ] **Step 3 : Implémenter le schéma**

```rust
fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS processed_files (
            source_path TEXT PRIMARY KEY,
            mtime       INTEGER NOT NULL,
            size        INTEGER NOT NULL,
            dest_path   TEXT,
            status      TEXT NOT NULL,
            last_seen   INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS fingerprint_cache (
            content_hash TEXT PRIMARY KEY,
            chromaprint  TEXT NOT NULL,
            duration     INTEGER NOT NULL,
            created_at   INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS api_cache (
            endpoint   TEXT NOT NULL,
            cache_key  TEXT NOT NULL,
            response   TEXT NOT NULL,
            fetched_at INTEGER NOT NULL,
            PRIMARY KEY (endpoint, cache_key)
        );
        CREATE TABLE IF NOT EXISTS cover_cache (
            release_id TEXT PRIMARY KEY,
            image      BLOB NOT NULL,
            fetched_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS artists (
            canonical_lower TEXT PRIMARY KEY,
            canonical       TEXT NOT NULL,
            mbid            TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_processed_status ON processed_files(status);
    "#)?;
    Ok(())
}
```

- [ ] **Step 4 : Lancer le test (passe)**

Run: `cargo test cache::tests::test_open_creates_db_and_tables`
Expected: PASS.

- [ ] **Step 5 : Commit**

```bash
git add src/cache.rs
git commit -m "Schéma SQLite (processed_files, fingerprint, api, cover, artists)"
```

---

## Task 3 : Calcul du content-hash et clés API

**Files:**
- Create: `src/cache_keys.rs`
- Modify: `src/main.rs`

- [ ] **Step 1 : Écrire les tests**

```rust
// src/cache_keys.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_content_hash_stable() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        let h1 = content_hash(f.path()).unwrap();
        let h2 = content_hash(f.path()).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_content_hash_differs_per_content() {
        let mut a = NamedTempFile::new().unwrap();
        a.write_all(b"foo").unwrap();
        let mut b = NamedTempFile::new().unwrap();
        b.write_all(b"bar").unwrap();
        assert_ne!(content_hash(a.path()).unwrap(), content_hash(b.path()).unwrap());
    }

    #[test]
    fn test_api_key_normalizes_case_and_whitespace() {
        assert_eq!(api_key("  Hello   World  "), api_key("hello world"));
    }
}
```

- [ ] **Step 2 : Lancer les tests (échouent)**

Run: `cargo test cache_keys`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
// src/cache_keys.rs
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Hash SHA-256 du contenu binaire d'un fichier (en hex)
pub fn content_hash(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Normalise une chaîne pour servir de clé de cache API
pub fn api_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}
```

- [ ] **Step 4 : Déclarer dans main.rs**

```rust
mod cache_keys;
```

- [ ] **Step 5 : Lancer les tests (passent)**

Run: `cargo test cache_keys`
Expected: PASS (3 tests).

- [ ] **Step 6 : Commit**

```bash
git add src/cache_keys.rs src/main.rs
git commit -m "Calcul du content-hash et clés API normalisées"
```

---

## Task 4 : Index des fichiers traités (skip total)

**Files:**
- Modify: `src/cache.rs`

- [ ] **Step 1 : Écrire les tests**

```rust
// dans src/cache.rs tests
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn test_processed_lookup_miss() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    let result = cache.lookup_processed("/foo/bar.mp3", 100, 200).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_processed_record_and_lookup_hit() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.record_processed("/foo/bar.mp3", 100, 200, Some("/dest"), "organized").unwrap();
    let r = cache.lookup_processed("/foo/bar.mp3", 100, 200).unwrap();
    assert_eq!(r, Some(("organized".into(), Some("/dest".into()))));
}

#[test]
fn test_processed_miss_when_mtime_changed() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.record_processed("/foo.mp3", 100, 200, Some("/dest"), "organized").unwrap();
    let r = cache.lookup_processed("/foo.mp3", 999, 200).unwrap();
    assert!(r.is_none());
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test cache::tests::test_processed`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
// dans impl Cache
pub fn lookup_processed(
    &self,
    source_path: &str,
    mtime: i64,
    size: i64,
) -> Result<Option<(String, Option<String>)>> {
    let mut stmt = self.conn.prepare(
        "SELECT status, dest_path FROM processed_files
         WHERE source_path = ?1 AND mtime = ?2 AND size = ?3"
    )?;
    let mut rows = stmt.query(rusqlite::params![source_path, mtime, size])?;
    if let Some(row) = rows.next()? {
        let status: String = row.get(0)?;
        let dest: Option<String> = row.get(1)?;
        Ok(Some((status, dest)))
    } else {
        Ok(None)
    }
}

pub fn record_processed(
    &self,
    source_path: &str,
    mtime: i64,
    size: i64,
    dest_path: Option<&str>,
    status: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    self.conn.execute(
        "INSERT OR REPLACE INTO processed_files
         (source_path, mtime, size, dest_path, status, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![source_path, mtime, size, dest_path, status, now],
    )?;
    Ok(())
}
```

- [ ] **Step 4 : Lancer (passe)**

Run: `cargo test cache::tests::test_processed`
Expected: PASS.

- [ ] **Step 5 : Commit**

```bash
git add src/cache.rs
git commit -m "Cache des fichiers traités (lookup et record)"
```

---

## Task 5 : Cache fingerprint AcoustID/fpcalc

**Files:**
- Modify: `src/cache.rs`
- Modify: `src/fingerprint.rs`

- [ ] **Step 1 : Tests cache fingerprint**

```rust
#[test]
fn test_fingerprint_lookup_miss_then_hit() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    assert!(cache.lookup_fingerprint("hash1").unwrap().is_none());
    cache.record_fingerprint("hash1", "FPDATA", 200).unwrap();
    let r = cache.lookup_fingerprint("hash1").unwrap();
    assert_eq!(r, Some(("FPDATA".into(), 200)));
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test cache::tests::test_fingerprint`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
pub fn lookup_fingerprint(&self, content_hash: &str) -> Result<Option<(String, i64)>> {
    let mut stmt = self.conn.prepare(
        "SELECT chromaprint, duration FROM fingerprint_cache WHERE content_hash = ?1"
    )?;
    let mut rows = stmt.query(rusqlite::params![content_hash])?;
    if let Some(row) = rows.next()? {
        Ok(Some((row.get(0)?, row.get(1)?)))
    } else {
        Ok(None)
    }
}

pub fn record_fingerprint(&self, content_hash: &str, fp: &str, duration: i64) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    self.conn.execute(
        "INSERT OR REPLACE INTO fingerprint_cache (content_hash, chromaprint, duration, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![content_hash, fp, duration, now],
    )?;
    Ok(())
}
```

- [ ] **Step 4 : Brancher dans fingerprint.rs**

```rust
// nouveau wrapper public
pub fn generate_or_cached(
    cache: &crate::cache::Cache,
    path: &std::path::Path,
) -> anyhow::Result<crate::fingerprint::Fingerprint> {
    let hash = crate::cache_keys::content_hash(path)?;
    if let Some((fp, duration)) = cache.lookup_fingerprint(&hash)? {
        return Ok(Fingerprint { fingerprint: fp, duration: duration as u32 });
    }
    let fp = generate_fingerprint(path)?;
    cache.record_fingerprint(&hash, &fp.fingerprint, fp.duration as i64)?;
    Ok(fp)
}
```

> Note : si `Fingerprint` n'a pas cette structure exacte, adapter au type effectif retourné par `generate_fingerprint` (probablement `(String, u32)`).

- [ ] **Step 5 : Lancer (passe)**

Run: `cargo test`
Expected: tous les tests passent.

- [ ] **Step 6 : Commit**

```bash
git add src/cache.rs src/fingerprint.rs
git commit -m "Cache fingerprint indexé sur content-hash SHA-256"
```

---

## Task 6 : Cache générique des réponses API

**Files:**
- Modify: `src/cache.rs`

- [ ] **Step 1 : Tests**

```rust
#[test]
fn test_api_cache_miss_then_hit_with_ttl() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    assert!(cache.lookup_api("musicbrainz", "key1", 86400).unwrap().is_none());
    cache.record_api("musicbrainz", "key1", "{\"a\":1}").unwrap();
    let r = cache.lookup_api("musicbrainz", "key1", 86400).unwrap();
    assert_eq!(r, Some("{\"a\":1}".into()));
}

#[test]
fn test_api_cache_expires() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.conn.execute(
        "INSERT INTO api_cache VALUES ('mb', 'k', '{}', 0)", [],
    ).unwrap();
    let r = cache.lookup_api("mb", "k", 86400).unwrap();
    assert!(r.is_none(), "entrée datée de 1970 doit être expirée");
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test cache::tests::test_api_cache`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
pub fn lookup_api(&self, endpoint: &str, key: &str, ttl_secs: i64) -> Result<Option<String>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    let cutoff = now - ttl_secs;
    let mut stmt = self.conn.prepare(
        "SELECT response FROM api_cache
         WHERE endpoint = ?1 AND cache_key = ?2 AND fetched_at >= ?3"
    )?;
    let mut rows = stmt.query(rusqlite::params![endpoint, key, cutoff])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn record_api(&self, endpoint: &str, key: &str, response: &str) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    self.conn.execute(
        "INSERT OR REPLACE INTO api_cache (endpoint, cache_key, response, fetched_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![endpoint, key, response, now],
    )?;
    Ok(())
}
```

- [ ] **Step 4 : Lancer (passe)**

Run: `cargo test cache::tests::test_api_cache`
Expected: PASS.

- [ ] **Step 5 : Commit**

```bash
git add src/cache.rs
git commit -m "Cache API générique avec TTL configurable"
```

---

## Task 7 : Brancher le cache sur MusicBrainz

**Files:**
- Modify: `src/musicbrainz.rs`

- [ ] **Step 1 : Test d'intégration cache MB (avec fixture JSON)**

```rust
// dans src/musicbrainz.rs tests
#[test]
fn test_mb_uses_cache_on_second_call() {
    let dir = tempfile::tempdir().unwrap();
    let cache = crate::cache::Cache::open(dir.path()).unwrap();
    let json = r#"{"recordings":[{"score":95,"title":"X","artist-credit":[{"name":"Y"}],
        "releases":[{"id":"r1","title":"Z","release-group":{"primary-type":"Album"},
        "status":"Official","media":[{"track-count":10}]}]}]}"#;
    cache.record_api("mb_search", &crate::cache_keys::api_key("Y|X|"), json).unwrap();

    // Avec cache, on doit retrouver le résultat sans appel HTTP
    let result = MusicBrainzClient::search_by_text_cached(&cache, "Y", "X", None).unwrap();
    assert!(result.is_some());
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test musicbrainz::tests::test_mb_uses_cache`
Expected: FAIL — fonction inexistante.

- [ ] **Step 3 : Refactoriser MusicBrainzClient**

```rust
// dans src/musicbrainz.rs, ajouter une variante "_cached"
impl MusicBrainzClient {
    pub fn search_by_text_cached(
        cache: &crate::cache::Cache,
        artist: &str,
        title: &str,
        existing_album: Option<&str>,
    ) -> Result<Option<(TrackInfo, String)>> {
        let key = crate::cache_keys::api_key(&format!("{}|{}|{}", artist, title, existing_album.unwrap_or("")));
        if let Some(json_str) = cache.lookup_api("mb_search", &key, 30 * 86400)? {
            let json: serde_json::Value = serde_json::from_str(&json_str)?;
            return Ok(parse_search_response(&json, existing_album));
        }
        Ok(None)
    }

    pub fn search_by_text_with_cache(
        &self,
        cache: &crate::cache::Cache,
        artist: &str,
        title: &str,
        existing_album: Option<&str>,
    ) -> Result<Option<(TrackInfo, String)>> {
        let key = crate::cache_keys::api_key(&format!("{}|{}|{}", artist, title, existing_album.unwrap_or("")));
        if let Some(json_str) = cache.lookup_api("mb_search", &key, 30 * 86400)? {
            let json: serde_json::Value = serde_json::from_str(&json_str)?;
            return Ok(parse_search_response(&json, existing_album));
        }
        // Sinon HTTP
        let query = format!("artist:\"{}\" AND recording:\"{}\"", artist, title);
        let encoded = url_encode(&query);
        let url = format!("{}/recording/?query={}&fmt=json&limit=5", BASE_URL, encoded);
        self.rate_limiter.wait();
        let response = self.client.get(&url).send()?;
        if !response.status().is_success() { return Ok(None); }
        let body = response.text()?;
        cache.record_api("mb_search", &key, &body)?;
        let json: serde_json::Value = serde_json::from_str(&body)?;
        Ok(parse_search_response(&json, existing_album))
    }
}
```

> Faire le même traitement pour `lookup_by_recording_id` avec endpoint `mb_lookup`.

- [ ] **Step 4 : Lancer (passe)**

Run: `cargo test musicbrainz`
Expected: PASS.

- [ ] **Step 5 : Commit**

```bash
git add src/musicbrainz.rs
git commit -m "Cache MusicBrainz lookup et search (TTL 30j)"
```

---

## Task 8 : Cache Discogs et Cover Art

**Files:**
- Modify: `src/discogs.rs`
- Modify: `src/coverart.rs`
- Modify: `src/cache.rs`

- [ ] **Step 1 : Test cover binary cache**

```rust
// src/cache.rs tests
#[test]
fn test_cover_cache_roundtrip() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    assert!(cache.lookup_cover("rid").unwrap().is_none());
    cache.record_cover("rid", &[1, 2, 3, 4]).unwrap();
    assert_eq!(cache.lookup_cover("rid").unwrap(), Some(vec![1,2,3,4]));
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test cache::tests::test_cover_cache`
Expected: FAIL.

- [ ] **Step 3 : Implémenter cover cache**

```rust
// src/cache.rs
pub fn lookup_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>> {
    let mut stmt = self.conn.prepare("SELECT image FROM cover_cache WHERE release_id = ?1")?;
    let mut rows = stmt.query(rusqlite::params![release_id])?;
    if let Some(row) = rows.next()? { Ok(Some(row.get(0)?)) } else { Ok(None) }
}

pub fn record_cover(&self, release_id: &str, image: &[u8]) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    self.conn.execute(
        "INSERT OR REPLACE INTO cover_cache (release_id, image, fetched_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![release_id, image, now],
    )?;
    Ok(())
}
```

- [ ] **Step 4 : Wrappers cover et discogs**

```rust
// src/coverart.rs : ajouter
impl CoverArtClient {
    pub fn fetch_cover_cached(
        &self,
        cache: &crate::cache::Cache,
        release_id: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(b) = cache.lookup_cover(release_id)? { return Ok(Some(b)); }
        match self.fetch_cover(release_id)? {
            Some(bytes) => {
                cache.record_cover(release_id, &bytes)?;
                Ok(Some(bytes))
            }
            None => Ok(None),
        }
    }
}
```

```rust
// src/discogs.rs : suivre le même schéma pour search_release et get_release_details
// endpoint = "discogs_search" / "discogs_release", TTL 90j
```

- [ ] **Step 5 : Lancer**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6 : Commit**

```bash
git add src/cache.rs src/coverart.rs src/discogs.rs
git commit -m "Cache Discogs (search/details) et binaire Cover Art"
```

---

## Task 9 : Registre canonique des artistes

**Files:**
- Create: `src/artist_registry.rs`
- Modify: `src/cache.rs`
- Modify: `src/main.rs`

- [ ] **Step 1 : Tests cache artists**

```rust
// src/cache.rs tests
#[test]
fn test_artist_register_and_lookup_case_insensitive() {
    let dir = tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.record_artist("Boards of Canada", None).unwrap();
    let r = cache.lookup_artist("BOARDS OF CANADA").unwrap();
    assert_eq!(r, Some("Boards of Canada".into()));
    let r2 = cache.lookup_artist("boards of canada").unwrap();
    assert_eq!(r2, Some("Boards of Canada".into()));
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test cache::tests::test_artist_register`
Expected: FAIL.

- [ ] **Step 3 : Implémenter cache artists**

```rust
// src/cache.rs
pub fn lookup_artist(&self, name: &str) -> Result<Option<String>> {
    let key = name.to_lowercase();
    let mut stmt = self.conn.prepare("SELECT canonical FROM artists WHERE canonical_lower = ?1")?;
    let mut rows = stmt.query(rusqlite::params![key])?;
    if let Some(row) = rows.next()? { Ok(Some(row.get(0)?)) } else { Ok(None) }
}

pub fn record_artist(&self, canonical: &str, mbid: Option<&str>) -> Result<()> {
    let key = canonical.to_lowercase();
    self.conn.execute(
        "INSERT OR IGNORE INTO artists (canonical_lower, canonical, mbid) VALUES (?1, ?2, ?3)",
        rusqlite::params![key, canonical, mbid],
    )?;
    Ok(())
}
```

- [ ] **Step 4 : Module artist_registry**

```rust
// src/artist_registry.rs
use crate::cache::Cache;

/// Renvoie le nom canonique pour cet artiste, en l'enregistrant s'il est nouveau
pub fn canonicalize(cache: &Cache, name: &str) -> anyhow::Result<String> {
    if let Some(existing) = cache.lookup_artist(name)? {
        return Ok(existing);
    }
    cache.record_artist(name, None)?;
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_first_seen_wins() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let a = canonicalize(&cache, "Boards Of Canada").unwrap();
        assert_eq!(a, "Boards Of Canada");
        let b = canonicalize(&cache, "boards of canada").unwrap();
        assert_eq!(b, "Boards Of Canada");
    }
}
```

- [ ] **Step 5 : Déclarer et lancer**

Run: `cargo test artist_registry`
Expected: PASS.

- [ ] **Step 6 : Commit**

```bash
git add src/cache.rs src/artist_registry.rs src/main.rs
git commit -m "Registre canonique des artistes (résout le bug de casse)"
```

---

## Task 10 : Intégrer le cache dans Enricher (skip si déjà traité)

**Files:**
- Modify: `src/enricher.rs`
- Modify: `src/main.rs`

- [ ] **Step 1 : Modifier Enricher pour porter une référence cache**

```rust
// src/enricher.rs
pub struct Enricher {
    musicbrainz: MusicBrainzClient,
    discogs: Option<DiscogsClient>,
    coverart: CoverArtClient,
    fpcalc_available: bool,
    acoustid_api_key: Option<String>,
    cache: std::sync::Arc<crate::cache::Cache>,
}

impl Enricher {
    pub fn new(config: &Config, cache: std::sync::Arc<crate::cache::Cache>) -> Result<Self> {
        // ... existant inchangé sauf l'ajout du champ cache
    }
}
```

> `Cache` doit être thread-safe : envelopper la `Connection` dans un `Mutex` à l'intérieur du struct (ajouter dans Task 1bis si nécessaire). En pratique, plusieurs `Connection` partagées via `Arc<Mutex<Connection>>` ou un pool simple. Étape suivante.

- [ ] **Step 2 : Rendre Cache thread-safe**

Modifier `Cache` pour utiliser `Mutex<Connection>` :

```rust
use std::sync::Mutex;

pub struct Cache {
    conn: Mutex<Connection>,
}
```

Et adapter chaque méthode : `let conn = self.conn.lock().unwrap();` puis utiliser `conn` au lieu de `self.conn`.

- [ ] **Step 3 : Refaire passer les tests**

Run: `cargo test cache`
Expected: PASS.

- [ ] **Step 4 : Brancher le skip dans main.rs**

```rust
// src/main.rs, fn process_file, AVANT enricher.enrich :
let metadata = std::fs::metadata(file)?;
let mtime = metadata.modified()?
    .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
let size = metadata.len() as i64;

if let Ok(Some((status, dest_opt))) = cache.lookup_processed(&file.to_string_lossy(), mtime, size) {
    if status == "organized" {
        if let Some(d) = dest_opt {
            return ProcessResult::Organized {
                from: file.to_path_buf(),
                to: std::path::PathBuf::from(d),
            };
        }
    }
}
```

> Adapter la signature de `process_file` pour recevoir `&Cache` (Arc).

- [ ] **Step 5 : Enregistrer après traitement**

Après chaque branche de succès dans `process_file`, ajouter :
```rust
let _ = cache.record_processed(
    &file.to_string_lossy(),
    mtime, size,
    Some(&dest.to_string_lossy()),
    "organized", // ou "unsorted", "error"
);
```

- [ ] **Step 6 : Tester manuellement**

```bash
cargo build --release
./target/release/music-sorter --source /tmp/test_music --target /tmp/out
# 2e run :
time ./target/release/music-sorter --source /tmp/test_music --target /tmp/out
```
Expected : 2e run beaucoup plus rapide.

- [ ] **Step 7 : Commit**

```bash
git add src/enricher.rs src/main.rs src/cache.rs
git commit -m "Intégration cache : skip total des fichiers déjà organisés"
```

---

## Task 11 : Album-grouping (lookup partagé par dossier source)

**Files:**
- Create: `src/album_group.rs`
- Modify: `src/main.rs`

- [ ] **Step 1 : Tests groupement**

```rust
// src/album_group.rs
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_group_by_parent_dir() {
        let files = vec![
            PathBuf::from("/a/album1/1.mp3"),
            PathBuf::from("/a/album1/2.mp3"),
            PathBuf::from("/a/album2/1.mp3"),
        ];
        let groups = group_by_parent(&files);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[&PathBuf::from("/a/album1")].len(), 2);
        assert_eq!(groups[&PathBuf::from("/a/album2")].len(), 1);
    }
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test album_group`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
// src/album_group.rs
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn group_by_parent(files: &[PathBuf]) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut map: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for f in files {
        if let Some(parent) = f.parent() {
            map.entry(parent.to_path_buf()).or_default().push(f.clone());
        }
    }
    map
}
```

- [ ] **Step 4 : Brancher dans main.rs**

Avant le traitement parallèle, regrouper et traiter dossier par dossier. Pour chaque groupe : exécuter `enrich` sur le 1er fichier, puis pour les suivants — si `(artist, album)` correspond — réutiliser la cover et les infos d'album.

> Implémentation détaillée : créer `Enricher::enrich_album_aware(files: &[PathBuf]) -> Vec<(PathBuf, TrackInfo)>` qui partage la cover déjà téléchargée entre tous les fichiers du groupe.

- [ ] **Step 5 : Lancer**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6 : Commit**

```bash
git add src/album_group.rs src/main.rs
git commit -m "Album-grouping : partage cover et release_id par dossier source"
```

---

## Task 12 : reflink CoW pour la copie

**Files:**
- Modify: `src/organizer.rs`

- [ ] **Step 1 : Test reflink avec fallback**

```rust
// src/organizer.rs tests
#[test]
fn test_copy_falls_back_to_regular_copy() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("a.mp3");
    std::fs::write(&src, b"data").unwrap();
    let dst = dir.path().join("b.mp3");
    copy_with_reflink(&src, &dst).unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), b"data");
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test test_copy_falls_back_to_regular_copy`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
// src/organizer.rs
fn copy_with_reflink(source: &Path, destination: &Path) -> Result<()> {
    match reflink_copy::reflink(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(source, destination)?;
            Ok(())
        }
    }
}
```

Remplacer les `std::fs::copy(source, destination)?` dans `copy_to_destination` par `copy_with_reflink(source, destination)?`.

- [ ] **Step 4 : Lancer**

Run: `cargo test organizer`
Expected: PASS.

- [ ] **Step 5 : Commit**

```bash
git add src/organizer.rs
git commit -m "Copie via reflink CoW (btrfs/xfs/zfs) avec fallback fs::copy"
```

---

## Task 13 : Optimisations HTTP (gzip, HTTP/2, connection pool)

**Files:**
- Modify: `src/musicbrainz.rs`
- Modify: `src/discogs.rs`
- Modify: `src/coverart.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1 : Activer gzip dans reqwest**

```toml
reqwest = { version = "0.12", features = ["blocking", "json", "gzip", "http2"] }
```

- [ ] **Step 2 : Configurer Client builder**

Dans chaque client (mb, discogs, coverart) :

```rust
let client = Client::builder()
    .user_agent("music-sorter/0.1.0 (https://github.com/music-sorter)")
    .timeout(Duration::from_secs(10))
    .gzip(true)
    .pool_max_idle_per_host(4)
    .build()?;
```

- [ ] **Step 3 : Build et test**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4 : Commit**

```bash
git add Cargo.toml Cargo.lock src/musicbrainz.rs src/discogs.rs src/coverart.rs
git commit -m "HTTP : gzip, http2, connection pool"
```

---

## Task 14 : Skip si tags déjà complets (zéro API call)

**Files:**
- Modify: `src/enricher.rs`
- Modify: `src/models.rs`

- [ ] **Step 1 : Tester `has_full_metadata`**

```rust
// src/models.rs tests
#[test]
fn test_has_full_metadata_requires_all_fields() {
    let mut info = TrackInfo {
        artist: Some("a".into()), album: Some("b".into()), title: Some("c".into()),
        year: Some(2020), track_number: Some(1), genre: Some("g".into()),
        cover_art: Some(vec![1]), ..Default::default()
    };
    assert!(info.has_full_metadata());
    info.cover_art = None;
    assert!(!info.has_full_metadata());
}
```

- [ ] **Step 2 : Lancer (échec)**

Run: `cargo test test_has_full_metadata`
Expected: FAIL.

- [ ] **Step 3 : Implémenter**

```rust
// src/models.rs impl TrackInfo
pub fn has_full_metadata(&self) -> bool {
    self.artist.is_some() && self.album.is_some() && self.title.is_some()
        && self.year.is_some() && self.track_number.is_some() && self.cover_art.is_some()
}
```

- [ ] **Step 4 : Court-circuiter dans enrich**

```rust
// src/enricher.rs, début de enrich(), après lecture des tags :
if info.has_full_metadata() {
    return Ok(info);
}
```

- [ ] **Step 5 : Tester**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6 : Commit**

```bash
git add src/models.rs src/enricher.rs
git commit -m "Skip total des appels API si les tags sont déjà complets"
```

---

## Task 15 : Configuration TTL et activation cache

**Files:**
- Modify: `src/config.rs`
- Modify: `src/main.rs`

- [ ] **Step 1 : Ajouter options config**

```rust
// src/config.rs
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
}
```

- [ ] **Step 2 : Test parsing**

```rust
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
```

- [ ] **Step 3 : Lancer (passe directement)**

Run: `cargo test config`
Expected: PASS.

- [ ] **Step 4 : Branchement main.rs**

```rust
let cache = if config.cache_enabled.unwrap_or(true) {
    Some(std::sync::Arc::new(cache::Cache::open(&args.target)?))
} else {
    None
};
```

Adapter `Enricher::new` pour accepter `Option<Arc<Cache>>`. Si `None`, comportement original (pas de cache).

- [ ] **Step 5 : Lancer**

Run: `cargo test && cargo build --release`
Expected: PASS, build OK.

- [ ] **Step 6 : Commit**

```bash
git add src/config.rs src/main.rs src/enricher.rs
git commit -m "Configuration : cache_enabled et api_cache_ttl_days"
```

---

## Task 16 : Documentation et benchmark

**Files:**
- Modify: `docs/superpowers/specs/2026-04-16-music-sorter-design.md`

- [ ] **Step 1 : Mettre à jour le design doc**

Ajouter une section "Cache et performances" décrivant :
- Emplacement : `<target>/.music-sorter.db`
- Tables et leur rôle
- TTL par défaut
- Comment réinitialiser : `rm <target>/.music-sorter.db*`

- [ ] **Step 2 : Bench sur dossier réel**

Procédure :
```bash
rm -f ~/Music/.music-sorter.db*
time ./target/release/music-sorter --source ~/Téléchargements --target ~/Music
time ./target/release/music-sorter --source ~/Téléchargements --target ~/Music
```

Documenter le ratio premier-run / second-run dans le design.

- [ ] **Step 3 : Commit**

```bash
git add docs/superpowers/specs/2026-04-16-music-sorter-design.md
git commit -m "Documentation cache et procédure de bench"
```

---

## Récapitulatif

| Task | Gain attendu | Coût impl |
|------|--------------|-----------|
| 4 — index processed | re-run: 100× sur fichiers déjà OK | M |
| 5 — cache fingerprint | re-run: skip fpcalc (sec→µs) | S |
| 6 + 7 + 8 — caches API | re-run: skip rate-limit MB/Discogs | M |
| 9 — registre artistes | corrige bug casse | S |
| 11 — album-grouping | premier run: ÷N par album sur le réseau | M |
| 12 — reflink | premier run: copie quasi-gratuite (btrfs) | XS |
| 13 — HTTP gzip/h2 | premier run: -30% bande passante | XS |
| 14 — skip tags complets | premier run: ignore 100% API si tags OK | XS |
