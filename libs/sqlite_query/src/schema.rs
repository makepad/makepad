//! `sqlite_master` scanning and DDL parsing.
//!
//! The catalog is recovered exactly the way SQLite does it: read the schema
//! table rooted at page 1, then parse each object's stored `CREATE` statement.
//! Automatic indexes (`sqlite_autoindex_<table>_<n>`, which have no SQL of
//! their own) are reconstructed from the table's PRIMARY KEY and UNIQUE
//! constraints in declaration order.

use crate::btree::TableCursor;
use crate::error::{Error, Result};
use crate::pager::Pager;
use crate::sql::lexer::{tokenize, Tok, Token};
use crate::value::{affinity_of, Affinity, Collation, TextMode, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Raw sqlite_master rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SchemaObject {
    pub obj_type: String,
    pub name: String,
    pub tbl_name: String,
    pub root_page: u32,
    pub sql: String,
}

// ---------------------------------------------------------------------------
// Parsed tables and indexes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub decl_type: String,
    pub affinity: Affinity,
    pub collation: Collation,
    pub not_null: bool,
    /// Literal DEFAULT, materialized for rows written before an
    /// `ALTER TABLE ... ADD COLUMN`.
    pub default: Value,
    /// Raw text of a DEFAULT that is not a literal (function calls etc).
    pub default_expr: Option<String>,
    /// 1-based position in the table's PRIMARY KEY, if any.
    pub pk_position: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct IndexColumn {
    /// Index into [`TableInfo::columns`].
    pub column: usize,
    pub collation: Collation,
    pub desc: bool,
}

#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub table: String,
    pub root_page: u32,
    pub columns: Vec<IndexColumn>,
    pub unique: bool,
    /// Automatic index behind a PRIMARY KEY / UNIQUE constraint.
    pub auto: bool,
    /// `CREATE INDEX ... WHERE <expr>`: the planner must not use it.
    pub partial: bool,
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    pub root_page: u32,
    pub columns: Vec<Column>,
    /// Column indices forming the PRIMARY KEY, in key order.
    pub pk_columns: Vec<usize>,
    /// The `INTEGER PRIMARY KEY` column, which *is* the rowid.
    pub rowid_alias: Option<usize>,
    pub without_rowid: bool,
    pub indexes: Vec<IndexInfo>,
    /// True when any column has REAL affinity. SQLite may store an integral
    /// REAL as an integer to save space and converts it back on read, so those
    /// tables need a fix-up pass after decoding a record.
    pub any_real_affinity: bool,
    /// Implicit PRIMARY KEY / UNIQUE constraints in declaration order; the nth
    /// one is what `sqlite_autoindex_<table>_<n>` indexes.
    pub auto_specs: Vec<Vec<IndexColumn>>,
    pub check_exprs: Vec<String>,
    /// A trigger names this table. This engine does not run triggers, so a
    /// statement that would fire one is refused instead of silently skipping
    /// the trigger's effect.
    pub has_triggers: bool,
    pub sql: String,
    /// Set when the DDL uses something this engine cannot model (virtual
    /// tables, generated columns, `CREATE TABLE ... AS SELECT`).
    pub unsupported: Option<String>,
}

impl TableInfo {
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    }
    /// Restore REAL values that were stored as integers (see
    /// [`TableInfo::any_real_affinity`]).
    pub fn fix_real_affinity(&self, values: &mut [Value]) {
        if !self.any_real_affinity {
            return;
        }
        for (i, v) in values.iter_mut().enumerate() {
            let Some(col) = self.columns.get(i) else { break };
            if col.affinity == Affinity::Real {
                if let Value::Integer(n) = v {
                    *v = Value::Real(*n as f64);
                }
            }
        }
    }

    /// Pad a decoded record to the table's column count using DEFAULTs, and
    /// substitute the rowid for an INTEGER PRIMARY KEY column (which is stored
    /// as NULL in the record).
    pub fn materialize(&self, rowid: i64, mut values: Vec<Value>) -> Vec<Value> {
        while values.len() < self.columns.len() {
            values.push(self.columns[values.len()].default.clone());
        }
        self.fix_real_affinity(&mut values);
        if let Some(i) = self.rowid_alias {
            if let Some(slot) = values.get_mut(i) {
                *slot = Value::Integer(rowid);
            }
        }
        values
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Schema {
    pub objects: Vec<SchemaObject>,
    pub tables: Vec<TableInfo>,
    by_name: HashMap<String, usize>,
}

impl Schema {
    pub fn load(pager: &mut Pager) -> Result<Schema> {
        let objects = read_objects(pager)?;
        Schema::from_objects(objects)
    }

    pub fn from_objects(objects: Vec<SchemaObject>) -> Result<Schema> {
        let mut tables: Vec<TableInfo> = vec![sqlite_master_table()];
        for obj in &objects {
            if obj.obj_type != "table" {
                continue;
            }
            tables.push(parse_create_table(&obj.name, obj.root_page, &obj.sql));
        }
        let mut by_name = HashMap::new();
        for (i, t) in tables.iter().enumerate() {
            by_name.insert(t.name.to_ascii_lowercase(), i);
        }
        by_name.insert("sqlite_schema".into(), 0);

        // Note triggers: they are not run by this engine, so tables they name
        // become read-only rather than quietly losing the trigger's effect.
        for obj in &objects {
            if obj.obj_type != "trigger" {
                continue;
            }
            if let Some(&ti) = by_name.get(&obj.tbl_name.to_ascii_lowercase()) {
                tables[ti].has_triggers = true;
            }
        }

        // Attach indexes.
        for obj in &objects {
            if obj.obj_type != "index" {
                continue;
            }
            let Some(&ti) = by_name.get(&obj.tbl_name.to_ascii_lowercase()) else {
                continue;
            };
            let info = if obj.sql.trim().is_empty() {
                auto_index_for(&tables[ti], &obj.name, obj.root_page)
            } else {
                parse_create_index(&tables[ti], &obj.name, obj.root_page, &obj.sql)
            };
            if let Some(info) = info {
                tables[ti].indexes.push(info);
            }
        }
        Ok(Schema {
            objects,
            tables,
            by_name,
        })
    }

    pub fn table(&self, name: &str) -> Option<&TableInfo> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|&i| &self.tables[i])
    }

    pub fn table_names(&self) -> Vec<&str> {
        self.tables.iter().map(|t| t.name.as_str()).collect()
    }
}

fn sqlite_master_table() -> TableInfo {
    let col = |n: &str, t: &str| Column {
        name: n.to_string(),
        decl_type: t.to_string(),
        affinity: affinity_of(t),
        collation: Collation::Binary,
        not_null: false,
        default: Value::Null,
        default_expr: None,
        pk_position: None,
    };
    TableInfo {
        name: "sqlite_master".into(),
        root_page: 1,
        columns: vec![
            col("type", "TEXT"),
            col("name", "TEXT"),
            col("tbl_name", "TEXT"),
            col("rootpage", "INTEGER"),
            col("sql", "TEXT"),
        ],
        pk_columns: Vec::new(),
        rowid_alias: None,
        without_rowid: false,
        indexes: Vec::new(),
        any_real_affinity: false,
        auto_specs: Vec::new(),
        check_exprs: Vec::new(),
        has_triggers: false,
        sql: String::new(),
        unsupported: None,
    }
}

/// Read the raw `sqlite_master` rows without parsing any DDL.
pub fn read_objects(pager: &mut Pager) -> Result<Vec<SchemaObject>> {
    let mut cursor = TableCursor::new(1);
    cursor.rewind(pager)?;
    let mut out = Vec::new();
    while let Some(row) = cursor.next(pager)? {
        let vals = row.payload.values(pager, TextMode::Lossy)?;
        if vals.len() < 5 {
            return Err(Error::corrupt("sqlite_master row has fewer than 5 columns"));
        }
        out.push(SchemaObject {
            obj_type: vals[0].as_text().unwrap_or("").to_string(),
            name: vals[1].as_text().unwrap_or("").to_string(),
            tbl_name: vals[2].as_text().unwrap_or("").to_string(),
            root_page: vals[3].as_integer().unwrap_or(0) as u32,
            sql: vals[4].as_text().unwrap_or("").to_string(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Token cursor helpers
// ---------------------------------------------------------------------------

struct Cur {
    toks: Vec<Token>,
    i: usize,
}

impl Cur {
    fn new(sql: &str) -> Result<Cur> {
        Ok(Cur {
            toks: tokenize(sql)?,
            i: 0,
        })
    }
    fn peek(&self) -> &Token {
        &self.toks[self.i.min(self.toks.len() - 1)]
    }
    fn at_eof(&self) -> bool {
        matches!(self.peek().tok, Tok::Eof)
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.i.min(self.toks.len() - 1)].clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.peek().is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_punct(&mut self, p: &str) -> bool {
        if self.peek().is_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, p: &str) -> Result<()> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(Error::sql(format!("expected {p:?} in DDL")))
        }
    }
    fn ident(&mut self) -> Result<String> {
        match self.peek().ident_text() {
            Some(s) => {
                let s = s.to_string();
                self.bump();
                Ok(s)
            }
            None => Err(Error::sql("expected an identifier in DDL")),
        }
    }
    /// Skip a balanced parenthesised group starting at the current '('.
    fn skip_parens(&mut self) -> Result<String> {
        let start = self.i;
        self.expect_punct("(")?;
        let mut depth = 1;
        while depth > 0 {
            if self.at_eof() {
                return Err(Error::sql("unbalanced parentheses in DDL"));
            }
            let t = self.bump();
            if t.is_punct("(") {
                depth += 1;
            } else if t.is_punct(")") {
                depth -= 1;
            }
        }
        Ok(render(&self.toks[start..self.i]))
    }
}

fn render(toks: &[Token]) -> String {
    let mut s = String::new();
    for t in toks {
        if !s.is_empty() && !matches!(t.tok, Tok::Punct(")") | Tok::Punct(",")) {
            s.push(' ');
        }
        match &t.tok {
            Tok::Ident { text, quoted } => {
                if *quoted {
                    s.push('"');
                    s.push_str(text);
                    s.push('"');
                } else {
                    s.push_str(text);
                }
            }
            Tok::Int(v) => s.push_str(&v.to_string()),
            Tok::Real(v) => s.push_str(&v.to_string()),
            Tok::Str(v) => {
                s.push('\'');
                s.push_str(&v.replace('\'', "''"));
                s.push('\'');
            }
            Tok::Blob(b) => {
                s.push_str("x'");
                for byte in b {
                    s.push_str(&format!("{byte:02x}"));
                }
                s.push('\'');
            }
            Tok::Param(_) => s.push('?'),
            Tok::Punct(p) => s.push_str(p),
            Tok::Eof => {}
        }
    }
    s
}

const CONSTRAINT_STARTS: [&str; 11] = [
    "CONSTRAINT",
    "PRIMARY",
    "NOT",
    "NULL",
    "UNIQUE",
    "CHECK",
    "DEFAULT",
    "COLLATE",
    "REFERENCES",
    "GENERATED",
    "AS",
];

fn starts_constraint(t: &Token) -> bool {
    CONSTRAINT_STARTS.iter().any(|k| t.is_kw(k))
}

// ---------------------------------------------------------------------------
// CREATE TABLE
// ---------------------------------------------------------------------------

/// One implicit unique constraint, still keyed by column name because a
/// table-level constraint can name columns declared further down.
struct UniqueSpec {
    spec: Vec<(String, Collation, bool)>,
    is_pk: bool,
}

fn unsupported_table(name: &str, root: u32, sql: &str, why: &str) -> TableInfo {
    TableInfo {
        name: name.to_string(),
        root_page: root,
        columns: Vec::new(),
        pk_columns: Vec::new(),
        rowid_alias: None,
        without_rowid: false,
        indexes: Vec::new(),
        any_real_affinity: false,
        auto_specs: Vec::new(),
        check_exprs: Vec::new(),
        has_triggers: false,
        sql: sql.to_string(),
        unsupported: Some(why.to_string()),
    }
}

pub fn parse_create_table(name: &str, root_page: u32, sql: &str) -> TableInfo {
    match try_parse_create_table(name, root_page, sql) {
        Ok(t) => t,
        Err(e) => unsupported_table(name, root_page, sql, &e.to_string()),
    }
}

fn try_parse_create_table(name: &str, root_page: u32, sql: &str) -> Result<TableInfo> {
    let mut c = Cur::new(sql)?;
    if !c.eat_kw("CREATE") {
        return Err(Error::sql("schema entry does not start with CREATE"));
    }
    c.eat_kw("TEMP");
    c.eat_kw("TEMPORARY");
    if c.peek().is_kw("VIRTUAL") {
        return Err(Error::unsupported("virtual table"));
    }
    if !c.eat_kw("TABLE") {
        return Err(Error::sql("expected TABLE"));
    }
    if c.eat_kw("IF") {
        c.eat_kw("NOT");
        c.eat_kw("EXISTS");
    }
    let _ = c.ident()?; // table name (possibly schema-qualified)
    if c.eat_punct(".") {
        let _ = c.ident()?;
    }
    if c.peek().is_kw("AS") {
        return Err(Error::unsupported("CREATE TABLE ... AS SELECT"));
    }
    c.expect_punct("(")?;

    let mut columns: Vec<Column> = Vec::new();
    // PRIMARY KEY and UNIQUE constraints in declaration order: this order is
    // what numbers the sqlite_autoindex_<table>_<n> entries.
    let mut uniques: Vec<UniqueSpec> = Vec::new();
    let mut checks: Vec<String> = Vec::new();
    let mut pk_autoincrement = false;

    loop {
        // table constraint?
        let is_table_constraint = c.peek().is_kw("CONSTRAINT")
            || c.peek().is_kw("PRIMARY")
            || c.peek().is_kw("UNIQUE")
            || c.peek().is_kw("CHECK")
            || c.peek().is_kw("FOREIGN");
        if is_table_constraint {
            if c.eat_kw("CONSTRAINT") {
                let _ = c.ident();
            }
            if c.eat_kw("PRIMARY") {
                if !c.eat_kw("KEY") {
                    return Err(Error::sql("expected KEY after PRIMARY"));
                }
                let cols = parse_indexed_columns(&mut c)?;
                eat_conflict_clause(&mut c);
                if uniques.iter().any(|u| u.is_pk) {
                    return Err(Error::sql("table has more than one PRIMARY KEY"));
                }
                uniques.push(UniqueSpec {
                    spec: cols,
                    is_pk: true,
                });
            } else if c.eat_kw("UNIQUE") {
                let cols = parse_indexed_columns(&mut c)?;
                eat_conflict_clause(&mut c);
                uniques.push(UniqueSpec {
                    spec: cols,
                    is_pk: false,
                });
            } else if c.eat_kw("CHECK") {
                checks.push(c.skip_parens()?);
            } else if c.eat_kw("FOREIGN") {
                c.eat_kw("KEY");
                c.skip_parens()?;
                parse_references_clause(&mut c)?;
            }
        } else {
            // column definition
            let col_name = c.ident()?;
            let mut decl = String::new();
            while !c.at_eof()
                && !c.peek().is_punct(",")
                && !c.peek().is_punct(")")
                && !starts_constraint(c.peek())
            {
                let t = c.bump();
                if t.is_punct("(") {
                    // type argument list: (10) or (10,5)
                    let mut depth = 1;
                    decl.push('(');
                    while depth > 0 && !c.at_eof() {
                        let t = c.bump();
                        if t.is_punct("(") {
                            depth += 1;
                        }
                        if t.is_punct(")") {
                            depth -= 1;
                        }
                        decl.push_str(&render(std::slice::from_ref(&t)));
                    }
                } else {
                    if !decl.is_empty() {
                        decl.push(' ');
                    }
                    decl.push_str(&render(std::slice::from_ref(&t)));
                }
            }
            let mut col = Column {
                name: col_name.clone(),
                affinity: affinity_of(&decl),
                decl_type: decl,
                collation: Collation::Binary,
                not_null: false,
                default: Value::Null,
                default_expr: None,
                pk_position: None,
            };
            let mut col_pk: Option<bool> = None; // Some(desc)
            let mut col_unique = false;
            loop {
                if c.eat_kw("CONSTRAINT") {
                    let _ = c.ident();
                    continue;
                }
                if c.eat_kw("PRIMARY") {
                    if !c.eat_kw("KEY") {
                        return Err(Error::sql("expected KEY after PRIMARY"));
                    }
                    let mut desc = false;
                    if c.eat_kw("ASC") {
                    } else if c.eat_kw("DESC") {
                        desc = true;
                    }
                    eat_conflict_clause(&mut c);
                    if c.eat_kw("AUTOINCREMENT") {
                        pk_autoincrement = true;
                    }
                    col_pk = Some(desc);
                    continue;
                }
                if c.eat_kw("NOT") {
                    if c.eat_kw("NULL") {
                        col.not_null = true;
                        eat_conflict_clause(&mut c);
                        continue;
                    }
                    // NOT DEFERRABLE
                    c.eat_kw("DEFERRABLE");
                    eat_deferrable_tail(&mut c);
                    continue;
                }
                if c.eat_kw("NULL") {
                    eat_conflict_clause(&mut c);
                    continue;
                }
                if c.eat_kw("UNIQUE") {
                    eat_conflict_clause(&mut c);
                    col_unique = true;
                    continue;
                }
                if c.eat_kw("CHECK") {
                    checks.push(c.skip_parens()?);
                    continue;
                }
                if c.eat_kw("DEFAULT") {
                    parse_default(&mut c, &mut col)?;
                    continue;
                }
                if c.eat_kw("COLLATE") {
                    let name = c.ident()?;
                    col.collation = Collation::from_name(&name)
                        .ok_or_else(|| Error::unsupported(format!("collation {name}")))?;
                    continue;
                }
                if c.peek().is_kw("REFERENCES") {
                    c.bump();
                    parse_references_clause(&mut c)?;
                    continue;
                }
                if c.peek().is_kw("GENERATED") || c.peek().is_kw("AS") {
                    return Err(Error::unsupported("generated columns"));
                }
                break;
            }
            let idx = columns.len();
            let coll = col.collation;
            columns.push(col);
            let _ = idx;
            if let Some(desc) = col_pk {
                if uniques.iter().any(|u| u.is_pk) {
                    return Err(Error::sql("table has more than one PRIMARY KEY"));
                }
                uniques.push(UniqueSpec {
                    spec: vec![(col_name.clone(), coll, desc)],
                    is_pk: true,
                });
            }
            if col_unique {
                uniques.push(UniqueSpec {
                    spec: vec![(col_name.clone(), coll, false)],
                    is_pk: false,
                });
            }
        }
        if c.eat_punct(",") {
            continue;
        }
        c.expect_punct(")")?;
        break;
    }

    let mut without_rowid = false;
    while !c.at_eof() {
        if c.eat_kw("WITHOUT") {
            c.eat_kw("ROWID");
            without_rowid = true;
            continue;
        }
        c.bump();
    }

    // Resolve constraints against the finished column list.
    let mut pk_columns = Vec::new();
    let mut rowid_alias = None;
    let mut auto_specs = Vec::new();
    for u in &uniques {
        let resolved = resolve_index_columns(&columns, &u.spec)?;
        if u.is_pk {
            for (n, ic) in resolved.iter().enumerate() {
                pk_columns.push(ic.column);
                columns[ic.column].pk_position = Some(n + 1);
            }
            // "INTEGER PRIMARY KEY" (ascending, single column, rowid table) is
            // the rowid itself and has no index of its own. "INTEGER PRIMARY
            // KEY DESC" is a documented exception and stays a real index.
            let single_integer = resolved.len() == 1
                && !resolved[0].desc
                && columns[resolved[0].column]
                    .decl_type
                    .eq_ignore_ascii_case("INTEGER");
            if !without_rowid && single_integer {
                rowid_alias = Some(resolved[0].column);
                continue;
            }
            if without_rowid {
                // The PRIMARY KEY *is* the table b-tree key: no auto index.
                continue;
            }
        }
        auto_specs.push(resolved);
    }
    let _ = pk_autoincrement;
    if without_rowid {
        return Err(Error::unsupported("WITHOUT ROWID tables"));
    }

    let any_real_affinity = columns.iter().any(|c| c.affinity == Affinity::Real);
    Ok(TableInfo {
        name: name.to_string(),
        root_page,
        columns,
        pk_columns,
        rowid_alias,
        without_rowid,
        indexes: Vec::new(),
        any_real_affinity,
        auto_specs,
        check_exprs: checks,
        has_triggers: false,
        sql: sql.to_string(),
        unsupported: None,
    })
}

fn eat_conflict_clause(c: &mut Cur) {
    if c.peek().is_kw("ON") {
        // ON CONFLICT <action>
        let save = c.i;
        c.bump();
        if c.eat_kw("CONFLICT") {
            c.bump();
        } else {
            c.i = save;
        }
    }
}

fn eat_deferrable_tail(c: &mut Cur) {
    if c.eat_kw("INITIALLY") {
        c.bump();
    }
}

fn parse_default(c: &mut Cur, col: &mut Column) -> Result<()> {
    if c.peek().is_punct("(") {
        col.default_expr = Some(c.skip_parens()?);
        return Ok(());
    }
    let mut negate = false;
    if c.eat_punct("-") {
        negate = true;
    } else {
        c.eat_punct("+");
    }
    let t = c.bump();
    col.default = match &t.tok {
        Tok::Int(v) => Value::Integer(if negate { -*v } else { *v }),
        Tok::Real(v) => Value::Real(if negate { -*v } else { *v }),
        Tok::Str(s) => Value::Text(s.clone()),
        Tok::Blob(b) => Value::Blob(b.clone()),
        Tok::Ident { text, quoted } if !quoted => {
            let up = text.to_ascii_uppercase();
            match up.as_str() {
                "NULL" => Value::Null,
                "TRUE" => Value::Integer(1),
                "FALSE" => Value::Integer(0),
                _ => {
                    col.default_expr = Some(text.clone());
                    Value::Null
                }
            }
        }
        Tok::Ident { text, .. } => Value::Text(text.clone()),
        other => {
            return Err(Error::sql(format!("unexpected DEFAULT value {other:?}")));
        }
    };
    Ok(())
}

fn parse_references_clause(c: &mut Cur) -> Result<()> {
    let _ = c.ident()?; // foreign table
    if c.peek().is_punct("(") {
        c.skip_parens()?;
    }
    loop {
        if c.eat_kw("ON") {
            // ON DELETE|UPDATE action
            c.bump(); // DELETE / UPDATE
            if c.eat_kw("SET") {
                c.bump(); // NULL / DEFAULT
            } else if c.eat_kw("CASCADE") || c.eat_kw("RESTRICT") {
            } else if c.eat_kw("NO") {
                c.eat_kw("ACTION");
            }
            continue;
        }
        if c.eat_kw("MATCH") {
            let _ = c.ident();
            continue;
        }
        if c.peek().is_kw("NOT") {
            // Only "NOT DEFERRABLE" belongs to this clause; a following
            // "NOT NULL" is a column constraint again.
            let save = c.i;
            c.bump();
            if c.eat_kw("DEFERRABLE") {
                eat_deferrable_tail(c);
                continue;
            }
            c.i = save;
            break;
        }
        if c.eat_kw("DEFERRABLE") {
            eat_deferrable_tail(c);
            continue;
        }
        break;
    }
    Ok(())
}

fn parse_indexed_columns(c: &mut Cur) -> Result<Vec<(String, Collation, bool)>> {
    c.expect_punct("(")?;
    let mut out = Vec::new();
    loop {
        let name = c.ident()?;
        let mut coll = Collation::Binary;
        let mut desc = false;
        loop {
            if c.eat_kw("COLLATE") {
                let cn = c.ident()?;
                coll = Collation::from_name(&cn)
                    .ok_or_else(|| Error::unsupported(format!("collation {cn}")))?;
                continue;
            }
            if c.eat_kw("ASC") {
                continue;
            }
            if c.eat_kw("DESC") {
                desc = true;
                continue;
            }
            break;
        }
        out.push((name, coll, desc));
        if c.eat_punct(",") {
            continue;
        }
        c.expect_punct(")")?;
        break;
    }
    Ok(out)
}

fn resolve_index_columns(
    columns: &[Column],
    spec: &[(String, Collation, bool)],
) -> Result<Vec<IndexColumn>> {
    let mut out = Vec::new();
    for (name, coll, desc) in spec {
        let idx = columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| Error::sql(format!("index refers to unknown column {name}")))?;
        let coll = if *coll == Collation::Binary {
            columns[idx].collation
        } else {
            *coll
        };
        out.push(IndexColumn {
            column: idx,
            collation: coll,
            desc: *desc,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Index attachment
// ---------------------------------------------------------------------------

/// Match `sqlite_autoindex_<table>_<n>` against the table's implicit unique
/// constraints, which `parse_create_table` left in `indexes` in declaration
/// order with an empty name.
fn auto_index_for(table: &TableInfo, name: &str, root_page: u32) -> Option<IndexInfo> {
    let n: usize = name.rsplit('_').next().and_then(|s| s.parse().ok())?;
    let columns = table.auto_specs.get(n.checked_sub(1)?)?.clone();
    Some(IndexInfo {
        name: name.to_string(),
        table: table.name.clone(),
        root_page,
        columns,
        unique: true,
        auto: true,
        partial: false,
    })
}

fn parse_create_index(
    table: &TableInfo,
    name: &str,
    root_page: u32,
    sql: &str,
) -> Option<IndexInfo> {
    let mut c = Cur::new(sql).ok()?;
    if !c.eat_kw("CREATE") {
        return None;
    }
    let unique = c.eat_kw("UNIQUE");
    if !c.eat_kw("INDEX") {
        return None;
    }
    if c.eat_kw("IF") {
        c.eat_kw("NOT");
        c.eat_kw("EXISTS");
    }
    let _ = c.ident().ok()?;
    if c.eat_punct(".") {
        let _ = c.ident().ok()?;
    }
    if !c.eat_kw("ON") {
        return None;
    }
    let _ = c.ident().ok()?;
    let spec = parse_indexed_columns(&mut c).ok()?;
    let columns = resolve_index_columns(&table.columns, &spec).ok()?;
    let partial = c.peek().is_kw("WHERE");
    Some(IndexInfo {
        name: name.to_string(),
        table: table.name.clone(),
        root_page,
        columns,
        unique,
        auto: false,
        partial,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_table() {
        let t = parse_create_table(
            "t",
            4,
            "CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT NOT NULL, c BLOB DEFAULT x'00')",
        );
        assert!(t.unsupported.is_none());
        assert_eq!(t.columns.len(), 3);
        assert_eq!(t.rowid_alias, Some(0));
        assert!(t.columns[1].not_null);
        assert_eq!(t.columns[2].default.as_blob(), Some(&[0u8][..]));
        assert_eq!(t.columns[1].affinity, Affinity::Text);
    }

    #[test]
    fn blob_pk_makes_auto_index() {
        let t = parse_create_table(
            "blobs",
            2,
            "CREATE TABLE blobs(blob_id BLOB PRIMARY KEY, size INTEGER NOT NULL)",
        );
        assert_eq!(t.rowid_alias, None);
        assert_eq!(t.pk_columns, vec![0]);
        assert_eq!(t.auto_specs.len(), 1);
        assert_eq!(t.auto_specs[0][0].column, 0);
    }

    #[test]
    fn auto_index_numbering_follows_declaration_order() {
        // Verified against the sqlite3 CLI: the nth constraint in declaration
        // order is sqlite_autoindex_<table>_<n>.
        let t = parse_create_table(
            "x",
            2,
            "CREATE TABLE x(a TEXT UNIQUE, b TEXT, c TEXT, PRIMARY KEY(b, c))",
        );
        assert_eq!(t.auto_specs.len(), 2);
        assert_eq!(t.auto_specs[0][0].column, 0);
        assert!(t.auto_specs[1].iter().map(|c| c.column).eq([1, 2]));
        assert_eq!(t.pk_columns, vec![1, 2]);

        let t = parse_create_table(
            "p",
            2,
            "CREATE TABLE p(pid BLOB PRIMARY KEY, name TEXT NOT NULL UNIQUE)",
        );
        assert_eq!(t.auto_specs.len(), 2);
        assert_eq!(t.auto_specs[0][0].column, 0);
        assert_eq!(t.auto_specs[1][0].column, 1);

        // INTEGER PRIMARY KEY is the rowid, so only the UNIQUE gets an index.
        let t = parse_create_table("i", 2, "CREATE TABLE i(a INTEGER PRIMARY KEY, b TEXT UNIQUE)");
        assert_eq!(t.auto_specs.len(), 1);
        assert_eq!(t.auto_specs[0][0].column, 1);
    }

    #[test]
    fn checks_and_defaults() {
        let t = parse_create_table(
            "c",
            2,
            "CREATE TABLE c(kind TEXT NOT NULL CHECK(kind IN ('a','b')), n INT DEFAULT 5, s TEXT NOT NULL DEFAULT '')",
        );
        assert_eq!(t.check_exprs.len(), 1);
        assert_eq!(t.columns[1].default.as_integer(), Some(5));
        assert_eq!(t.columns[2].default.as_text(), Some(""));
    }

    #[test]
    fn quoted_table_name_and_alter_added_column() {
        let t = parse_create_table(
            "search_annotations",
            9,
            "CREATE TABLE IF NOT EXISTS \"search_annotations\"(asset_id BLOB PRIMARY KEY, live INTEGER NOT NULL, canon_alias TEXT NOT NULL DEFAULT '')",
        );
        assert!(t.unsupported.is_none(), "{:?}", t.unsupported);
        assert_eq!(t.columns.len(), 3);
        let padded = t.materialize(1, vec![Value::Blob(vec![1]), Value::Integer(1)]);
        assert_eq!(padded.len(), 3);
        assert_eq!(padded[2].as_text(), Some(""));
    }

    #[test]
    fn unsupported_shapes_are_flagged() {
        let t = parse_create_table("v", 0, "CREATE VIRTUAL TABLE v USING fts5(x)");
        assert!(t.unsupported.is_some());
        let g = parse_create_table("g", 3, "CREATE TABLE g(a INT, b AS (a+1))");
        assert!(g.unsupported.is_some());
    }

    #[test]
    fn foreign_keys_do_not_confuse_the_parser() {
        let t = parse_create_table(
            "f",
            2,
            "CREATE TABLE f(a INTEGER REFERENCES p(id) ON DELETE SET NULL NOT NULL, b TEXT)",
        );
        assert!(t.unsupported.is_none(), "{:?}", t.unsupported);
        assert_eq!(t.columns.len(), 2);
        assert!(t.columns[0].not_null);
    }
}
