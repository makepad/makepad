//! The library index: one SQLite file, two tables.
//!
//! `items` is every picture the baker was ever asked for — its source URL,
//! a little display metadata, and once baked, the shard and slot its pixels
//! live at. `shards` records which tape files exist and are complete. The
//! whole thing is deliberately small so people can point their own tools
//! (or an AI) at it: add rows with any SQLite writer, run the baker, and the
//! grid draws whatever reached `status = 1`.

use makepad_sqlite::{Connection, Database, Value};
use std::path::Path;
use std::time::Duration;

pub type ItemId = i64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemStatus {
    Pending,
    Ready,
    Failed,
}

impl ItemStatus {
    fn as_i64(self) -> i64 {
        match self {
            ItemStatus::Pending => 0,
            ItemStatus::Ready => 1,
            ItemStatus::Failed => 2,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ItemRow {
    pub id: ItemId,
    pub url: String,
    pub title: String,
    pub link: String,
    pub width: i64,
    pub height: i64,
    pub aspect: f64,
    pub shard: Option<i64>,
    pub slot: Option<i64>,
}

#[derive(Clone, Copy, Debug)]
pub struct ShardRow {
    pub id: i64,
    pub count: i64,
    pub sealed: bool,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS items(
    id INTEGER PRIMARY KEY,
    url TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL DEFAULT '',
    link TEXT NOT NULL DEFAULT '',
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    aspect REAL NOT NULL DEFAULT 1.0,
    shard INTEGER,
    slot INTEGER,
    status INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS shards(
    id INTEGER PRIMARY KEY,
    count INTEGER NOT NULL,
    sealed INTEGER NOT NULL
);
";

/// The baker's handle on the index. Writable when the library is ours to
/// change; read-only when it lives where nothing may be written (an
/// application bundle, a read-only volume), in which case every write
/// answers [`READ_ONLY_LIBRARY`] and the reads are the same.
pub struct TileDb {
    conn: Connection,
    read_only: bool,
}

/// What every write on a read-only library answers.
pub const READ_ONLY_LIBRARY: &str = "this library is read-only";

impl TileDb {
    /// Open for writing, creating the index when it is missing. When the OS
    /// refuses write access to the file or its directory, the library is
    /// opened read-only instead (one log line says so); any other failure
    /// is an error.
    pub fn open(path: &Path) -> Result<TileDb, String> {
        match Connection::open(path, Duration::from_secs(5)) {
            Ok(mut conn) => {
                conn.execute_batch(SCHEMA).map_err(|e| format!("schema: {e:?}"))?;
                Ok(TileDb { conn, read_only: false })
            }
            Err(makepad_sqlite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
                ) && path.is_file() =>
            {
                makepad_widgets::log!("image-tiles: {} is read-only ({error}); writes are refused", path.display());
                Self::open_read_only(path)
            }
            Err(e) => Err(format!("open {}: {e:?}", path.display())),
        }
    }

    /// Open an existing index for reading only: no schema statement, no
    /// journal, no lock beyond the shared one a read takes.
    pub fn open_read_only(path: &Path) -> Result<TileDb, String> {
        let conn = Connection::open_read_only(path, Duration::from_secs(5))
            .map_err(|e| format!("open {} read-only: {e:?}", path.display()))?;
        Ok(TileDb { conn, read_only: true })
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn writable(&self) -> Result<(), String> {
        if self.read_only {
            Err(READ_ONLY_LIBRARY.to_string())
        } else {
            Ok(())
        }
    }

    /// Add a source URL to bake. Already-known URLs keep their row (and
    /// their pixels); title/link are refreshed.
    pub fn add_source(&mut self, url: &str, title: &str, link: &str) -> Result<(), String> {
        self.writable()?;
        self.conn
            .execute(
                "INSERT INTO items(url, title, link) VALUES(?, ?, ?)
                 ON CONFLICT(url) DO UPDATE SET title = ?, link = ?",
                &[Value::text(url), Value::text(title), Value::text(link), Value::text(title), Value::text(link)],
            )
            .map(|_| ())
            .map_err(|e| format!("add {url}: {e:?}"))
    }

    /// Everything still waiting for pixels, oldest first.
    pub fn pending(&mut self) -> Result<Vec<(ItemId, String)>, String> {
        let result = self
            .conn
            .query("SELECT id, url FROM items WHERE status = 0 ORDER BY id", &[])
            .map_err(|e| format!("pending: {e:?}"))?;
        Ok(result
            .rows
            .iter()
            .filter_map(|r| Some((r[0].as_integer()?, r[1].as_text()?.to_string())))
            .collect())
    }

    /// Put permanently-failed items back in the queue for another try.
    pub fn retry_failed(&mut self) -> Result<u64, String> {
        self.writable()?;
        self.conn
            .execute("UPDATE items SET status = 0, error = '' WHERE status = 2", &[])
            .map_err(|e| format!("retry: {e:?}"))
    }

    pub fn set_ready(
        &mut self,
        id: ItemId,
        width: u32,
        height: u32,
        shard: i64,
        slot: u32,
    ) -> Result<(), String> {
        self.writable()?;
        let aspect = width.max(1) as f64 / height.max(1) as f64;
        self.conn
            .execute(
                "UPDATE items SET status = 1, width = ?, height = ?, aspect = ?, shard = ?, slot = ?, error = '' WHERE id = ?",
                &[
                    Value::Integer(width as i64),
                    Value::Integer(height as i64),
                    Value::Real(aspect),
                    Value::Integer(shard),
                    Value::Integer(slot as i64),
                    Value::Integer(id),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("ready {id}: {e:?}"))
    }

    pub fn set_failed(&mut self, id: ItemId, error: &str) -> Result<(), String> {
        self.writable()?;
        self.conn
            .execute(
                "UPDATE items SET status = 2, error = ? WHERE id = ?",
                &[Value::text(error), Value::Integer(id)],
            )
            .map(|_| ())
            .map_err(|e| format!("fail {id}: {e:?}"))
    }

    pub fn upsert_shard(&mut self, shard: ShardRow) -> Result<(), String> {
        self.writable()?;
        self.conn
            .execute(
                "INSERT INTO shards(id, count, sealed) VALUES(?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET count = ?, sealed = ?",
                &[
                    Value::Integer(shard.id),
                    Value::Integer(shard.count),
                    Value::Integer(shard.sealed as i64),
                    Value::Integer(shard.count),
                    Value::Integer(shard.sealed as i64),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("shard {}: {e:?}", shard.id))
    }

    pub fn shards(&mut self) -> Result<Vec<ShardRow>, String> {
        let result = self
            .conn
            .query("SELECT id, count, sealed FROM shards ORDER BY id", &[])
            .map_err(|e| format!("shards: {e:?}"))?;
        Ok(result
            .rows
            .iter()
            .filter_map(|r| {
                Some(ShardRow { id: r[0].as_integer()?, count: r[1].as_integer()?, sealed: r[2].as_integer()? != 0 })
            })
            .collect())
    }

    /// A shard whose tapes never got written (a crash while it was open)
    /// holds no pixels: its items go back to pending so they are fetched
    /// again, and the shard id is freed.
    pub fn reset_unsealed_shards(&mut self) -> Result<usize, String> {
        self.writable()?;
        let open: Vec<i64> = self.shards()?.into_iter().filter(|s| !s.sealed).map(|s| s.id).collect();
        for id in &open {
            self.conn
                .execute(
                    "UPDATE items SET status = 0, shard = NULL, slot = NULL WHERE shard = ?",
                    &[Value::Integer(*id)],
                )
                .map_err(|e| format!("reset shard {id}: {e:?}"))?;
            self.conn
                .execute("DELETE FROM shards WHERE id = ?", &[Value::Integer(*id)])
                .map_err(|e| format!("drop shard {id}: {e:?}"))?;
        }
        Ok(open.len())
    }

    pub fn next_shard(&mut self) -> Result<i64, String> {
        let result = self.conn.query("SELECT MAX(id) FROM shards", &[]).map_err(|e| format!("next shard: {e:?}"))?;
        Ok(result.scalar().and_then(|v| v.as_integer()).unwrap_or(-1) + 1)
    }

    pub fn counts(&mut self) -> Result<(i64, i64, i64), String> {
        let q = |conn: &mut Connection, status: i64| -> Result<i64, String> {
            Ok(conn
                .query("SELECT COUNT(*) FROM items WHERE status = ?", &[Value::Integer(status)])
                .map_err(|e| format!("count: {e:?}"))?
                .scalar()
                .and_then(|v| v.as_integer())
                .unwrap_or(0))
        };
        let pending = q(&mut self.conn, ItemStatus::Pending.as_i64())?;
        let ready = q(&mut self.conn, ItemStatus::Ready.as_i64())?;
        let failed = q(&mut self.conn, ItemStatus::Failed.as_i64())?;
        Ok((pending, ready, failed))
    }
}

/// What a viewer needs, read without taking the writer's lock: every baked
/// picture in id order, plus which shards are sealed.
pub fn read_items(path: &Path) -> Result<(Vec<ItemRow>, Vec<ShardRow>), String> {
    let mut db = Database::open(path).map_err(|e| format!("open {}: {e:?}", path.display()))?;
    let result = db
        .query(
            "SELECT id, url, title, link, width, height, aspect, shard, slot FROM items \
             WHERE status = 1 AND shard IS NOT NULL ORDER BY id",
            &[],
        )
        .map_err(|e| format!("items: {e:?}"))?;
    let items = result
        .rows
        .iter()
        .filter_map(|r| {
            Some(ItemRow {
                id: r[0].as_integer()?,
                url: r[1].as_text().unwrap_or("").to_string(),
                title: r[2].as_text().unwrap_or("").to_string(),
                link: r[3].as_text().unwrap_or("").to_string(),
                width: r[4].as_integer().unwrap_or(0),
                height: r[5].as_integer().unwrap_or(0),
                aspect: r[6].as_real().or_else(|| r[6].as_integer().map(|i| i as f64)).unwrap_or(1.0),
                shard: r[7].as_integer(),
                slot: r[8].as_integer(),
            })
        })
        .collect();
    let result = db.query("SELECT id, count, sealed FROM shards ORDER BY id", &[]).map_err(|e| format!("shards: {e:?}"))?;
    let shards = result
        .rows
        .iter()
        .filter_map(|r| Some(ShardRow { id: r[0].as_integer()?, count: r[1].as_integer()?, sealed: r[2].as_integer()? != 0 }))
        .collect();
    Ok((items, shards))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("image-tiles-db-{tag}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn set_read_only(path: &Path, read_only: bool) {
        use std::os::unix::fs::PermissionsExt;
        let mode = if read_only { 0o555 } else { 0o755 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// A library where nothing may be written — an application bundle —
    /// opens read-only on the ordinary path: reads are the same, every
    /// write says why it did not happen, and no sidecar file appears.
    #[test]
    #[cfg(unix)]
    fn a_read_only_library_opens_for_reading_and_refuses_writes() {
        let dir = scratch("read-only");
        let path = dir.join("library.sqlite");
        {
            let mut db = TileDb::open(&path).unwrap();
            assert!(!db.is_read_only());
            db.add_source("https://example.invalid/1.png", "one", "").unwrap();
            db.upsert_shard(ShardRow { id: 0, count: 1, sealed: true }).unwrap();
        }
        set_read_only(&path, true);
        set_read_only(&dir, true);
        let outcome = std::panic::catch_unwind(|| {
            let mut db = TileDb::open(&path).unwrap();
            assert!(db.is_read_only(), "the OS refused writing: the library is read-only");
            assert_eq!(db.pending().unwrap().len(), 1);
            assert_eq!(db.shards().unwrap().len(), 1);
            assert_eq!(db.counts().unwrap(), (1, 0, 0));
            for result in [
                db.add_source("https://example.invalid/2.png", "two", ""),
                db.set_failed(1, "nope"),
                db.upsert_shard(ShardRow { id: 1, count: 0, sealed: false }),
                db.retry_failed().map(|_| ()),
                db.reset_unsealed_shards().map(|_| ()),
            ] {
                assert_eq!(result, Err(READ_ONLY_LIBRARY.to_string()));
            }
            assert!(TileDb::open_read_only(&path).unwrap().is_read_only());
            let siblings: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
            assert_eq!(siblings.len(), 1, "no journal, WAL or shm: {siblings:?}");
            // The viewer's own reader is unaffected.
            assert!(read_items(&path).is_ok());
        });
        set_read_only(&dir, false);
        set_read_only(&path, false);
        let _ = std::fs::remove_dir_all(&dir);
        outcome.unwrap();
    }
}
