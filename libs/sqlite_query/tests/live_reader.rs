//! One long-lived reader against a live writer, in one process.
//!
//! This is the asset store's shape: the chat broker keeps a single read-only
//! [`Database`] for the life of the process and calls `refresh()` before every
//! statement, while the store — same process, same file — runs an annotation
//! pass that rewrites each record's `search_labels` rows (delete-all, then
//! re-insert a dozen brand-new distinct labels) in one transaction per record.
//! Hundreds of such transactions in a burst split index pages, free pages and
//! hand them back out again, and roll the write-ahead log over.
//!
//! The invariant under test is the one a query language rests on: a predicate
//! filters. A label that no row carries returns no rows, and a label two rows
//! carry returns those two — whatever the writer is doing at the time.

mod common;

use common::*;
use makepad_sqlite::{Connection, Database, Value};
use std::collections::BTreeSet;
use std::time::Duration;

const ASSETS: usize = 60;
const LABELS_PER_ASSET: usize = 12;

/// The broker's query, verbatim in shape: a join whose selectivity comes from
/// the label predicate alone.
const JOIN_SQL: &str = "SELECT a.canon_alias FROM search_annotations a \
     JOIN search_labels l ON l.asset_id = a.asset_id \
     WHERE a.live=1 AND a.kind='character' AND l.label IN (?1) LIMIT 20";

fn asset_id(i: usize) -> Value {
    let mut b = vec![0u8; 16];
    b[0..8].copy_from_slice(&(i as u64).to_be_bytes());
    b[8..16].copy_from_slice(&(0xa5a5_0000_0000_0000u64 ^ i as u64).to_be_bytes());
    Value::Blob(b)
}

fn alias(i: usize) -> String {
    format!("asset-{i:05}")
}

/// The labels asset `i` carries in generation `g`. Every generation is a fresh
/// set of distinct strings, the way a vision pass hands out `vlm-age-*`,
/// `vlm-hair-*` and friends: the old keys are freed and the new ones split
/// pages somewhere else in the index.
fn labels(i: usize, g: usize) -> Vec<String> {
    (0..LABELS_PER_ASSET)
        .map(|k| format!("vlm-g{g:03}-{k}-{}", (i * 7 + k * 13) % 997))
        .collect()
}

fn catalog_schema(w: &mut Connection) {
    w.execute("PRAGMA journal_mode=WAL", &[]).unwrap();
    w.execute(
        "CREATE TABLE search_annotations(\
            asset_id BLOB PRIMARY KEY, \
            kind TEXT, \
            live INTEGER NOT NULL, \
            description TEXT NOT NULL, \
            canon_alias TEXT NOT NULL DEFAULT '')",
        &[],
    )
    .unwrap();
    w.execute(
        "CREATE TABLE search_labels(\
            asset_id BLOB NOT NULL, \
            kind TEXT NOT NULL, \
            label TEXT NOT NULL, \
            PRIMARY KEY(asset_id, kind, label))",
        &[],
    )
    .unwrap();
    w.execute(
        "CREATE INDEX search_labels_by_label ON search_labels(kind, label)",
        &[],
    )
    .unwrap();
    w.execute(
        "CREATE INDEX search_annotations_by_kind ON search_annotations(kind)",
        &[],
    )
    .unwrap();
}

fn seed(w: &mut Connection) {
    w.execute("BEGIN", &[]).unwrap();
    for i in 0..ASSETS {
        w.execute(
            "INSERT INTO search_annotations(asset_id, kind, live, description, canon_alias) \
             VALUES(?1,'character',1,?2,?3)",
            &[
                asset_id(i),
                Value::text(format!("seed description for {i}")),
                Value::text(alias(i)),
            ],
        )
        .unwrap();
        for l in labels(i, 0) {
            w.execute(
                "INSERT INTO search_labels(asset_id, kind, label) VALUES(?1,'tag',?2)",
                &[asset_id(i), Value::text(l)],
            )
            .unwrap();
        }
    }
    w.execute("COMMIT", &[]).unwrap();
}

/// One annotation PUT: a whole-record rewrite in a single transaction.
fn put(w: &mut Connection, i: usize, g: usize) {
    let r = (|| -> makepad_sqlite::Result<()> {
        w.execute("BEGIN", &[])?;
        w.execute(
            "UPDATE search_annotations SET description = ?2 WHERE asset_id = ?1",
            &[
                asset_id(i),
                Value::text(format!("generation {g} description for {i}")),
            ],
        )?;
        w.execute(
            "DELETE FROM search_labels WHERE asset_id = ?1",
            &[asset_id(i)],
        )?;
        for l in labels(i, g) {
            w.execute(
                "INSERT INTO search_labels(asset_id, kind, label) VALUES(?1,'tag',?2)",
                &[asset_id(i), Value::text(l)],
            )?;
        }
        w.execute("COMMIT", &[])?;
        Ok(())
    })();
    if let Err(e) = r {
        panic!("PUT of asset {i} in generation {g} failed: {e}");
    }
}

fn check_integrity(w: &mut Connection, what: &str) {
    let schema = w.schema().clone();
    let report = makepad_sqlite::integrity::check(w.pager(), &schema, true).unwrap();
    assert!(report.ok(), "{what}: {:?}", report.problems);
}

fn aliases_for_label(reader: &mut Database, label: &str) -> BTreeSet<String> {
    reader
        .query(JOIN_SQL, &[Value::text(label)])
        .unwrap_or_else(|e| panic!("query for {label:?} failed: {e}"))
        .rows
        .iter()
        .map(|r| r[0].as_text().unwrap_or_default().to_string())
        .collect()
}

/// Rewriting a record's labels must leave the index describing the table.
///
/// Deleting a key that sits on an interior index page pulls its successor up
/// from a leaf. The path to that leaf names a child slot in every page it
/// crosses, and the slot in the page holding the separator is the separator's
/// own — one to the left of the subtree actually walked into. Emptying that
/// leaf then unlinked it using the wrong slot: the parent kept pointing at the
/// freed page and lost the pointer to a live subtree instead. The freed page is
/// zeroed and handed straight back out by the freelist, so an index scan walks
/// into a page belonging to some other b-tree.
#[test]
fn an_annotation_burst_keeps_the_index_describing_the_table() {
    let scratch = Scratch::new("live-reader-burst");
    let path = scratch.path("catalog.db");
    let mut w = Connection::open(&path, Duration::from_secs(5)).unwrap();
    catalog_schema(&mut w);
    seed(&mut w);
    check_integrity(&mut w, "after the seed");

    // One label at a time, so the check lands between individual b-tree edits
    // rather than after a whole statement.
    for g in 1..=2usize {
        for i in 0..ASSETS {
            for (k, l) in labels(i, g - 1).into_iter().enumerate() {
                w.execute(
                    "DELETE FROM search_labels WHERE asset_id = ?1 AND kind='tag' AND label = ?2",
                    &[asset_id(i), Value::text(&l)],
                )
                .unwrap_or_else(|e| panic!("delete {l:?} of asset {i}: {e}"));
                check_integrity(&mut w, &format!("after deleting {l:?} (asset {i}, label {k})"));
            }
            for l in labels(i, g) {
                w.execute(
                    "INSERT INTO search_labels(asset_id, kind, label) VALUES(?1,'tag',?2)",
                    &[asset_id(i), Value::text(l)],
                )
                .unwrap_or_else(|e| panic!("insert for asset {i}: {e}"));
            }
            check_integrity(&mut w, &format!("after rewriting asset {i} to generation {g}"));
        }
        let n = w.query("SELECT COUNT(*) FROM search_labels", &[]).unwrap();
        assert_eq!(
            n.rows[0][0].as_integer(),
            Some((ASSETS * LABELS_PER_ASSET) as i64),
            "generation {g}: the label count drifted"
        );
    }
    // A connection owns the log for its whole life; the CLI cross-check runs
    // after it closes and hands the log back.
    drop(w);
    if have_sqlite3() {
        assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");
    }
}

/// The broker's loop: one handle, `refresh()` between statements, an
/// annotation burst underneath it.
#[test]
fn a_long_lived_reader_never_answers_from_a_stale_index() {
    let scratch = Scratch::new("live-reader-loop");
    let path = scratch.path("catalog.db");
    let mut w = Connection::open(&path, Duration::from_secs(5)).unwrap();
    catalog_schema(&mut w);
    seed(&mut w);

    let mut reader = Database::open(&path).unwrap();
    let g0 = labels(0, 0);
    assert_eq!(
        aliases_for_label(&mut reader, &g0[0]),
        BTreeSet::from([alias(0)]),
        "the warm-up read is already wrong"
    );

    for generation in 1..=12usize {
        for i in 0..ASSETS {
            put(&mut w, i, generation);
        }
        reader.refresh().unwrap();

        // A label no row carries any more must return nothing at all. This is
        // the live failure: twenty arbitrary characters came back for a label
        // that did not exist.
        let gone = labels(0, generation - 1);
        let got = aliases_for_label(&mut reader, &gone[0]);
        assert!(
            got.is_empty(),
            "generation {generation}: no row carries {:?}, yet the reader returned {got:?}",
            gone[0]
        );

        // And a label that does exist resolves to exactly its own asset.
        for i in [0usize, ASSETS / 2, ASSETS - 1] {
            let live = labels(i, generation);
            assert_eq!(
                aliases_for_label(&mut reader, &live[0]),
                BTreeSet::from([alias(i)]),
                "generation {generation}: {:?} belongs to {} alone",
                live[0],
                alias(i)
            );
        }

        // The table side stayed fresh throughout the live failure while the
        // label predicate did not filter, so it is checked separately.
        let desc = reader
            .query(
                "SELECT description FROM search_annotations WHERE canon_alias = ?1",
                &[Value::text(alias(3))],
            )
            .unwrap();
        assert_eq!(
            desc.rows[0][0].as_text(),
            Some(format!("generation {generation} description for 3").as_str()),
            "generation {generation}: the description is stale"
        );

        let n = reader
            .query("SELECT COUNT(*) FROM search_labels", &[])
            .unwrap();
        assert_eq!(
            n.rows[0][0].as_integer(),
            Some((ASSETS * LABELS_PER_ASSET) as i64),
            "generation {generation}: the label count drifted"
        );
    }
    // A connection owns the log for its whole life; the CLI cross-check runs
    // after both handles close and the log is handed back.
    drop(reader);
    drop(w);
    if have_sqlite3() {
        assert_eq!(sqlite3(&path, "PRAGMA integrity_check;\n").trim(), "ok");
    }
}
