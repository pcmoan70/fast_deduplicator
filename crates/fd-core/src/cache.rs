//! On-disk score cache (SQLite). Keyed by content, not path, so it survives
//! card remounts and drive-letter changes.

use std::path::Path;

use rusqlite::Connection;

use crate::meta::FileMeta;

pub struct Cache {
    conn: Connection,
}

/// Content key: xxh3 of the first 64 KB + file size. RAW files are
/// immutable, and any rewrite changes the header bytes.
pub fn file_key(meta: &FileMeta) -> String {
    format!("{:016x}-{:x}", meta.content_key, meta.size)
}

impl Cache {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scores (
                key    TEXT PRIMARY KEY,
                global REAL NOT NULL
            );",
        )?;
        Ok(Cache { conn })
    }

    pub fn get_score(&self, key: &str) -> Option<f32> {
        self.conn
            .query_row("SELECT global FROM scores WHERE key = ?1", [key], |r| {
                r.get::<_, f64>(0)
            })
            .ok()
            .map(|v| v as f32)
    }

    pub fn put_scores<'a>(
        &mut self,
        items: impl Iterator<Item = (&'a str, f32)>,
    ) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt =
                tx.prepare("INSERT OR REPLACE INTO scores (key, global) VALUES (?1, ?2)")?;
            for (k, v) in items {
                stmt.execute(rusqlite::params![k, v as f64])?;
            }
        }
        tx.commit()
    }
}
