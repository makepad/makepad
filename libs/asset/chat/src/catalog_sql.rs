//! `assets.query` — the read-only SQL surface the chat broker gives an LLM
//! over the live asset catalog.
//!
//! This lives in the chat crate (not a crate of its own) because the ONLY
//! place it runs is the asset server's chat broker, which shares a process
//! with the catalog: the broker opens its server's own `catalog.sqlite3`
//! read-only through makepad-sqlite's WAL-interop snapshot reads while the
//! store keeps serving. No client process ever runs SQL — the sandbox and
//! every other app reach this through the chat channel's tool round trip.
//!
//! Safety model, in order:
//! 1. **Parse first.** The SQL goes through makepad-sqlite's own parser; the
//!    read-only [`makepad_sqlite::Database`] handle refuses to prepare
//!    anything but a single `SELECT` at the AST level (the parser itself
//!    rejects `;`-chained statements as "unexpected text after the
//!    statement", and `ATTACH`/triggers/multi-statement have no grammar).
//!    There is no regex filter to sneak past — writes are unrepresentable
//!    because the handle has no write path at all.
//! 2. **Bounded execution.** Row cap, per-value character cap, b-tree step
//!    budget and a wall-clock deadline. Exceeding the row cap truncates the
//!    result (flagged), exceeding the step budget or deadline fails the
//!    query with a bounded error.
//! 3. **Live reads.** [`CatalogReader::query`] refreshes to the newest
//!    committed WAL snapshot before every statement, so the co-located
//!    Asset Server can keep serving while we read (same-file concurrent
//!    reads are covered by the engine's concurrency suite).
//!
//! Results render as a compact aligned text table sized for an LLM context.

use makepad_sqlite::{Database, Error, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Everything a query run is allowed to cost.
#[derive(Clone, Copy, Debug)]
pub struct QueryLimits {
    /// Most result rows returned; more rows truncate (flagged), never error.
    pub max_rows: usize,
    /// Most characters one rendered value keeps (longer values are elided).
    pub max_value_chars: usize,
    /// Most bytes of SQL text accepted.
    pub max_sql_bytes: usize,
    /// Most b-tree rows the statement may visit (the deterministic budget).
    pub max_steps: u64,
    /// Wall-clock deadline, checked between result rows.
    pub deadline: Duration,
}

impl Default for QueryLimits {
    fn default() -> QueryLimits {
        QueryLimits {
            max_rows: 200,
            max_value_chars: 160,
            max_sql_bytes: 4096,
            max_steps: 5_000_000,
            deadline: Duration::from_secs(2),
        }
    }
}

/// One bounded, rendered result set.
#[derive(Clone, Debug)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    /// Already stringified and width-capped.
    pub rows: Vec<Vec<String>>,
    /// True when more rows existed than `max_rows`.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

impl QueryOutput {
    /// Compact aligned text: header, separator, rows, and a footer line with
    /// the row count (and a truncation note when the cap hit).
    pub fn to_text(&self) -> String {
        let mut widths: Vec<usize> = self.columns.iter().map(|c| c.chars().count()).collect();
        for row in &self.rows {
            for (i, v) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(v.chars().count());
                }
            }
        }
        // Alignment is a readability aid, not a promise: past this width a
        // padded table costs more context than it saves.
        const ALIGN_CAP: usize = 40;
        let pad = |s: &str, w: usize, last: bool| -> String {
            let w = w.min(ALIGN_CAP);
            let len = s.chars().count();
            if last || len >= w {
                s.to_string()
            } else {
                let mut out = String::with_capacity(w);
                out.push_str(s);
                for _ in len..w {
                    out.push(' ');
                }
                out
            }
        };
        let mut out = String::new();
        let ncols = self.columns.len();
        for (i, c) in self.columns.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            out.push_str(&pad(c, widths[i], i + 1 == ncols));
        }
        out.push('\n');
        for row in &self.rows {
            for (i, v) in row.iter().enumerate() {
                if i > 0 {
                    out.push_str("  ");
                }
                out.push_str(&pad(v, widths[i], i + 1 == ncols));
            }
            out.push('\n');
        }
        if self.truncated {
            out.push_str(&format!(
                "({} rows shown, MORE EXIST — narrow with WHERE or LIMIT)\n",
                self.rows.len()
            ));
        } else {
            out.push_str(&format!("({} rows)\n", self.rows.len()));
        }
        out
    }
}

/// A read-only handle on one catalog database file.
pub struct CatalogReader {
    path: PathBuf,
    db: Option<Database>,
    pub limits: QueryLimits,
}

impl CatalogReader {
    /// Bind to a catalog file. The file may not exist yet — every query
    /// re-tries the open, so a server that starts later just works.
    pub fn new(path: impl Into<PathBuf>) -> CatalogReader {
        CatalogReader { path: path.into(), db: None, limits: QueryLimits::default() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn database(&mut self) -> Result<&mut Database, String> {
        if self.db.is_none() {
            let db = Database::open(&self.path).map_err(|e| {
                format!("cannot open catalog {}: {e}", self.path.display())
            })?;
            self.db = Some(db);
        }
        let db = self.db.as_mut().expect("just opened");
        // Newest committed snapshot; also re-reads the schema when it moved.
        if let Err(e) = db.refresh() {
            // A truncated/rotated file invalidates the pager: drop the
            // handle so the next call re-opens from scratch.
            self.db = None;
            return Err(format!("catalog refresh failed: {e}"));
        }
        Ok(self.db.as_mut().expect("still open"))
    }

    /// Run one SELECT under the limits. Refusals and failures are bounded,
    /// model-facing strings.
    pub fn query(&mut self, sql: &str) -> Result<QueryOutput, String> {
        let sql = sql.trim();
        if sql.is_empty() {
            return Err("empty SQL".to_string());
        }
        if sql.len() > self.limits.max_sql_bytes {
            return Err(format!(
                "SQL too large ({} bytes; the cap is {})",
                sql.len(),
                self.limits.max_sql_bytes
            ));
        }
        let limits = self.limits;
        let db = self.database()?;
        db.limits_mut().max_rows = usize::MAX; // our own cap truncates instead
        db.limits_mut().max_steps = limits.max_steps;
        // Parse + plan. Only a single SELECT prepares; everything else is
        // refused here by the engine, before any page is read.
        let stmt = db.prepare(sql).map_err(|e| refusal_text(&e))?;
        let started = Instant::now();
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut truncated = false;
        let run = stmt.for_each(db, &[], |row| {
            if started.elapsed() > limits.deadline {
                return Err(Error::Budget(format!(
                    "query exceeded the {} ms deadline",
                    limits.deadline.as_millis()
                )));
            }
            if rows.len() >= limits.max_rows {
                truncated = true;
                return Ok(false);
            }
            rows.push(row.iter().map(|v| render_value(v, limits.max_value_chars)).collect());
            Ok(true)
        });
        if let Err(e) = run {
            return Err(failure_text(&e));
        }
        Ok(QueryOutput {
            columns: stmt.column_names().to_vec(),
            rows,
            truncated,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Table/column/index summary generated from the live `sqlite_master`,
    /// with usage notes for the catalog's model-facing tables.
    ///
    /// Only the CONTENT-facing tables are rendered: the catalog also holds
    /// ~30 operational tables (jobs, leases, GC, auth) that cost the model
    /// hundreds of tokens each and answer no content question. Token
    /// economy is a design principle here — this text lands in a local
    /// model's context on every schema call.
    pub fn schema_text(&mut self) -> Result<String, String> {
        const CONTENT_TABLES: &[&str] = &[
            "assets",
            "asset_aliases",
            "asset_revisions",
            "search_annotations",
            "search_labels",
        ];
        let db = self.database()?;
        let mut out = String::from("Catalog tables (SELECT-only; single statement):\n");
        for t in &db.schema().tables {
            if t.name.starts_with("sqlite_") {
                continue;
            }
            if !CONTENT_TABLES.contains(&t.name.as_str()) {
                continue;
            }
            out.push_str("- ");
            out.push_str(&t.name);
            out.push('(');
            for (i, c) in t.columns.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&c.name);
                if !c.decl_type.is_empty() {
                    out.push(' ');
                    out.push_str(&c.decl_type);
                }
            }
            out.push_str(")\n");
            for ix in &t.indexes {
                if ix.auto {
                    continue;
                }
                out.push_str("    index ");
                out.push_str(&ix.name);
                if ix.unique {
                    out.push_str(" UNIQUE");
                }
                out.push_str(" on (");
                for (i, ic) in ix.columns.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(
                        t.columns.get(ic.column).map(|c| c.name.as_str()).unwrap_or("?"),
                    );
                }
                out.push_str(")\n");
            }
        }
        out.push_str(SCHEMA_NOTES);
        Ok(out)
    }
}

impl CatalogReader {
    /// One line per model "kit" (the first two alias path segments):
    /// `kenney/car-kit 50 · kenney/building-kit 62 · …`. Fed into the game
    /// session's dynamic context so the model SEES the inventory up front
    /// instead of paging the catalog 20 rows per tool call (iteration 5
    /// burned its whole tool budget browsing).
    pub fn kit_summary(&mut self) -> Result<String, String> {
        let limits = self.limits;
        let db = self.database()?;
        db.limits_mut().max_rows = usize::MAX;
        db.limits_mut().max_steps = limits.max_steps;
        let stmt = db
            .prepare(
                "SELECT canon_alias FROM search_annotations \
                 WHERE live=1 AND kind='mesh' AND canon_alias <> ''",
            )
            .map_err(|e| refusal_text(&e))?;
        let mut counts: Vec<(String, u32)> = Vec::new();
        let mut scanned = 0usize;
        stmt.for_each(db, &[], |row| {
            scanned += 1;
            if scanned > 50_000 {
                return Ok(false);
            }
            if let Some(Value::Text(alias)) = row.first() {
                let kit = match alias.match_indices('/').nth(1) {
                    Some((i, _)) => &alias[..i],
                    None => alias.as_str(),
                };
                match counts.iter_mut().find(|(k, _)| k == kit) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((kit.to_string(), 1)),
                }
            }
            Ok(true)
        })
        .map_err(|e| failure_text(&e))?;
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.truncate(48);
        let mut out = String::from("Model kits in the store (alias-prefix count):\n");
        for (i, (kit, n)) in counts.iter().enumerate() {
            if i > 0 {
                out.push_str(" · ");
            }
            out.push_str(kit);
            out.push(' ');
            out.push_str(&n.to_string());
        }
        out.push('\n');
        Ok(out)
    }
}

/// Advisory notes the model gets with the generated schema. They describe
/// the well-known catalog tables; the generated part above is the truth.
const SCHEMA_NOTES: &str = "\nNotes:\n\
- search_annotations is the main listing: one row per asset with \
canon_alias (the readable id you place with), kind, title, description, \
prompt, live (1 = current). Always filter live=1.\n\
- search_labels holds per-asset labels: kind is 'category' or 'tag', \
label is the value. Join on asset_id.\n\
- asset_aliases maps alias -> asset_id/head_revision (canon_alias in \
search_annotations is usually what you want).\n\
- asset_id columns are 16-byte blobs; they render as 32 hex chars here.\n\
- Operational tables (jobs, operations, gc, auth) exist but are omitted: \
they answer no content question.\n";

/// The engine's refusals for non-SELECT/multi-statement parse cleanly; keep
/// their text but add the contract line so the model self-corrects.
fn refusal_text(e: &Error) -> String {
    format!("refused: {e} (only a single SELECT statement is allowed)")
}

fn failure_text(e: &Error) -> String {
    match e {
        Error::Budget(what) => format!("query over budget: {what}"),
        other => format!("query failed: {other}"),
    }
}

fn render_value(v: &Value, max_chars: usize) -> String {
    let full = match v {
        Value::Null => "NULL".to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => format!("{f}"),
        Value::Text(t) => sanitize_text(t),
        Value::Blob(b) => {
            // Asset/revision ids are 16/32-byte blobs; hex is their readable
            // form. Anything bigger is content, not an id.
            if b.len() <= 32 {
                let mut s = String::with_capacity(b.len() * 2);
                for byte in b {
                    s.push_str(&format!("{byte:02x}"));
                }
                s
            } else {
                format!("blob({}B)", b.len())
            }
        }
    };
    elide(&full, max_chars)
}

/// Control characters would break the aligned table (and a newline could
/// spoof extra rows); flatten them.
fn sanitize_text(t: &str) -> String {
    if t.chars().any(|c| c.is_control()) {
        t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    } else {
        t.to_string()
    }
}

fn elide(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_sqlite::Connection;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "asset-query-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.sqlite3");
        let mut conn = Connection::open(&path, std::time::Duration::from_secs(2)).unwrap();
        conn.execute(
            "CREATE TABLE search_annotations(
                asset_id BLOB PRIMARY KEY,
                canon_alias TEXT NOT NULL,
                kind TEXT,
                title TEXT NOT NULL,
                live INTEGER NOT NULL)",
            &[],
        )
        .unwrap();
        conn.execute("CREATE INDEX ann_alias ON search_annotations(canon_alias)", &[]).unwrap();
        for i in 0..30 {
            conn.execute(
                "INSERT INTO search_annotations VALUES (?, ?, ?, ?, ?)",
                &[
                    Value::blob(vec![i as u8; 16]),
                    Value::text(format!("kenney/props/fence-{i:02}")),
                    Value::text("prop"),
                    Value::text(format!("Fence {i}")),
                    Value::Integer(1),
                ],
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn selects_render_compactly() {
        let path = fixture("select");
        let mut reader = CatalogReader::new(&path);
        let out = reader
            .query("SELECT canon_alias, kind FROM search_annotations WHERE live=1 ORDER BY canon_alias LIMIT 3")
            .unwrap();
        assert_eq!(out.columns, vec!["canon_alias", "kind"]);
        assert_eq!(out.rows.len(), 3);
        assert!(!out.truncated);
        let text = out.to_text();
        assert!(text.contains("kenney/props/fence-00"), "{text}");
        assert!(text.ends_with("(3 rows)\n"), "{text}");
    }

    #[test]
    fn hostile_sql_is_refused_at_the_ast() {
        let path = fixture("hostile");
        let mut reader = CatalogReader::new(&path);
        let hostile = [
            "INSERT INTO search_annotations VALUES (x'00', 'a', 'b', 'c', 1)",
            "UPDATE search_annotations SET live=0",
            "DELETE FROM search_annotations",
            "DROP TABLE search_annotations",
            "CREATE TABLE pwned(x)",
            "ALTER TABLE search_annotations ADD COLUMN pwned",
            "PRAGMA user_version",
            "PRAGMA journal_mode=delete",
            "ATTACH DATABASE '/tmp/x' AS x",
            "BEGIN",
            "COMMIT",
            "SELECT 1; DROP TABLE search_annotations",
            "SELECT 1; SELECT 2",
            "VACUUM",
            "REPLACE INTO search_annotations VALUES (x'00', 'a', 'b', 'c', 1)",
            "",
            "   ",
        ];
        for sql in hostile {
            let err = reader.query(sql).expect_err(sql);
            assert!(
                err.starts_with("refused:") || err.starts_with("empty"),
                "{sql} -> {err}"
            );
        }
        // The table is intact and still has every row.
        let out = reader.query("SELECT COUNT(*) FROM search_annotations").unwrap();
        assert_eq!(out.rows[0][0], "30");
    }

    #[test]
    fn a_trailing_semicolon_is_fine_but_a_chain_is_not() {
        let path = fixture("semi");
        let mut reader = CatalogReader::new(&path);
        assert!(reader.query("SELECT 1;").is_ok());
        assert!(reader.query("SELECT 1;;").is_err());
        assert!(reader.query("SELECT 1; --x\nSELECT 2").is_err());
    }

    #[test]
    fn the_row_cap_truncates_instead_of_erroring() {
        let path = fixture("rowcap");
        let mut reader = CatalogReader::new(&path);
        reader.limits.max_rows = 10;
        let out = reader.query("SELECT canon_alias FROM search_annotations").unwrap();
        assert_eq!(out.rows.len(), 10);
        assert!(out.truncated);
        assert!(out.to_text().contains("MORE EXIST"));
    }

    #[test]
    fn values_are_width_capped_and_blobs_hex() {
        let path = fixture("width");
        let mut reader = CatalogReader::new(&path);
        reader.limits.max_value_chars = 8;
        let out = reader
            .query("SELECT asset_id, title FROM search_annotations ORDER BY canon_alias LIMIT 1")
            .unwrap();
        assert_eq!(out.rows[0][0], "0000000…");
        assert!(out.rows[0][1].chars().count() <= 8);
    }

    #[test]
    fn oversized_sql_is_refused_before_parsing() {
        let path = fixture("size");
        let mut reader = CatalogReader::new(&path);
        reader.limits.max_sql_bytes = 64;
        let big = format!("SELECT {}", "1+".repeat(200));
        let err = reader.query(&big).unwrap_err();
        assert!(err.contains("SQL too large"), "{err}");
    }

    #[test]
    fn schema_text_lists_tables_columns_and_indexes() {
        let path = fixture("schema");
        let mut reader = CatalogReader::new(&path);
        let text = reader.schema_text().unwrap();
        assert!(text.contains("search_annotations("), "{text}");
        assert!(text.contains("canon_alias TEXT"), "{text}");
        assert!(text.contains("index ann_alias"), "{text}");
        assert!(text.contains("Notes:"), "{text}");
    }

    #[test]
    fn a_missing_file_reports_and_recovers_once_it_exists() {
        let dir = std::env::temp_dir().join(format!("asset-query-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.sqlite3");
        let mut reader = CatalogReader::new(&path);
        assert!(reader.query("SELECT 1").unwrap_err().contains("cannot open"));
        let mut conn = Connection::open(&path, std::time::Duration::from_secs(2)).unwrap();
        conn.execute("CREATE TABLE t(x)", &[]).unwrap();
        conn.execute("INSERT INTO t VALUES (7)", &[]).unwrap();
        drop(conn);
        let out = reader.query("SELECT x FROM t").unwrap();
        assert_eq!(out.rows[0][0], "7");
    }

    #[test]
    fn live_writes_are_visible_on_the_next_query() {
        let path = fixture("live");
        let mut reader = CatalogReader::new(&path);
        let before = reader.query("SELECT COUNT(*) FROM search_annotations").unwrap();
        assert_eq!(before.rows[0][0], "30");
        // Another connection (the "server") commits while our reader stays
        // open — the refresh must pick the new snapshot up.
        let mut conn = Connection::open(&path, std::time::Duration::from_secs(2)).unwrap();
        conn.execute(
            "INSERT INTO search_annotations VALUES (?, 'new/one', 'prop', 'New', 1)",
            &[Value::blob(vec![0xAA; 16])],
        )
        .unwrap();
        drop(conn);
        let after = reader.query("SELECT COUNT(*) FROM search_annotations").unwrap();
        assert_eq!(after.rows[0][0], "31");
    }

    #[test]
    fn control_characters_cannot_spoof_table_rows() {
        let path = fixture("ctl");
        let mut conn = Connection::open(&path, std::time::Duration::from_secs(2)).unwrap();
        conn.execute(
            "INSERT INTO search_annotations VALUES (?, 'evil', 'prop', ?, 1)",
            &[
                Value::blob(vec![0xBB; 16]),
                Value::text("line1\nfake_alias  prop\u{7}"),
            ],
        )
        .unwrap();
        drop(conn);
        let mut reader = CatalogReader::new(&path);
        let out = reader
            .query("SELECT title FROM search_annotations WHERE canon_alias='evil'")
            .unwrap();
        assert!(!out.rows[0][0].contains('\n'));
        assert!(!out.rows[0][0].contains('\u{7}'));
    }
}
