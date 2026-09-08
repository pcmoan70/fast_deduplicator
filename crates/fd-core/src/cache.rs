//! On-disk score cache (SQLite). Keyed by content, not path, so it survives
//! card remounts and drive-letter changes.

use std::path::Path;

use rusqlite::Connection;

use crate::formats::Source;
use crate::meta::FileMeta;

pub struct Cache {
    conn: Connection,
}

/// Content key: xxh3 of the first 64 KB + file size. RAW files are
/// immutable, and any rewrite changes the header bytes. Scores from the
/// full image are not comparable with preview scores, so `Source::Full`
/// gets its own key (`-full`), and every key carries the scoring formula
/// version so a formula change orphans old rows instead of mixing scales.
pub fn file_key(meta: &FileMeta, source: Source) -> String {
    let v = crate::score::SCORE_VERSION;
    match source {
        Source::Embedded => format!("{:016x}-{:x}-s{v}", meta.content_key, meta.size),
        Source::Full => format!("{:016x}-{:x}-full-s{v}", meta.content_key, meta.size),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::FileKind;

    #[test]
    fn full_source_gets_its_own_key() {
        let mut m = FileMeta::new("x.jpg".into(), 0x1234, FileKind::Jpeg);
        m.content_key = 0xabc;
        assert_eq!(file_key(&m, Source::Embedded), "0000000000000abc-1234-s2");
        assert_eq!(file_key(&m, Source::Full), "0000000000000abc-1234-full-s2");
    }
}
