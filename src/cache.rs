use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub struct Cache {
    conn: Connection,
}

impl Cache {
    pub fn lookup_processed(
        &self,
        source_path: &str,
        mtime: i64,
        size: i64,
    ) -> Result<Option<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT status, dest_path FROM processed_files
             WHERE source_path = ?1 AND mtime = ?2 AND size = ?3",
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
        assert_eq!(r, Some(("organized".into(), Some("/dest".into()))));
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
}
