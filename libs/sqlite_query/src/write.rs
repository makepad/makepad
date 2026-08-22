//! Writable connections: DML, DDL, transactions and constraint enforcement.
//!
//! A [`Connection`] owns a writable pager and the parsed schema. Statements run
//! in autocommit mode unless an explicit `BEGIN` is open; every mutating
//! statement therefore either lands whole or leaves no trace, because the
//! rollback journal is what makes it durable.

use crate::btree::TableCursor;
use crate::btree_write::BtreeWriter;
use crate::error::{Error, Result};
use crate::exec::{self, Limits, Runtime};
use crate::pager::Pager;
use crate::plan::Planner;
use crate::schema::{IndexInfo, Schema, TableInfo};
use crate::sql::ast::*;
use crate::sql::parse::parse;
use crate::value::{
    apply_affinity, compare_records, encode_record, Collation, TextMode, Value,
};
use crate::{QueryResult, Statement};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

/// Most prepared statements kept per connection.
const PLAN_CACHE_LIMIT: usize = 128;

/// Header offsets this module maintains.
const HDR_SCHEMA_COOKIE: usize = 40;
const HDR_USER_VERSION: usize = 60;

pub struct Connection {
    pager: Pager,
    schema: Schema,
    /// Schema cookie the cached schema and plans were built from.
    schema_cookie: u32,
    /// Prepared SELECT plans, keyed by SQL text. Dropped whenever the schema
    /// cookie moves, exactly like SQLite re-preparing on a schema change.
    plans: HashMap<String, Rc<Statement>>,
    limits: Limits,
    /// Rows changed by the last mutating statement.
    changes: u64,
    total_changes: u64,
    /// False while an explicit BEGIN is open.
    autocommit: bool,
}

impl Connection {
    /// Open (or create) a database for reading and writing.
    pub fn open(path: &Path, busy_timeout: Duration) -> Result<Connection> {
        if !path.exists() || std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) == 0 {
            create_empty_database(path)?;
        }
        let mut pager = Pager::open_rw(path, busy_timeout)?;
        pager.begin_read()?;
        let schema = Schema::load(&mut pager)?;
        let schema_cookie = pager.header().schema_cookie;
        Ok(Connection {
            pager,
            schema,
            schema_cookie,
            plans: HashMap::new(),
            limits: Limits::default(),
            changes: 0,
            total_changes: 0,
            autocommit: true,
        })
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }
    pub fn pager(&mut self) -> &mut Pager {
        &mut self.pager
    }
    pub fn changes(&self) -> u64 {
        self.changes
    }
    pub fn total_changes(&self) -> u64 {
        self.total_changes
    }
    pub fn autocommit(&self) -> bool {
        self.autocommit
    }
    pub fn limits_mut(&mut self) -> &mut Limits {
        &mut self.limits
    }
    pub fn user_version(&self) -> i32 {
        self.pager.header().user_version
    }

    /// Run one statement that returns no rows; returns the number of rows it
    /// changed.
    pub fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64> {
        self.refresh_view()?;
        let stmt = parse(sql)?;
        let result = self.run_stmt(stmt, params);
        self.release_idle_locks();
        result?;
        Ok(self.changes)
    }

    /// Move to the newest committed snapshot and re-read the schema if another
    /// connection changed it. Both are cheap when nothing moved: one lock, one
    /// header read, and a cookie comparison.
    fn refresh_view(&mut self) -> Result<()> {
        if !self.autocommit {
            return Ok(());
        }
        self.pager.begin_read()?;
        self.reload_schema()
    }

    /// Between autocommit statements a connection holds no locks, exactly like
    /// SQLite: otherwise an idle reader would keep every other process out.
    fn release_idle_locks(&mut self) {
        if self.autocommit {
            let _ = self.pager.unlock();
        }
    }

    /// Run several statements separated by semicolons.
    pub fn execute_batch(&mut self, sql: &str) -> Result<()> {
        for part in split_statements(sql) {
            if part.trim().is_empty() {
                continue;
            }
            self.execute(&part, &[])?;
        }
        Ok(())
    }

    /// Run a query and collect its rows.
    pub fn query(&mut self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        let result = self.query_locked(sql, params);
        self.release_idle_locks();
        result
    }

    fn query_locked(&mut self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        self.refresh_view()?;
        let stmt = parse(sql)?;
        match stmt {
            Stmt::Select(select) => {
                let prepared = self.cached_select(sql, &select)?;
                self.run_select(&prepared, params)
            }
            Stmt::Pragma { name, value } => self.run_pragma(&name, value.as_ref(), params),
            other => {
                self.run_stmt(other, params)?;
                Ok(QueryResult {
                    columns: Vec::new(),
                    rows: Vec::new(),
                })
            }
        }
    }

    /// The plan for a SELECT, as `EXPLAIN QUERY PLAN` would show it.
    pub fn explain(&mut self, sql: &str) -> Result<String> {
        let stmt = parse(sql)?;
        let Stmt::Select(select) = stmt else {
            return Err(Error::sql("only SELECT has a query plan"));
        };
        Ok(self.prepare_select(&select)?.explain())
    }

    /// A plan for this exact SQL text, reused until the schema changes.
    fn cached_select(&mut self, sql: &str, select: &SelectStmt) -> Result<Rc<Statement>> {
        if let Some(plan) = self.plans.get(sql) {
            return Ok(plan.clone());
        }
        let prepared = Rc::new(self.prepare_select(select)?);
        if self.plans.len() >= PLAN_CACHE_LIMIT {
            self.plans.clear();
        }
        self.plans.insert(sql.to_string(), prepared.clone());
        Ok(prepared)
    }

    fn prepare_select(&mut self, select: &SelectStmt) -> Result<Statement> {
        let mut planner = Planner::new(&self.schema);
        let plan = planner.plan_select(select)?;
        Ok(Statement {
            sql: String::new(),
            columns: plan.column_names(),
            slots: planner.slot_total().max(1),
            parameter_count: planner.max_param,
            plan,
        })
    }

    fn run_select(&mut self, stmt: &Statement, params: &[Value]) -> Result<QueryResult> {
        self.pager.begin_read()?;
        let mut rows = Vec::new();
        let max = self.limits.max_rows;
        let limits = self.limits;
        let mut rt = Runtime::new(&mut self.pager, params, stmt.slots, limits);
        let mut overflow = false;
        exec::run(&mut rt, &stmt.plan, &mut |row| {
            if rows.len() >= max {
                overflow = true;
                return Ok(false);
            }
            rows.push(row);
            Ok(true)
        })?;
        if overflow {
            return Err(Error::Budget(format!("more than {max} result rows")));
        }
        Ok(QueryResult {
            columns: stmt.columns.clone(),
            rows,
        })
    }

    fn run_stmt(&mut self, stmt: Stmt, params: &[Value]) -> Result<()> {
        match stmt {
            Stmt::Begin(kind) => {
                if !self.autocommit {
                    return Err(Error::sql("a transaction is already open"));
                }
                self.pager
                    .begin_write(!matches!(kind, TxKind::Deferred))?;
                self.autocommit = false;
                Ok(())
            }
            Stmt::Commit => {
                if self.autocommit {
                    return Err(Error::sql("no transaction is open"));
                }
                self.pager.commit()?;
                self.autocommit = true;
                self.reload_schema()
            }
            Stmt::Rollback => {
                if self.autocommit {
                    return Err(Error::sql("no transaction is open"));
                }
                self.pager.rollback()?;
                self.autocommit = true;
                self.reload_schema()
            }
            Stmt::Pragma { name, value } => {
                self.run_pragma(&name, value.as_ref(), params)?;
                Ok(())
            }
            other => self.in_transaction(|c| c.run_write(other, params)),
        }
    }

    /// Wrap `f` in a transaction when running in autocommit mode.
    fn in_transaction<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if !self.autocommit {
            return f(self);
        }
        self.pager.begin_write(true)?;
        match f(self) {
            Ok(v) => {
                self.pager.commit()?;
                self.reload_schema()?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.pager.rollback();
                let _ = self.reload_schema();
                Err(e)
            }
        }
    }

    /// Re-read the schema only when something changed it. Parsing every
    /// `CREATE TABLE` again after each statement is otherwise the dominant
    /// cost of a write.
    fn reload_schema(&mut self) -> Result<()> {
        let cookie = self.pager.header().schema_cookie;
        if cookie == self.schema_cookie && !self.schema.tables.is_empty() {
            return Ok(());
        }
        self.schema = Schema::load(&mut self.pager)?;
        self.schema_cookie = cookie;
        self.plans.clear();
        Ok(())
    }

    /// Force a reload after this connection changed the schema itself.
    fn schema_changed(&mut self) -> Result<()> {
        self.schema = Schema::load(&mut self.pager)?;
        self.schema_cookie = self.pager.header().schema_cookie;
        self.plans.clear();
        Ok(())
    }

    fn run_write(&mut self, stmt: Stmt, params: &[Value]) -> Result<()> {
        match stmt {
            Stmt::Insert(insert) => self.run_insert(&insert, params),
            Stmt::Update(update) => self.run_update(&update, params),
            Stmt::Delete(delete) => self.run_delete(&delete, params),
            Stmt::CreateTable {
                name,
                if_not_exists,
                sql,
            } => self.create_table(&name, if_not_exists, &sql),
            Stmt::CreateIndex {
                name,
                table,
                unique,
                if_not_exists,
                sql,
            } => self.create_index(&name, &table, unique, if_not_exists, &sql),
            Stmt::DropTable { name, if_exists } => self.drop_table(&name, if_exists),
            Stmt::DropIndex { name, if_exists } => self.drop_index(&name, if_exists),
            Stmt::AlterAddColumn { table, column_sql } => self.alter_add_column(&table, &column_sql),
            Stmt::AlterRenameTable { table, new_name } => self.alter_rename(&table, &new_name),
            Stmt::Select(_) | Stmt::Pragma { .. } | Stmt::Begin(_) | Stmt::Commit | Stmt::Rollback => {
                Err(Error::sql("statement does not modify the database"))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Row helpers
    // -----------------------------------------------------------------------

    fn table(&self, name: &str) -> Result<TableInfo> {
        let t = self
            .schema
            .table(name)
            .ok_or_else(|| Error::sql(format!("no such table: {name}")))?;
        if let Some(why) = &t.unsupported {
            return Err(Error::sql(format!("table {name} is unsupported: {why}")));
        }
        if t.root_page == 0 {
            return Err(Error::sql(format!("table {name} has no b-tree")));
        }
        if t.has_triggers {
            // Running triggers is not implemented; skipping them would change
            // what the database means, so the write is refused instead.
            return Err(Error::unsupported(format!(
                "table {name} has a trigger, which this engine does not run"
            )));
        }
        Ok(t.clone())
    }

    /// Evaluate an expression with no row in scope (VALUES lists, defaults).
    fn eval_constant(&mut self, expr: &Expr, params: &[Value]) -> Result<Value> {
        let plan = {
            let mut planner = Planner::new(&self.schema);
            planner.plan_select(&SelectStmt {
                columns: vec![ResultColumn::Expr {
                    expr: expr.clone(),
                    alias: None,
                }],
                ..SelectStmt::empty()
            })?
        };
        let limits = self.limits;
        let mut rt = Runtime::new(&mut self.pager, params, 4, limits);
        let mut out = Value::Null;
        exec::run(&mut rt, &plan, &mut |row| {
            out = row.into_iter().next().unwrap_or(Value::Null);
            Ok(false)
        })?;
        Ok(out)
    }

    /// The index key for a row: the indexed columns followed by the rowid.
    fn index_key(index: &IndexInfo, values: &[Value], rowid: i64) -> Vec<Value> {
        let mut key: Vec<Value> = index
            .columns
            .iter()
            .map(|c| values.get(c.column).cloned().unwrap_or(Value::Null))
            .collect();
        key.push(Value::Integer(rowid));
        key
    }

    fn index_collations(index: &IndexInfo) -> Vec<Collation> {
        let mut colls: Vec<Collation> = index.columns.iter().map(|c| c.collation).collect();
        colls.push(Collation::Binary);
        colls
    }

    /// Find the rowid an existing entry of `index` points at for `key`
    /// (ignoring the trailing rowid), if any.
    fn find_unique_conflict(
        &mut self,
        index: &IndexInfo,
        key: &[Value],
    ) -> Result<Option<i64>> {
        let prefix = &key[..key.len() - 1];
        if prefix.iter().any(|v| v.is_null()) {
            // NULLs never conflict in a unique index.
            return Ok(None);
        }
        let colls: Vec<Collation> = index.columns.iter().map(|c| c.collation).collect();
        let mut cursor = crate::btree::IndexCursor::new(index.root_page);
        cursor.seek_ge(&mut self.pager, prefix, &colls)?;
        if let Some(entry) = cursor.next(&mut self.pager)? {
            let vals = entry.values(&mut self.pager, TextMode::Strict)?;
            if compare_records(&vals, prefix, &colls) == Ordering::Equal {
                return Ok(vals.last().and_then(Value::as_integer));
            }
        }
        Ok(None)
    }

    /// Next rowid for a table: one past the largest in use.
    fn next_rowid(&mut self, table: &TableInfo) -> Result<i64> {
        let mut cursor = TableCursor::new(table.root_page);
        cursor.seek_ge(&mut self.pager, i64::MIN)?;
        // Walking to the end is O(rows); instead descend the right edge.
        let max = self.max_rowid(table.root_page)?;
        Ok(max.saturating_add(1).max(1))
    }

    /// Largest rowid in a table b-tree, found by walking its right edge.
    fn max_rowid(&mut self, root: u32) -> Result<i64> {
        use crate::btree::{table_leaf_cell, BtreePage, PageType};
        let mut pgno = root;
        for _ in 0..64 {
            let page = BtreePage::load(&mut self.pager, pgno)?;
            match page.page_type {
                PageType::TableLeaf => {
                    if page.n_cells == 0 {
                        return Ok(0);
                    }
                    return Ok(table_leaf_cell(&page, page.n_cells - 1)?.0);
                }
                PageType::TableInterior => {
                    pgno = page
                        .right_child
                        .ok_or_else(|| Error::corrupt("interior page without a right child"))?;
                }
                _ => return Err(Error::corrupt("table root points at an index page")),
            }
        }
        Err(Error::corrupt("b-tree deeper than 64 levels"))
    }

    /// Check NOT NULL and CHECK constraints for a row about to be stored. The
    /// rowid matters: an INTEGER PRIMARY KEY column reads back as the rowid,
    /// which is exactly what a `CHECK(id = 1)` looks at.
    fn check_constraints(
        &mut self,
        table: &TableInfo,
        values: &[Value],
        rowid: i64,
    ) -> Result<()> {
        for (i, col) in table.columns.iter().enumerate() {
            if col.not_null
                && values.get(i).map(|v| v.is_null()).unwrap_or(true)
                && table.rowid_alias != Some(i)
            {
                return Err(Error::Constraint(format!(
                    "NOT NULL constraint failed: {}.{}",
                    table.name, col.name
                )));
            }
        }
        if table.check_exprs.is_empty() {
            return Ok(());
        }
        for text in &table.check_exprs.clone() {
            // The stored text keeps the constraint's own parentheses.
            let body = text.trim();
            let body = body
                .strip_prefix('(')
                .and_then(|b| b.strip_suffix(')'))
                .unwrap_or(body);
            let expr = crate::sql::parse::parse_expr(body)
                .map_err(|e| Error::sql(format!("CHECK constraint {text}: {e}")))?;
            let ok = self.eval_row_expr(table, values, rowid, &expr)?;
            // A CHECK passes unless it evaluates to false; NULL passes.
            if ok.truth() == Some(false) {
                return Err(Error::Constraint(format!(
                    "CHECK constraint failed: {}",
                    table.name
                )));
            }
        }
        Ok(())
    }

    /// Evaluate an expression against one in-memory row of `table`.
    fn eval_row_expr(
        &mut self,
        table: &TableInfo,
        values: &[Value],
        rowid: i64,
        expr: &Expr,
    ) -> Result<Value> {
        let select = SelectStmt {
            columns: vec![ResultColumn::Expr {
                expr: expr.clone(),
                alias: None,
            }],
            from: Some(FromClause {
                base: TableRef::Named {
                    name: table.name.clone(),
                    alias: None,
                },
                joins: Vec::new(),
            }),
            ..SelectStmt::empty()
        };
        let (plan, slots) = {
            let mut planner = Planner::new(&self.schema);
            let plan = planner.plan_select(&select)?;
            let slots = planner.slot_total().max(1);
            (plan, slots)
        };
        let limits = self.limits;
        let mut rt = Runtime::new(&mut self.pager, &[], slots, limits);
        exec::eval_with_row(&mut rt, &plan, values, rowid)
    }

    // -----------------------------------------------------------------------
    // INSERT
    // -----------------------------------------------------------------------

    fn run_insert(&mut self, insert: &InsertStmt, params: &[Value]) -> Result<()> {
        let table = self.table(&insert.table)?;
        self.changes = 0;
        let rows: Vec<Vec<Value>> = match &insert.source {
            InsertSource::Values(rows) => {
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let mut values = Vec::with_capacity(row.len());
                    for e in row {
                        values.push(self.eval_constant(e, params)?);
                    }
                    out.push(values);
                }
                out
            }
            InsertSource::DefaultValues => vec![Vec::new()],
            InsertSource::Select(select) => {
                let stmt = self.prepare_select(select)?;
                self.run_select(&stmt, params)?.rows
            }
        };
        for supplied in rows {
            self.insert_row(&table, insert, &supplied, params)?;
        }
        self.total_changes += self.changes;
        Ok(())
    }

    fn insert_row(
        &mut self,
        table: &TableInfo,
        insert: &InsertStmt,
        supplied: &[Value],
        params: &[Value],
    ) -> Result<()> {
        // Map the supplied values onto the table's columns.
        let mut values: Vec<Value> = table
            .columns
            .iter()
            .map(|c| c.default.clone())
            .collect();
        if insert.columns.is_empty() {
            if !supplied.is_empty() && supplied.len() != table.columns.len() {
                return Err(Error::sql(format!(
                    "table {} has {} columns but {} values were supplied",
                    table.name,
                    table.columns.len(),
                    supplied.len()
                )));
            }
            for (i, v) in supplied.iter().enumerate() {
                values[i] = v.clone();
            }
        } else {
            if supplied.len() != insert.columns.len() {
                return Err(Error::sql(
                    "the number of values does not match the number of columns",
                ));
            }
            for (name, v) in insert.columns.iter().zip(supplied.iter()) {
                let idx = table.column_index(name).ok_or_else(|| {
                    Error::sql(format!("table {} has no column {name}", table.name))
                })?;
                values[idx] = v.clone();
            }
        }
        // Column affinity applies on the way in.
        for (i, col) in table.columns.iter().enumerate() {
            values[i] = apply_affinity(values[i].clone(), col.affinity);
        }

        let rowid = match table.rowid_alias {
            Some(i) if !values[i].is_null() => match &values[i] {
                Value::Integer(v) => *v,
                other => other
                    .as_real()
                    .filter(|f| f.floor() == *f)
                    .map(|f| f as i64)
                    .ok_or_else(|| {
                        Error::Constraint("rowid must be an integer".to_string())
                    })?,
            },
            _ => self.next_rowid(table)?,
        };
        if let Some(i) = table.rowid_alias {
            values[i] = Value::Null; // the record stores NULL for the alias
        }
        self.check_constraints(table, &values, rowid)?;
        self.store_row(table, rowid, values, &insert.on_conflict, params, true)
    }

    /// Write one row and its index entries, applying the conflict policy.
    fn store_row(
        &mut self,
        table: &TableInfo,
        rowid: i64,
        values: Vec<Value>,
        on_conflict: &OnConflict,
        params: &[Value],
        is_insert: bool,
    ) -> Result<()> {
        // Rowid conflict (an explicit INTEGER PRIMARY KEY that already exists).
        let mut conflicts: Vec<i64> = Vec::new();
        if is_insert {
            let mut cursor = TableCursor::new(table.root_page);
            if cursor.seek_exact(&mut self.pager, rowid)?.is_some() {
                conflicts.push(rowid);
            }
        }
        for index in &table.indexes {
            if !index.unique || index.root_page == 0 {
                continue;
            }
            let key = Self::index_key(index, &values, rowid);
            if let Some(other) = self.find_unique_conflict(index, &key)? {
                if other != rowid {
                    conflicts.push(other);
                }
            }
        }
        if !conflicts.is_empty() {
            match on_conflict {
                OnConflict::Abort => {
                    return Err(Error::Constraint(format!(
                        "UNIQUE constraint failed on table {}",
                        table.name
                    )))
                }
                OnConflict::Ignore | OnConflict::DoNothing { .. } => return Ok(()),
                OnConflict::Replace => {
                    conflicts.sort_unstable();
                    conflicts.dedup();
                    for victim in conflicts {
                        self.delete_row(table, victim)?;
                    }
                }
                OnConflict::DoUpdate {
                    sets,
                    where_clause,
                    ..
                } => {
                    let victim = conflicts[0];
                    return self.apply_do_update(table, victim, sets, where_clause.as_ref(), params);
                }
            }
        }
        let payload = encode_record(&values);
        {
            let mut w = BtreeWriter::new(&mut self.pager);
            w.insert_table(table.root_page, rowid, &payload)?;
        }
        for index in &table.indexes.clone() {
            if index.root_page == 0 {
                continue;
            }
            let key = Self::index_key(index, &values, rowid);
            let bytes = encode_record(&key);
            let colls = Self::index_collations(index);
            let mut w = BtreeWriter::new(&mut self.pager);
            w.insert_index(index.root_page, &bytes, &colls)?;
        }
        self.changes += 1;
        Ok(())
    }

    /// `ON CONFLICT ... DO UPDATE`: update the row that conflicted.
    fn apply_do_update(
        &mut self,
        table: &TableInfo,
        rowid: i64,
        sets: &[(String, Expr)],
        where_clause: Option<&Expr>,
        params: &[Value],
    ) -> Result<()> {
        let Some(old) = self.read_row(table, rowid)? else {
            return Ok(());
        };
        if let Some(cond) = where_clause {
            if self.eval_row_expr(table, &old, rowid, cond)?.truth() != Some(true) {
                return Ok(());
            }
        }
        let mut values = old.clone();
        for (name, expr) in sets {
            let idx = table
                .column_index(name)
                .ok_or_else(|| Error::sql(format!("table {} has no column {name}", table.name)))?;
            let mut v = self.eval_row_expr_with_params(table, &old, rowid, expr, params)?;
            v = apply_affinity(v, table.columns[idx].affinity);
            values[idx] = v;
        }
        if let Some(i) = table.rowid_alias {
            values[i] = Value::Null;
        }
        self.check_constraints(table, &values, rowid)?;
        self.delete_row(table, rowid)?;
        self.store_row(table, rowid, values, &OnConflict::Abort, params, false)
    }

    fn eval_row_expr_with_params(
        &mut self,
        table: &TableInfo,
        values: &[Value],
        rowid: i64,
        expr: &Expr,
        params: &[Value],
    ) -> Result<Value> {
        let select = SelectStmt {
            columns: vec![ResultColumn::Expr {
                expr: expr.clone(),
                alias: None,
            }],
            from: Some(FromClause {
                base: TableRef::Named {
                    name: table.name.clone(),
                    alias: None,
                },
                joins: Vec::new(),
            }),
            ..SelectStmt::empty()
        };
        let (plan, slots) = {
            let mut planner = Planner::new(&self.schema);
            let plan = planner.plan_select(&select)?;
            let slots = planner.slot_total().max(1);
            (plan, slots)
        };
        let limits = self.limits;
        let mut rt = Runtime::new(&mut self.pager, params, slots, limits);
        exec::eval_with_row(&mut rt, &plan, values, rowid)
    }

    fn read_row(&mut self, table: &TableInfo, rowid: i64) -> Result<Option<Vec<Value>>> {
        let mut cursor = TableCursor::new(table.root_page);
        let Some(row) = cursor.seek_exact(&mut self.pager, rowid)? else {
            return Ok(None);
        };
        let mut values = row.payload.values(&mut self.pager, TextMode::Strict)?;
        while values.len() < table.columns.len() {
            values.push(table.columns[values.len()].default.clone());
        }
        table.fix_real_affinity(&mut values);
        Ok(Some(values))
    }

    /// Remove a row and all of its index entries.
    fn delete_row(&mut self, table: &TableInfo, rowid: i64) -> Result<()> {
        let Some(values) = self.read_row(table, rowid)? else {
            return Ok(());
        };
        for index in &table.indexes.clone() {
            if index.root_page == 0 {
                continue;
            }
            let key = Self::index_key(index, &values, rowid);
            let bytes = encode_record(&key);
            let colls = Self::index_collations(index);
            let mut w = BtreeWriter::new(&mut self.pager);
            w.delete_index(index.root_page, &bytes, &colls)?;
        }
        let mut w = BtreeWriter::new(&mut self.pager);
        w.delete_table(table.root_page, rowid)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // UPDATE / DELETE
    // -----------------------------------------------------------------------

    /// Rowids matching a WHERE clause, collected before anything is modified.
    fn matching_rowids(
        &mut self,
        table: &TableInfo,
        where_clause: Option<&Expr>,
        params: &[Value],
    ) -> Result<Vec<i64>> {
        let select = SelectStmt {
            columns: vec![ResultColumn::Expr {
                expr: Expr::Column {
                    table: None,
                    name: "rowid".to_string(),
                },
                alias: None,
            }],
            from: Some(FromClause {
                base: TableRef::Named {
                    name: table.name.clone(),
                    alias: None,
                },
                joins: Vec::new(),
            }),
            where_clause: where_clause.cloned(),
            ..SelectStmt::empty()
        };
        let (plan, slots) = {
            let mut planner = Planner::new(&self.schema);
            let plan = planner.plan_select(&select)?;
            let slots = planner.slot_total().max(1);
            (plan, slots)
        };
        let limits = self.limits;
        let mut rowids = Vec::new();
        let mut rt = Runtime::new(&mut self.pager, params, slots, limits);
        exec::run(&mut rt, &plan, &mut |row| {
            if let Some(Value::Integer(id)) = row.into_iter().next() {
                rowids.push(id);
            }
            Ok(true)
        })?;
        Ok(rowids)
    }

    fn run_update(&mut self, update: &UpdateStmt, params: &[Value]) -> Result<()> {
        let table = self.table(&update.table)?;
        self.changes = 0;
        let rowids = self.matching_rowids(&table, update.where_clause.as_ref(), params)?;
        for rowid in rowids {
            let Some(old) = self.read_row(&table, rowid)? else {
                continue;
            };
            let mut values = old.clone();
            let mut new_rowid = rowid;
            for (name, expr) in &update.sets {
                let idx = table.column_index(name).ok_or_else(|| {
                    Error::sql(format!("table {} has no column {name}", table.name))
                })?;
                let v = self.eval_row_expr_with_params(&table, &old, rowid, expr, params)?;
                if table.rowid_alias == Some(idx) {
                    new_rowid = match apply_affinity(v.clone(), crate::value::Affinity::Integer) {
                        Value::Integer(x) => x,
                        _ => {
                            return Err(Error::Constraint("rowid must be an integer".to_string()))
                        }
                    };
                    values[idx] = Value::Null;
                } else {
                    values[idx] = apply_affinity(v, table.columns[idx].affinity);
                }
            }
            self.check_constraints(&table, &values, new_rowid)?;
            self.delete_row(&table, rowid)?;
            let before = self.changes;
            self.store_row(
                &table,
                new_rowid,
                values,
                &update.or_conflict,
                params,
                new_rowid != rowid,
            )?;
            if self.changes == before {
                // The conflict policy dropped the row: put the old one back.
                self.store_row(&table, rowid, old, &OnConflict::Abort, params, false)?;
                self.changes = before;
            }
        }
        self.total_changes += self.changes;
        Ok(())
    }

    fn run_delete(&mut self, delete: &DeleteStmt, params: &[Value]) -> Result<()> {
        let table = self.table(&delete.table)?;
        self.changes = 0;
        if delete.where_clause.is_none() {
            // DELETE FROM t with no WHERE: empty every b-tree of the table.
            let count = self.count_rows(&table)?;
            {
                let mut w = BtreeWriter::new(&mut self.pager);
                w.clear_btree(table.root_page, true)?;
            }
            for index in &table.indexes.clone() {
                if index.root_page == 0 {
                    continue;
                }
                let mut w = BtreeWriter::new(&mut self.pager);
                w.clear_btree(index.root_page, true)?;
            }
            self.changes = count;
            self.total_changes += count;
            return Ok(());
        }
        let rowids = self.matching_rowids(&table, delete.where_clause.as_ref(), params)?;
        for rowid in rowids {
            if self.read_row(&table, rowid)?.is_some() {
                self.delete_row(&table, rowid)?;
                self.changes += 1;
            }
        }
        self.total_changes += self.changes;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Schema changes
    // -----------------------------------------------------------------------

    /// Every row of `sqlite_master` with its rowid.
    fn master_rows(&mut self) -> Result<Vec<(i64, Vec<Value>)>> {
        let mut cursor = TableCursor::new(1);
        cursor.rewind(&mut self.pager)?;
        let mut out = Vec::new();
        while let Some(row) = cursor.next(&mut self.pager)? {
            let vals = row.payload.values(&mut self.pager, TextMode::Lossy)?;
            out.push((row.rowid, vals));
        }
        Ok(out)
    }

    fn insert_master(
        &mut self,
        obj_type: &str,
        name: &str,
        tbl_name: &str,
        root: u32,
        sql: Option<&str>,
    ) -> Result<()> {
        let rowid = self.max_rowid(1)?.saturating_add(1).max(1);
        let values = vec![
            Value::text(obj_type),
            Value::text(name),
            Value::text(tbl_name),
            Value::Integer(root as i64),
            match sql {
                Some(s) => Value::text(s),
                None => Value::Null,
            },
        ];
        let payload = encode_record(&values);
        let mut w = BtreeWriter::new(&mut self.pager);
        w.insert_table(1, rowid, &payload)
    }

    fn replace_master(&mut self, rowid: i64, values: &[Value]) -> Result<()> {
        let payload = encode_record(values);
        let mut w = BtreeWriter::new(&mut self.pager);
        w.insert_table(1, rowid, &payload)
    }

    fn delete_master(&mut self, rowid: i64) -> Result<()> {
        let mut w = BtreeWriter::new(&mut self.pager);
        w.delete_table(1, rowid)?;
        Ok(())
    }

    /// Bump the schema cookie so every other connection reloads the schema.
    fn bump_schema_cookie(&mut self) -> Result<()> {
        let page = self.pager.page(1)?;
        let cookie = crate::value::be_u32(&page, HDR_SCHEMA_COOKIE)?;
        self.pager
            .set_header_u32(HDR_SCHEMA_COOKIE, cookie.wrapping_add(1))
    }

    fn create_table(&mut self, name: &str, if_not_exists: bool, sql: &str) -> Result<()> {
        if self.schema.table(name).is_some() {
            if if_not_exists {
                return Ok(());
            }
            return Err(Error::sql(format!("table {name} already exists")));
        }
        // Validate the DDL before anything is written.
        let parsed = crate::schema::parse_create_table(name, 0, sql);
        if let Some(why) = &parsed.unsupported {
            return Err(Error::unsupported(why.clone()));
        }
        let root = {
            let mut w = BtreeWriter::new(&mut self.pager);
            w.create_btree(false)?
        };
        self.insert_master("table", name, name, root, Some(sql))?;
        // Automatic indexes for PRIMARY KEY and UNIQUE constraints.
        for n in 1..=parsed.auto_specs.len() {
            let index_root = {
                let mut w = BtreeWriter::new(&mut self.pager);
                w.create_btree(true)?
            };
            let index_name = format!("sqlite_autoindex_{name}_{n}");
            self.insert_master("index", &index_name, name, index_root, None)?;
        }
        self.bump_schema_cookie()?;
        self.schema_changed()
    }

    fn create_index(
        &mut self,
        name: &str,
        table_name: &str,
        _unique: bool,
        if_not_exists: bool,
        sql: &str,
    ) -> Result<()> {
        let table = self.table(table_name)?;
        if table.indexes.iter().any(|i| i.name.eq_ignore_ascii_case(name)) {
            if if_not_exists {
                return Ok(());
            }
            return Err(Error::sql(format!("index {name} already exists")));
        }
        let root = {
            let mut w = BtreeWriter::new(&mut self.pager);
            w.create_btree(true)?
        };
        self.insert_master("index", name, table_name, root, Some(sql))?;
        self.bump_schema_cookie()?;
        self.schema_changed()?;
        // Populate it from the table.
        let table = self.table(table_name)?;
        let index = table
            .indexes
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| Error::sql(format!("index {name} did not load")))?;
        let mut cursor = TableCursor::new(table.root_page);
        cursor.rewind(&mut self.pager)?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next(&mut self.pager)? {
            let mut values = row.payload.values(&mut self.pager, TextMode::Strict)?;
            while values.len() < table.columns.len() {
                values.push(table.columns[values.len()].default.clone());
            }
            table.fix_real_affinity(&mut values);
            rows.push((row.rowid, values));
        }
        let colls = Self::index_collations(&index);
        for (rowid, values) in rows {
            let key = Self::index_key(&index, &values, rowid);
            if index.unique {
                if let Some(other) = self.find_unique_conflict(&index, &key)? {
                    if other != rowid {
                        return Err(Error::Constraint(format!(
                            "UNIQUE constraint failed while building index {name}"
                        )));
                    }
                }
            }
            let bytes = encode_record(&key);
            let mut w = BtreeWriter::new(&mut self.pager);
            w.insert_index(index.root_page, &bytes, &colls)?;
        }
        Ok(())
    }

    fn drop_table(&mut self, name: &str, if_exists: bool) -> Result<()> {
        let Some(table) = self.schema.table(name).cloned() else {
            if if_exists {
                return Ok(());
            }
            return Err(Error::sql(format!("no such table: {name}")));
        };
        if table.root_page <= 1 {
            return Err(Error::sql("cannot drop the schema table"));
        }
        for index in &table.indexes {
            if index.root_page > 1 {
                let mut w = BtreeWriter::new(&mut self.pager);
                w.clear_btree(index.root_page, false)?;
            }
        }
        {
            let mut w = BtreeWriter::new(&mut self.pager);
            w.clear_btree(table.root_page, false)?;
        }
        let rows = self.master_rows()?;
        for (rowid, values) in rows {
            let tbl = values.get(2).and_then(Value::as_text).unwrap_or("");
            if tbl.eq_ignore_ascii_case(name) {
                self.delete_master(rowid)?;
            }
        }
        self.bump_schema_cookie()?;
        self.schema_changed()
    }

    fn drop_index(&mut self, name: &str, if_exists: bool) -> Result<()> {
        let mut found = None;
        for table in &self.schema.tables {
            if let Some(i) = table.indexes.iter().find(|i| i.name.eq_ignore_ascii_case(name)) {
                found = Some(i.clone());
                break;
            }
        }
        let Some(index) = found else {
            if if_exists {
                return Ok(());
            }
            return Err(Error::sql(format!("no such index: {name}")));
        };
        if index.auto {
            return Err(Error::sql("an automatic index cannot be dropped"));
        }
        if index.root_page > 1 {
            let mut w = BtreeWriter::new(&mut self.pager);
            w.clear_btree(index.root_page, false)?;
        }
        let rows = self.master_rows()?;
        for (rowid, values) in rows {
            let obj = values.first().and_then(Value::as_text).unwrap_or("");
            let n = values.get(1).and_then(Value::as_text).unwrap_or("");
            if obj == "index" && n.eq_ignore_ascii_case(name) {
                self.delete_master(rowid)?;
            }
        }
        self.bump_schema_cookie()?;
        self.schema_changed()
    }

    fn alter_add_column(&mut self, table_name: &str, column_sql: &str) -> Result<()> {
        let table = self.table(table_name)?;
        let rows = self.master_rows()?;
        let Some((rowid, mut values)) = rows.into_iter().find(|(_, v)| {
            v.first().and_then(Value::as_text) == Some("table")
                && v.get(1)
                    .and_then(Value::as_text)
                    .map(|n| n.eq_ignore_ascii_case(table_name))
                    .unwrap_or(false)
        }) else {
            return Err(Error::sql(format!("no such table: {table_name}")));
        };
        let sql = values.get(4).and_then(Value::as_text).unwrap_or("").to_string();
        let close = sql
            .rfind(')')
            .ok_or_else(|| Error::corrupt("stored CREATE TABLE has no closing paren"))?;
        let new_sql = format!("{}, {}{}", &sql[..close], column_sql, &sql[close..]);
        // The new definition must still parse, and a NOT NULL column needs a
        // usable default because existing rows have no value for it.
        let parsed = crate::schema::parse_create_table(table_name, table.root_page, &new_sql);
        if let Some(why) = &parsed.unsupported {
            return Err(Error::sql(format!("ALTER TABLE would break the schema: {why}")));
        }
        if parsed.columns.len() != table.columns.len() + 1 {
            return Err(Error::sql("ADD COLUMN did not add exactly one column"));
        }
        let added = parsed.columns.last().expect("a column");
        if added.not_null && added.default.is_null() {
            return Err(Error::sql(
                "cannot add a NOT NULL column with a NULL default",
            ));
        }
        if added.pk_position.is_some() {
            return Err(Error::sql("cannot add a PRIMARY KEY column"));
        }
        values[4] = Value::text(new_sql);
        self.replace_master(rowid, &values)?;
        self.bump_schema_cookie()?;
        self.schema_changed()
    }

    fn alter_rename(&mut self, table_name: &str, new_name: &str) -> Result<()> {
        let _ = self.table(table_name)?;
        if self.schema.table(new_name).is_some() {
            return Err(Error::sql(format!("table {new_name} already exists")));
        }
        let rows = self.master_rows()?;
        for (rowid, mut values) in rows {
            let obj = values.first().and_then(Value::as_text).unwrap_or("").to_string();
            let name = values.get(1).and_then(Value::as_text).unwrap_or("").to_string();
            let tbl = values.get(2).and_then(Value::as_text).unwrap_or("").to_string();
            if !tbl.eq_ignore_ascii_case(table_name) {
                continue;
            }
            values[2] = Value::text(new_name);
            if obj == "table" {
                values[1] = Value::text(new_name);
                let sql = values.get(4).and_then(Value::as_text).unwrap_or("").to_string();
                let paren = sql
                    .find('(')
                    .ok_or_else(|| Error::corrupt("stored CREATE TABLE has no column list"))?;
                values[4] = Value::text(format!(
                    "CREATE TABLE \"{new_name}\" {}",
                    &sql[paren..]
                ));
            } else if obj == "index" {
                // An automatic index is named after its table; SQLite renames
                // it with the table, and refuses to open a database where the
                // two disagree ("orphan index").
                let prefix = format!("sqlite_autoindex_{table_name}_");
                if let Some(suffix) = name.strip_prefix(&prefix) {
                    values[1] = Value::text(format!("sqlite_autoindex_{new_name}_{suffix}"));
                }
                if let Some(sql) = values.get(4).and_then(Value::as_text) {
                    // Rewrite the "ON <table>" part of the index definition.
                    let rewritten = rewrite_index_table(sql, &name, new_name);
                    values[4] = Value::text(rewritten);
                }
            }
            self.replace_master(rowid, &values)?;
        }
        self.bump_schema_cookie()?;
        self.schema_changed()
    }

    // -----------------------------------------------------------------------
    // PRAGMA
    // -----------------------------------------------------------------------

    fn run_pragma(
        &mut self,
        name: &str,
        value: Option<&Expr>,
        params: &[Value],
    ) -> Result<QueryResult> {
        let lower = name.to_ascii_lowercase();
        let arg = match value {
            Some(e) => Some(self.eval_constant(e, params)?),
            None => None,
        };
        let one = |col: &str, v: Value| QueryResult {
            columns: vec![col.to_string()],
            rows: vec![vec![v]],
        };
        let empty = QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
        };
        Ok(match lower.as_str() {
            "user_version" => match arg {
                Some(v) => {
                    let n = v.as_integer().unwrap_or(0) as i32;
                    self.in_transaction(|c| c.pager.set_header_u32(HDR_USER_VERSION, n as u32))?;
                    empty
                }
                None => one("user_version", Value::Integer(self.user_version() as i64)),
            },
            "schema_version" => one(
                "schema_version",
                Value::Integer(self.pager.header().schema_cookie as i64),
            ),
            "page_size" => one("page_size", Value::Integer(self.pager.page_size() as i64)),
            "page_count" => one("page_count", Value::Integer(self.pager.page_count() as i64)),
            "freelist_count" => one(
                "freelist_count",
                Value::Integer(self.pager.header().freelist_pages as i64),
            ),
            // Both modes are real: WAL frames or a rollback journal. The
            // pragma returns the mode in force afterwards, like SQLite's does.
            "journal_mode" => match arg {
                Some(v) => {
                    let want = crate::exec::to_text(&v).to_ascii_lowercase();
                    let mode = match want.as_str() {
                        "wal" => self.pager.set_journal_mode(true)?,
                        "delete" | "truncate" | "persist" => {
                            self.pager.set_journal_mode(false)?
                        }
                        other => {
                            return Err(Error::unsupported(format!("journal_mode={other}")))
                        }
                    };
                    one("journal_mode", Value::text(mode))
                }
                None => one("journal_mode", Value::text(self.pager.journal_mode())),
            },
            "wal_checkpoint" => {
                let moved = self.pager.checkpoint()?;
                QueryResult {
                    columns: vec!["busy".into(), "log".into(), "checkpointed".into()],
                    rows: vec![vec![
                        Value::Integer(0),
                        Value::Integer(moved as i64),
                        Value::Integer(moved as i64),
                    ]],
                }
            }
            // Accepted and ignored: this engine always syncs at commit and does
            // not implement foreign keys or a WAL checkpointer.
            "synchronous" | "foreign_keys" | "wal_autocheckpoint" | "cache_size"
            | "busy_timeout" | "temp_store" | "locking_mode" | "legacy_file_format"
            | "auto_vacuum" | "secure_delete" => match arg {
                Some(_) => empty,
                None => one(&lower, Value::Integer(0)),
            },
            "integrity_check" | "quick_check" => {
                let report = {
                    let schema = self.schema.clone();
                    crate::integrity::check(&mut self.pager, &schema, true)?
                };
                if report.ok() {
                    one("integrity_check", Value::text("ok"))
                } else {
                    QueryResult {
                        columns: vec!["integrity_check".into()],
                        rows: report
                            .problems
                            .into_iter()
                            .map(|p| vec![Value::text(p)])
                            .collect(),
                    }
                }
            }
            "table_info" => {
                let table_name = arg
                    .as_ref()
                    .map(crate::exec::to_text)
                    .ok_or_else(|| Error::sql("PRAGMA table_info needs a table name"))?;
                let table = self.table(&table_name)?;
                QueryResult {
                    columns: vec![
                        "cid".into(),
                        "name".into(),
                        "type".into(),
                        "notnull".into(),
                        "dflt_value".into(),
                        "pk".into(),
                    ],
                    rows: table
                        .columns
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            vec![
                                Value::Integer(i as i64),
                                Value::text(c.name.clone()),
                                Value::text(c.decl_type.clone()),
                                Value::Integer(i64::from(c.not_null)),
                                c.default.clone(),
                                Value::Integer(c.pk_position.unwrap_or(0) as i64),
                            ]
                        })
                        .collect(),
                }
            }
            other => return Err(Error::unsupported(format!("PRAGMA {other}"))),
        })
    }

    fn count_rows(&mut self, table: &TableInfo) -> Result<u64> {
        let mut cursor = TableCursor::new(table.root_page);
        cursor.rewind(&mut self.pager)?;
        let mut n = 0u64;
        while cursor.next(&mut self.pager)?.is_some() {
            n += 1;
        }
        Ok(n)
    }
}

/// Rewrite the table name in a stored `CREATE INDEX ... ON <table> (...)`.
fn rewrite_index_table(sql: &str, _index_name: &str, new_table: &str) -> String {
    let upper = sql.to_ascii_uppercase();
    let Some(on) = upper.rfind(" ON ") else {
        return sql.to_string();
    };
    let after = &sql[on + 4..];
    let Some(paren) = after.find('(') else {
        return sql.to_string();
    };
    format!("{} ON \"{new_table}\" {}", &sql[..on], &after[paren..])
}

/// Split a script into statements on top-level semicolons.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut in_string: Option<char> = None;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while let Some(c) = chars.next() {
        if in_line_comment {
            current.push(c);
            if c == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if in_block_comment {
            current.push(c);
            if c == '*' && chars.peek() == Some(&'/') {
                current.push(chars.next().unwrap_or('/'));
                in_block_comment = false;
            }
            continue;
        }
        if let Some(q) = in_string {
            current.push(c);
            if c == q {
                if chars.peek() == Some(&q) {
                    current.push(chars.next().unwrap_or(q));
                } else {
                    in_string = None;
                }
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                in_string = Some(c);
                current.push(c);
            }
            '-' if chars.peek() == Some(&'-') => {
                in_line_comment = true;
                current.push(c);
            }
            '/' if chars.peek() == Some(&'*') => {
                in_block_comment = true;
                current.push(c);
            }
            ';' => {
                out.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

/// Write a fresh, empty database: a 100-byte header and page 1 as an empty
/// `sqlite_master` leaf.
pub fn create_empty_database(path: &Path) -> Result<()> {
    let page_size: usize = 4096;
    let mut page = vec![0u8; page_size];
    page[0..16].copy_from_slice(crate::pager::MAGIC);
    page[16..18].copy_from_slice(&(page_size as u16).to_be_bytes());
    page[18] = 1; // write version: rollback journal
    page[19] = 1; // read version
    page[20] = 0; // reserved space
    page[21] = 64;
    page[22] = 32;
    page[23] = 32;
    page[24..28].copy_from_slice(&1u32.to_be_bytes()); // change counter
    page[28..32].copy_from_slice(&1u32.to_be_bytes()); // size in pages
    page[44..48].copy_from_slice(&4u32.to_be_bytes()); // schema format 4
    page[56..60].copy_from_slice(&1u32.to_be_bytes()); // UTF-8
    page[92..96].copy_from_slice(&1u32.to_be_bytes()); // version-valid-for
    page[96..100].copy_from_slice(&3_045_000u32.to_be_bytes());
    // Page 1's b-tree header starts at byte 100: an empty table leaf.
    page[100] = 13;
    page[101..103].copy_from_slice(&0u16.to_be_bytes());
    page[103..105].copy_from_slice(&0u16.to_be_bytes());
    let content_start = page_size as u16;
    page[105..107].copy_from_slice(&content_start.to_be_bytes());
    page[107] = 0;
    std::fs::write(path, &page)?;
    Ok(())
}
