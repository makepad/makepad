//! The asset store's real search statements, which exercise nearly every
//! feature at once: a compound subquery in FROM, a join onto it, an aggregate
//! over a CASE, GROUP BY with HAVING COUNT(DISTINCT …), bare columns, ORDER BY
//! on a result alias, and bound parameters throughout.
//!
//! The second statement is the faceted one: the same candidate set, this time
//! as a derived table on the *right* of a join, grouped two columns deep and
//! ordered by the count's alias.

mod common;

use common::*;
use makepad_sqlite::{Connection, Database, Value};
use std::time::Duration;

const SQL: &str = "SELECT a.asset_id, a.namespace, a.title, a.description, a.live, SUM((CASE WHEN a.owner IS NOT NULL AND a.owner = ?1 THEN p.weight_owner ELSE p.weight_public END)) AS score, a.kind, a.canon_alias FROM ( SELECT term, asset_id, weight_public, weight_owner FROM search_postings UNION ALL SELECT term, asset_id, weight, weight FROM search_alias_postings ) p JOIN search_annotations a ON a.asset_id = p.asset_id WHERE p.term IN (?2,?3) AND (CASE WHEN a.owner IS NOT NULL AND a.owner = ?4 THEN p.weight_owner ELSE p.weight_public END) > 0 AND (a.visibility = 'public' OR (a.owner IS NOT NULL AND a.owner = ?5)) GROUP BY a.asset_id HAVING COUNT(DISTINCT p.term) = ?6 ORDER BY score DESC, a.canon_alias ASC, a.asset_id ASC LIMIT ?7";

#[test]
fn the_stores_search_query_matches_sqlite() {
    if !have_sqlite3() { return; }
    let scratch = Scratch::new("search-repro");
    let path = scratch.path("s.db");
    let schema = "CREATE TABLE search_annotations(asset_id BLOB PRIMARY KEY, namespace TEXT NOT NULL, kind TEXT, visibility TEXT NOT NULL, owner BLOB, title TEXT NOT NULL, description TEXT NOT NULL, live INTEGER NOT NULL, canon_alias TEXT NOT NULL DEFAULT '');
CREATE TABLE search_postings(term TEXT NOT NULL, asset_id BLOB NOT NULL, weight_public INTEGER NOT NULL, weight_owner INTEGER NOT NULL, PRIMARY KEY(term, asset_id));
CREATE TABLE search_alias_postings(term TEXT NOT NULL, asset_id BLOB NOT NULL, weight INTEGER NOT NULL, PRIMARY KEY(term, asset_id));";
    {
        let mut db = Connection::open(&path, Duration::from_secs(5)).unwrap();
        db.execute_batch(schema).unwrap();
        for (id, title, alias) in [(1u8,"mesh from image","alpha"), (2,"other thing","beta"), (3,"mesh image world","gamma")] {
            db.execute("INSERT INTO search_annotations(asset_id,namespace,kind,visibility,owner,title,description,live,canon_alias) VALUES(?1,'ns','mesh','public',NULL,?2,'',1,?3)",
                &[Value::Blob(vec![id;4]), Value::text(title), Value::text(alias)]).unwrap();
            for (t, w) in [("mesh", 3i64), ("image", 2)] {
                if id == 2 && t == "mesh" { continue; }
                db.execute("INSERT INTO search_postings(term,asset_id,weight_public,weight_owner) VALUES(?1,?2,?3,?3)",
                    &[Value::text(t), Value::Blob(vec![id;4]), Value::Integer(w)]).unwrap();
            }
            db.execute("INSERT INTO search_alias_postings(term,asset_id,weight) VALUES(?1,?2,1)",
                &[Value::text(alias), Value::Blob(vec![id;4])]).unwrap();
        }
    }
    let params = vec![Value::Null, Value::text("mesh"), Value::text("image"), Value::Null, Value::Null, Value::Integer(2), Value::Integer(10)];
    let mut db = Connection::open(&path, Duration::from_secs(5)).unwrap();
    let ours = db.query(SQL, &params).unwrap();

    // the CLI with the parameters inlined
    let cli_sql = SQL.replace("?1","NULL").replace("?2","'mesh'").replace("?3","'image'")
        .replace("?4","NULL").replace("?5","NULL").replace("?6","2").replace("?7","10");
    let out = sqlite3(&path, &format!(".mode quote\n.headers off\n.separator |\n{cli_sql};\n"));
    assert_eq!(
        ours.to_quoted_lines(),
        out.lines().map(|l| l.to_string()).collect::<Vec<_>>(),
        "the store's search query must return exactly what sqlite3 returns"
    );
    assert_eq!(ours.rows.len(), 2);
}

// ---------------------------------------------------------------------------
// The faceted statement
// ---------------------------------------------------------------------------

/// The candidate set the store builds when nothing is being searched for, and
/// the one it builds for a text search. Both go into the facet query as a
/// derived table on the right of the join.
const BROWSE_CANDIDATES: &str = "SELECT a.asset_id, a.namespace, a.title, a.description, a.live, 0 AS score, a.kind, a.canon_alias FROM search_annotations a WHERE 1=1 AND (a.visibility = 'public' OR (a.owner IS NOT NULL AND a.owner = ?))";

const TEXT_CANDIDATES: &str = "SELECT a.asset_id, a.namespace, a.title, a.description, a.live, SUM((CASE WHEN a.owner IS NOT NULL AND a.owner = ? THEN p.weight_owner ELSE p.weight_public END)) AS score, a.kind, a.canon_alias FROM ( SELECT term, asset_id, weight_public, weight_owner FROM search_postings UNION ALL SELECT term, asset_id, weight, weight FROM search_alias_postings ) p JOIN search_annotations a ON a.asset_id = p.asset_id WHERE p.term IN (?) AND (a.visibility = 'public' OR (a.owner IS NOT NULL AND a.owner = ?)) GROUP BY a.asset_id";

/// `build_facet_sql`, verbatim.
fn facet_sql(candidates: &str) -> String {
    format!(
        "SELECT l.kind, l.label, COUNT(*) AS n
         FROM search_labels l
         JOIN ({candidates}) c ON c.asset_id = l.asset_id
         GROUP BY l.kind, l.label
         ORDER BY n DESC, l.kind ASC, l.label ASC
         LIMIT ?"
    )
}

const FACET_SCHEMA: &str = r#"
CREATE TABLE search_annotations(asset_id BLOB PRIMARY KEY, namespace TEXT NOT NULL, kind TEXT, visibility TEXT NOT NULL, owner BLOB, title TEXT NOT NULL, description TEXT NOT NULL, live INTEGER NOT NULL, canon_alias TEXT NOT NULL DEFAULT '');
CREATE TABLE search_postings(term TEXT NOT NULL, asset_id BLOB NOT NULL, weight_public INTEGER NOT NULL, weight_owner INTEGER NOT NULL, PRIMARY KEY(term, asset_id));
CREATE TABLE search_alias_postings(term TEXT NOT NULL, asset_id BLOB NOT NULL, weight INTEGER NOT NULL, PRIMARY KEY(term, asset_id));
CREATE TABLE search_labels(asset_id BLOB NOT NULL, kind TEXT NOT NULL, label TEXT NOT NULL);
CREATE INDEX search_labels_by_asset ON search_labels(asset_id);

INSERT INTO search_annotations
  SELECT printf('a%04d', i), 'ns',
         CASE i%3 WHEN 0 THEN 'mesh' WHEN 1 THEN 'audio' ELSE 'image' END,
         CASE WHEN i%10 = 0 THEN 'private' ELSE 'public' END,
         NULL, printf('title %d', i), '', 1, printf('alias-%04d', i)
  FROM (WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i < 60) SELECT i FROM s);
INSERT INTO search_labels
  SELECT printf('a%04d', i), 'tag', printf('t%d', i%7)
  FROM (WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i < 60) SELECT i FROM s);
INSERT INTO search_labels
  SELECT printf('a%04d', i), 'category', printf('c%d', i%4)
  FROM (WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i < 60) SELECT i FROM s);
INSERT INTO search_postings
  SELECT printf('term%d', i%12), printf('a%04d', i), 3, 5
  FROM (WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i < 60) SELECT i FROM s);
INSERT INTO search_postings
  SELECT printf('other%d', i%5), printf('a%04d', i), 2, 4
  FROM (WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i < 60) SELECT i FROM s);
INSERT INTO search_alias_postings
  SELECT printf('alias-%04d', i), printf('a%04d', i), 1
  FROM (WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM s WHERE i < 60) SELECT i FROM s);
"#;

/// Substitute each bound value into the statement text, left to right, so the
/// CLI runs the identical query.
fn inline(sql: &str, literals: &[&str]) -> String {
    let mut out = String::new();
    let mut next = literals.iter();
    for ch in sql.chars() {
        if ch == '?' {
            out.push_str(next.next().expect("a literal for every parameter"));
        } else {
            out.push(ch);
        }
    }
    out
}

fn ask_cli(path: &std::path::Path, sql: &str) -> Vec<String> {
    sqlite3(path, &format!(".mode quote\n.headers off\n.separator |\n{sql};\n"))
        .lines()
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn the_stores_facet_query_matches_sqlite() {
    if !have_sqlite3() {
        return;
    }
    // The facet statement puts the candidate set on the right of the join,
    // where its columns are reached as `c.asset_id`. It then groups two columns
    // deep, counts, and orders by the count's alias — so this covers the whole
    // path from resolving the derived table to emitting grouped rows in the
    // right order, not merely preparing the statement.
    let scratch = Scratch::new("facet");
    let path = build_db(&scratch.dir, "f.db", FACET_SCHEMA);
    let mut db = Connection::open(&path, Duration::from_secs(5)).unwrap();

    // Browsing: every public asset is a candidate.
    let browse = facet_sql(BROWSE_CANDIDATES);
    let ours = db
        .query(
            &browse,
            &[Value::text(""), Value::Integer(12)],
        )
        .expect("the browse facet query must run");
    assert_eq!(
        ours.columns,
        vec!["kind", "label", "n"],
        "a facet row is (kind, label, count)"
    );
    assert_eq!(
        ours.to_quoted_lines(),
        ask_cli(&path, &inline(&browse, &["''", "12"])),
        "browse facets must be exactly what sqlite3 returns"
    );
    // 4 categories + 7 tags, minus the six private assets' contribution.
    assert_eq!(ours.rows.len(), 11);
    // Ordered by count descending: the first row is at least as big as the last.
    let first = ours.rows[0][2].as_integer().unwrap();
    let last = ours.rows[ours.rows.len() - 1][2].as_integer().unwrap();
    assert!(first >= last, "{:?}", ours.to_quoted_lines());
    assert_eq!(
        ours.rows.iter().map(|r| r[2].as_integer().unwrap()).sum::<i64>(),
        108,
        "every label of every public asset is counted once"
    );

    // Text search: the candidate set is itself a join onto a compound subquery.
    let text = facet_sql(TEXT_CANDIDATES);
    let ours = db
        .query(
            &text,
            &[
                Value::text(""),
                Value::text("term3"),
                Value::text(""),
                Value::Integer(12),
            ],
        )
        .expect("the text facet query must run");
    assert_eq!(
        ours.to_quoted_lines(),
        ask_cli(&path, &inline(&text, &["''", "'term3'", "''", "12"])),
        "text-search facets must be exactly what sqlite3 returns"
    );
    assert!(!ours.rows.is_empty(), "term3 has matching assets");

    // LIMIT applies after the ordering, so asking for fewer keeps the top rows.
    let two = db
        .query(&browse, &[Value::text(""), Value::Integer(2)])
        .unwrap();
    assert_eq!(two.rows.len(), 2);
    assert_eq!(
        two.to_quoted_lines(),
        ask_cli(&path, &inline(&browse, &["''", "2"]))
    );
}

#[test]
fn a_faceted_search_does_not_buffer_the_whole_posting_list() {
    if !have_sqlite3() {
        return;
    }
    // The term is written in the outer WHERE, but the posting list it selects
    // from is a derived table. Unless the term reaches inside, every posting in
    // the database is materialized before a single row is filtered — which on a
    // real catalog means the query dies on its row budget instead of answering.
    let scratch = Scratch::new("facet-budget");
    let path = build_db(&scratch.dir, "b.db", FACET_SCHEMA);
    let mut db = Database::open(&path).unwrap();
    // The posting arm of the derived table holds 120 rows on its own, and the
    // query is given room for fewer than that.
    db.limits_mut().max_rows = 100;

    let text = facet_sql(TEXT_CANDIDATES);
    let params = [
        Value::text(""),
        Value::text("term3"),
        Value::text(""),
        Value::Integer(12),
    ];
    let ours = db
        .query(&text, &params)
        .expect("the posting list must be filtered as it is scanned, not buffered whole");
    assert_eq!(
        ours.to_quoted_lines(),
        ask_cli(&path, &inline(&text, &["''", "'term3'", "''", "12"]))
    );
    assert!(!ours.rows.is_empty());

    // The budget is still real: a term that selects everything cannot fit.
    let all = facet_sql(
        "SELECT a.asset_id, 0 AS score FROM ( SELECT term, asset_id FROM search_postings UNION ALL SELECT term, asset_id FROM search_alias_postings ) p JOIN search_annotations a ON a.asset_id = p.asset_id WHERE p.term IS NOT NULL AND a.namespace = ? GROUP BY a.asset_id",
    );
    let mut tight = Database::open(&path).unwrap();
    tight.limits_mut().max_rows = 100;
    assert!(
        tight
            .query(&all, &[Value::text("ns"), Value::Integer(12)])
            .is_err(),
        "an unfiltered posting list still has to trip the budget"
    );
}
