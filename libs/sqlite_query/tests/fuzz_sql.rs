//! Differential fuzzing: random-but-valid SQL is run through this engine and
//! through the system `sqlite3` CLI, and the results must match exactly.
//!
//! Any statement that ever failed lives on in `tests/corpus/queries.txt`, which
//! is replayed on every run — the corpus only grows.

mod common;

use common::*;
use makepad_sqlite::{Database, Value};
use std::path::{Path, PathBuf};

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
CREATE INDEX t_by_name ON t(name);
CREATE TABLE u(id INTEGER PRIMARY KEY, t_id INTEGER, tag TEXT, w INTEGER);
CREATE INDEX u_by_t ON u(t_id, tag);
CREATE TABLE k(k TEXT PRIMARY KEY, v BLOB NOT NULL, m INTEGER);
INSERT INTO t VALUES
 (1,'alpha','a',10,1.5,x'01'),
 (2,'beta','b',20,2.5,x'02'),
 (3,'gamma','a',30,NULL,NULL),
 (4,'delta',NULL,40,4.5,x'04'),
 (5,'Epsilon','b',NULL,5.5,x'05'),
 (6,'zeta','c',60,6.0,x'0607'),
 (7,'eta','a',-5,-1.25,x''),
 (8,NULL,'b',0,0.0,x'00'),
 (9,'theta','c',1000000,1e10,x'ffee'),
 (10,'iota','a',7,7.75,x'0a');
INSERT INTO u VALUES
 (1,1,'red',5),(2,1,'blue',6),(3,2,'red',7),(4,6,'green',8),
 (5,99,'orphan',9),(6,3,NULL,10),(7,3,'red',NULL);
INSERT INTO k VALUES
 ('key-a', x'aa', 1), ('key-b', x'bb', 2), ('key-c', x'cc', NULL), ('zzz', x'00', 4);
"#;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }
    fn chance(&mut self, n: u64) -> bool {
        self.next() % n == 0
    }
}

fn gen_literal(rng: &mut Rng) -> String {
    match rng.next() % 8 {
        0 => "NULL".into(),
        1 => format!("{}", (rng.next() % 100) as i64 - 20),
        2 => format!("{:.2}", (rng.next() % 1000) as f64 / 10.0),
        3 => "'a'".into(),
        4 => "'red'".into(),
        5 => "x'02'".into(),
        6 => "'alpha'".into(),
        _ => format!("'{}'", ["a", "b", "c", "key-a", "zeta", ""][(rng.next() % 6) as usize]),
    }
}

fn gen_predicate(rng: &mut Rng, cols: &[&str]) -> String {
    let col = *rng.pick(cols);
    match rng.next() % 10 {
        0 => format!("{col} IS NULL"),
        1 => format!("{col} IS NOT NULL"),
        2 => format!("{col} IN ({}, {})", gen_literal(rng), gen_literal(rng)),
        3 => format!("{col} NOT IN ({})", gen_literal(rng)),
        4 => format!(
            "{col} BETWEEN {} AND {}",
            gen_literal(rng),
            gen_literal(rng)
        ),
        5 => format!("{col} LIKE '{}%'", ["a", "b", "z", "e"][(rng.next() % 4) as usize]),
        6 => format!("NOT {col} = {}", gen_literal(rng)),
        7 => format!("COALESCE({col}, {}) = {}", gen_literal(rng), gen_literal(rng)),
        _ => {
            let op = *rng.pick(&["=", "<>", "<", "<=", ">", ">="]);
            format!("{col} {op} {}", gen_literal(rng))
        }
    }
}

fn gen_query(rng: &mut Rng) -> String {
    let joined = rng.chance(3);
    let (table, cols): (&str, Vec<&str>) = if joined {
        (
            "t JOIN u ON u.t_id = t.id",
            vec!["t.id", "t.name", "t.kind", "t.n", "t.r", "u.tag", "u.w"],
        )
    } else if rng.chance(4) {
        ("k", vec!["k", "v", "m"])
    } else if rng.chance(2) {
        ("u", vec!["id", "t_id", "tag", "w"])
    } else {
        ("t", vec!["id", "name", "kind", "n", "r", "b"])
    };

    let mut sql = String::from("SELECT ");
    if rng.chance(6) {
        sql.push_str("DISTINCT ");
    }
    let group = rng.chance(4);
    let mut order_cols: Vec<String> = Vec::new();
    if group {
        let g = *rng.pick(&cols);
        // Aggregate over a column the chosen table actually has.
        let any = *rng.pick(&cols);
        let agg = match rng.next() % 5 {
            0 => "COUNT(*)".to_string(),
            1 => format!("COUNT(DISTINCT {any})"),
            2 => "SUM(1)".to_string(),
            3 => format!("MIN({any})"),
            _ => format!("MAX({any})"),
        };
        sql.push_str(&format!("{g}, {agg} FROM {table}"));
        order_cols.push("1".into());
        order_cols.push("2".into());
        if !rng.chance(3) {
            sql.push_str(&format!(" WHERE {}", gen_predicate(rng, &cols)));
        }
        sql.push_str(&format!(" GROUP BY {g}"));
        if rng.chance(3) {
            sql.push_str(" HAVING COUNT(*) > 1");
        }
    } else {
        let n = 1 + rng.next() % 3;
        let mut picked = Vec::new();
        for _ in 0..n {
            picked.push((*rng.pick(&cols)).to_string());
        }
        sql.push_str(&picked.join(", "));
        sql.push_str(&format!(" FROM {table}"));
        let terms = rng.next() % 3;
        if terms > 0 {
            let mut parts = Vec::new();
            for _ in 0..terms {
                parts.push(gen_predicate(rng, &cols));
            }
            let joiner = if rng.chance(4) { " OR " } else { " AND " };
            sql.push_str(&format!(" WHERE {}", parts.join(joiner)));
        }
        for i in 0..picked.len() {
            order_cols.push(format!("{}", i + 1));
        }
    }
    // Always order fully, so row order is defined for both engines.
    sql.push_str(&format!(" ORDER BY {}", order_cols.join(", ")));
    if rng.chance(3) {
        sql.push_str(&format!(" LIMIT {}", 1 + rng.next() % 5));
        if rng.chance(2) {
            sql.push_str(&format!(" OFFSET {}", rng.next() % 3));
        }
    }
    sql
}

fn rows_from_cli(db: &Path, sql: &str) -> Option<Vec<Vec<Value>>> {
    let out = std::process::Command::new("sqlite3")
        .arg(db)
        .arg("-cmd")
        .arg(".mode quote")
        .arg("-cmd")
        .arg(".separator |")
        .arg(format!("{sql};"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.split('|').map(parse_quoted).collect())
            .collect(),
    )
}

fn same(a: &Value, b: &Value) -> bool {
    a.class() == b.class() && a == b
}

fn check(db_path: &Path, db: &mut Database, sql: &str) -> Result<(), String> {
    let ours = match db.query(sql, &[]) {
        Ok(r) => r,
        Err(e) => return Err(format!("engine error: {e}")),
    };
    let Some(theirs) = rows_from_cli(db_path, sql) else {
        return Err("sqlite3 rejected the statement".into());
    };
    if ours.rows.len() != theirs.len() {
        return Err(format!(
            "row count {} vs {}\nours: {:?}\ncli:  {:?}",
            ours.rows.len(),
            theirs.len(),
            ours.to_quoted_lines(),
            theirs
                .iter()
                .map(|r| r.iter().map(quote).collect::<Vec<_>>().join("|"))
                .collect::<Vec<_>>()
        ));
    }
    for (i, (a, b)) in ours.rows.iter().zip(theirs.iter()).enumerate() {
        if a.len() != b.len() {
            return Err(format!("row {i}: {} vs {} columns", a.len(), b.len()));
        }
        for (c, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if !same(x, y) {
                return Err(format!(
                    "row {i} column {c}: ours {} vs cli {}",
                    quote(x),
                    quote(y)
                ));
            }
        }
    }
    Ok(())
}

fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/queries.txt")
}

#[test]
fn corpus_replays_clean() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("corpus");
    let path = build_db(&scratch.dir, "fuzz.db", FIXTURE);
    let mut db = Database::open(&path).unwrap();
    let text = std::fs::read_to_string(corpus_path()).expect("corpus file");
    let mut checked = 0;
    for line in text.lines() {
        let sql = line.trim();
        if sql.is_empty() || sql.starts_with('#') {
            continue;
        }
        if let Err(e) = check(&path, &mut db, sql) {
            panic!("corpus statement failed:\n  {sql}\n  {e}");
        }
        checked += 1;
    }
    assert!(checked > 20, "corpus is suspiciously small: {checked}");
}

#[test]
fn random_queries_match_sqlite() {
    if !have_sqlite3() {
        return;
    }
    let scratch = Scratch::new("fuzz");
    let path = build_db(&scratch.dir, "fuzz.db", FIXTURE);
    let mut db = Database::open(&path).unwrap();
    // Fixed seed: a failure is always reproducible, and any statement it finds
    // gets added to the corpus by hand.
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut failures = Vec::new();
    let rounds = 400;
    for _ in 0..rounds {
        let sql = gen_query(&mut rng);
        if let Err(e) = check(&path, &mut db, &sql) {
            failures.push(format!("{sql}\n    {e}"));
            if failures.len() > 6 {
                break;
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {rounds} generated queries disagreed with sqlite3:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
