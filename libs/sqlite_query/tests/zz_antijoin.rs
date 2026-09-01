//! Does the engine answer an anti-join the way sqlite does, at real size?
mod common;
use common::*;
use makepad_sqlite::{Database, Value};

/// 61k items, the first 2k of them carrying ~20 terms each — the exact shape
/// the picture cache was in when the engine answered "nothing left to index".
fn setup(n_items: usize, n_indexed: usize) -> String {
    let mut s = String::from(
        "CREATE TABLE items(id INTEGER PRIMARY KEY, title TEXT NOT NULL, extra TEXT NOT NULL);
         CREATE TABLE terms(term TEXT NOT NULL, item INTEGER NOT NULL, UNIQUE(term, item));
         CREATE INDEX terms_term ON terms(term, item);
         CREATE INDEX terms_item ON terms(item);
         BEGIN;",
    );
    for i in 1..=n_items {
        s.push_str(&format!("INSERT INTO items(id,title,extra) VALUES({i},'t{i}','{{}}');"));
    }
    for i in 1..=n_indexed {
        for w in 0..20 {
            s.push_str(&format!("INSERT INTO terms(term,item) VALUES('w{}',{i});", (i * 31 + w) % 5000));
        }
    }
    s.push_str("COMMIT;");
    s
}

#[test]
fn the_unindexed_rows_are_found_at_real_size() {
    if !have_sqlite3() { eprintln!("no sqlite3 CLI; skipping"); return }
    let s = Scratch::new("antijoin-big");
    let db = build_db(s.path("").parent().unwrap(), "big2.sqlite", &setup(61_000, 2_000));

    let sql = "SELECT COUNT(*) FROM items i LEFT JOIN terms t ON t.item = i.id WHERE t.item IS NULL";
    let theirs = sqlite3_column(&db, sql);
    let mut d = Database::open(&db).unwrap();
    let mine: Vec<Value> = d.query(sql, &[]).unwrap().rows.iter().map(|r| r[0].clone()).collect();
    assert_eq!(mine, theirs, "\nsqlite3 said {theirs:?}\nours said   {mine:?}");

    // And the actual batch query the indexer used.
    let batch = "SELECT i.id, i.title, i.extra FROM items i LEFT JOIN terms t ON t.item = i.id \
                 WHERE t.item IS NULL LIMIT 2000";
    let theirs_n = sqlite3(&db, batch).lines().count();
    let mine_n = d
        .query("SELECT i.id, i.title, i.extra FROM items i LEFT JOIN terms t ON t.item = i.id WHERE t.item IS NULL LIMIT ?", &[Value::Integer(2000)])
        .unwrap()
        .rows
        .len();
    assert_eq!(mine_n, theirs_n, "batch query: sqlite3 gave {theirs_n} rows, ours gave {mine_n}");
}
