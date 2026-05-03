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
