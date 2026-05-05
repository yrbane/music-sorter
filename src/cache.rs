use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Cache {
    pub(crate) conn: Mutex<Connection>,
}

/// Une entrée de processed_files exposée pour le mode rollback.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedEntry {
    pub source_path: String,
    pub dest_path: String,
    pub status: String,
    pub last_seen: i64,
}

impl Cache {
    /// Liste toutes les entrées avec un dest_path défini, triées par date décroissante.
    pub fn list_all_processed(&self) -> Result<Vec<ProcessedEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source_path, dest_path, status, last_seen
             FROM processed_files
             WHERE dest_path IS NOT NULL
             ORDER BY last_seen DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProcessedEntry {
                source_path: row.get(0)?,
                dest_path: row.get(1)?,
                status: row.get(2)?,
                last_seen: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Supprime une entrée de processed_files (utilisé après rollback réussi).
    pub fn delete_processed(&self, source_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM processed_files WHERE source_path = ?1",
            rusqlite::params![source_path],
        )?;
        Ok(())
    }

    /// Retourne `(status, dest_path, last_seen)` si l'entrée existe et matche mtime+size.
    pub fn lookup_processed(
        &self,
        source_path: &str,
        mtime: i64,
        size: i64,
    ) -> Result<Option<(String, Option<String>, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT status, dest_path, last_seen FROM processed_files
             WHERE source_path = ?1 AND mtime = ?2 AND size = ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![source_path, mtime, size])?;
        if let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let dest: Option<String> = row.get(1)?;
            let last_seen: i64 = row.get(2)?;
            Ok(Some((status, dest, last_seen)))
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO processed_files
             (source_path, mtime, size, dest_path, status, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![source_path, mtime, size, dest_path, status, now],
        )?;
        Ok(())
    }

    pub fn lookup_fingerprint(&self, content_hash: &str) -> Result<Option<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO fingerprint_cache (content_hash, chromaprint, duration, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![content_hash, fp, duration, now],
        )?;
        Ok(())
    }

    pub fn lookup_api(&self, endpoint: &str, key: &str, ttl_secs: i64) -> Result<Option<String>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        let cutoff = now - ttl_secs;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO api_cache (endpoint, cache_key, response, fetched_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![endpoint, key, response, now],
        )?;
        Ok(())
    }

    pub fn lookup_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT image FROM cover_cache WHERE release_id = ?1"
        )?;
        let mut rows = stmt.query(rusqlite::params![release_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn lookup_artist(&self, name: &str) -> Result<Option<String>> {
        let key = name.to_lowercase();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT canonical FROM artists WHERE canonical_lower = ?1"
        )?;
        let mut rows = stmt.query(rusqlite::params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn record_artist(&self, canonical: &str, mbid: Option<&str>) -> Result<()> {
        let key = canonical.to_lowercase();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO artists (canonical_lower, canonical, mbid) VALUES (?1, ?2, ?3)",
            rusqlite::params![key, canonical, mbid],
        )?;
        Ok(())
    }

    pub fn record_cover(&self, release_id: &str, image: &[u8]) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO cover_cache (release_id, image, fetched_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![release_id, image, now],
        )?;
        Ok(())
    }

    /// Ouvre un cache SQLite éphémère en mémoire (utile quand cache_enabled = false).
    /// Aucun fichier n'est créé sur disque.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Self::init_schema(&conn)?;
        Ok(Self { conn: std::sync::Mutex::new(conn) })
    }

    pub fn open(target: &Path) -> Result<Self> {
        let db_path = target.join(".music-sorter.db");
        std::fs::create_dir_all(target)?;
        let conn = Connection::open(&db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Self::init_schema(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
        cache
            .record_processed("/foo/bar.mp3", 100, 200, Some("/dest"), "organized")
            .unwrap();
        let r = cache.lookup_processed("/foo/bar.mp3", 100, 200).unwrap();
        assert!(r.is_some());
        let (status, dest, last_seen) = r.unwrap();
        assert_eq!(status, "organized");
        assert_eq!(dest, Some("/dest".into()));
        // last_seen est un timestamp Unix récent (> 0)
        assert!(last_seen > 0);
    }

    #[test]
    fn test_processed_miss_when_mtime_changed() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        cache
            .record_processed("/foo.mp3", 100, 200, Some("/dest"), "organized")
            .unwrap();
        let r = cache.lookup_processed("/foo.mp3", 999, 200).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn test_open_creates_db_and_tables() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        let conn = cache.conn.lock().unwrap();
        let names: Vec<String> = conn
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

    #[test]
    fn test_fingerprint_lookup_miss_then_hit() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        assert!(cache.lookup_fingerprint("hash1").unwrap().is_none());
        cache.record_fingerprint("hash1", "FPDATA", 200).unwrap();
        let r = cache.lookup_fingerprint("hash1").unwrap();
        assert_eq!(r, Some(("FPDATA".into(), 200)));
    }

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
    fn test_cover_cache_roundtrip() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        assert!(cache.lookup_cover("rid").unwrap().is_none());
        cache.record_cover("rid", &[1, 2, 3, 4]).unwrap();
        assert_eq!(cache.lookup_cover("rid").unwrap(), Some(vec![1,2,3,4]));
    }

    #[test]
    fn test_api_cache_expires() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        cache.conn.lock().unwrap().execute(
            "INSERT INTO api_cache VALUES ('mb', 'k', '{}', 0)", [],
        ).unwrap();
        let r = cache.lookup_api("mb", "k", 86400).unwrap();
        assert!(r.is_none(), "entrée datée de 1970 doit être expirée");
    }

    #[test]
    fn test_open_in_memory_creates_schema() {
        let cache = Cache::open_in_memory().unwrap();
        cache.record_processed("/foo", 1, 2, Some("/dest"), "organized").unwrap();
        let r = cache.lookup_processed("/foo", 1, 2).unwrap();
        assert!(r.is_some());
        let (status, dest, _last_seen) = r.unwrap();
        assert_eq!(status, "organized");
        assert_eq!(dest, Some("/dest".into()));
    }

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

    #[test]
    fn test_list_all_processed_returns_entries_with_dest() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        cache.record_processed("/a.mp3", 1, 100, Some("/dst/a.mp3"), "organized").unwrap();
        cache.record_processed("/b.mp3", 2, 200, Some("/dst/b.mp3"), "conflict").unwrap();
        cache.record_processed("/c.mp3", 3, 300, None, "error").unwrap();

        let entries = cache.list_all_processed().unwrap();
        assert_eq!(entries.len(), 2);
        let sources: Vec<&str> = entries.iter().map(|e| e.source_path.as_str()).collect();
        assert!(sources.contains(&"/a.mp3"));
        assert!(sources.contains(&"/b.mp3"));
        assert!(!sources.contains(&"/c.mp3"));
    }

    #[test]
    fn test_delete_processed_removes_entry() {
        let dir = tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        cache.record_processed("/a.mp3", 1, 100, Some("/dst/a.mp3"), "organized").unwrap();
        assert_eq!(cache.list_all_processed().unwrap().len(), 1);
        cache.delete_processed("/a.mp3").unwrap();
        assert_eq!(cache.list_all_processed().unwrap().len(), 0);
    }
}
