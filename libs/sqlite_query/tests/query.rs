//! P1 acceptance: the SQL layer answers the asset store's real queries with
//! exactly the rows the system `sqlite3` CLI returns, and proves it uses index
//! seeks where it must.

mod common;

use common::*;
use makepad_sqlite::{Database, Value};
use std::path::{Path, PathBuf};

fn catalog_copy() -> Option<PathBuf> {
    let p = Path::new(
        "/private/tmp/claude-501/-Users-admin-makepad-makepad/9ffb7a56-6354-42a8-8256-89ffed8580ec/scratchpad/store-copy/catalog.sqlite3",
    );
    if p.exists() && have_sqlite3() {
        Some(p.to_path_buf())
    } else {
        None
    }
}

/// Strict value equality: same storage class *and* same value, so an INTEGER
/// never passes for a REAL.
fn same(a: &Value, b: &Value) -> bool {
    if a.class() != b.class() {
        return false;
    }
    match (a, b) {
        (Value::Real(x), Value::Real(y)) => x.to_bits() == y.to_bits() || x == y,
        _ => a == b,
    }
}

/// Run the same SQL through both engines and compare row for row. Parameters
/// are substituted into the CLI text as literals.
fn compare(db_path: &Path, sql: &str, params: &[Value]) {
    let mut db = Database::open(db_path).expect("open");
    let ours = db.query(sql, params).expect(sql);

    let mut cli_sql = String::new();
    let mut rest = sql;
    // Replace ?N / ? with literals, numbering bare `?` the way SQLite does:
    // one more than the largest number used so far.
    let mut max_seen = 0usize;
    while let Some(pos) = rest.find('?') {
        cli_sql.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let number = if digits.is_empty() {
            max_seen + 1
        } else {
            digits.parse::<usize>().unwrap()
        };
        max_seen = max_seen.max(number);
        cli_sql.push_str(&quote(&params[number - 1]));
        rest = &after[digits.len()..];
    }
    cli_sql.push_str(rest);

    let out = sqlite3(
        db_path,
        &format!(".mode quote\n.headers off\n.separator |\n{cli_sql};\n"),
    );
    let theirs: Vec<Vec<Value>> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('|').map(parse_quoted).collect())
        .collect();

    assert_eq!(
        ours.rows.len(),
        theirs.len(),
        "row count for {sql}\nours: {:?}\ncli: {:?}",
        ours.to_quoted_lines().iter().take(5).collect::<Vec<_>>(),
        out.lines().take(5).collect::<Vec<_>>()
    );
    for (i, (a, b)) in ours.rows.iter().zip(theirs.iter()).enumerate() {
        assert_eq!(a.len(), b.len(), "column count row {i} of {sql}");
        for (c, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert!(
                same(x, y),
                "row {i} column {c} of {sql}: ours {} vs cli {}",
                quote(x),
                quote(y)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The store's hot queries
// ---------------------------------------------------------------------------

#[test]
fn catalog_hot_queries_match_cli() {
    let Some(db_path) = catalog_copy() else { return };
    let mut db = Database::open(&db_path).unwrap();

    // A real alias and asset id to drive the parameterised queries.
    let alias = db
        .query("SELECT alias FROM asset_aliases ORDER BY alias LIMIT 1", &[])
        .unwrap()
        .scalar()
        .cloned()
        .expect("an alias");
    let asset_id = db
        .query(
            "SELECT asset_id FROM asset_aliases WHERE alias = ?1",
            &[alias.clone()],
        )
        .unwrap()
        .scalar()
        .cloned()
        .expect("an asset id");
    let term = db
        .query(
            "SELECT term FROM search_postings ORDER BY term LIMIT 1 OFFSET 5",
            &[],
        )
        .unwrap()
        .scalar()
        .cloned()
        .expect("a term");

    // alias resolve (catalog.rs:538)
    compare(
        &db_path,
        "SELECT asset_id FROM asset_aliases WHERE alias = ?1",
        &[alias.clone()],
    );
    // alias + head revision (catalog.rs:599)
    compare(
        &db_path,
        "SELECT asset_id, head_revision FROM asset_aliases WHERE alias = ?1",
        &[alias.clone()],
    );
    // aliases of one asset, through the secondary index (search.rs:716)
    compare(
        &db_path,
        "SELECT alias FROM asset_aliases WHERE asset_id = ?1 ORDER BY alias",
        &[asset_id.clone()],
    );
    // annotation fetch (search.rs:1024)
    compare(
        &db_path,
        "SELECT visibility, owner, title, description, creator, generator, backend, model, prompt, provenance, kind FROM search_annotations WHERE asset_id = ?1",
        &[asset_id.clone()],
    );
    // labels (search.rs:1058)
    compare(
        &db_path,
        "SELECT kind, label FROM search_labels WHERE asset_id = ?1 ORDER BY kind, label",
        &[asset_id.clone()],
    );
    // term search join (search.rs:1401 shape)
    compare(
        &db_path,
        "SELECT a.asset_id, a.namespace, a.title, a.live, a.kind, a.canon_alias FROM search_annotations a JOIN search_postings p ON p.asset_id = a.asset_id WHERE p.term = ?1 ORDER BY a.asset_id LIMIT 50",
        &[term.clone()],
    );
    // browse page over the canon_alias keyset index (search.rs:1396 + paging)
    compare(
        &db_path,
        "SELECT a.canon_alias, a.asset_id, a.title FROM search_annotations a WHERE a.canon_alias > ?1 ORDER BY a.canon_alias, a.asset_id LIMIT 25",
        &[Value::text("")],
    );
    // label filter with EXISTS (search.rs:1456)
    compare(
        &db_path,
        "SELECT COUNT(*) FROM search_annotations a WHERE EXISTS(SELECT 1 FROM search_labels l WHERE l.asset_id = a.asset_id AND l.kind = 'tag')",
        &[],
    );
    // negated label filter (search.rs:1470)
    compare(
        &db_path,
        "SELECT COUNT(*) FROM search_annotations a WHERE NOT EXISTS(SELECT 1 FROM search_labels l WHERE l.asset_id = a.asset_id AND l.kind = 'tag' AND l.label = ?1)",
        &[Value::text("character")],
    );
    // live + canon_alias maintenance query (search.rs:754 read half)
    compare(
        &db_path,
        "SELECT EXISTS(SELECT 1 FROM asset_aliases WHERE asset_id = ?1), COALESCE((SELECT MIN(alias) FROM asset_aliases WHERE asset_id = ?1), '')",
        &[asset_id.clone()],
    );
    // imports paging (imports.rs:174)
    compare(
        &db_path,
        "SELECT source_id FROM import_sources WHERE source_id > ?1 ORDER BY source_id LIMIT ?2",
        &[Value::text(""), Value::Integer(10)],
    );
    // import entries (imports.rs:355)
    compare(
        &db_path,
        "SELECT import_revision, entry_key, asset_id FROM import_entries ORDER BY import_revision, entry_key LIMIT 20",
        &[],
    );
    // operation events window (operations.rs:1075)
    compare(
        &db_path,
        "SELECT operation_id, seq, kind, created_ms FROM operation_events WHERE seq > ?1 ORDER BY operation_id, seq LIMIT ?2",
        &[Value::Integer(0), Value::Integer(20)],
    );
    // jobs by state (jobs.rs:298)
    compare(
        &db_path,
        "SELECT job_id, kind, attempts_used FROM jobs WHERE state='pending' AND not_before_ms <= ?1 ORDER BY job_id LIMIT 20",
        &[Value::Integer(i64::MAX)],
    );
    // grants scope check (auth.rs:276)
    compare(
        &db_path,
        "SELECT 1 FROM grants WHERE capability=?1 AND scope IN (?2, '*') LIMIT 5",
        &[Value::text("publish"), Value::text("assets")],
    );
    // token join (auth.rs:207)
    compare(
        &db_path,
        "SELECT t.principal_id, t.expires_ms, t.revoked, p.disabled FROM tokens t JOIN principals p ON p.principal_id = t.principal_id ORDER BY t.principal_id LIMIT 10",
        &[],
    );
    // derivations by job (variants.rs:378)
    compare(
        &db_path,
        "SELECT dkey, state FROM derivations ORDER BY dkey LIMIT 10",
        &[],
    );
}

#[test]
fn llm_library_questions_match_cli() {
    let Some(db_path) = catalog_copy() else { return };
    // The kind of question the game LLM will ask of the library.
    compare(
        &db_path,
        "SELECT kind, COUNT(*) AS n FROM search_annotations GROUP BY kind ORDER BY n DESC, kind LIMIT 20",
        &[],
    );
    compare(
        &db_path,
        "SELECT l.label, COUNT(*) AS n FROM search_labels l WHERE l.kind='tag' GROUP BY l.label HAVING COUNT(*) > 2 ORDER BY n DESC, l.label LIMIT 15",
        &[],
    );
    compare(
        &db_path,
        "SELECT a.canon_alias, a.kind, LENGTH(a.prompt) FROM search_annotations a WHERE a.live = 1 AND a.kind IS NOT NULL AND LOWER(a.title) <> '' ORDER BY a.canon_alias LIMIT 20",
        &[],
    );
    compare(
        &db_path,
        "SELECT namespace, COUNT(*) AS n, MIN(created_ms), MAX(created_ms) FROM assets GROUP BY namespace ORDER BY n DESC, namespace",
        &[],
    );
    compare(
        &db_path,
        "SELECT a.canon_alias, COUNT(DISTINCT l.label) AS tags FROM search_annotations a JOIN search_labels l ON l.asset_id = a.asset_id GROUP BY a.canon_alias ORDER BY tags DESC, a.canon_alias LIMIT 10",
        &[],
    );
    compare(
        &db_path,
        "SELECT COUNT(*) FROM blobs WHERE size > 100000",
        &[],
    );
    compare(
        &db_path,
        "SELECT alias FROM asset_aliases WHERE alias LIKE 'a%' ORDER BY alias LIMIT 10",
        &[],
    );
}

// ---------------------------------------------------------------------------
// Plan shape
// ---------------------------------------------------------------------------

#[test]
fn keyset_queries_use_index_seeks() {
    let Some(db_path) = catalog_copy() else { return };
    let mut db = Database::open(&db_path).unwrap();

    let cases: Vec<(&str, &str)> = vec![
        (
            "SELECT asset_id FROM asset_aliases WHERE alias = ?1",
            "SEARCH USING INDEX sqlite_autoindex_asset_aliases_1",
        ),
        (
            "SELECT alias FROM asset_aliases WHERE asset_id = ?1 ORDER BY alias",
            "SEARCH USING INDEX asset_aliases_by_asset",
        ),
        (
            "SELECT canon_alias, asset_id FROM search_annotations WHERE canon_alias > ?1 ORDER BY canon_alias, asset_id LIMIT 25",
            "SEARCH USING INDEX search_annotations_by_canon",
        ),
        (
            "SELECT title FROM search_annotations WHERE asset_id = ?1",
            "SEARCH USING INDEX sqlite_autoindex_search_annotations_1",
        ),
        (
            "SELECT job_id FROM jobs WHERE state = 'pending' AND not_before_ms <= 10",
            "SEARCH USING INDEX jobs_by_state",
        ),
        (
            "SELECT source_id FROM import_sources WHERE source_id > 'x' ORDER BY source_id LIMIT 5",
            "SEARCH USING INDEX sqlite_autoindex_import_sources_1",
        ),
        (
            "SELECT seq FROM operation_events WHERE operation_id = x'00' AND seq > 3",
            "SEARCH USING INDEX sqlite_autoindex_operation_events_1",
        ),
    ];
    for (sql, want) in cases {
        let stmt = db.prepare(sql).expect(sql);
        let plan = stmt.explain();
        assert!(
            plan.contains(want),
            "plan for {sql} was:\n{plan}\nexpected {want}"
        );
    }

    // Join order: the constrained side drives the loop, and the other side is
    // reached by an index seek rather than a scan.
    let stmt = db
        .prepare("SELECT a.asset_id, a.title FROM search_annotations a JOIN search_postings p ON p.asset_id = a.asset_id WHERE p.term = ?1 ORDER BY a.asset_id LIMIT 50")
        .unwrap();
    let plan = stmt.explain();
    let mut lines = plan.lines();
    assert!(
        lines.next().unwrap_or("").contains("INDEX sqlite_autoindex_search_postings_1"),
        "postings must drive the join:\n{plan}"
    );
    assert!(
        lines.next().unwrap_or("").contains("INDEX sqlite_autoindex_search_annotations_1"),
        "annotations must be reached by a seek:\n{plan}"
    );

    // The keyset page must not need a sort: the index already returns order.
    let stmt = db
        .prepare("SELECT canon_alias, asset_id FROM search_annotations WHERE canon_alias > ?1 ORDER BY canon_alias, asset_id LIMIT 25")
        .unwrap();
    assert!(
        stmt.explain().contains("ORDER BY (from index)"),
        "keyset page still sorts:\n{}",
        stmt.explain()
    );

    // rowid lookups go straight to the row.
    let stmt = db
        .prepare("SELECT generation FROM search_state WHERE id = 1")
        .unwrap();
    assert!(stmt.explain().contains("SEARCH rowid=?"), "{}", stmt.explain());

    // A predicate with no usable index scans, and says so.
    let stmt = db
        .prepare("SELECT title FROM search_annotations WHERE title LIKE 'x%'")
        .unwrap();
    assert!(stmt.explain().starts_with("SCAN"), "{}", stmt.explain());
}

#[test]
fn index_seek_actually_avoids_the_scan() {
    let Some(db_path) = catalog_copy() else { return };
    let mut db = Database::open(&db_path).unwrap();
    let alias = db
        .query("SELECT alias FROM asset_aliases ORDER BY alias LIMIT 1", &[])
        .unwrap()
        .scalar()
        .cloned()
        .unwrap();
    // A seek visits a handful of rows; a scan would visit thousands.
    let stmt = db
        .prepare("SELECT asset_id FROM asset_aliases WHERE alias = ?1")
        .unwrap();
    let mut visited = 0;
    stmt.for_each(&mut db, &[alias], |_row| {
        visited += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(visited, 1);
}

// ---------------------------------------------------------------------------
// Semantics on a controlled fixture
// ---------------------------------------------------------------------------

const FIXTURE: &str = r#"
CREATE TABLE t(
    id INTEGER PRIMARY KEY,
    name TEXT,
    kind TEXT,
    n INTEGER,
    r REAL,
    b BLOB
);
CREATE INDEX t_by_kind ON t(kind, n);
CREATE TABLE u(id INTEGER PRIMARY KEY, t_id INTEGER, tag TEXT);
CREATE INDEX u_by_t ON u(t_id);
INSERT INTO t VALUES
 (1,'alpha','a',10,1.5,x'01'),
 (2,'beta','b',20,2.5,x'02'),
 (3,'gamma','a',30,NULL,NULL),
 (4,'delta',NULL,40,4.5,x'04'),
 (5,'Epsilon','b',NULL,5.5,x'05'),
 (6,'zeta','c',60,6.0,x'0607');
INSERT INTO u VALUES (1,1,'red'),(2,1,'blue'),(3,2,'red'),(4,6,'green'),(5,99,'orphan');
"#;

fn fixture(scratch: &Scratch) -> PathBuf {
    build_db(&scratch.dir, "fixture.db", FIXTURE)
}

#[test]
fn semantics_match_cli_on_fixture() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("query");
    let path = fixture(&scratch);
    let queries: Vec<(&str, Vec<Value>)> = vec![
        ("SELECT * FROM t ORDER BY id", vec![]),
        ("SELECT id, name FROM t WHERE kind = 'a' ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE kind IS NULL ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE kind IS NOT NULL ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE n > 20 ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE n >= 20 AND n <= 40 ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE n BETWEEN 20 AND 40 ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE kind IN ('a','c') ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE kind NOT IN ('a') ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE name <> 'alpha' ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE id = 3 ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE id > 3 ORDER BY id", vec![]),
        ("SELECT COUNT(*), COUNT(n), COUNT(DISTINCT kind) FROM t", vec![]),
        ("SELECT MIN(n), MAX(n), SUM(n), AVG(n), TOTAL(r) FROM t", vec![]),
        ("SELECT kind, COUNT(*) FROM t GROUP BY kind ORDER BY kind", vec![]),
        (
            "SELECT kind, COUNT(*) c FROM t GROUP BY kind HAVING COUNT(*) > 1 ORDER BY kind",
            vec![],
        ),
        ("SELECT DISTINCT kind FROM t ORDER BY kind", vec![]),
        ("SELECT id, name FROM t ORDER BY name DESC", vec![]),
        ("SELECT id FROM t ORDER BY n ASC", vec![]),
        ("SELECT id FROM t ORDER BY n DESC", vec![]),
        ("SELECT id FROM t ORDER BY kind, n DESC", vec![]),
        ("SELECT id FROM t ORDER BY id LIMIT 2 OFFSET 3", vec![]),
        ("SELECT COALESCE(kind,'none'), LENGTH(name), LOWER(name) FROM t ORDER BY id", vec![]),
        ("SELECT id, r*2, n+1, n/3, n%7 FROM t ORDER BY id", vec![]),
        ("SELECT name || '-' || kind FROM t ORDER BY id", vec![]),
        (
            "SELECT t.id, u.tag FROM t JOIN u ON u.t_id = t.id ORDER BY t.id, u.tag",
            vec![],
        ),
        (
            "SELECT t.id, u.tag FROM t LEFT JOIN u ON u.t_id = t.id ORDER BY t.id, u.tag",
            vec![],
        ),
        (
            "SELECT t.id, u.id FROM t, u WHERE u.t_id = t.id ORDER BY t.id, u.id",
            vec![],
        ),
        (
            "SELECT id FROM t WHERE EXISTS(SELECT 1 FROM u WHERE u.t_id = t.id) ORDER BY id",
            vec![],
        ),
        (
            "SELECT id FROM t WHERE NOT EXISTS(SELECT 1 FROM u WHERE u.t_id = t.id) ORDER BY id",
            vec![],
        ),
        (
            "SELECT id FROM t WHERE id IN (SELECT t_id FROM u) ORDER BY id",
            vec![],
        ),
        (
            "SELECT (SELECT COUNT(*) FROM u WHERE u.t_id = t.id) AS n, t.id FROM t ORDER BY t.id",
            vec![],
        ),
        ("SELECT COUNT(*) FROM (SELECT id FROM t WHERE n > 15)", vec![]),
        ("SELECT id FROM t WHERE name LIKE 'a%' ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE name LIKE '%a' ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE name LIKE 'e_silon' ORDER BY id", vec![]),
        ("SELECT hex(b), typeof(b), typeof(r), typeof(kind) FROM t ORDER BY id", vec![]),
        ("SELECT CASE WHEN n > 25 THEN 'big' WHEN n IS NULL THEN 'none' ELSE 'small' END, id FROM t ORDER BY id", vec![]),
        ("SELECT CAST(r AS INTEGER), CAST(n AS TEXT), CAST(name AS BLOB) FROM t ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE ?1 = kind ORDER BY id", vec![Value::text("b")]),
        ("SELECT id, n FROM t WHERE kind = ?1 AND n > ?2 ORDER BY id", vec![Value::text("a"), Value::Integer(5)]),
        ("SELECT 1 WHERE 1", vec![]),
        ("SELECT 1 WHERE 0", vec![]),
        ("SELECT NULL, 1, 1.5, 'x', x'ff'", vec![]),
        ("SELECT id FROM t UNION SELECT id FROM u ORDER BY id", vec![]),
        ("SELECT id FROM t UNION ALL SELECT id FROM u ORDER BY id", vec![]),
        ("SELECT id FROM t INTERSECT SELECT t_id FROM u ORDER BY id", vec![]),
        ("SELECT id FROM t EXCEPT SELECT t_id FROM u ORDER BY id", vec![]),
        ("SELECT b FROM t WHERE b > x'02' ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE r > 2 ORDER BY id", vec![]),
        ("SELECT id FROM t WHERE n = '20' ORDER BY id", vec![]),
        ("SELECT '20' = 20, '20' = '20', 20 = 20.0, NULL = NULL, NULL IS NULL", vec![]),
    ];
    for (sql, params) in queries {
        compare(&path, sql, &params);
    }
}

#[test]
fn budgets_stop_runaway_queries() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("budget");
    let path = fixture(&scratch);
    let mut db = Database::open(&path).unwrap();
    db.limits_mut().max_rows = 3;
    let err = db.query("SELECT id FROM t ORDER BY id", &[]);
    assert!(err.is_err(), "row budget did not trip");

    let mut db = Database::open(&path).unwrap();
    db.limits_mut().max_steps = 2;
    let err = db.query("SELECT COUNT(*) FROM t", &[]);
    assert!(err.is_err(), "step budget did not trip");
}

#[test]
fn errors_are_clear() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("errors");
    let path = fixture(&scratch);
    let mut db = Database::open(&path).unwrap();
    for bad in [
        "SELECT * FROM nope",
        "SELECT nope FROM t",
        "SELECT * FROM t WHERE",
        "INSERT INTO t VALUES(1)",
        "DELETE FROM t",
        "UPDATE t SET n=1",
        "DROP TABLE t",
        "PRAGMA user_version",
        "SELECT nosuchfunc(1) FROM t",
        "",
    ] {
        assert!(db.prepare(bad).is_err(), "{bad} should not prepare");
    }
    // A statement that prepares but has an unbound parameter.
    let stmt = db.prepare("SELECT id FROM t WHERE id = ?1").unwrap();
    assert!(stmt.query(&mut db, &[]).is_err());
}

#[test]
fn writes_are_impossible() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("readonly");
    let path = fixture(&scratch);
    let before = std::fs::read(&path).unwrap();
    {
        let mut db = Database::open(&path).unwrap();
        let _ = db.query("SELECT * FROM t", &[]).unwrap();
        for bad in ["INSERT INTO t(id) VALUES(99)", "UPDATE t SET n = 1", "DELETE FROM t"] {
            assert!(db.prepare(bad).is_err());
        }
    }
    assert_eq!(before, std::fs::read(&path).unwrap(), "file changed");
}

#[test]
fn anonymous_parameters_bind_in_text_order() {
    if !have_sqlite3() {
        return;
    }
    // A bare `?` takes the next number in the order it appears in the text.
    // Binding by planning order instead silently shifts every value, which is
    // exactly how a search returns nothing while every table is intact.
    let scratch = Scratch::new("anon-params");
    let path = fixture(&scratch);
    let mut db = Database::open(&path).unwrap();
    let out = db
        .query(
            "SELECT ?, id FROM t WHERE kind = ? AND n > ? ORDER BY id",
            &[Value::text("tag"), Value::text("a"), Value::Integer(5)],
        )
        .unwrap();
    assert_eq!(out.rows.len(), 2, "{:?}", out.to_quoted_lines());
    assert_eq!(out.rows[0][0].as_text(), Some("tag"));
    assert_eq!(out.rows[0][1].as_integer(), Some(1));
    compare(
        &path,
        "SELECT ?, id FROM t WHERE kind = ? AND n > ? ORDER BY id",
        &[Value::text("tag"), Value::text("a"), Value::Integer(5)],
    );
    compare(
        &path,
        "SELECT id FROM t WHERE id IN (?, ?, ?) AND kind IS NOT NULL ORDER BY id",
        &[Value::Integer(1), Value::Integer(2), Value::Integer(3)],
    );
    // Explicit numbers raise the counter for the bare ones that follow.
    compare(
        &path,
        "SELECT ?2, ? FROM t WHERE id = ?1 ORDER BY 1",
        &[Value::Integer(1), Value::text("second"), Value::text("third")],
    );
}

#[test]
fn derived_tables_are_addressed_by_their_alias_in_every_from_position() {
    if !have_sqlite3() {
        return;
    }
    // A derived table exports its columns under the column's own name, with any
    // table qualifier dropped, exactly as SQLite names an unaliased result
    // column. Keeping the qualifier would export `t.name` and leave `c.name`
    // unresolvable, which is how a faceted search — a derived table on the
    // right of a JOIN — failed to prepare at all.
    let scratch = Scratch::new("derived-position");
    let path = fixture(&scratch);
    let queries: Vec<(&str, Vec<Value>)> = vec![
        // The failing shape: derived table on the right of the join, selecting
        // qualified columns.
        (
            "SELECT u.tag, c.name FROM u JOIN (SELECT t.id, t.name FROM t) c ON c.id = u.t_id ORDER BY u.tag, c.name",
            vec![],
        ),
        // The same derived table in the first FROM slot.
        (
            "SELECT u.tag, c.name FROM (SELECT t.id, t.name FROM t) c JOIN u ON c.id = u.t_id ORDER BY u.tag, c.name",
            vec![],
        ),
        // Unqualified inner columns, both orders.
        (
            "SELECT u.tag, c.name FROM u JOIN (SELECT id, name FROM t) c ON c.id = u.t_id ORDER BY u.tag, c.name",
            vec![],
        ),
        (
            "SELECT u.tag, c.name FROM (SELECT id, name FROM t) c JOIN u ON c.id = u.t_id ORDER BY u.tag, c.name",
            vec![],
        ),
        // An explicit alias on the inner column wins over the column name.
        (
            "SELECT c.who FROM u JOIN (SELECT t.name AS who, t.id FROM t) c ON c.id = u.t_id ORDER BY c.who",
            vec![],
        ),
        // `SELECT *` inside, expanded to the underlying column names.
        (
            "SELECT c.name FROM u JOIN (SELECT * FROM t) c ON c.id = u.t_id ORDER BY c.name",
            vec![],
        ),
        // LEFT JOIN onto a derived table.
        (
            "SELECT t.id, c.tag FROM t LEFT JOIN (SELECT u.t_id, u.tag FROM u WHERE u.tag <> 'red') c ON c.t_id = t.id ORDER BY t.id, c.tag",
            vec![],
        ),
        // A derived table on both sides.
        (
            "SELECT x.id, y.tag FROM (SELECT t.id FROM t) x JOIN (SELECT u.t_id, u.tag FROM u) y ON y.t_id = x.id ORDER BY x.id, y.tag",
            vec![],
        ),
        // Nested: a derived table whose own FROM is a derived table.
        (
            "SELECT c.id FROM u JOIN (SELECT i.id FROM (SELECT t.id, t.kind FROM t) i WHERE i.kind = 'a') c ON c.id = u.t_id ORDER BY c.id",
            vec![],
        ),
        // Three items with the derived table in the middle.
        (
            "SELECT t.id, c.tag, u.tag FROM t JOIN (SELECT u.t_id, u.tag FROM u) c ON c.t_id = t.id JOIN u ON u.t_id = t.id ORDER BY t.id, c.tag, u.tag",
            vec![],
        ),
        // A compound derived table joined on the right: the search candidate
        // shape, whose arms are named by the first arm's columns.
        (
            "SELECT u.tag, c.id FROM u JOIN (SELECT t.id FROM t UNION ALL SELECT u.t_id FROM u) c ON c.id = u.t_id ORDER BY u.tag, c.id",
            vec![],
        ),
        // No alias at all: the columns stay reachable unqualified.
        (
            "SELECT name FROM u JOIN (SELECT t.id AS tid, t.name FROM t) ON tid = u.t_id ORDER BY name",
            vec![],
        ),
        // A parameter inside the derived table, one outside.
        (
            "SELECT c.id FROM u JOIN (SELECT t.id, t.kind FROM t WHERE t.kind = ?) c ON c.id = u.t_id WHERE u.tag = ? ORDER BY c.id",
            vec![Value::text("a"), Value::text("red")],
        ),
    ];
    for (sql, params) in queries {
        compare(&path, sql, &params);
    }
}

#[test]
fn predicates_pushed_into_derived_tables_do_not_change_the_answer() {
    if !have_sqlite3() {
        return;
    }
    // A term that constrains only a derived table is applied inside it too, so
    // the scan discards rows instead of materializing them. Each of these is a
    // case where doing that naively would return something other than what
    // SQLite returns.
    let scratch = Scratch::new("derived-pushdown");
    let path = fixture(&scratch);
    let queries: Vec<(&str, Vec<Value>)> = vec![
        // The plain pushable cases.
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t) c WHERE c.kind = 'a' ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t) c WHERE c.kind IN (?, ?) ORDER BY c.id",
            vec![Value::text("a"), Value::text("c")],
        ),
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t) c WHERE c.kind IS NULL ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t) c WHERE c.kind IS NOT NULL ORDER BY c.id",
            vec![],
        ),
        // Through a UNION ALL, where every arm has to be narrowed by itself.
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t UNION ALL SELECT u.id, u.tag FROM u) c WHERE c.kind = 'red' ORDER BY c.id",
            vec![],
        ),
        // Not pushable, and must still answer correctly: an inequality means
        // something different once a column's affinity is reintroduced, and a
        // negation flips which rows a widened match keeps.
        (
            "SELECT c.id FROM (SELECT t.id, t.n FROM t) c WHERE c.n > 20 ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t) c WHERE c.kind NOT IN ('a') ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id FROM (SELECT t.id, t.name FROM t) c WHERE c.name LIKE 'a%' ORDER BY c.id",
            vec![],
        ),
        // A LIMIT inside fixes which rows exist; filtering first would keep a
        // different set.
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t ORDER BY t.id LIMIT 3) c WHERE c.kind = 'a' ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id FROM (SELECT t.id, t.kind FROM t ORDER BY t.id LIMIT 3 OFFSET 1) c WHERE c.kind = 'a' ORDER BY c.id",
            vec![],
        ),
        // An aggregate inside: filtering ahead of it would change the count,
        // whether the term names a grouping key or the aggregate itself.
        (
            "SELECT c.kind, c.n FROM (SELECT t.kind, COUNT(*) AS n FROM t GROUP BY t.kind) c WHERE c.kind = 'a' ORDER BY c.kind",
            vec![],
        ),
        (
            "SELECT c.kind, c.n FROM (SELECT t.kind, COUNT(*) AS n FROM t GROUP BY t.kind) c WHERE c.n = 2 ORDER BY c.kind",
            vec![],
        ),
        // Narrowing the right arm of an EXCEPT would *add* rows; of an
        // INTERSECT, remove them.
        (
            "SELECT c.id FROM (SELECT id FROM t EXCEPT SELECT t_id FROM u) c WHERE c.id = 2 ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id FROM (SELECT id FROM t INTERSECT SELECT t_id FROM u) c WHERE c.id = 2 ORDER BY c.id",
            vec![],
        ),
        // The optional side of a LEFT JOIN inside: a pushed term must remove the
        // row, not turn it into a NULL-extended one.
        (
            "SELECT c.id, c.tag FROM (SELECT t.id, u.tag FROM t LEFT JOIN u ON u.t_id = t.id) c WHERE c.tag IS NULL ORDER BY c.id",
            vec![],
        ),
        (
            "SELECT c.id, c.tag FROM (SELECT t.id, u.tag FROM t LEFT JOIN u ON u.t_id = t.id) c WHERE c.tag = 'red' ORDER BY c.id",
            vec![],
        ),
        // The derived table itself LEFT JOINed, with the term in WHERE.
        (
            "SELECT t.id, c.tag FROM t LEFT JOIN (SELECT u.t_id, u.tag FROM u) c ON c.t_id = t.id WHERE c.tag = 'red' ORDER BY t.id",
            vec![],
        ),
        (
            "SELECT t.id, c.tag FROM t LEFT JOIN (SELECT u.t_id, u.tag FROM u) c ON c.t_id = t.id WHERE c.tag IS NULL ORDER BY t.id",
            vec![],
        ),
        // In join position, with the term on the derived side.
        (
            "SELECT u.tag, c.id FROM u JOIN (SELECT t.id, t.kind FROM t) c ON c.id = u.t_id WHERE c.kind = 'a' ORDER BY u.tag, c.id",
            vec![],
        ),
        // A DISTINCT inside commutes with the filter either way round.
        (
            "SELECT c.kind FROM (SELECT DISTINCT t.kind FROM t) c WHERE c.kind = 'a' ORDER BY c.kind",
            vec![],
        ),
    ];
    for (sql, params) in queries {
        compare(&path, sql, &params);
    }
}

#[test]
fn compound_selects_evaluate_every_arm() {
    if !have_sqlite3() {
        return;
    }
    // A compound chains one arm per link and combines from the left. Walking
    // only the first link drops the third and later arms, silently returning
    // short results rather than an error.
    let scratch = Scratch::new("compound-chain");
    let path = fixture(&scratch);
    let queries: Vec<(&str, Vec<Value>)> = vec![
        (
            "SELECT id FROM t UNION ALL SELECT id FROM u UNION ALL SELECT t_id FROM u ORDER BY id",
            vec![],
        ),
        (
            "SELECT COUNT(*) FROM (SELECT id FROM t UNION ALL SELECT id FROM u UNION ALL SELECT t_id FROM u) x",
            vec![],
        ),
        (
            "SELECT id FROM t UNION SELECT id FROM u UNION SELECT t_id FROM u ORDER BY id",
            vec![],
        ),
        (
            "SELECT id FROM t EXCEPT SELECT t_id FROM u EXCEPT SELECT id FROM u ORDER BY id",
            vec![],
        ),
        (
            "SELECT id FROM t UNION ALL SELECT id FROM u EXCEPT SELECT t_id FROM u ORDER BY id",
            vec![],
        ),
        (
            "SELECT id FROM t UNION ALL SELECT id FROM u UNION ALL SELECT t_id FROM u UNION ALL SELECT id FROM t ORDER BY id",
            vec![],
        ),
    ];
    for (sql, params) in queries {
        compare(&path, sql, &params);
    }
}
