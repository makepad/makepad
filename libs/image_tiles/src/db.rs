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

/// The baker's writable handle on the index.
pub struct TileDb {
    conn: Connection,
}

impl TileDb {
    pub fn open(path: &Path) -> Result<TileDb, String> {
        let mut conn = Connection::open(path, Duration::from_secs(5))
            .map_err(|e| format!("open {}: {e:?}", path.display()))?;
        conn.execute_batch(SCHEMA).map_err(|e| format!("schema: {e:?}"))?;
        Ok(TileDb { conn })
    }

    /// Add a source URL to bake. Already-known URLs keep their row (and
    /// their pixels); title/link are refreshed.
    pub fn add_source(&mut self, url: &str, title: &str, link: &str) -> Result<(), String> {
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
        self.conn
            .execute(
                "UPDATE items SET status = 2, error = ? WHERE id = ?",
                &[Value::text(error), Value::Integer(id)],
            )
            .map(|_| ())
            .map_err(|e| format!("fail {id}: {e:?}"))
    }

    pub fn upsert_shard(&mut self, shard: ShardRow) -> Result<(), String> {
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
