//! Query planning: name resolution, access-path selection and the plan tree
//! the executor walks.
//!
//! The planner is rule based, in the spirit of SQLite's own "next best index"
//! loop but far smaller: for every FROM item it picks the cheapest access path
//! it can prove correct — rowid equality, then a unique index equality, then
//! the index matching the most equality columns (optionally with a range on the
//! following column), then a rowid range, then a full scan. Every choice is
//! visible in [`Plan::explain`], which the tests assert on.

use crate::error::{Error, Result};
use crate::schema::{Schema, TableInfo};
use crate::sql::ast::*;
use crate::sql::lexer::ParamRef;
use crate::value::{affinity_of, Affinity, Collation, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Resolved expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PExpr {
    Literal(Value),
    /// 0-based index into the caller's parameter slice.
    Param(usize),
    Column {
        slot: usize,
        col: usize,
        affinity: Affinity,
        coll: Collation,
    },
    Rowid {
        slot: usize,
    },
    Unary {
        op: UnaryOp,
        expr: Box<PExpr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<PExpr>,
        rhs: Box<PExpr>,
        coll: Collation,
    },
    IsNull {
        expr: Box<PExpr>,
        negated: bool,
    },
    Between {
        expr: Box<PExpr>,
        low: Box<PExpr>,
        high: Box<PExpr>,
        negated: bool,
        coll: Collation,
    },
    InList {
        expr: Box<PExpr>,
        list: Vec<PExpr>,
        negated: bool,
        coll: Collation,
    },
    InSelect {
        expr: Box<PExpr>,
        plan: Box<Plan>,
        negated: bool,
        coll: Collation,
    },
    Exists {
        plan: Box<Plan>,
        negated: bool,
    },
    Subquery(Box<Plan>),
    Func {
        func: ScalarFunc,
        args: Vec<PExpr>,
    },
    /// Index into [`Plan::aggregates`].
    Agg(usize),
    Case {
        operand: Option<Box<PExpr>>,
        whens: Vec<(PExpr, PExpr)>,
        else_result: Option<Box<PExpr>>,
    },
    Cast {
        expr: Box<PExpr>,
        affinity: Affinity,
    },
    Collate {
        expr: Box<PExpr>,
        coll: Collation,
    },
    Like {
        lhs: Box<PExpr>,
        rhs: Box<PExpr>,
        escape: Option<Box<PExpr>>,
        negated: bool,
        glob: bool,
    },
}

impl PExpr {
    /// An explicit `COLLATE` on this operand, which outranks a column's own
    /// collation in a comparison.
    fn explicit_collation(&self) -> Option<Collation> {
        match self {
            PExpr::Collate { coll, .. } => Some(*coll),
            PExpr::Cast { expr, .. } => expr.explicit_collation(),
            _ => None,
        }
    }
    /// Affinity this expression contributes to a comparison.
    pub fn affinity(&self) -> Affinity {
        match self {
            PExpr::Column { affinity, .. } => *affinity,
            PExpr::Rowid { .. } => Affinity::Integer,
            PExpr::Cast { affinity, .. } => *affinity,
            PExpr::Collate { expr, .. } => expr.affinity(),
            _ => Affinity::Blob,
        }
    }
    /// Explicit or column collation, for comparison operands.
    fn collation(&self) -> Option<Collation> {
        match self {
            PExpr::Column { coll, .. } => Some(*coll),
            PExpr::Collate { coll, .. } => Some(*coll),
            PExpr::Cast { expr, .. } => expr.collation(),
            _ => None,
        }
    }
    /// Which row slots this expression reads.
    fn slots(&self, out: &mut Vec<usize>) {
        match self {
            PExpr::Literal(_) | PExpr::Param(_) | PExpr::Agg(_) => {}
            PExpr::Column { slot, .. } | PExpr::Rowid { slot } => out.push(*slot),
            PExpr::Unary { expr, .. }
            | PExpr::IsNull { expr, .. }
            | PExpr::Cast { expr, .. }
            | PExpr::Collate { expr, .. } => expr.slots(out),
            PExpr::Binary { lhs, rhs, .. } => {
                lhs.slots(out);
                rhs.slots(out);
            }
            PExpr::Between {
                expr, low, high, ..
            } => {
                expr.slots(out);
                low.slots(out);
                high.slots(out);
            }
            PExpr::InList { expr, list, .. } => {
                expr.slots(out);
                for e in list {
                    e.slots(out);
                }
            }
            PExpr::InSelect { expr, plan, .. } => {
                expr.slots(out);
                plan.outer_slots(out);
            }
            PExpr::Exists { plan, .. } | PExpr::Subquery(plan) => plan.outer_slots(out),
            PExpr::Func { args, .. } => {
                for a in args {
                    a.slots(out);
                }
            }
            PExpr::Case {
                operand,
                whens,
                else_result,
            } => {
                if let Some(o) = operand {
                    o.slots(out);
                }
                for (w, t) in whens {
                    w.slots(out);
                    t.slots(out);
                }
                if let Some(e) = else_result {
                    e.slots(out);
                }
            }
            PExpr::Like {
                lhs, rhs, escape, ..
            } => {
                lhs.slots(out);
                rhs.slots(out);
                if let Some(e) = escape {
                    e.slots(out);
                }
            }
        }
    }
    fn contains_agg(&self) -> bool {
        let mut found = false;
        self.walk(&mut |e| {
            if matches!(e, PExpr::Agg(_)) {
                found = true;
            }
        });
        found
    }
    fn walk(&self, f: &mut dyn FnMut(&PExpr)) {
        f(self);
        match self {
            PExpr::Unary { expr, .. }
            | PExpr::IsNull { expr, .. }
            | PExpr::Cast { expr, .. }
            | PExpr::Collate { expr, .. } => expr.walk(f),
            PExpr::Binary { lhs, rhs, .. } => {
                lhs.walk(f);
                rhs.walk(f);
            }
            PExpr::Between {
                expr, low, high, ..
            } => {
                expr.walk(f);
                low.walk(f);
                high.walk(f);
            }
            PExpr::InList { expr, list, .. } => {
                expr.walk(f);
                for e in list {
                    e.walk(f);
                }
            }
            PExpr::InSelect { expr, .. } => expr.walk(f),
            PExpr::Func { args, .. } => {
                for a in args {
                    a.walk(f);
                }
            }
            PExpr::Case {
                operand,
                whens,
                else_result,
            } => {
                if let Some(o) = operand {
                    o.walk(f);
                }
                for (w, t) in whens {
                    w.walk(f);
                    t.walk(f);
                }
                if let Some(e) = else_result {
                    e.walk(f);
                }
            }
            PExpr::Like {
                lhs, rhs, escape, ..
            } => {
                lhs.walk(f);
                rhs.walk(f);
                if let Some(e) = escape {
                    e.walk(f);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarFunc {
    Coalesce,
    IfNull,
    NullIf,
    Length,
    Lower,
    Upper,
    Abs,
    Substr,
    Instr,
    Replace,
    Trim,
    LTrim,
    RTrim,
    Hex,
    Quote,
    TypeOf,
    Round,
    Min,
    Max,
    Iif,
    Unicode,
    Char,
    Printf,
    Format,
}

impl ScalarFunc {
    pub fn from_name(name: &str) -> Option<ScalarFunc> {
        Some(match name.to_ascii_lowercase().as_str() {
            "coalesce" => ScalarFunc::Coalesce,
            "ifnull" => ScalarFunc::IfNull,
            "nullif" => ScalarFunc::NullIf,
            "length" => ScalarFunc::Length,
            "lower" => ScalarFunc::Lower,
            "upper" => ScalarFunc::Upper,
            "abs" => ScalarFunc::Abs,
            "substr" | "substring" => ScalarFunc::Substr,
            "instr" => ScalarFunc::Instr,
            "replace" => ScalarFunc::Replace,
            "trim" => ScalarFunc::Trim,
            "ltrim" => ScalarFunc::LTrim,
            "rtrim" => ScalarFunc::RTrim,
            "hex" => ScalarFunc::Hex,
            "quote" => ScalarFunc::Quote,
            "typeof" => ScalarFunc::TypeOf,
            "round" => ScalarFunc::Round,
            "min" => ScalarFunc::Min,
            "max" => ScalarFunc::Max,
            "iif" => ScalarFunc::Iif,
            "unicode" => ScalarFunc::Unicode,
            "char" => ScalarFunc::Char,
            "printf" => ScalarFunc::Printf,
            "format" => ScalarFunc::Format,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Count,
    CountStar,
    Sum,
    Total,
    Avg,
    Min,
    Max,
    GroupConcat,
}

impl AggFunc {
    fn from_name(name: &str) -> Option<AggFunc> {
        Some(match name.to_ascii_lowercase().as_str() {
            "count" => AggFunc::Count,
            "sum" => AggFunc::Sum,
            "total" => AggFunc::Total,
            "avg" => AggFunc::Avg,
            "min" => AggFunc::Min,
            "max" => AggFunc::Max,
            "group_concat" => AggFunc::GroupConcat,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AggSpec {
    pub func: AggFunc,
    pub args: Vec<PExpr>,
    pub distinct: bool,
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Access {
    /// Walk the whole table b-tree in rowid order.
    Scan,
    /// One row, by rowid.
    RowidEq(PExpr),
    RowidRange {
        low: Option<(PExpr, bool)>,
        high: Option<(PExpr, bool)>,
    },
    /// Seek an index b-tree, then fetch each row by the trailing rowid.
    Index {
        name: String,
        root: u32,
        /// Every key column of the index: (table column, collation).
        columns: Vec<(usize, Collation)>,
        /// Equality values for the leading key columns.
        eq: Vec<PExpr>,
        /// Optional range on the key column right after the equality prefix.
        low: Option<(PExpr, bool)>,
        high: Option<(PExpr, bool)>,
        /// The seek can stop after one row.
        unique_eq: bool,
    },
}

#[derive(Debug, Clone)]
pub enum Source {
    Table { root: u32, access: Access },
    Subquery(Box<Plan>),
}

#[derive(Debug, Clone)]
pub struct PlanLevel {
    /// Name rows are addressed by (alias or table name).
    pub name: String,
    pub slot: usize,
    pub source: Source,
    /// Predicates that can be checked once this level's row is available.
    pub filter: Option<PExpr>,
    /// LEFT JOIN: emit a NULL row when nothing matches.
    pub outer: bool,
    pub column_names: Vec<String>,
    pub table: Option<TableInfo>,
    /// How many leading columns of a row the statement actually reads. Rows are
    /// decoded only that far, so `COUNT(*)` decodes nothing at all.
    pub needed_columns: usize,
}

#[derive(Debug, Clone)]
pub struct ResultCol {
    pub name: String,
    pub expr: PExpr,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub levels: Vec<PlanLevel>,
    /// WHERE terms that must run after the whole row is assembled: a WHERE on
    /// a LEFT-joined table filters the NULL-extended row too, unlike an ON
    /// term which only decides whether the join found a match.
    pub post_filter: Option<PExpr>,
    pub result: Vec<ResultCol>,
    pub distinct: bool,
    pub group_by: Vec<PExpr>,
    pub having: Option<PExpr>,
    pub order_by: Vec<(PExpr, bool, bool)>,
    /// For a compound select, ORDER BY names result columns rather than table
    /// columns: (result column index, desc, nulls first).
    pub order_by_positions: Option<Vec<(usize, bool, bool)>>,
    pub limit: Option<PExpr>,
    pub offset: Option<PExpr>,
    pub aggregates: Vec<AggSpec>,
    /// True when the access path already returns rows in ORDER BY order.
    pub ordered_by_access: bool,
    /// Slots owned by this plan; anything below `slot_base` is an outer
    /// reference from an enclosing query.
    pub slot_base: usize,
    pub slot_count: usize,
    pub compound: Option<(CompoundOp, Box<Plan>)>,
}

impl Plan {
    /// Slots this plan reads from enclosing queries (correlation).
    fn outer_slots(&self, out: &mut Vec<usize>) {
        let mut inner = Vec::new();
        for level in &self.levels {
            if let Some(f) = &level.filter {
                f.slots(&mut inner);
            }
            match &level.source {
                Source::Table { access, .. } => access.slots(&mut inner),
                Source::Subquery(p) => p.outer_slots(&mut inner),
            }
        }
        for r in &self.result {
            r.expr.slots(&mut inner);
        }
        for g in &self.group_by {
            g.slots(&mut inner);
        }
        if let Some(h) = &self.having {
            h.slots(&mut inner);
        }
        for a in &self.aggregates {
            for arg in &a.args {
                arg.slots(&mut inner);
            }
        }
        for s in inner {
            if s < self.slot_base {
                out.push(s);
            }
        }
    }

    /// Highest column index (plus one) read from each row slot, including the
    /// slots a nested subquery reaches back into.
    pub fn column_usage(&self) -> HashMap<usize, usize> {
        let mut out = HashMap::new();
        self.note_columns(&mut out);
        out
    }

    fn note_columns(&self, out: &mut HashMap<usize, usize>) {
        let note = |e: &PExpr, out: &mut HashMap<usize, usize>| note_expr_columns(e, out);
        for level in &self.levels {
            if let Some(f) = &level.filter {
                note(f, out);
            }
            match &level.source {
                Source::Table { access, .. } => access.note_columns(out),
                Source::Subquery(p) => p.note_columns(out),
            }
        }
        if let Some(f) = &self.post_filter {
            note(f, out);
        }
        for r in &self.result {
            note(&r.expr, out);
        }
        for g in &self.group_by {
            note(g, out);
        }
        if let Some(h) = &self.having {
            note(h, out);
        }
        for (e, _, _) in &self.order_by {
            note(e, out);
        }
        for a in &self.aggregates {
            for arg in &a.args {
                note(arg, out);
            }
        }
        if let Some(e) = &self.limit {
            note(e, out);
        }
        if let Some(e) = &self.offset {
            note(e, out);
        }
        if let Some((_, p)) = &self.compound {
            p.note_columns(out);
        }
    }

    pub fn is_correlated(&self) -> bool {
        let mut v = Vec::new();
        self.outer_slots(&mut v);
        !v.is_empty()
    }

    pub fn column_names(&self) -> Vec<String> {
        self.result.iter().map(|c| c.name.clone()).collect()
    }

    /// One line per level plus the post-processing steps, in the spirit of
    /// `EXPLAIN QUERY PLAN`. Tests assert on this to prove a seek was chosen.
    pub fn explain(&self) -> String {
        let mut out = String::new();
        self.explain_into(&mut out, 0);
        out
    }

    fn explain_into(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        for level in &self.levels {
            match &level.source {
                Source::Table { access, .. } => {
                    let how = match access {
                        Access::Scan => "SCAN".to_string(),
                        Access::RowidEq(_) => "SEARCH rowid=?".to_string(),
                        Access::RowidRange { low, high } => format!(
                            "SEARCH rowid{}{}",
                            match low {
                                Some((_, true)) => ">=?",
                                Some((_, false)) => ">?",
                                None => "",
                            },
                            match high {
                                Some((_, true)) => "<=?",
                                Some((_, false)) => "<?",
                                None => "",
                            }
                        ),
                        Access::Index {
                            name, eq, low, high, ..
                        } => {
                            let mut s = format!("SEARCH USING INDEX {name} (");
                            for i in 0..eq.len() {
                                if i > 0 {
                                    s.push_str(" AND ");
                                }
                                s.push_str("=?");
                            }
                            if low.is_some() || high.is_some() {
                                if !eq.is_empty() {
                                    s.push_str(" AND ");
                                }
                                if low.is_some() {
                                    s.push_str(">?");
                                }
                                if high.is_some() {
                                    s.push_str("<?");
                                }
                            }
                            s.push(')');
                            s
                        }
                    };
                    let kind = if level.outer { "LEFT " } else { "" };
                    out.push_str(&format!(
                        "{pad}{kind}{how} {} (cols {})\n",
                        level.name, level.needed_columns
                    ));
                }
                Source::Subquery(p) => {
                    out.push_str(&format!("{pad}SUBQUERY {}\n", level.name));
                    p.explain_into(out, depth + 1);
                }
            }
        }
        if !self.group_by.is_empty() {
            out.push_str(&format!("{pad}GROUP BY\n"));
        } else if !self.aggregates.is_empty() {
            out.push_str(&format!("{pad}AGGREGATE\n"));
        }
        if self.distinct {
            out.push_str(&format!("{pad}DISTINCT\n"));
        }
        if !self.order_by.is_empty() {
            if self.ordered_by_access {
                out.push_str(&format!("{pad}ORDER BY (from index)\n"));
            } else {
                out.push_str(&format!("{pad}ORDER BY (sort)\n"));
            }
        }
        if self.limit.is_some() {
            out.push_str(&format!("{pad}LIMIT\n"));
        }
        if let Some((op, p)) = &self.compound {
            out.push_str(&format!("{pad}{op:?}\n"));
            p.explain_into(out, depth);
        }
    }
}

fn note_expr_columns(e: &PExpr, out: &mut HashMap<usize, usize>) {
    e.walk(&mut |x| {
        if let PExpr::Column { slot, col, .. } = x {
            let entry = out.entry(*slot).or_insert(0);
            *entry = (*entry).max(col + 1);
        }
    });
    // `walk` does not descend into nested plans; do that here.
    match e {
        PExpr::InSelect { plan, .. } | PExpr::Exists { plan, .. } | PExpr::Subquery(plan) => {
            plan.note_columns(out)
        }
        PExpr::Binary { lhs, rhs, .. } => {
            note_expr_columns(lhs, out);
            note_expr_columns(rhs, out);
        }
        PExpr::Unary { expr, .. }
        | PExpr::IsNull { expr, .. }
        | PExpr::Cast { expr, .. }
        | PExpr::Collate { expr, .. } => note_expr_columns(expr, out),
        PExpr::Between {
            expr, low, high, ..
        } => {
            note_expr_columns(expr, out);
            note_expr_columns(low, out);
            note_expr_columns(high, out);
        }
        PExpr::InList { expr, list, .. } => {
            note_expr_columns(expr, out);
            for i in list {
                note_expr_columns(i, out);
            }
        }
        PExpr::Func { args, .. } => {
            for a in args {
                note_expr_columns(a, out);
            }
        }
        PExpr::Case {
            operand,
            whens,
            else_result,
        } => {
            if let Some(o) = operand {
                note_expr_columns(o, out);
            }
            for (w, t) in whens {
                note_expr_columns(w, out);
                note_expr_columns(t, out);
            }
            if let Some(x) = else_result {
                note_expr_columns(x, out);
            }
        }
        PExpr::Like {
            lhs, rhs, escape, ..
        } => {
            note_expr_columns(lhs, out);
            note_expr_columns(rhs, out);
            if let Some(x) = escape {
                note_expr_columns(x, out);
            }
        }
        _ => {}
    }
}

impl Access {
    fn note_columns(&self, out: &mut HashMap<usize, usize>) {
        match self {
            Access::Scan => {}
            Access::RowidEq(e) => note_expr_columns(e, out),
            Access::RowidRange { low, high } => {
                if let Some((e, _)) = low {
                    note_expr_columns(e, out);
                }
                if let Some((e, _)) = high {
                    note_expr_columns(e, out);
                }
            }
            Access::Index { eq, low, high, .. } => {
                for e in eq {
                    note_expr_columns(e, out);
                }
                if let Some((e, _)) = low {
                    note_expr_columns(e, out);
                }
                if let Some((e, _)) = high {
                    note_expr_columns(e, out);
                }
            }
        }
    }

    fn slots(&self, out: &mut Vec<usize>) {
        match self {
            Access::Scan => {}
            Access::RowidEq(e) => e.slots(out),
            Access::RowidRange { low, high } => {
                if let Some((e, _)) = low {
                    e.slots(out);
                }
                if let Some((e, _)) = high {
                    e.slots(out);
                }
            }
            Access::Index { eq, low, high, .. } => {
                for e in eq {
                    e.slots(out);
                }
                if let Some((e, _)) = low {
                    e.slots(out);
                }
                if let Some((e, _)) = high {
                    e.slots(out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------------

struct Scope {
    name: String,
    slot: usize,
    table: Option<TableInfo>,
    columns: Vec<String>,
}

pub struct Planner<'a> {
    schema: &'a Schema,
    scopes: Vec<Scope>,
    next_slot: usize,
    /// Anonymous `?` parameters are numbered in the order they are planned,
    /// which is the order they appear in the statement text.
    next_param: usize,
    named_params: HashMap<String, usize>,
    /// Result-column aliases, visible to HAVING and ORDER BY the way SQLite
    /// resolves them once the projection is known.
    aliases: Vec<(String, PExpr)>,
    pub max_param: usize,
}

impl<'a> Planner<'a> {
    pub fn new(schema: &'a Schema) -> Planner<'a> {
        Planner {
            schema,
            scopes: Vec::new(),
            next_slot: 0,
            next_param: 0,
            named_params: HashMap::new(),
            aliases: Vec::new(),
            max_param: 0,
        }
    }

    /// Total row slots the statement needs, including subquery levels.
    pub fn slot_total(&self) -> usize {
        self.next_slot
    }

    pub fn plan_select(&mut self, stmt: &SelectStmt) -> Result<Plan> {
        let slot_base = self.next_slot;
        let scope_base = self.scopes.len();
        let mut levels = Vec::new();

        // ---- FROM ----------------------------------------------------------
        let mut items: Vec<(TableRef, bool, JoinConstraint)> = Vec::new();
        if let Some(from) = &stmt.from {
            items.push((from.base.clone(), false, JoinConstraint::None));
            for join in &from.joins {
                let outer = join.kind == JoinKind::Left;
                items.push((join.table.clone(), outer, join.constraint.clone()));
            }
        }
        for (item, outer, _constraint) in &items {
            let slot = self.next_slot;
            self.next_slot += 1;
            match item {
                TableRef::Named { name, alias } => {
                    let table = self
                        .schema
                        .table(name)
                        .ok_or_else(|| Error::sql(format!("no such table: {name}")))?;
                    if let Some(why) = &table.unsupported {
                        return Err(Error::sql(format!("table {name} is unsupported: {why}")));
                    }
                    let binding = alias.clone().unwrap_or_else(|| table.name.clone());
                    let columns = table.columns.iter().map(|c| c.name.clone()).collect();
                    self.scopes.push(Scope {
                        name: binding.clone(),
                        slot,
                        table: Some(table.clone()),
                        columns,
                    });
                    levels.push(PlanLevel {
                        name: binding,
                        slot,
                        source: Source::Table {
                            root: table.root_page,
                            access: Access::Scan,
                        },
                        filter: None,
                        outer: *outer,
                        column_names: table.columns.iter().map(|c| c.name.clone()).collect(),
                        table: Some(table.clone()),
                        needed_columns: 0,
                    });
                }
                TableRef::Subquery { select, alias } => {
                    let sub = self.plan_select(select)?;
                    if sub.is_correlated() {
                        return Err(Error::sql(
                            "correlated subquery in FROM is not supported",
                        ));
                    }
                    let binding = alias.clone().unwrap_or_else(|| format!("subquery{slot}"));
                    let columns = sub.column_names();
                    self.scopes.push(Scope {
                        name: binding.clone(),
                        slot,
                        table: None,
                        columns: columns.clone(),
                    });
                    levels.push(PlanLevel {
                        name: binding,
                        slot,
                        source: Source::Subquery(Box::new(sub)),
                        filter: None,
                        outer: *outer,
                        column_names: columns,
                        table: None,
                        needed_columns: 0,
                    });
                }
            }
        }

        // ---- WHERE and join constraints -----------------------------------
        // Slot numbers do not follow level positions once a FROM item is a
        // subquery (its own levels consume slots too), so terms are placed by
        // this map rather than by arithmetic.
        let level_of_slot: HashMap<usize, usize> = levels
            .iter()
            .enumerate()
            .map(|(i, l)| (l.slot, i))
            .collect();

        // (level the term belongs to, term, came from WHERE)
        let mut conjuncts: Vec<(usize, PExpr)> = Vec::new();
        #[allow(unused_mut)]
        let mut from_where: Vec<bool> = Vec::new();
        if let Some(w) = &stmt.where_clause {
            for term in split_and(w) {
                let e = self.resolve(&term)?;
                let lvl = term_level(&e, &level_of_slot);
                conjuncts.push((lvl, e));
                from_where.push(true);
            }
        }
        for (i, (item, _outer, constraint)) in items.iter().enumerate() {
            match constraint {
                JoinConstraint::On(expr) => {
                    for term in split_and(expr) {
                        let e = self.resolve(&term)?;
                        let lvl = term_level(&e, &level_of_slot).max(i);
                        conjuncts.push((lvl, e));
                        from_where.push(false);
                    }
                }
                JoinConstraint::Using(cols) => {
                    for col in cols {
                        // `USING (c)` means left.c = right.c for the nearest
                        // preceding item that has the column.
                        let right = self.resolve(&Expr::Column {
                            table: Some(
                                item.binding()
                                    .ok_or_else(|| Error::sql("USING needs a named table"))?
                                    .to_string(),
                            ),
                            name: col.clone(),
                        })?;
                        let mut left = None;
                        for prev in items[..i].iter().rev() {
                            let Some(b) = prev.0.binding() else { continue };
                            if let Ok(e) = self.resolve(&Expr::Column {
                                table: Some(b.to_string()),
                                name: col.clone(),
                            }) {
                                left = Some(e);
                                break;
                            }
                        }
                        let left = left.ok_or_else(|| {
                            Error::sql(format!("USING column {col} is not on the left side"))
                        })?;
                        let coll = comparison_collation(&left, &right);
                        conjuncts.push((
                            i,
                            PExpr::Binary {
                                op: BinOp::Eq,
                                lhs: Box::new(left),
                                rhs: Box::new(right),
                                coll,
                            },
                        ));
                        from_where.push(false);
                    }
                }
                JoinConstraint::None => {}
            }
        }

        // ---- push predicates into derived tables ---------------------------
        // A term that constrains only a derived table is also applied inside
        // it, so its rows are discarded as they are scanned instead of being
        // materialized and thrown away. Without this a search over a posting
        // list buffers every posting in the database before the outer WHERE
        // ever sees a row.
        //
        // The outer copy of the term is deliberately kept. That is what makes
        // this always safe: the inner copy can only discard rows the outer one
        // would discard anyway, so it narrows the scan without being able to
        // change the answer.
        //
        // Except on the optional side of a LEFT JOIN, where discarding a row is
        // not the same as losing it: the join NULL-extends instead, and a term
        // like `c.tag IS NULL` then matches the very rows it was meant to
        // reject. Those items are left alone.
        for level in levels.iter_mut() {
            if level.outer {
                continue;
            }
            let Source::Subquery(inner) = &mut level.source else {
                continue;
            };
            for (_, term) in conjuncts.iter() {
                if constrains_only(term, level.slot) && is_safe_to_push(term) {
                    push_into_derived(inner, level.slot, term);
                }
            }
        }

        // ---- join order ----------------------------------------------------
        // Nested loops are cheapest when the most constrained table runs
        // outermost. Reordering is only safe when no LEFT JOIN is involved,
        // since an outer join fixes which side may produce NULL rows.
        if levels.len() > 1 && levels.iter().all(|l| !l.outer) {
            let order = greedy_join_order(&levels, &conjuncts, slot_base);
            if order.iter().enumerate().any(|(i, o)| i != *o) {
                let mut reordered: Vec<PlanLevel> = Vec::with_capacity(levels.len());
                for &idx in &order {
                    reordered.push(levels[idx].clone());
                }
                levels = reordered;
                // Terms are keyed by level position, so re-derive them.
                let position: HashMap<usize, usize> = levels
                    .iter()
                    .enumerate()
                    .map(|(pos, l)| (l.slot, pos))
                    .collect();
                for (lvl, expr) in conjuncts.iter_mut() {
                    *lvl = term_level(expr, &position);
                }
            }
        }

        // ---- access paths --------------------------------------------------
        // Terms usable as seek keys are consumed here; the rest stay filters.
        let mut used: Vec<bool> = vec![false; conjuncts.len()];
        let mut post: Vec<PExpr> = Vec::new();
        for (j, (lvl, expr)) in conjuncts.iter().enumerate() {
            let outer_level = levels
                .get(*lvl)
                .map(|l| l.outer)
                .unwrap_or(false);
            if from_where[j] && outer_level {
                post.push(expr.clone());
                used[j] = true;
            }
        }
        let post_filter = combine_and(post);
        let earlier_slots: Vec<usize> = levels.iter().map(|l| l.slot).collect();
        for (i, level) in levels.iter_mut().enumerate() {
            let Source::Table { root: _, access } = &mut level.source else {
                continue;
            };
            let Some(table) = &level.table else { continue };
            let chosen = choose_access(
                table,
                level.slot,
                &conjuncts,
                &mut used,
                i,
                &earlier_slots[..i],
                slot_base,
            );
            *access = chosen;
        }
        for (i, level) in levels.iter_mut().enumerate() {
            let mut terms: Vec<PExpr> = Vec::new();
            for (j, (lvl, expr)) in conjuncts.iter().enumerate() {
                if used[j] || *lvl != i {
                    continue;
                }
                terms.push(expr.clone());
            }
            level.filter = combine_and(terms);
        }
        // Terms that only reference outer queries (or nothing) belong to the
        // first level so they are evaluated once per candidate row.
        if !levels.is_empty() {
            let mut extra: Vec<PExpr> = Vec::new();
            for (j, (lvl, expr)) in conjuncts.iter().enumerate() {
                if used[j] {
                    continue;
                }
                if *lvl >= levels.len() {
                    extra.push(expr.clone());
                }
            }
            if !extra.is_empty() {
                let mut terms = extra;
                if let Some(f) = levels[0].filter.take() {
                    terms.push(f);
                }
                levels[0].filter = combine_and(terms);
            }
        } else if !conjuncts.is_empty() {
            // No FROM: the WHERE clause is a constant predicate.
            let terms: Vec<PExpr> = conjuncts.iter().map(|(_, e)| e.clone()).collect();
            let filter = combine_and(terms);
            levels.push(PlanLevel {
                name: "(constant)".into(),
                slot: self.next_slot,
                source: Source::Table {
                    root: 0,
                    access: Access::Scan,
                },
                filter,
                outer: false,
                column_names: Vec::new(),
                table: None,
                needed_columns: 0,
            });
            self.next_slot += 1;
        }

        // ---- projection, grouping, ordering --------------------------------
        let mut aggregates: Vec<AggSpec> = Vec::new();
        let mut result = Vec::new();
        for rc in &stmt.columns {
            match rc {
                ResultColumn::Star => {
                    for scope in &self.scopes[scope_base..] {
                        for (ci, cname) in scope.columns.iter().enumerate() {
                            result.push(ResultCol {
                                name: cname.clone(),
                                expr: self.column_ref(scope.slot, ci, scope.table.as_ref()),
                            });
                        }
                    }
                }
                ResultColumn::TableStar(t) => {
                    let scope = self.scopes[scope_base..]
                        .iter()
                        .find(|s| s.name.eq_ignore_ascii_case(t))
                        .ok_or_else(|| Error::sql(format!("no such table: {t}")))?;
                    let (slot, table) = (scope.slot, scope.table.clone());
                    let names = scope.columns.clone();
                    for (ci, cname) in names.iter().enumerate() {
                        result.push(ResultCol {
                            name: cname.clone(),
                            expr: self.column_ref(slot, ci, table.as_ref()),
                        });
                    }
                }
                ResultColumn::Expr { expr, alias } => {
                    let e = self.resolve_agg(expr, &mut aggregates)?;
                    let name = alias.clone().unwrap_or_else(|| display_name(expr));
                    result.push(ResultCol { name, expr: e });
                }
            }
        }

        let mut group_by = Vec::new();
        for g in &stmt.group_by {
            group_by.push(self.resolve(g)?);
        }
        let saved_aliases = std::mem::replace(
            &mut self.aliases,
            result
                .iter()
                .map(|c| (c.name.clone(), c.expr.clone()))
                .collect(),
        );
        let having = match &stmt.having {
            Some(h) => Some(self.resolve_agg(h, &mut aggregates)?),
            None => None,
        };
        let mut order_by = Vec::new();
        for term in &stmt.order_by {
            // ORDER BY 1 refers to the first result column.
            let e = match &term.expr {
                Expr::Literal(Value::Integer(n)) if *n >= 1 => {
                    let idx = (*n - 1) as usize;
                    result
                        .get(idx)
                        .ok_or_else(|| Error::sql("ORDER BY index out of range"))?
                        .expr
                        .clone()
                }
                other => {
                    // An alias from the result list may be used here.
                    match alias_lookup(&result, other) {
                        Some(e) => e,
                        None => self.resolve_agg(other, &mut aggregates)?,
                    }
                }
            };
            let nulls_first = term.nulls_first.unwrap_or(!term.desc);
            order_by.push((e, term.desc, nulls_first));
        }
        self.aliases = saved_aliases;
        let limit = match &stmt.limit {
            Some(e) => Some(self.resolve(e)?),
            None => None,
        };
        let offset = match &stmt.offset {
            Some(e) => Some(self.resolve(e)?),
            None => None,
        };

        let compound = match &stmt.compound {
            Some(c) => {
                let sub = self.plan_select(&c.select)?;
                Some((c.op, Box::new(sub)))
            }
            None => None,
        };
        // Over a compound select, ORDER BY may only name a result column
        // (by position or by name), because the rows come from both sides.
        let order_by_positions = if compound.is_some() && !stmt.order_by.is_empty() {
            let mut positions = Vec::new();
            for term in &stmt.order_by {
                let idx = match &term.expr {
                    Expr::Literal(Value::Integer(n)) if *n >= 1 => (*n - 1) as usize,
                    Expr::Column { table: None, name } => result
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| {
                            Error::sql(format!(
                                "ORDER BY {name} does not name a result column of the compound select"
                            ))
                        })?,
                    _ => {
                        return Err(Error::unsupported(
                            "ORDER BY expression over a compound select",
                        ))
                    }
                };
                if idx >= result.len() {
                    return Err(Error::sql("ORDER BY index out of range"));
                }
                positions.push((idx, term.desc, term.nulls_first.unwrap_or(!term.desc)));
            }
            Some(positions)
        } else {
            None
        };

        let slot_count = self.next_slot - slot_base;
        let mut plan = Plan {
            levels,
            post_filter,
            result,
            distinct: stmt.distinct,
            group_by,
            having,
            order_by,
            order_by_positions,
            limit,
            offset,
            aggregates,
            ordered_by_access: false,
            slot_base,
            slot_count,
            compound,
        };
        plan.ordered_by_access = order_is_satisfied(&plan);
        // Only decode the columns this statement reads.
        let usage = plan.column_usage();
        for level in &mut plan.levels {
            let width = level
                .table
                .as_ref()
                .map(|t| t.columns.len())
                .unwrap_or(level.column_names.len());
            level.needed_columns = usage.get(&level.slot).copied().unwrap_or(0).min(width);
        }
        self.scopes.truncate(scope_base);
        Ok(plan)
    }

    fn column_ref(&self, slot: usize, col: usize, table: Option<&TableInfo>) -> PExpr {
        match table {
            Some(t) => {
                if t.rowid_alias == Some(col) {
                    PExpr::Rowid { slot }
                } else {
                    PExpr::Column {
                        slot,
                        col,
                        affinity: t.columns[col].affinity,
                        coll: t.columns[col].collation,
                    }
                }
            }
            None => PExpr::Column {
                slot,
                col,
                affinity: Affinity::Blob,
                coll: Collation::Binary,
            },
        }
    }

    fn param_index(&mut self, p: &ParamRef) -> usize {
        let idx = match p {
            ParamRef::Next => {
                self.next_param += 1;
                self.next_param - 1
            }
            ParamRef::Index(n) => {
                let i = (*n).max(1) as usize - 1;
                self.next_param = self.next_param.max(i + 1);
                i
            }
            ParamRef::Name(name) => {
                if let Some(i) = self.named_params.get(name) {
                    *i
                } else {
                    let i = self.next_param;
                    self.next_param += 1;
                    self.named_params.insert(name.clone(), i);
                    i
                }
            }
        };
        self.max_param = self.max_param.max(idx + 1);
        idx
    }

    fn resolve(&mut self, e: &Expr) -> Result<PExpr> {
        let mut aggs = Vec::new();
        let out = self.resolve_agg(e, &mut aggs)?;
        if !aggs.is_empty() {
            return Err(Error::sql("aggregate function is not allowed here"));
        }
        Ok(out)
    }

    fn resolve_agg(&mut self, e: &Expr, aggs: &mut Vec<AggSpec>) -> Result<PExpr> {
        Ok(match e {
            Expr::Literal(v) => PExpr::Literal(v.clone()),
            Expr::Param(p) => PExpr::Param(self.param_index(p)),
            Expr::Column { table, name } => self.resolve_column(table.as_deref(), name)?,
            Expr::Unary { op, expr } => PExpr::Unary {
                op: *op,
                expr: Box::new(self.resolve_agg(expr, aggs)?),
            },
            Expr::Binary { op, lhs, rhs } => {
                let l = self.resolve_agg(lhs, aggs)?;
                let r = self.resolve_agg(rhs, aggs)?;
                let coll = comparison_collation(&l, &r);
                PExpr::Binary {
                    op: *op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                    coll,
                }
            }
            Expr::IsNull { expr, negated } => PExpr::IsNull {
                expr: Box::new(self.resolve_agg(expr, aggs)?),
                negated: *negated,
            },
            Expr::Between {
                expr,
                low,
                high,
                negated,
            } => {
                let e = self.resolve_agg(expr, aggs)?;
                let l = self.resolve_agg(low, aggs)?;
                let h = self.resolve_agg(high, aggs)?;
                let coll = comparison_collation(&e, &l);
                PExpr::Between {
                    expr: Box::new(e),
                    low: Box::new(l),
                    high: Box::new(h),
                    negated: *negated,
                    coll,
                }
            }
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let e = self.resolve_agg(expr, aggs)?;
                let mut items = Vec::new();
                for item in list {
                    items.push(self.resolve_agg(item, aggs)?);
                }
                let coll = e.collation().unwrap_or(Collation::Binary);
                PExpr::InList {
                    expr: Box::new(e),
                    list: items,
                    negated: *negated,
                    coll,
                }
            }
            Expr::InSelect {
                expr,
                select,
                negated,
            } => {
                let e = self.resolve_agg(expr, aggs)?;
                let plan = self.plan_select(select)?;
                let coll = e.collation().unwrap_or(Collation::Binary);
                PExpr::InSelect {
                    expr: Box::new(e),
                    plan: Box::new(plan),
                    negated: *negated,
                    coll,
                }
            }
            Expr::Exists { select, negated } => PExpr::Exists {
                plan: Box::new(self.plan_select(select)?),
                negated: *negated,
            },
            Expr::Subquery(select) => PExpr::Subquery(Box::new(self.plan_select(select)?)),
            Expr::Function {
                name,
                args,
                distinct,
                star,
            } => {
                if let Some(f) = AggFunc::from_name(name) {
                    let func = if *star && f == AggFunc::Count {
                        AggFunc::CountStar
                    } else {
                        f
                    };
                    // min()/max() with two or more arguments are scalar.
                    if matches!(f, AggFunc::Min | AggFunc::Max) && args.len() > 1 {
                        let mut pargs = Vec::new();
                        for a in args {
                            pargs.push(self.resolve_agg(a, aggs)?);
                        }
                        return Ok(PExpr::Func {
                            func: if f == AggFunc::Min {
                                ScalarFunc::Min
                            } else {
                                ScalarFunc::Max
                            },
                            args: pargs,
                        });
                    }
                    let mut pargs = Vec::new();
                    for a in args {
                        pargs.push(self.resolve_agg(a, aggs)?);
                    }
                    if pargs.iter().any(|a| a.contains_agg()) {
                        return Err(Error::sql("nested aggregate functions"));
                    }
                    aggs.push(AggSpec {
                        func,
                        args: pargs,
                        distinct: *distinct,
                    });
                    return Ok(PExpr::Agg(aggs.len() - 1));
                }
                let func = ScalarFunc::from_name(name)
                    .ok_or_else(|| Error::sql(format!("no such function: {name}")))?;
                let mut pargs = Vec::new();
                for a in args {
                    pargs.push(self.resolve_agg(a, aggs)?);
                }
                PExpr::Func { func, args: pargs }
            }
            Expr::Case {
                operand,
                whens,
                else_result,
            } => {
                let op = match operand {
                    Some(o) => Some(Box::new(self.resolve_agg(o, aggs)?)),
                    None => None,
                };
                let mut w = Vec::new();
                for (cond, res) in whens {
                    w.push((self.resolve_agg(cond, aggs)?, self.resolve_agg(res, aggs)?));
                }
                let e = match else_result {
                    Some(x) => Some(Box::new(self.resolve_agg(x, aggs)?)),
                    None => None,
                };
                PExpr::Case {
                    operand: op,
                    whens: w,
                    else_result: e,
                }
            }
            Expr::Cast { expr, type_name } => PExpr::Cast {
                expr: Box::new(self.resolve_agg(expr, aggs)?),
                affinity: affinity_of(type_name),
            },
            Expr::Collate { expr, collation } => {
                let coll = Collation::from_name(collation)
                    .ok_or_else(|| Error::unsupported(format!("collation {collation}")))?;
                PExpr::Collate {
                    expr: Box::new(self.resolve_agg(expr, aggs)?),
                    coll,
                }
            }
            Expr::Like {
                lhs,
                rhs,
                escape,
                negated,
                glob,
            } => {
                let l = self.resolve_agg(lhs, aggs)?;
                let r = self.resolve_agg(rhs, aggs)?;
                let esc = match escape {
                    Some(e) => Some(Box::new(self.resolve_agg(e, aggs)?)),
                    None => None,
                };
                PExpr::Like {
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                    escape: esc,
                    negated: *negated,
                    glob: *glob,
                }
            }
        })
    }

    fn resolve_column(&mut self, table: Option<&str>, name: &str) -> Result<PExpr> {
        let mut found: Option<PExpr> = None;
        for scope in self.scopes.iter().rev() {
            if let Some(t) = table {
                if !scope.name.eq_ignore_ascii_case(t) {
                    continue;
                }
            }
            if name.eq_ignore_ascii_case("rowid")
                || name.eq_ignore_ascii_case("_rowid_")
                || name.eq_ignore_ascii_case("oid")
            {
                if let Some(info) = &scope.table {
                    if info.column_index(name).is_none() {
                        return Ok(PExpr::Rowid { slot: scope.slot });
                    }
                }
            }
            let Some(ci) = scope
                .columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
            else {
                continue;
            };
            let e = match &scope.table {
                Some(t) => {
                    if t.rowid_alias == Some(ci) {
                        PExpr::Rowid { slot: scope.slot }
                    } else {
                        PExpr::Column {
                            slot: scope.slot,
                            col: ci,
                            affinity: t.columns[ci].affinity,
                            coll: t.columns[ci].collation,
                        }
                    }
                }
                None => PExpr::Column {
                    slot: scope.slot,
                    col: ci,
                    affinity: Affinity::Blob,
                    coll: Collation::Binary,
                },
            };
            if found.is_some() && table.is_none() {
                // Ambiguity only matters within the innermost scope group; the
                // first match walking outward wins, like SQLite.
                break;
            }
            found = Some(e);
            break;
        }
        if found.is_none() && table.is_none() {
            // A result-column alias, when nothing in scope matches.
            if let Some((_, e)) = self
                .aliases
                .iter()
                .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
            {
                return Ok(e.clone());
            }
        }
        found.ok_or_else(|| match table {
            Some(t) => Error::sql(format!("no such column: {t}.{name}")),
            None => Error::sql(format!("no such column: {name}")),
        })
    }
}

/// The last level a term can be evaluated at: the highest position among the
/// levels it reads. `usize::MAX` when it reads none of them (a constant, or a
/// correlated reference to an enclosing query).
fn term_level(e: &PExpr, level_of_slot: &HashMap<usize, usize>) -> usize {
    let mut slots = Vec::new();
    e.slots(&mut slots);
    let mut max = None;
    for s in slots {
        if let Some(pos) = level_of_slot.get(&s) {
            max = Some(max.map_or(*pos, |m: usize| m.max(*pos)));
        }
    }
    max.unwrap_or(usize::MAX)
}

// ---------------------------------------------------------------------------
// Pushing predicates into derived tables
// ---------------------------------------------------------------------------

/// True when `term` reads at least one column and every column it reads comes
/// from `slot`, so it constrains that FROM item and nothing else.
fn constrains_only(term: &PExpr, slot: usize) -> bool {
    let mut slots = Vec::new();
    term.slots(&mut slots);
    !slots.is_empty() && slots.iter().all(|s| *s == slot)
}

/// Whether a term may also be applied inside the derived table it constrains.
///
/// The rule this has to satisfy is one-sided: the inner copy must never reject
/// a row the outer copy accepts. Rejecting *more* is fine, because the outer
/// copy is kept and has the final say.
///
/// That is not automatic. A derived table's column carries no affinity or
/// collation of its own, so the outer copy compares raw values, while the inner
/// copy re-introduces the base column's. For equality that can only match more:
/// applying a column's affinity to a value that already compares equal to it is
/// a no-op, and every collation the engine has treats identical text as equal.
/// Inequalities are excluded for exactly this reason — TEXT affinity turns a
/// true `n > 5` into a false `n > '5'`, dropping rows the outer term keeps —
/// and so is anything negated, which flips the direction.
fn is_safe_to_push(e: &PExpr) -> bool {
    match e {
        PExpr::Binary {
            op: BinOp::Eq | BinOp::Is,
            lhs,
            rhs,
            ..
        } => is_plain_operand(lhs) && is_plain_operand(rhs),
        PExpr::Binary {
            op: BinOp::And | BinOp::Or,
            lhs,
            rhs,
            ..
        } => is_safe_to_push(lhs) && is_safe_to_push(rhs),
        PExpr::InList {
            expr,
            list,
            negated: false,
            ..
        } => is_plain_operand(expr) && list.iter().all(is_plain_operand),
        // A null test reads the stored value without converting it, so it means
        // the same on either side of the derived table.
        PExpr::IsNull { expr, .. } => is_plain_operand(expr),
        _ => false,
    }
}

/// An operand with no aggregate and no nested query plan, so it can be copied
/// into another plan and evaluated there unchanged.
fn is_plain_operand(e: &PExpr) -> bool {
    let mut ok = true;
    e.walk(&mut |x| {
        if matches!(
            x,
            PExpr::Agg(_) | PExpr::InSelect { .. } | PExpr::Exists { .. } | PExpr::Subquery(_)
        ) {
            ok = false;
        }
    });
    ok
}

/// Apply `term` inside the plan of the derived table occupying `slot`.
///
/// References to the materialized row are rewritten into the expressions the
/// plan produces, so `c.asset_id` becomes whatever `c`'s select list computes
/// for that column.
fn push_into_derived(plan: &mut Plan, slot: usize, term: &PExpr) {
    // The arms of a UNION are independent, and narrowing one narrows the whole.
    // INTERSECT and EXCEPT are not: dropping rows from the right of an EXCEPT
    // *adds* rows to the result, so only the left arm is pushed into. A LIMIT
    // or OFFSET belongs to the whole compound and is checked before any arm.
    if plan.limit.is_some() || plan.offset.is_some() {
        // Filtering ahead of a LIMIT picks a different set of rows to keep.
        return;
    }
    if let Some((op, right)) = &mut plan.compound {
        if matches!(op, CompoundOp::Union | CompoundOp::UnionAll) {
            push_into_derived(right, slot, term);
        }
    }
    if !plan.group_by.is_empty() || !plan.aggregates.is_empty() || plan.levels.is_empty() {
        // Filtering ahead of an aggregate changes the aggregate, which is a
        // different answer rather than a narrower scan.
        return;
    }
    let Some(mapped) = substitute(term, slot, &plan.result) else {
        return;
    };

    // Attach the term to the level that reads it, so the scan discards rows as
    // it goes. When it spans levels, or lands on the optional side of a LEFT
    // JOIN — where a level filter would turn a dropped row into a NULL-extended
    // one instead of removing it — fall back to the plan's post filter, which
    // still runs before any row is buffered.
    let level_of_slot: HashMap<usize, usize> = plan
        .levels
        .iter()
        .enumerate()
        .map(|(i, l)| (l.slot, i))
        .collect();
    let mut slots = Vec::new();
    mapped.slots(&mut slots);
    let mut positions: Vec<usize> = slots
        .iter()
        .filter_map(|s| level_of_slot.get(s).copied())
        .collect();
    positions.sort_unstable();
    positions.dedup();
    let target = match positions.as_slice() {
        [only] if !plan.levels[*only].outer => Some(*only),
        _ => None,
    };
    let slot_holder = match target {
        Some(i) => &mut plan.levels[i].filter,
        None => &mut plan.post_filter,
    };
    let mut terms: Vec<PExpr> = slot_holder.take().into_iter().collect();
    terms.push(mapped);
    *slot_holder = combine_and(terms);
}

/// Rewrite every reference to the derived table's materialized row into the
/// expression its plan computes for that column. `None` when some part of the
/// term has no meaning inside the plan, which abandons the push.
fn substitute(e: &PExpr, slot: usize, result: &[ResultCol]) -> Option<PExpr> {
    let sub = |x: &PExpr| substitute(x, slot, result);
    let boxed = |x: &PExpr| sub(x).map(Box::new);
    Some(match e {
        PExpr::Column { slot: s, col, .. } => {
            if *s != slot {
                return None; // a level of some other query; not ours to move
            }
            return Some(result.get(*col)?.expr.clone());
        }
        // A derived table has no rowid to speak of.
        PExpr::Rowid { .. } => return None,
        PExpr::Literal(v) => PExpr::Literal(v.clone()),
        PExpr::Param(i) => PExpr::Param(*i),
        PExpr::Unary { op, expr } => PExpr::Unary {
            op: *op,
            expr: boxed(expr)?,
        },
        PExpr::Binary { op, lhs, rhs, coll } => PExpr::Binary {
            op: *op,
            lhs: boxed(lhs)?,
            rhs: boxed(rhs)?,
            coll: *coll,
        },
        PExpr::IsNull { expr, negated } => PExpr::IsNull {
            expr: boxed(expr)?,
            negated: *negated,
        },
        PExpr::Between {
            expr,
            low,
            high,
            negated,
            coll,
        } => PExpr::Between {
            expr: boxed(expr)?,
            low: boxed(low)?,
            high: boxed(high)?,
            negated: *negated,
            coll: *coll,
        },
        PExpr::InList {
            expr,
            list,
            negated,
            coll,
        } => PExpr::InList {
            expr: boxed(expr)?,
            list: list.iter().map(sub).collect::<Option<Vec<_>>>()?,
            negated: *negated,
            coll: *coll,
        },
        PExpr::Func { func, args } => PExpr::Func {
            func: *func,
            args: args.iter().map(sub).collect::<Option<Vec<_>>>()?,
        },
        PExpr::Case {
            operand,
            whens,
            else_result,
        } => PExpr::Case {
            operand: match operand {
                Some(o) => Some(boxed(o)?),
                None => None,
            },
            whens: whens
                .iter()
                .map(|(w, t)| Some((sub(w)?, sub(t)?)))
                .collect::<Option<Vec<_>>>()?,
            else_result: match else_result {
                Some(x) => Some(boxed(x)?),
                None => None,
            },
        },
        PExpr::Cast { expr, affinity } => PExpr::Cast {
            expr: boxed(expr)?,
            affinity: *affinity,
        },
        PExpr::Collate { expr, coll } => PExpr::Collate {
            expr: boxed(expr)?,
            coll: *coll,
        },
        PExpr::Like {
            lhs,
            rhs,
            escape,
            negated,
            glob,
        } => PExpr::Like {
            lhs: boxed(lhs)?,
            rhs: boxed(rhs)?,
            escape: match escape {
                Some(x) => Some(boxed(x)?),
                None => None,
            },
            negated: *negated,
            glob: *glob,
        },
        // Nothing with its own plan or accumulator is moved.
        PExpr::Agg(_) | PExpr::InSelect { .. } | PExpr::Exists { .. } | PExpr::Subquery(_) => {
            return None
        }
    })
}

fn alias_lookup(result: &[ResultCol], e: &Expr) -> Option<PExpr> {
    if let Expr::Column { table: None, name } = e {
        for r in result {
            if r.name.eq_ignore_ascii_case(name) {
                return Some(r.expr.clone());
            }
        }
    }
    None
}

/// The name an unaliased result column is known by.
///
/// A plain column reference is named by its column, with any table qualifier
/// dropped: `SELECT a.asset_id` yields `asset_id`, which is what SQLite does
/// under its default `short_column_names`. This is not cosmetic. A derived
/// table's columns are addressed by these names, so keeping the qualifier here
/// would name the column `a.asset_id` and make `c.asset_id` unresolvable in
/// `FROM (SELECT a.asset_id …) c`.
fn display_name(e: &Expr) -> String {
    match e {
        Expr::Column { name, .. } => name.clone(),
        Expr::Function { name, star, .. } if *star => format!("{name}(*)"),
        Expr::Function { name, args, .. } => {
            let inner: Vec<String> = args.iter().map(display_name).collect();
            format!("{name}({})", inner.join(", "))
        }
        Expr::Literal(Value::Integer(i)) => i.to_string(),
        Expr::Literal(Value::Text(t)) => format!("'{t}'"),
        _ => "expr".to_string(),
    }
}

fn split_and(e: &Expr) -> Vec<Expr> {
    match e {
        Expr::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => {
            let mut v = split_and(lhs);
            v.extend(split_and(rhs));
            v
        }
        other => vec![other.clone()],
    }
}

fn combine_and(mut terms: Vec<PExpr>) -> Option<PExpr> {
    if terms.is_empty() {
        return None;
    }
    let mut acc = terms.remove(0);
    for t in terms {
        acc = PExpr::Binary {
            op: BinOp::And,
            lhs: Box::new(acc),
            rhs: Box::new(t),
            coll: Collation::Binary,
        };
    }
    Some(acc)
}

/// Collation for a comparison, in SQLite's documented order: an explicit
/// COLLATE on either side wins (left first), otherwise a column's own
/// collation (left first), otherwise BINARY.
fn comparison_collation(l: &PExpr, r: &PExpr) -> Collation {
    l.explicit_collation()
        .or_else(|| r.explicit_collation())
        .or_else(|| l.collation())
        .or_else(|| r.collation())
        .unwrap_or(Collation::Binary)
}

// ---------------------------------------------------------------------------
// Access path selection
// ---------------------------------------------------------------------------

/// A comparison of one column of `slot` against an expression that does not
/// read this level or any later one.
struct Constraint<'a> {
    term: usize,
    col: Option<usize>,
    rowid: bool,
    op: BinOp,
    value: &'a PExpr,
    /// Collation this comparison uses; an index can only serve the constraint
    /// when its key column sorts the same way.
    coll: Collation,
}

fn constraints_for<'a>(
    slot: usize,
    level: usize,
    conjuncts: &'a [(usize, PExpr)],
    used: &[bool],
    earlier_slots: &[usize],
    slot_base: usize,
) -> Vec<Constraint<'a>> {
    let mut out = Vec::new();
    for (i, (lvl, e)) in conjuncts.iter().enumerate() {
        if used[i] || *lvl != level {
            continue;
        }
        let PExpr::Binary { op, lhs, rhs, coll } = e else {
            continue;
        };
        if !op.is_comparison() || *op == BinOp::Ne {
            continue;
        }
        // Either side may hold the column.
        let (col_side, val_side, op) = match (side_column(lhs, slot), side_column(rhs, slot)) {
            (Some(c), None) => (c, rhs.as_ref(), *op),
            (None, Some(c)) => (c, lhs.as_ref(), flip(*op)),
            _ => continue,
        };
        // The value must be computable before this level runs: it may only
        // read levels already placed in the join order, or an enclosing query.
        let mut slots = Vec::new();
        val_side.slots(&mut slots);
        if slots
            .iter()
            .any(|s| *s >= slot_base && !earlier_slots.contains(s))
        {
            continue;
        }
        out.push(Constraint {
            term: i,
            col: col_side.0,
            rowid: col_side.1,
            op,
            value: val_side,
            coll: *coll,
        });
    }
    out
}

/// (column index, is_rowid) when `e` is a plain column of `slot`.
fn side_column(e: &PExpr, slot: usize) -> Option<(Option<usize>, bool)> {
    match e {
        PExpr::Column { slot: s, col, .. } if *s == slot => Some((Some(*col), false)),
        PExpr::Rowid { slot: s } if *s == slot => Some((None, true)),
        _ => None,
    }
}

fn flip(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        other => other,
    }
}

/// Rank an access path; higher is more selective.
fn access_score(access: &Access) -> u32 {
    match access {
        Access::RowidEq(_) => 100,
        Access::Index {
            unique_eq: true, ..
        } => 90,
        Access::Index { eq, low, high, .. } => {
            40 + (eq.len() as u32) * 10 + u32::from(low.is_some() || high.is_some()) * 5
        }
        Access::RowidRange { .. } => 30,
        Access::Scan => 0,
    }
}

/// Greedy join order: repeatedly place the table whose best access path, given
/// everything already placed, is the most selective. This is the small
/// rule-based cousin of SQLite's next-best-index loop.
fn greedy_join_order(
    levels: &[PlanLevel],
    conjuncts: &[(usize, PExpr)],
    slot_base: usize,
) -> Vec<usize> {
    let n = levels.len();
    let mut placed: Vec<usize> = Vec::with_capacity(n);
    let mut remaining: Vec<usize> = (0..n).collect();
    while !remaining.is_empty() {
        let mut best: Option<(u32, usize, usize)> = None; // (score, original index, position in remaining)
        for (ri, &idx) in remaining.iter().enumerate() {
            let level = &levels[idx];
            let Some(table) = &level.table else {
                // A materialized subquery has no index; keep it late.
                let score = 1;
                if best.as_ref().map_or(true, |(s, _, _)| score > *s) {
                    best = Some((score, idx, ri));
                }
                continue;
            };
            // Terms usable at this position: those whose other side comes from
            // an already-placed level (or from a constant).
            let placed_slots: Vec<usize> = placed.iter().map(|&p| levels[p].slot).collect();
            let usable: Vec<(usize, PExpr)> = conjuncts
                .iter()
                .filter(|(_, e)| {
                    let mut slots = Vec::new();
                    e.slots(&mut slots);
                    slots
                        .iter()
                        .all(|s| *s == level.slot || placed_slots.contains(s))
                })
                .map(|(_, e)| (placed.len(), e.clone()))
                .collect();
            let mut used = vec![false; usable.len()];
            let access = choose_access(
                table,
                level.slot,
                &usable,
                &mut used,
                placed.len(),
                &placed_slots,
                slot_base,
            );
            let score = access_score(&access);
            if best
                .as_ref()
                .map_or(true, |(s, bi, _)| score > *s || (score == *s && idx < *bi))
            {
                best = Some((score, idx, ri));
            }
        }
        let (_, idx, ri) = best.expect("a level to place");
        placed.push(idx);
        remaining.remove(ri);
    }
    placed
}

fn choose_access(
    table: &TableInfo,
    slot: usize,
    conjuncts: &[(usize, PExpr)],
    used: &mut [bool],
    level: usize,
    earlier_slots: &[usize],
    slot_base: usize,
) -> Access {
    let cons = constraints_for(slot, level, conjuncts, used, earlier_slots, slot_base);

    // 1. rowid equality: one row, no index needed.
    if let Some(c) = cons.iter().find(|c| c.rowid && c.op == BinOp::Eq) {
        used[c.term] = true;
        return Access::RowidEq(c.value.clone());
    }

    // 2. the index matching the most equality columns, then a range.
    let mut best: Option<(usize, Access, Vec<usize>)> = None;
    for index in &table.indexes {
        if index.partial || index.root_page == 0 {
            continue;
        }
        if index.columns.iter().any(|c| c.desc) {
            continue; // descending keys need a reverse cursor
        }
        let mut eq: Vec<PExpr> = Vec::new();
        let mut terms: Vec<usize> = Vec::new();
        for ic in &index.columns {
            let Some(c) = cons.iter().find(|c| {
                c.col == Some(ic.column)
                    && c.op == BinOp::Eq
                    && !terms.contains(&c.term)
                    && c.coll == ic.collation
                    && collation_ok(table, ic.column, ic.collation)
            }) else {
                break;
            };
            eq.push(c.value.clone());
            terms.push(c.term);
        }
        // A range on the column right after the equality prefix.
        let mut low = None;
        let mut high = None;
        if eq.len() < index.columns.len() {
            let ic = &index.columns[eq.len()];
            if collation_ok(table, ic.column, ic.collation) {
                for c in cons.iter() {
                    if c.col != Some(ic.column)
                        || terms.contains(&c.term)
                        || c.coll != ic.collation
                    {
                        continue;
                    }
                    match c.op {
                        BinOp::Gt => {
                            low = Some((c.value.clone(), false));
                            terms.push(c.term);
                        }
                        BinOp::Ge => {
                            low = Some((c.value.clone(), true));
                            terms.push(c.term);
                        }
                        BinOp::Lt => {
                            high = Some((c.value.clone(), false));
                            terms.push(c.term);
                        }
                        BinOp::Le => {
                            high = Some((c.value.clone(), true));
                            terms.push(c.term);
                        }
                        _ => {}
                    }
                }
            }
        }
        if eq.is_empty() && low.is_none() && high.is_none() {
            continue;
        }
        let unique_eq = index.unique && eq.len() == index.columns.len();
        let score = eq.len() * 2 + usize::from(low.is_some() || high.is_some());
        let access = Access::Index {
            name: index.name.clone(),
            root: index.root_page,
            columns: index
                .columns
                .iter()
                .map(|ic| (ic.column, ic.collation))
                .collect(),
            eq,
            low,
            high,
            unique_eq,
        };
        if best.as_ref().map_or(true, |(s, _, _)| score > *s) {
            best = Some((score, access, terms));
        }
    }

    // 3. rowid range.
    let mut rowid_low = None;
    let mut rowid_high = None;
    let mut rowid_terms = Vec::new();
    for c in cons.iter().filter(|c| c.rowid) {
        match c.op {
            BinOp::Gt => {
                rowid_low = Some((c.value.clone(), false));
                rowid_terms.push(c.term);
            }
            BinOp::Ge => {
                rowid_low = Some((c.value.clone(), true));
                rowid_terms.push(c.term);
            }
            BinOp::Lt => {
                rowid_high = Some((c.value.clone(), false));
                rowid_terms.push(c.term);
            }
            BinOp::Le => {
                rowid_high = Some((c.value.clone(), true));
                rowid_terms.push(c.term);
            }
            _ => {}
        }
    }
    let rowid_score = usize::from(rowid_low.is_some()) + usize::from(rowid_high.is_some());
    if rowid_score > 0 && best.as_ref().map_or(true, |(s, _, _)| rowid_score * 2 > *s) {
        for t in rowid_terms {
            used[t] = true;
        }
        return Access::RowidRange {
            low: rowid_low,
            high: rowid_high,
        };
    }
    match best {
        Some((_, access, terms)) => {
            for t in terms {
                used[t] = true;
            }
            access
        }
        None => Access::Scan,
    }
}

/// The index's collation must match the column's declared collation for a seek
/// to be sound (we compare with the index's, so they have to agree).
fn collation_ok(table: &TableInfo, col: usize, index_coll: Collation) -> bool {
    table
        .columns
        .get(col)
        .map(|c| c.collation == index_coll)
        .unwrap_or(false)
}

/// True when the first level's access path already produces rows in the
/// requested ORDER BY order, so the sort can be skipped.
fn order_is_satisfied(plan: &Plan) -> bool {
    if plan.order_by.is_empty() {
        return true;
    }
    if plan.compound.is_some() {
        // The combined result is not in any single access path's order.
        return false;
    }
    if !plan.group_by.is_empty() || !plan.aggregates.is_empty() || plan.distinct {
        return false;
    }
    if plan.order_by.iter().any(|(_, desc, _)| *desc) {
        return false;
    }
    let Some(level) = plan.levels.first() else {
        return false;
    };
    // Ordering is only guaranteed by the outermost loop.
    if plan.levels.len() > 1 {
        let mut slots = Vec::new();
        for (e, _, _) in &plan.order_by {
            e.slots(&mut slots);
        }
        if slots.iter().any(|s| *s != level.slot) {
            return false;
        }
    }
    let Source::Table { access, .. } = &level.source else {
        return false;
    };
    match access {
        Access::Scan | Access::RowidRange { .. } => {
            // Table scans come back in rowid order.
            plan.order_by.len() == 1
                && matches!(&plan.order_by[0].0, PExpr::Rowid { slot } if *slot == level.slot)
        }
        Access::RowidEq(_) => true,
        Access::Index { columns, eq, .. } => {
            // Rows come back ordered by the key columns after the equality
            // prefix, then by rowid. Columns pinned by an equality are constant
            // within the scan, so ORDER BY may name them anywhere.
            let mut want = Vec::new();
            for (e, _, _) in &plan.order_by {
                match e {
                    PExpr::Column { slot, col, coll, .. } if *slot == level.slot => {
                        want.push((*col, *coll))
                    }
                    PExpr::Rowid { slot } if *slot == level.slot => {
                        want.push((usize::MAX, Collation::Binary))
                    }
                    _ => return false,
                }
            }
            let mut pos = eq.len();
            for (col, coll) in want {
                if col != usize::MAX && columns[..eq.len()].iter().any(|c| c.0 == col) {
                    continue; // constant for the whole scan
                }
                if col == usize::MAX {
                    // The rowid is the last key of an index entry, so it only
                    // orders rows once every key column is pinned or matched.
                    if pos != columns.len() {
                        return false;
                    }
                    pos = columns.len() + 1;
                    continue;
                }
                match columns.get(pos) {
                    Some((kc, kcoll)) if *kc == col && *kcoll == coll => pos += 1,
                    _ => return false,
                }
            }
            true
        }
    }
}
