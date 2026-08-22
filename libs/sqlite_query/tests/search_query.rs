//! The asset store's real search statement, which exercises nearly every
//! feature at once: a compound subquery in FROM, a join onto it, an aggregate
//! over a CASE, GROUP BY with HAVING COUNT(DISTINCT …), bare columns, ORDER BY
//! on a result alias, and bound parameters throughout.

mod common;

use common::*;
use makepad_sqlite::{Connection, Value};
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
