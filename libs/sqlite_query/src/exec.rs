//! Plan execution: nested-loop joins over b-tree cursors, SQLite value
//! semantics for every operator, then grouping/ordering/limiting.
//!
//! Rows stream straight to the consumer whenever the plan allows it (no
//! GROUP BY, no aggregates, no DISTINCT, no sort, no compound select), so a
//! `LIMIT`ed query stops the cursors as soon as it has enough rows. Anything
//! that needs to see all rows first buffers them under an explicit row budget.

use crate::btree::{IndexCursor, TableCursor};
use crate::error::{Error, Result};
use crate::pager::Pager;
use crate::plan::{Access, AggFunc, AggSpec, PExpr, Plan, ScalarFunc, Source};
use crate::sql::ast::{BinOp, CompoundOp, UnaryOp};
use crate::value::{
    apply_affinity, apply_numeric_affinity, compare, encode_record, format_real,
    text_to_number_prefix, Affinity, Collation, TextMode, Value,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// Guard rails for one statement. The engine has no write path, so these are
/// the only resources a query can spend.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most rows a query may buffer or return.
    pub max_rows: usize,
    /// Most b-tree rows a query may visit (including rows filtered out).
    pub max_steps: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_rows: 100_000,
            max_steps: 50_000_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RowData {
    pub rowid: i64,
    pub values: Vec<Value>,
}

pub struct Runtime<'a> {
    pub pager: &'a mut Pager,
    params: &'a [Value],
    rows: Vec<Option<RowData>>,
    steps: u64,
    limits: Limits,
}

impl<'a> Runtime<'a> {
    pub fn new(pager: &'a mut Pager, params: &'a [Value], slots: usize, limits: Limits) -> Runtime<'a> {
        Runtime {
            pager,
            params,
            rows: vec![None; slots],
            steps: 0,
            limits,
        }
    }
    pub fn steps(&self) -> u64 {
        self.steps
    }
    fn step(&mut self) -> Result<()> {
        self.steps += 1;
        if self.steps > self.limits.max_steps {
            return Err(Error::Budget(format!(
                "visited more than {} rows",
                self.limits.max_steps
            )));
        }
        Ok(())
    }
    fn slot(&self, slot: usize) -> Option<&RowData> {
        self.rows.get(slot).and_then(|r| r.as_ref())
    }
    /// A copy of every row register, so a group can remember the row its bare
    /// columns come from.
    fn snapshot_rows(&self) -> Vec<Option<RowData>> {
        self.rows.clone()
    }
    fn restore_rows(&mut self, rows: &[Option<RowData>]) {
        self.rows = rows.to_vec();
    }
    /// Put a row into a slot directly, for expressions evaluated against a row
    /// that is being built rather than read (DEFAULT, CHECK, UPDATE SET).
    pub fn set_row(&mut self, slot: usize, row: RowData) {
        if slot >= self.rows.len() {
            self.rows.resize(slot + 1, None);
        }
        self.rows[slot] = Some(row);
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Evaluate a single-expression plan against one in-memory row placed in the
/// plan's first level. Used for CHECK constraints and `SET` expressions.
pub fn eval_with_row(
    rt: &mut Runtime,
    plan: &Plan,
    values: &[Value],
    rowid: i64,
) -> Result<Value> {
    let slot = plan.levels.first().map(|l| l.slot).unwrap_or(0);
    rt.set_row(
        slot,
        RowData {
            rowid,
            values: values.to_vec(),
        },
    );
    let expr = &plan
        .result
        .first()
        .ok_or_else(|| Error::sql("expression plan has no result column"))?
        .expr;
    eval(rt, expr, &[])
}

/// Run a plan, handing each result row to `sink`. The sink returns `false` to
/// stop early.
pub fn run(
    rt: &mut Runtime,
    plan: &Plan,
    sink: &mut dyn FnMut(Vec<Value>) -> Result<bool>,
) -> Result<()> {
    let limit = eval_count(rt, plan.limit.as_ref())?;
    let offset = eval_count(rt, plan.offset.as_ref())?.unwrap_or(0);
    let streaming = plan.compound.is_none()
        && plan.group_by.is_empty()
        && plan.aggregates.is_empty()
        && !plan.distinct
        && (plan.order_by.is_empty() || plan.ordered_by_access);

    if streaming {
        let mut skipped = 0u64;
        let mut emitted = 0u64;
        let mut stop = false;
        for_each_joined_row(rt, plan, &mut |rt| {
            let row = eval_result_row(rt, plan, &[])?;
            if skipped < offset {
                skipped += 1;
                return Ok(true);
            }
            if !sink(row)? {
                stop = true;
                return Ok(false);
            }
            emitted += 1;
            if let Some(l) = limit {
                if emitted >= l {
                    stop = true;
                    return Ok(false);
                }
            }
            Ok(true)
        })?;
        let _ = stop;
        return Ok(());
    }

    let mut rows = run_body(rt, plan)?;
    if let Some((op, right)) = &plan.compound {
        let other = run_body(rt, right)?;
        rows = combine(*op, rows, other);
        // ORDER BY over a compound sorts by result column position.
        if let Some(positions) = &plan.order_by_positions {
            for row in rows.iter_mut() {
                row.keys = positions
                    .iter()
                    .map(|(i, _, _)| row.values.get(*i).cloned().unwrap_or(Value::Null))
                    .collect();
            }
            let terms: Vec<(bool, bool)> =
                positions.iter().map(|(_, d, n)| (*d, *n)).collect();
            sort_by_terms(&mut rows, &terms);
        }
    } else if !plan.order_by.is_empty() && !plan.ordered_by_access {
        sort_rows(rt, plan, &mut rows)?;
    }
    let mut emitted = 0u64;
    for row in rows.into_iter().skip(offset as usize) {
        if let Some(l) = limit {
            if emitted >= l {
                break;
            }
        }
        if !sink(row.values)? {
            break;
        }
        emitted += 1;
    }
    Ok(())
}

/// Key bytes for DISTINCT / GROUP BY / set operations. SQLite compares values
/// numerically across INTEGER and REAL, so 1 and 1.0 must land in the same
/// bucket; the record encoding alone would separate them.
fn dedup_key(values: &[Value]) -> Vec<u8> {
    let normalized: Vec<Value> = values
        .iter()
        .map(|v| match v {
            Value::Real(f) if f.floor() == *f && f.abs() < 9.2e18 => Value::Integer(*f as i64),
            other => other.clone(),
        })
        .collect();
    encode_record(&normalized)
}

/// A buffered result row plus its sort keys.
struct OutRow {
    values: Vec<Value>,
    keys: Vec<Value>,
}

fn combine(op: CompoundOp, left: Vec<OutRow>, right: Vec<OutRow>) -> Vec<OutRow> {
    let key = |r: &OutRow| dedup_key(&r.values);
    match op {
        CompoundOp::UnionAll => {
            let mut out = left;
            out.extend(right);
            out
        }
        CompoundOp::Union => {
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for r in left.into_iter().chain(right.into_iter()) {
                if seen.insert(key(&r)) {
                    out.push(r);
                }
            }
            out
        }
        CompoundOp::Intersect => {
            let rset: HashSet<Vec<u8>> = right.iter().map(key).collect();
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for r in left {
                let k = key(&r);
                if rset.contains(&k) && seen.insert(k) {
                    out.push(r);
                }
            }
            out
        }
        CompoundOp::Except => {
            let rset: HashSet<Vec<u8>> = right.iter().map(key).collect();
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for r in left {
                let k = key(&r);
                if !rset.contains(&k) && seen.insert(k) {
                    out.push(r);
                }
            }
            out
        }
    }
}

/// Everything except ORDER BY / LIMIT / OFFSET: joins, filters, grouping,
/// aggregation, HAVING and DISTINCT.
fn run_body(rt: &mut Runtime, plan: &Plan) -> Result<Vec<OutRow>> {
    let max_rows = rt.limits.max_rows;
    let mut out: Vec<OutRow> = Vec::new();

    if plan.group_by.is_empty() && plan.aggregates.is_empty() {
        let mut seen: HashSet<Vec<u8>> = HashSet::new();
        let distinct = plan.distinct;
        let mut err_rows = false;
        for_each_joined_row(rt, plan, &mut |rt| {
            let values = eval_result_row(rt, plan, &[])?;
            if distinct && !seen.insert(dedup_key(&values)) {
                return Ok(true);
            }
            let keys = eval_sort_keys(rt, plan, &values, &[])?;
            out.push(OutRow { values, keys });
            if out.len() > max_rows {
                err_rows = true;
                return Ok(false);
            }
            Ok(true)
        })?;
        if err_rows {
            return Err(Error::Budget(format!("more than {max_rows} result rows")));
        }
        return Ok(out);
    }

    // Grouped or whole-table aggregation.
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<Vec<u8>, usize> = HashMap::new();
    let grouped = !plan.group_by.is_empty();
    // SQLite's rule for bare columns: with exactly one MIN or MAX aggregate,
    // they come from the row that produced that extreme.
    let single_extreme = plan
        .aggregates
        .iter()
        .filter(|a| matches!(a.func, AggFunc::Min | AggFunc::Max))
        .count()
        == 1;
    let mut overflow = false;
    for_each_joined_row(rt, plan, &mut |rt| {
        let key: Vec<Value> = if grouped {
            let mut k = Vec::with_capacity(plan.group_by.len());
            for g in &plan.group_by {
                k.push(eval(rt, g, &[])?);
            }
            k
        } else {
            Vec::new()
        };
        let ek = dedup_key(&key);
        let gi = match index.get(&ek) {
            Some(i) => *i,
            None => {
                if groups.len() >= max_rows {
                    overflow = true;
                    return Ok(false);
                }
                groups.push(Group::new(&plan.aggregates, key, rt.snapshot_rows()));
                index.insert(ek, groups.len() - 1);
                groups.len() - 1
            }
        };
        // Accumulate every aggregate for this row.
        let mut new_extreme = false;
        for (ai, spec) in plan.aggregates.iter().enumerate() {
            let mut args = Vec::with_capacity(spec.args.len());
            for a in &spec.args {
                args.push(eval(rt, a, &[])?);
            }
            new_extreme |= groups[gi].accumulate(ai, spec, args);
        }
        if new_extreme && single_extreme {
            groups[gi].rows = rt.snapshot_rows();
        }
        Ok(true)
    })?;
    if overflow {
        return Err(Error::Budget(format!("more than {max_rows} groups")));
    }

    if !grouped && groups.is_empty() {
        // Aggregates over an empty set still produce one row.
        groups.push(Group::new(&plan.aggregates, Vec::new(), rt.snapshot_rows()));
    }
    // Deterministic output order: by the GROUP BY key, like SQLite's sorter.
    groups.sort_by(|a, b| {
        for (x, y) in a.key.iter().zip(b.key.iter()) {
            match compare(x, y, Collation::Binary) {
                Ordering::Equal => {}
                o => return o,
            }
        }
        Ordering::Equal
    });

    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for group in &groups {
        let agg_values = group.finish(&plan.aggregates);
        // Put the group's row back so bare columns and the GROUP BY keys read
        // the values they had.
        rt.restore_rows(&group.rows);
        set_group_registers(rt, plan, group);
        if let Some(h) = &plan.having {
            if eval(rt, h, &agg_values)?.truth() != Some(true) {
                continue;
            }
        }
        let values = eval_result_row(rt, plan, &agg_values)?;
        if plan.distinct && !seen.insert(dedup_key(&values)) {
            continue;
        }
        let keys = eval_sort_keys(rt, plan, &values, &agg_values)?;
        out.push(OutRow { values, keys });
    }
    Ok(out)
}

/// Aggregation evaluates result expressions after the loop has ended, so bare
/// GROUP BY columns are restored from the group key.
fn set_group_registers(rt: &mut Runtime, plan: &Plan, group: &Group) {
    for (i, g) in plan.group_by.iter().enumerate() {
        let Some(v) = group.key.get(i) else { continue };
        if let PExpr::Column { slot, col, .. } = g {
            if let Some(slot_row) = rt.rows.get_mut(*slot) {
                let row = slot_row.get_or_insert_with(|| RowData {
                    rowid: 0,
                    values: Vec::new(),
                });
                while row.values.len() <= *col {
                    row.values.push(Value::Null);
                }
                row.values[*col] = v.clone();
            }
        } else if let PExpr::Rowid { slot } = g {
            if let Some(slot_row) = rt.rows.get_mut(*slot) {
                let row = slot_row.get_or_insert_with(|| RowData {
                    rowid: 0,
                    values: Vec::new(),
                });
                row.rowid = v.as_integer().unwrap_or(0);
            }
        }
    }
}

fn eval_result_row(rt: &mut Runtime, plan: &Plan, aggs: &[Value]) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(plan.result.len());
    for col in &plan.result {
        out.push(eval(rt, &col.expr, aggs)?);
    }
    Ok(out)
}

fn eval_sort_keys(
    rt: &mut Runtime,
    plan: &Plan,
    _values: &[Value],
    aggs: &[Value],
) -> Result<Vec<Value>> {
    if plan.order_by.is_empty() || plan.ordered_by_access {
        return Ok(Vec::new());
    }
    let mut keys = Vec::with_capacity(plan.order_by.len());
    for (e, _, _) in &plan.order_by {
        keys.push(eval(rt, e, aggs)?);
    }
    Ok(keys)
}

fn sort_rows(_rt: &mut Runtime, plan: &Plan, rows: &mut [OutRow]) -> Result<()> {
    let terms: Vec<(bool, bool)> = plan
        .order_by
        .iter()
        .map(|(_, desc, nulls_first)| (*desc, *nulls_first))
        .collect();
    sort_by_terms(rows, &terms);
    Ok(())
}

/// Stable sort by the buffered sort keys. NULLs come first for ASC and last
/// for DESC unless the statement said otherwise.
fn sort_by_terms(rows: &mut [OutRow], terms: &[(bool, bool)]) {
    rows.sort_by(|a, b| {
        for (i, (desc, nulls_first)) in terms.iter().enumerate() {
            let null = Value::Null;
            let x = a.keys.get(i).unwrap_or(&null);
            let y = b.keys.get(i).unwrap_or(&null);
            let ord = match (x.is_null(), y.is_null()) {
                (true, true) => Ordering::Equal,
                (true, false) => {
                    if *nulls_first {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                (false, true) => {
                    if *nulls_first {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                (false, false) => {
                    let o = compare(x, y, Collation::Binary);
                    if *desc {
                        o.reverse()
                    } else {
                        o
                    }
                }
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
}

fn eval_count(rt: &mut Runtime, e: Option<&PExpr>) -> Result<Option<u64>> {
    let Some(e) = e else { return Ok(None) };
    let v = eval(rt, e, &[])?;
    Ok(match apply_numeric_affinity(v) {
        // A negative LIMIT means "no limit"; a negative OFFSET means zero.
        Value::Integer(i) if i < 0 => None,
        Value::Integer(i) => Some(i as u64),
        Value::Real(f) if f < 0.0 => None,
        Value::Real(f) => Some(f as u64),
        Value::Null => None,
        _ => Some(0),
    })
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

struct Group {
    key: Vec<Value>,
    acc: Vec<Acc>,
    /// Row registers a column outside the GROUP BY reads from. SQLite calls
    /// these "bare columns": they take the value from one row of the group,
    /// and from the min/max row when the query has exactly one such aggregate.
    rows: Vec<Option<RowData>>,
}

struct Acc {
    count: i64,
    sum_int: i128,
    sum_real: f64,
    any_real: bool,
    any_value: bool,
    min: Option<Value>,
    max: Option<Value>,
    text: Vec<String>,
    sep: Option<String>,
    seen: Option<HashSet<Vec<u8>>>,
}

impl Group {
    fn new(specs: &[AggSpec], key: Vec<Value>, rows: Vec<Option<RowData>>) -> Group {
        Group {
            key,
            rows,
            acc: specs
                .iter()
                .map(|s| Acc {
                    count: 0,
                    sum_int: 0,
                    sum_real: 0.0,
                    any_real: false,
                    any_value: false,
                    min: None,
                    max: None,
                    text: Vec::new(),
                    sep: None,
                    seen: if s.distinct {
                        Some(HashSet::new())
                    } else {
                        None
                    },
                })
                .collect(),
        }
    }

    /// Returns true when this row became the min/max row of the group.
    fn accumulate(&mut self, i: usize, spec: &AggSpec, args: Vec<Value>) -> bool {
        let acc = &mut self.acc[i];
        if spec.func == AggFunc::CountStar {
            acc.count += 1;
            return false;
        }
        let Some(first) = args.first() else {
            acc.count += 1;
            return false;
        };
        if first.is_null() {
            return false;
        }
        if let Some(seen) = &mut acc.seen {
            if !seen.insert(dedup_key(std::slice::from_ref(first))) {
                return false;
            }
        }
        acc.count += 1;
        acc.any_value = true;
        match spec.func {
            AggFunc::Sum | AggFunc::Total | AggFunc::Avg => match apply_numeric_affinity(first.clone()) {
                Value::Integer(v) => acc.sum_int += v as i128,
                Value::Real(v) => {
                    acc.any_real = true;
                    acc.sum_real += v;
                }
                other => {
                    acc.any_real = true;
                    acc.sum_real += other.as_real().unwrap_or(0.0);
                }
            },
            AggFunc::Min => {
                if acc
                    .min
                    .as_ref()
                    .map_or(true, |m| compare(first, m, Collation::Binary) == Ordering::Less)
                {
                    acc.min = Some(first.clone());
                    return true;
                }
            }
            AggFunc::Max => {
                if acc.max.as_ref().map_or(true, |m| {
                    compare(first, m, Collation::Binary) == Ordering::Greater
                }) {
                    acc.max = Some(first.clone());
                    return true;
                }
            }
            AggFunc::GroupConcat => {
                acc.text.push(to_text(first));
                if let Some(sep) = args.get(1) {
                    acc.sep = Some(to_text(sep));
                }
            }
            AggFunc::Count | AggFunc::CountStar => {}
        }
        false
    }

    fn finish(&self, specs: &[AggSpec]) -> Vec<Value> {
        specs
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let acc = &self.acc[i];
                match spec.func {
                    AggFunc::Count | AggFunc::CountStar => Value::Integer(acc.count),
                    AggFunc::Sum => {
                        if !acc.any_value {
                            Value::Null
                        } else if acc.any_real {
                            Value::Real(acc.sum_real + acc.sum_int as f64)
                        } else {
                            Value::Integer(acc.sum_int as i64)
                        }
                    }
                    AggFunc::Total => Value::Real(acc.sum_real + acc.sum_int as f64),
                    AggFunc::Avg => {
                        if acc.count == 0 {
                            Value::Null
                        } else {
                            Value::Real((acc.sum_real + acc.sum_int as f64) / acc.count as f64)
                        }
                    }
                    AggFunc::Min => acc.min.clone().unwrap_or(Value::Null),
                    AggFunc::Max => acc.max.clone().unwrap_or(Value::Null),
                    AggFunc::GroupConcat => {
                        if acc.text.is_empty() {
                            Value::Null
                        } else {
                            Value::Text(acc.text.join(acc.sep.as_deref().unwrap_or(",")))
                        }
                    }
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Join loop
// ---------------------------------------------------------------------------

fn for_each_joined_row(
    rt: &mut Runtime,
    plan: &Plan,
    on_row: &mut dyn FnMut(&mut Runtime) -> Result<bool>,
) -> Result<()> {
    // Uncorrelated FROM subqueries are materialized once.
    let mut materialized: Vec<Option<Vec<RowData>>> = Vec::with_capacity(plan.levels.len());
    for level in &plan.levels {
        match &level.source {
            Source::Subquery(sub) => {
                let mut rows = Vec::new();
                run(rt, sub, &mut |values| {
                    rows.push(RowData { rowid: 0, values });
                    Ok(true)
                })?;
                materialized.push(Some(rows));
            }
            _ => materialized.push(None),
        }
    }
    match &plan.post_filter {
        Some(f) => {
            let f = f.clone();
            let mut guarded = |rt: &mut Runtime| -> Result<bool> {
                if eval(rt, &f, &[])?.truth() != Some(true) {
                    return Ok(true);
                }
                on_row(rt)
            };
            walk_level(rt, plan, 0, &materialized, &mut guarded)?;
        }
        None => {
            walk_level(rt, plan, 0, &materialized, on_row)?;
        }
    }
    Ok(())
}

/// Returns false when the consumer asked to stop.
fn walk_level(
    rt: &mut Runtime,
    plan: &Plan,
    i: usize,
    materialized: &[Option<Vec<RowData>>],
    on_row: &mut dyn FnMut(&mut Runtime) -> Result<bool>,
) -> Result<bool> {
    if i >= plan.levels.len() {
        return on_row(rt);
    }
    let level = &plan.levels[i];
    let slot = level.slot;
    let mut matched = false;
    let mut keep_going = true;

    {
        let mut visit = |rt: &mut Runtime, row: RowData| -> Result<bool> {
            rt.step()?;
            rt.rows[slot] = Some(row);
            if let Some(f) = &level.filter {
                if eval(rt, f, &[])?.truth() != Some(true) {
                    return Ok(true);
                }
            }
            matched = true;
            walk_level(rt, plan, i + 1, materialized, on_row)
        };

        match &level.source {
            Source::Subquery(_) => {
                let rows = materialized[i].as_ref().expect("materialized subquery");
                for row in rows.clone() {
                    if !visit(rt, row)? {
                        keep_going = false;
                        break;
                    }
                }
            }
            Source::Table { root, access } => {
                if *root == 0 {
                    // Constant level: one dummy row (SELECT with no FROM).
                    keep_going = visit(
                        rt,
                        RowData {
                            rowid: 0,
                            values: Vec::new(),
                        },
                    )?;
                } else {
                    keep_going = scan_access(
                        rt,
                        *root,
                        access,
                        level.table.as_ref(),
                        level.needed_columns,
                        &mut visit,
                    )?;
                }
            }
        }
    }

    if !matched && level.outer && keep_going {
        let width = level_columns(level);
        rt.rows[slot] = Some(RowData {
            rowid: 0,
            values: vec![Value::Null; width],
        });
        // The join condition already failed, so no filter is applied here.
        keep_going = walk_level(rt, plan, i + 1, materialized, on_row)?;
    }
    rt.rows[slot] = None;
    Ok(keep_going)
}

fn level_columns(level: &crate::plan::PlanLevel) -> usize {
    level
        .table
        .as_ref()
        .map(|t| t.columns.len())
        .unwrap_or(level.column_names.len())
}

fn scan_access(
    rt: &mut Runtime,
    root: u32,
    access: &Access,
    table: Option<&crate::schema::TableInfo>,
    needed: usize,
    visit: &mut dyn FnMut(&mut Runtime, RowData) -> Result<bool>,
) -> Result<bool> {
    match access {
        Access::Scan => {
            let mut cursor = TableCursor::new(root);
            cursor.rewind(rt.pager)?;
            while let Some(row) = cursor.next(rt.pager)? {
                let values = row.payload.prefix(rt.pager, needed, TextMode::Strict)?;
                let data = pad(values, table, needed);
                if !visit(rt, RowData { rowid: row.rowid, values: data })? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Access::RowidEq(e) => {
            let v = eval(rt, e, &[])?;
            let Some(id) = to_rowid(&v) else {
                return Ok(true);
            };
            let mut cursor = TableCursor::new(root);
            if let Some(row) = cursor.seek_exact(rt.pager, id)? {
                let values = row.payload.prefix(rt.pager, needed, TextMode::Strict)?;
                let data = pad(values, table, needed);
                return visit(rt, RowData { rowid: row.rowid, values: data });
            }
            Ok(true)
        }
        Access::RowidRange { low, high } => {
            let lo = match low {
                Some((e, inclusive)) => {
                    let v = eval(rt, e, &[])?;
                    match to_rowid_bound(&v, *inclusive, true) {
                        RowidBound::At(x) => x,
                        RowidBound::Unbounded => i64::MIN,
                        RowidBound::Empty => return Ok(true),
                    }
                }
                None => i64::MIN,
            };
            let hi = match high {
                Some((e, inclusive)) => {
                    let v = eval(rt, e, &[])?;
                    match to_rowid_bound(&v, *inclusive, false) {
                        RowidBound::At(x) => x,
                        RowidBound::Unbounded => i64::MAX,
                        RowidBound::Empty => return Ok(true),
                    }
                }
                None => i64::MAX,
            };
            if lo > hi {
                return Ok(true);
            }
            let mut cursor = TableCursor::new(root);
            cursor.seek_ge(rt.pager, lo)?;
            while let Some(row) = cursor.next(rt.pager)? {
                if row.rowid > hi {
                    break;
                }
                let values = row.payload.prefix(rt.pager, needed, TextMode::Strict)?;
                let data = pad(values, table, needed);
                if !visit(rt, RowData { rowid: row.rowid, values: data })? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Access::Index {
            root: index_root,
            columns,
            eq,
            low,
            high,
            unique_eq,
            ..
        } => {
            // Build the seek key with each column's affinity applied, the way
            // SQLite converts a comparison operand before probing an index.
            let mut key = Vec::with_capacity(eq.len() + 1);
            for (i, e) in eq.iter().enumerate() {
                let v = eval(rt, e, &[])?;
                if v.is_null() {
                    return Ok(true); // NULL never equals an indexed value
                }
                key.push(seek_value(v, table, columns.get(i).map(|c| c.0)));
            }
            let mut colls: Vec<Collation> = columns
                .iter()
                .take(eq.len())
                .map(|c| c.1)
                .collect();
            let range_col = columns.get(eq.len()).copied();
            let mut low_value = None;
            if let Some((e, inclusive)) = low {
                let v = eval(rt, e, &[])?;
                if v.is_null() {
                    return Ok(true);
                }
                let v = seek_value(v, table, range_col.map(|c| c.0));
                low_value = Some((v.clone(), *inclusive));
                key.push(v);
                colls.push(range_col.map(|c| c.1).unwrap_or(Collation::Binary));
            }
            let high_value = match high {
                Some((e, inclusive)) => {
                    let v = eval(rt, e, &[])?;
                    if v.is_null() {
                        return Ok(true);
                    }
                    Some((seek_value(v, table, range_col.map(|c| c.0)), *inclusive))
                }
                None => None,
            };

            let mut index_cursor = IndexCursor::new(*index_root);
            index_cursor.seek_ge(rt.pager, &key, &colls)?;
            let mut table_cursor = TableCursor::new(root);
            while let Some(entry) = index_cursor.next(rt.pager)? {
                let entry_values = entry.values(rt.pager, TextMode::Strict)?;
                // Equality prefix must still hold, otherwise the range is over.
                let mut done = false;
                for (i, want) in key.iter().enumerate().take(eq.len()) {
                    let coll = colls.get(i).copied().unwrap_or(Collation::Binary);
                    let got = entry_values.get(i).unwrap_or(&Value::Null);
                    if compare(got, want, coll) != Ordering::Equal {
                        done = true;
                        break;
                    }
                }
                if done {
                    break;
                }
                if let Some((lo, inclusive)) = &low_value {
                    let coll = colls.last().copied().unwrap_or(Collation::Binary);
                    let got = entry_values.get(eq.len()).unwrap_or(&Value::Null);
                    let ord = compare(got, lo, coll);
                    if ord == Ordering::Less || (!*inclusive && ord == Ordering::Equal) {
                        continue;
                    }
                }
                if let Some((hi, inclusive)) = &high_value {
                    let coll = range_col.map(|c| c.1).unwrap_or(Collation::Binary);
                    let got = entry_values.get(eq.len()).unwrap_or(&Value::Null);
                    let ord = compare(got, hi, coll);
                    if ord == Ordering::Greater || (!*inclusive && ord == Ordering::Equal) {
                        break;
                    }
                }
                let Some(rowid) = entry_values.last().and_then(Value::as_integer) else {
                    return Err(Error::corrupt("index entry without a rowid"));
                };
                let Some(row) = table_cursor.seek_exact(rt.pager, rowid)? else {
                    return Err(Error::corrupt("index entry points at a missing row"));
                };
                let values = row.payload.prefix(rt.pager, needed, TextMode::Strict)?;
                let data = pad(values, table, needed);
                if !visit(rt, RowData { rowid: row.rowid, values: data })? {
                    return Ok(false);
                }
                if *unique_eq && low_value.is_none() && high_value.is_none() {
                    break;
                }
            }
            Ok(true)
        }
    }
}

fn seek_value(v: Value, table: Option<&crate::schema::TableInfo>, col: Option<usize>) -> Value {
    match (table, col) {
        (Some(t), Some(c)) => match t.columns.get(c) {
            Some(info) => apply_affinity(v, info.affinity),
            None => v,
        },
        _ => v,
    }
}

/// Records may be shorter than the table (columns added by ALTER TABLE); fill
/// from the column defaults, exactly like SQLite.
fn pad(
    mut values: Vec<Value>,
    table: Option<&crate::schema::TableInfo>,
    needed: usize,
) -> Vec<Value> {
    let Some(t) = table else { return values };
    let want = needed.min(t.columns.len());
    while values.len() < want {
        values.push(t.columns[values.len()].default.clone());
    }
    t.fix_real_affinity(&mut values);
    values
}

fn to_rowid(v: &Value) -> Option<i64> {
    match apply_numeric_affinity(v.clone()) {
        Value::Integer(i) => Some(i),
        Value::Real(f) if f.floor() == f && f.abs() < 9.2e18 => Some(f as i64),
        _ => None,
    }
}

/// What a range bound means for a rowid scan.
enum RowidBound {
    /// A concrete rowid limit.
    At(i64),
    /// The bound excludes every row.
    Empty,
    /// The bound admits every row.
    Unbounded,
}

/// Turn a comparison bound into a rowid limit, honouring SQLite's rules: the
/// rowid has INTEGER affinity, so a text or blob bound never converts, and
/// every integer sorts before it.
fn to_rowid_bound(v: &Value, inclusive: bool, is_low: bool) -> RowidBound {
    let n = match apply_numeric_affinity(v.clone()) {
        Value::Integer(i) => i as f64,
        Value::Real(f) => f,
        // NULL compares false against everything.
        Value::Null => return RowidBound::Empty,
        // Text or blob that is not a number: integers are all smaller.
        _ => {
            return if is_low {
                RowidBound::Empty
            } else {
                RowidBound::Unbounded
            }
        }
    };
    if n.is_nan() {
        return RowidBound::Empty;
    }
    if is_low {
        if n < i64::MIN as f64 {
            return RowidBound::Unbounded;
        }
        if n > i64::MAX as f64 {
            return RowidBound::Empty;
        }
        let integral = n.floor() == n;
        let mut r = n.ceil() as i64;
        if !inclusive && integral {
            match r.checked_add(1) {
                Some(v) => r = v,
                None => return RowidBound::Empty,
            }
        }
        RowidBound::At(r)
    } else {
        if n > i64::MAX as f64 {
            return RowidBound::Unbounded;
        }
        if n < i64::MIN as f64 {
            return RowidBound::Empty;
        }
        let integral = n.floor() == n;
        let mut r = n.floor() as i64;
        if !inclusive && integral {
            match r.checked_sub(1) {
                Some(v) => r = v,
                None => return RowidBound::Empty,
            }
        }
        RowidBound::At(r)
    }
}

// ---------------------------------------------------------------------------
// Expression evaluation
// ---------------------------------------------------------------------------

pub fn eval(rt: &mut Runtime, e: &PExpr, aggs: &[Value]) -> Result<Value> {
    Ok(match e {
        PExpr::Literal(v) => v.clone(),
        PExpr::Param(i) => rt
            .params
            .get(*i)
            .cloned()
            .ok_or_else(|| Error::sql(format!("parameter {} was not bound", i + 1)))?,
        PExpr::Column { slot, col, .. } => rt
            .slot(*slot)
            .and_then(|r| r.values.get(*col).cloned())
            .unwrap_or(Value::Null),
        PExpr::Rowid { slot } => match rt.slot(*slot) {
            Some(r) => Value::Integer(r.rowid),
            None => Value::Null,
        },
        PExpr::Agg(i) => aggs.get(*i).cloned().unwrap_or(Value::Null),
        PExpr::Unary { op, expr } => {
            let v = eval(rt, expr, aggs)?;
            match op {
                UnaryOp::Identity => v,
                UnaryOp::Negate => match apply_numeric_affinity(v) {
                    Value::Integer(i) => Value::Integer(i.wrapping_neg()),
                    Value::Real(f) => Value::Real(-f),
                    Value::Null => Value::Null,
                    other => Value::Real(-numeric(&other)),
                },
                UnaryOp::Not => match v.truth() {
                    Some(b) => Value::Integer(i64::from(!b)),
                    None => Value::Null,
                },
                UnaryOp::BitNot => match apply_numeric_affinity(v) {
                    Value::Null => Value::Null,
                    other => Value::Integer(!to_int(&other)),
                },
            }
        }
        PExpr::Binary { op, lhs, rhs, coll } => {
            match op {
                BinOp::And => {
                    let l = eval(rt, lhs, aggs)?.truth();
                    if l == Some(false) {
                        return Ok(Value::Integer(0));
                    }
                    let r = eval(rt, rhs, aggs)?.truth();
                    return Ok(match (l, r) {
                        (_, Some(false)) => Value::Integer(0),
                        (Some(true), Some(true)) => Value::Integer(1),
                        _ => Value::Null,
                    });
                }
                BinOp::Or => {
                    let l = eval(rt, lhs, aggs)?.truth();
                    if l == Some(true) {
                        return Ok(Value::Integer(1));
                    }
                    let r = eval(rt, rhs, aggs)?.truth();
                    return Ok(match (l, r) {
                        (_, Some(true)) => Value::Integer(1),
                        (Some(false), Some(false)) => Value::Integer(0),
                        _ => Value::Null,
                    });
                }
                _ => {}
            }
            let l = eval(rt, lhs, aggs)?;
            let r = eval(rt, rhs, aggs)?;
            match op {
                BinOp::Is | BinOp::IsNot => {
                    let same = match (&l, &r) {
                        (Value::Null, Value::Null) => true,
                        (Value::Null, _) | (_, Value::Null) => false,
                        _ => {
                            let (lv, rv) = comparison_operands(&l, &r, lhs.affinity(), rhs.affinity());
                            compare(&lv, &rv, *coll) == Ordering::Equal
                        }
                    };
                    Value::Integer(i64::from(if *op == BinOp::Is { same } else { !same }))
                }
                op if op.is_comparison() => {
                    if l.is_null() || r.is_null() {
                        Value::Null
                    } else {
                        let (lv, rv) = comparison_operands(&l, &r, lhs.affinity(), rhs.affinity());
                        let ord = compare(&lv, &rv, *coll);
                        let res = match op {
                            BinOp::Eq => ord == Ordering::Equal,
                            BinOp::Ne => ord != Ordering::Equal,
                            BinOp::Lt => ord == Ordering::Less,
                            BinOp::Le => ord != Ordering::Greater,
                            BinOp::Gt => ord == Ordering::Greater,
                            BinOp::Ge => ord != Ordering::Less,
                            _ => false,
                        };
                        Value::Integer(i64::from(res))
                    }
                }
                BinOp::Concat => {
                    if l.is_null() || r.is_null() {
                        Value::Null
                    } else {
                        Value::Text(format!("{}{}", to_text(&l), to_text(&r)))
                    }
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                    arithmetic(*op, &l, &r)
                }
                BinOp::BitAnd | BinOp::BitOr | BinOp::Shl | BinOp::Shr => {
                    if l.is_null() || r.is_null() {
                        Value::Null
                    } else {
                        let (a, b) = (to_int(&l), to_int(&b_num(&r)));
                        Value::Integer(match op {
                            BinOp::BitAnd => a & b,
                            BinOp::BitOr => a | b,
                            BinOp::Shl => shift(a, b, true),
                            _ => shift(a, b, false),
                        })
                    }
                }
                _ => Value::Null,
            }
        }
        PExpr::IsNull { expr, negated } => {
            let v = eval(rt, expr, aggs)?;
            Value::Integer(i64::from(v.is_null() != *negated))
        }
        PExpr::Between {
            expr,
            low,
            high,
            negated,
            coll,
        } => {
            let v = eval(rt, expr, aggs)?;
            let lo = eval(rt, low, aggs)?;
            let hi = eval(rt, high, aggs)?;
            if v.is_null() || lo.is_null() || hi.is_null() {
                Value::Null
            } else {
                let (a, b) = comparison_operands(&v, &lo, expr.affinity(), low.affinity());
                let ge = compare(&a, &b, *coll) != Ordering::Less;
                let (a, c) = comparison_operands(&v, &hi, expr.affinity(), high.affinity());
                let le = compare(&a, &c, *coll) != Ordering::Greater;
                let inside = ge && le;
                Value::Integer(i64::from(inside != *negated))
            }
        }
        PExpr::InList {
            expr,
            list,
            negated,
            coll,
        } => {
            let v = eval(rt, expr, aggs)?;
            let mut found = false;
            let mut any_null = false;
            for item in list {
                let iv = eval(rt, item, aggs)?;
                if v.is_null() || iv.is_null() {
                    any_null = true;
                    continue;
                }
                let (a, b) = comparison_operands(&v, &iv, expr.affinity(), item.affinity());
                if compare(&a, &b, *coll) == Ordering::Equal {
                    found = true;
                    break;
                }
            }
            in_result(found, any_null, *negated)
        }
        PExpr::InSelect {
            expr,
            plan,
            negated,
            coll,
        } => {
            let v = eval(rt, expr, aggs)?;
            let mut found = false;
            let mut any_null = false;
            let mut items: Vec<Value> = Vec::new();
            run(rt, plan, &mut |row| {
                items.push(row.into_iter().next().unwrap_or(Value::Null));
                Ok(true)
            })?;
            for iv in items {
                if v.is_null() || iv.is_null() {
                    any_null = true;
                    continue;
                }
                let (a, b) = comparison_operands(&v, &iv, expr.affinity(), Affinity::Blob);
                if compare(&a, &b, *coll) == Ordering::Equal {
                    found = true;
                    break;
                }
            }
            in_result(found, any_null, *negated)
        }
        PExpr::Exists { plan, negated } => {
            let mut found = false;
            run(rt, plan, &mut |_row| {
                found = true;
                Ok(false)
            })?;
            Value::Integer(i64::from(found != *negated))
        }
        PExpr::Subquery(plan) => {
            let mut out = Value::Null;
            run(rt, plan, &mut |row| {
                out = row.into_iter().next().unwrap_or(Value::Null);
                Ok(false)
            })?;
            out
        }
        PExpr::Func { func, args } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval(rt, a, aggs)?);
            }
            scalar(*func, &vals)?
        }
        PExpr::Case {
            operand,
            whens,
            else_result,
        } => {
            let base = match operand {
                Some(o) => Some(eval(rt, o, aggs)?),
                None => None,
            };
            let mut out = Value::Null;
            let mut done = false;
            for (cond, result) in whens {
                let c = eval(rt, cond, aggs)?;
                let hit = match &base {
                    Some(b) => {
                        !b.is_null()
                            && !c.is_null()
                            && compare(b, &c, Collation::Binary) == Ordering::Equal
                    }
                    None => c.truth() == Some(true),
                };
                if hit {
                    out = eval(rt, result, aggs)?;
                    done = true;
                    break;
                }
            }
            if !done {
                if let Some(e) = else_result {
                    out = eval(rt, e, aggs)?;
                }
            }
            out
        }
        PExpr::Cast { expr, affinity } => {
            let v = eval(rt, expr, aggs)?;
            cast(v, *affinity)
        }
        PExpr::Collate { expr, .. } => eval(rt, expr, aggs)?,
        PExpr::Like {
            lhs,
            rhs,
            escape,
            negated,
            glob,
        } => {
            let text = eval(rt, lhs, aggs)?;
            let pattern = eval(rt, rhs, aggs)?;
            if text.is_null() || pattern.is_null() {
                Value::Null
            } else {
                let esc = match escape {
                    Some(e) => {
                        let v = eval(rt, e, aggs)?;
                        to_text(&v).chars().next()
                    }
                    None => None,
                };
                let m = if *glob {
                    glob_match(&to_text(&pattern), &to_text(&text))
                } else {
                    like_match(&to_text(&pattern), &to_text(&text), esc)
                };
                Value::Integer(i64::from(m != *negated))
            }
        }
    })
}

fn in_result(found: bool, any_null: bool, negated: bool) -> Value {
    if found {
        Value::Integer(i64::from(!negated))
    } else if any_null {
        Value::Null
    } else {
        Value::Integer(i64::from(negated))
    }
}

/// SQLite's comparison affinity rules (datatype3 section 4.2).
fn comparison_operands(l: &Value, r: &Value, la: Affinity, ra: Affinity) -> (Value, Value) {
    let numeric = |a: Affinity| matches!(a, Affinity::Integer | Affinity::Real | Affinity::Numeric);
    if numeric(la) && matches!(ra, Affinity::Text | Affinity::Blob) {
        return (l.clone(), apply_numeric_affinity(r.clone()));
    }
    if numeric(ra) && matches!(la, Affinity::Text | Affinity::Blob) {
        return (apply_numeric_affinity(l.clone()), r.clone());
    }
    if la == Affinity::Text && ra == Affinity::Blob {
        return (l.clone(), apply_affinity(r.clone(), Affinity::Text));
    }
    if ra == Affinity::Text && la == Affinity::Blob {
        return (apply_affinity(l.clone(), Affinity::Text), r.clone());
    }
    (l.clone(), r.clone())
}

fn b_num(v: &Value) -> Value {
    apply_numeric_affinity(v.clone())
}

fn shift(a: i64, b: i64, left: bool) -> i64 {
    let left = if b < 0 { !left } else { left };
    let n = b.unsigned_abs().min(64);
    if n >= 64 {
        return if left { 0 } else if a < 0 { -1 } else { 0 };
    }
    if left {
        ((a as u64) << n) as i64
    } else {
        a >> n
    }
}

fn numeric(v: &Value) -> f64 {
    match v {
        Value::Integer(i) => *i as f64,
        Value::Real(f) => *f,
        Value::Text(t) => text_to_number_prefix(t.as_bytes()),
        Value::Blob(b) => text_to_number_prefix(b),
        Value::Null => 0.0,
    }
}

fn to_int(v: &Value) -> i64 {
    match apply_numeric_affinity(v.clone()) {
        Value::Integer(i) => i,
        Value::Real(f) => f as i64,
        other => numeric(&other) as i64,
    }
}

pub fn to_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => format_real(*f),
        Value::Text(t) => t.clone(),
        Value::Blob(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

fn arithmetic(op: BinOp, l: &Value, r: &Value) -> Value {
    if l.is_null() || r.is_null() {
        return Value::Null;
    }
    let ln = apply_numeric_affinity(l.clone());
    let rn = apply_numeric_affinity(r.clone());
    let both_int = matches!(ln, Value::Integer(_)) && matches!(rn, Value::Integer(_));
    if both_int {
        let (a, b) = (ln.as_integer().unwrap_or(0), rn.as_integer().unwrap_or(0));
        return match op {
            BinOp::Add => match a.checked_add(b) {
                Some(v) => Value::Integer(v),
                None => Value::Real(a as f64 + b as f64),
            },
            BinOp::Sub => match a.checked_sub(b) {
                Some(v) => Value::Integer(v),
                None => Value::Real(a as f64 - b as f64),
            },
            BinOp::Mul => match a.checked_mul(b) {
                Some(v) => Value::Integer(v),
                None => Value::Real(a as f64 * b as f64),
            },
            BinOp::Div => {
                if b == 0 {
                    Value::Null
                } else {
                    match a.checked_div(b) {
                        Some(v) => Value::Integer(v),
                        None => Value::Real(a as f64 / b as f64),
                    }
                }
            }
            BinOp::Mod => {
                if b == 0 {
                    Value::Null
                } else {
                    match a.checked_rem(b) {
                        Some(v) => Value::Integer(v),
                        None => Value::Integer(0),
                    }
                }
            }
            _ => Value::Null,
        };
    }
    let (a, b) = (numeric(&ln), numeric(&rn));
    match op {
        BinOp::Add => Value::Real(a + b),
        BinOp::Sub => Value::Real(a - b),
        BinOp::Mul => Value::Real(a * b),
        BinOp::Div => {
            if b == 0.0 {
                Value::Null
            } else {
                Value::Real(a / b)
            }
        }
        BinOp::Mod => {
            let (ai, bi) = (a as i64, b as i64);
            if bi == 0 {
                Value::Null
            } else {
                Value::Integer(ai % bi)
            }
        }
        _ => Value::Null,
    }
}

fn cast(v: Value, aff: Affinity) -> Value {
    match aff {
        Affinity::Text => match v {
            Value::Null => Value::Null,
            other => Value::Text(to_text(&other)),
        },
        Affinity::Blob => match v {
            Value::Null => Value::Null,
            Value::Blob(b) => Value::Blob(b),
            other => Value::Blob(to_text(&other).into_bytes()),
        },
        Affinity::Integer => match v {
            Value::Null => Value::Null,
            other => Value::Integer(numeric(&other) as i64),
        },
        Affinity::Real => match v {
            Value::Null => Value::Null,
            other => Value::Real(numeric(&other)),
        },
        Affinity::Numeric => match v {
            Value::Null => Value::Null,
            other => {
                let n = numeric(&other);
                if n.floor() == n && n.abs() < 9.2e18 {
                    Value::Integer(n as i64)
                } else {
                    Value::Real(n)
                }
            }
        },
    }
}

fn scalar(func: ScalarFunc, args: &[Value]) -> Result<Value> {
    let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
    Ok(match func {
        ScalarFunc::Coalesce => args
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .unwrap_or(Value::Null),
        ScalarFunc::IfNull => {
            if arg(0).is_null() {
                arg(1)
            } else {
                arg(0)
            }
        }
        ScalarFunc::NullIf => {
            let (a, b) = (arg(0), arg(1));
            if !a.is_null() && !b.is_null() && compare(&a, &b, Collation::Binary) == Ordering::Equal
            {
                Value::Null
            } else {
                a
            }
        }
        ScalarFunc::Length => match arg(0) {
            Value::Null => Value::Null,
            Value::Blob(b) => Value::Integer(b.len() as i64),
            other => Value::Integer(to_text(&other).chars().count() as i64),
        },
        ScalarFunc::Lower => match arg(0) {
            Value::Null => Value::Null,
            other => Value::Text(to_text(&other).to_lowercase()),
        },
        ScalarFunc::Upper => match arg(0) {
            Value::Null => Value::Null,
            other => Value::Text(to_text(&other).to_uppercase()),
        },
        ScalarFunc::Abs => match apply_numeric_affinity(arg(0)) {
            Value::Null => Value::Null,
            Value::Integer(i) => Value::Integer(i.saturating_abs()),
            other => Value::Real(numeric(&other).abs()),
        },
        ScalarFunc::Substr => {
            let s = match arg(0) {
                Value::Null => return Ok(Value::Null),
                other => to_text(&other),
            };
            let chars: Vec<char> = s.chars().collect();
            let mut start = to_int(&arg(1));
            let len = if args.len() > 2 {
                Some(to_int(&arg(2)))
            } else {
                None
            };
            if start < 0 {
                start = (chars.len() as i64 + start + 1).max(1);
            }
            if start == 0 {
                start = 1;
            }
            let begin = (start - 1).max(0) as usize;
            let take = match len {
                Some(l) if l < 0 => 0usize,
                Some(l) => l as usize,
                None => chars.len().saturating_sub(begin),
            };
            Value::Text(chars.into_iter().skip(begin).take(take).collect())
        }
        ScalarFunc::Instr => {
            let (a, b) = (arg(0), arg(1));
            if a.is_null() || b.is_null() {
                Value::Null
            } else {
                let hay = to_text(&a);
                let needle = to_text(&b);
                match hay.find(&needle) {
                    Some(byte_pos) => Value::Integer(hay[..byte_pos].chars().count() as i64 + 1),
                    None => Value::Integer(0),
                }
            }
        }
        ScalarFunc::Replace => {
            let (a, b, c) = (arg(0), arg(1), arg(2));
            if a.is_null() || b.is_null() || c.is_null() {
                Value::Null
            } else {
                let pattern = to_text(&b);
                if pattern.is_empty() {
                    a
                } else {
                    Value::Text(to_text(&a).replace(&pattern, &to_text(&c)))
                }
            }
        }
        ScalarFunc::Trim | ScalarFunc::LTrim | ScalarFunc::RTrim => {
            let v = arg(0);
            if v.is_null() {
                Value::Null
            } else {
                let s = to_text(&v);
                let set: Vec<char> = if args.len() > 1 {
                    to_text(&arg(1)).chars().collect()
                } else {
                    vec![' ']
                };
                let pred = |c: char| set.contains(&c);
                Value::Text(match func {
                    ScalarFunc::Trim => s.trim_matches(pred).to_string(),
                    ScalarFunc::LTrim => s.trim_start_matches(pred).to_string(),
                    _ => s.trim_end_matches(pred).to_string(),
                })
            }
        }
        ScalarFunc::Hex => {
            let v = arg(0);
            let bytes = match &v {
                Value::Blob(b) => b.clone(),
                other => to_text(other).into_bytes(),
            };
            let mut s = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                s.push_str(&format!("{b:02X}"));
            }
            Value::Text(s)
        }
        ScalarFunc::Quote => Value::Text(quote_value(&arg(0))),
        ScalarFunc::TypeOf => Value::Text(
            match arg(0) {
                Value::Null => "null",
                Value::Integer(_) => "integer",
                Value::Real(_) => "real",
                Value::Text(_) => "text",
                Value::Blob(_) => "blob",
            }
            .to_string(),
        ),
        ScalarFunc::Round => {
            let v = arg(0);
            if v.is_null() {
                Value::Null
            } else {
                let digits = if args.len() > 1 { to_int(&arg(1)) } else { 0 };
                let n = numeric(&v);
                let f = 10f64.powi(digits.clamp(0, 15) as i32);
                Value::Real((n * f).round() / f)
            }
        }
        ScalarFunc::Min | ScalarFunc::Max => {
            let mut best: Option<Value> = None;
            for v in args {
                if v.is_null() {
                    return Ok(Value::Null);
                }
                best = Some(match best {
                    None => v.clone(),
                    Some(b) => {
                        let ord = compare(v, &b, Collation::Binary);
                        let take = if func == ScalarFunc::Min {
                            ord == Ordering::Less
                        } else {
                            ord == Ordering::Greater
                        };
                        if take {
                            v.clone()
                        } else {
                            b
                        }
                    }
                });
            }
            best.unwrap_or(Value::Null)
        }
        ScalarFunc::Iif => {
            if arg(0).truth() == Some(true) {
                arg(1)
            } else {
                arg(2)
            }
        }
        ScalarFunc::Unicode => match arg(0) {
            Value::Null => Value::Null,
            other => to_text(&other)
                .chars()
                .next()
                .map(|c| Value::Integer(c as i64))
                .unwrap_or(Value::Null),
        },
        ScalarFunc::Char => {
            let mut s = String::new();
            for v in args {
                if let Some(c) = char::from_u32(to_int(v) as u32) {
                    s.push(c);
                }
            }
            Value::Text(s)
        }
        ScalarFunc::Printf | ScalarFunc::Format => {
            return Err(Error::unsupported("printf()/format()"))
        }
    })
}

pub fn quote_value(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => {
            let s = format_real(*f);
            if s.contains('.') || s.contains('e') || s.contains("Inf") {
                s
            } else {
                format!("{s}.0")
            }
        }
        Value::Text(t) => format!("'{}'", t.replace('\'', "''")),
        Value::Blob(b) => {
            let mut s = String::from("X'");
            for byte in b {
                s.push_str(&format!("{byte:02X}"));
            }
            s.push('\'');
            s
        }
    }
}

/// SQLite's LIKE: `%` matches any run, `_` any single character, comparison is
/// ASCII case-insensitive, and ESCAPE disables the next wildcard.
///
/// Iterative backtracking (remember the last `%` and resume one character
/// later) so a pattern like `%%%%%x` stays linear instead of exponential.
fn like_match(pattern: &str, text: &str, escape: Option<char>) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let escaped = pi < p.len() && Some(p[pi]) == escape && pi + 1 < p.len();
        let pc = if escaped { Some(p[pi + 1]) } else { p.get(pi).copied() };
        match pc {
            Some('%') if !escaped => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some('_') if !escaped => {
                pi += 1;
                ti += 1;
            }
            Some(c) if eq_ci(c, t[ti]) => {
                pi += if escaped { 2 } else { 1 };
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    while pi < p.len() && p[pi] == '%' && Some(p[pi]) != escape {
        pi += 1;
    }
    pi >= p.len()
}

fn eq_ci(a: char, b: char) -> bool {
    a == b || a.to_ascii_lowercase() == b.to_ascii_lowercase()
}

/// GLOB: case sensitive, `*`, `?` and `[...]` character classes.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some('?') => {
                pi += 1;
                ti += 1;
            }
            Some('[') => match class_match(&p, pi, t[ti]) {
                Some((matched, next)) if matched => {
                    pi = next;
                    ti += 1;
                }
                Some(_) | None => match star {
                    Some((sp, st)) => {
                        pi = sp + 1;
                        ti = st + 1;
                        star = Some((sp, st + 1));
                    }
                    None => return false,
                },
            },
            Some(c) if *c == t[ti] => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi >= p.len()
}

/// Match one `[...]` class starting at `at`; returns (matched, index after the
/// closing bracket), or None when the class never closes.
fn class_match(p: &[char], at: usize, c: char) -> Option<(bool, usize)> {
    let mut i = at + 1;
    let negate = p.get(i) == Some(&'^');
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() && (p[i] != ']' || first) {
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if c >= p[i] && c <= p[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    if i >= p.len() {
        return None;
    }
    Some((matched != negate, i + 1))
}
