//! Checks of the lint attributes themselves (rustc_lint levels.rs `LintLevelsBuilder::add`):
//! unknown lint names (`unknown_lints`), renamed and removed lints (`renamed_and_removed_lints`),
//! `allow` / `warn` inside a `forbid` (E0453, or the `forbidden_lint_groups` warning when the
//! forbid named a group), and `#[expect(..)]` that no lint was reported under
//! (`unfulfilled_lint_expectations`).
//!
//! Check-only; `makepad run` never calls this.

use crate::ast::*;
use crate::ast_visit::Visitor;
use crate::lint::{self, AttrKey, Level, Lint};
use crate::lint_table::{self, LevelK};
use crate::program::{DefId, DefKind, Program, NO_DEF};
use relax_session::sync::Mutex;

pub const UNKNOWN_LINTS: Lint = lint::lint("unknown_lints", Level::Warn, &[]);
pub const RENAMED_AND_REMOVED_LINTS: Lint = lint::lint("renamed_and_removed_lints", Level::Warn, &[]);
pub const FORBIDDEN_LINT_GROUPS: Lint = lint::lint("forbidden_lint_groups", Level::Warn, &["future_incompatible"]);
pub const UNFULFILLED_LINT_EXPECTATIONS: Lint = lint::lint("unfulfilled_lint_expectations", Level::Warn, &[]);

/// The lints the check reports completely: an expectation of one of them that never fires is
/// unfulfilled. (Lints the check reports only in part are left out: it would call fulfilled
/// expectations unfulfilled.)
const IMPLEMENTED: &[&str] = &[
    "arithmetic_overflow", "bare_trait_objects", "bindings_with_variant_name", "deprecated_safe_2024",
    "deref_nullptr", "dropping_copy_types", "dropping_references", "ellipsis_inclusive_range_patterns", "for_loops_over_fallibles",
    "forgetting_copy_types", "forgetting_references", "invalid_atomic_ordering", "irrefutable_let_patterns", "keyword_idents_2018",
    "keyword_idents_2024", "let_underscore_drop", "let_underscore_lock", "map_unit_fn", "missing_unsafe_on_extern",
    "no_mangle_const_items", "no_mangle_generic_items", "non_camel_case_types", "non_snake_case", "non_upper_case_globals",
    "noop_method_call", "overflowing_literals", "path_statements", "redundant_semicolons", "single_use_lifetimes",
    "static_mut_refs", "suspicious_double_ref_op", "trivial_casts", "trivial_numeric_casts", "unconditional_panic",
    "undropped_manually_drops", "unreachable_code", "unreachable_patterns", "unsafe_attr_outside_unsafe", "unsafe_op_in_unsafe_fn",
    "unused_allocation", "unused_assignments", "unused_braces", "unused_labels", "unused_lifetimes",
    "unused_must_use", "unused_mut", "unused_parens", "unused_results", "unused_unsafe", "unused_variables", "while_true",
    "mismatched_lifetime_syntaxes", "elided_lifetimes_in_paths", "explicit_outlives_requirements", 
    "absolute_paths_not_starting_with_crate", "unknown_diagnostic_attributes",
];

/// A recorded `#[expect(name)]`.
struct Expectation {
    key: AttrKey,
    pos: u32,
    name: String,
    file: u32,
    def: DefId,
    before: Vec<Attr>,
}

static EXPECTATIONS: Mutex<Vec<Expectation>> = Mutex::new(Vec::new());

/// rustdoc lints rustc registers as ignored (they were moved to tool lints).
const IGNORED: &[&str] = &["broken_intra_doc_links", "private_intra_doc_links", "missing_crate_level_docs", "missing_doc_code_examples",
    "private_doc_tests", "invalid_codeblock_attributes", "invalid_html_tags", "non_autolinks"];

fn to_level(k: LevelK) -> Level {
    match k { LevelK::Allow => Level::Allow, LevelK::Warn => Level::Warn, LevelK::Deny => Level::Deny, LevelK::Forbid => Level::Forbid }
}

fn is_group(n: &str) -> bool {
    lint_table::GROUPS.contains(&n) || lint_table::LINTS.iter().any(|l| l.3.contains(&n))
}

/// Checks every lint attribute of the checked crates; returns the errors (E0453).
pub fn check(prog: &Program, own: &[bool]) -> Vec<String> {
    if !lint::enabled() { return Vec::new(); }
    EXPECTATIONS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    let mut errors = Vec::new();
    // module files' inner attributes
    let mut seen_files: Vec<u32> = Vec::new();
    for md in prog.mods.iter() {
        if !own.get(md.krate as usize).copied().unwrap_or(false) || md.lexical_parent.is_some() { continue; }
        if seen_files.contains(&md.file) { continue; }
        seen_files.push(md.file);
        let ast = &prog.files[md.file as usize].ast;
        let attrs: Vec<Attr> = ast.root_attrs.iter().copied().collect();
        check_list(prog, md.def, &[], &attrs, md.file, &mut errors);
    }
    for i in 0..prog.defs.len() {
        let def = prog.defs[i];
        if !own.get(def.krate as usize).copied().unwrap_or(false) { continue; }
        let d = DefId(i as u32);
        if matches!(def.kind, DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy | DefKind::Prim(_) | DefKind::Field | DefKind::Variant | DefKind::MacroRules) { continue; }
        if def.kind == DefKind::Mod && def.parent == NO_DEF { continue; }
        if lint::generated(prog, d) { continue; }
        let ast = &prog.files[def.file as usize].ast;
        let item = *ast.item(def.item);
        // a position inside a macro expansion is not the user's attribute
        let attrs = ast.list(item.attrs).to_vec();
        check_list(prog, d, &[], &attrs, def.file, &mut errors);
        // attributes of the parts of the item
        let inner_lists = item_part_attrs(ast, &item);
        for l in inner_lists { check_list(prog, d, &[], &l, def.file, &mut errors); }
        // bodies
        match item.kind {
            ItemKind::Fn(..) => {
                if let Ok(Some(b)) = prog.fn_body(def.file, def.item) {
                    let mut w = Walk { prog, def: d, file: def.file, ast, stack: Vec::new(), errors: &mut errors };
                    w.block(b);
                }
            }
            ItemKind::Const(..) | ItemKind::Static(..) => {
                if let Ok(Some(e)) = prog.const_body(def.file, def.item) {
                    let mut w = Walk { prog, def: d, file: def.file, ast, stack: Vec::new(), errors: &mut errors };
                    w.expr(e);
                }
            }
            _ => {}
        }
    }
    errors
}

/// The attribute lists of fields, variants, parameters and generic parameters of an item.
fn item_part_attrs(ast: &Ast, item: &Item) -> Vec<Vec<Attr>> {
    let mut out: Vec<Vec<Attr>> = Vec::new();
    let fields = |l: List<FieldDef>, out: &mut Vec<Vec<Attr>>| { for f in ast.list(l) { out.push(ast.list(f.attrs).to_vec()); } };
    match item.kind {
        ItemKind::Struct(r) => if let VariantData::Struct(l) | VariantData::Tuple(l) = ast.row(r).data { fields(l, &mut out) },
        ItemKind::Union(r) => fields(ast.row(r).fields, &mut out),
        ItemKind::Enum(r) => {
            for v in ast.list(ast.row(r).variants) {
                out.push(ast.list(v.attrs).to_vec());
                if let VariantData::Struct(l) | VariantData::Tuple(l) = v.data { fields(l, &mut out); }
            }
        }
        ItemKind::Fn(s, _, _) => {
            for p in ast.list(ast.fn_sig(s).params) { out.push(ast.list(p.attrs).to_vec()); }
        }
        _ => {}
    }
    if let Some(g) = ast.kind_generics(&item.kind) {
        for p in ast.list(g.params) { out.push(ast.list(p.attrs).to_vec()); }
    }
    out.retain(|l| !l.is_empty());
    out
}

struct Walk<'a, 'e> {
    prog: &'a Program,
    def: DefId,
    file: u32,
    ast: &'a Ast,
    stack: Vec<Attr>,
    errors: &'e mut Vec<String>,
}

impl<'a, 'e> Visitor<'a> for Walk<'a, 'e> {
    fn ast(&self) -> &'a Ast { self.ast }
    fn enter_attrs(&mut self, attrs: List<Attr>) {
        let list = self.ast.list(attrs).to_vec();
        if !list.is_empty() {
            let before = self.stack.clone();
            check_list(self.prog, self.def, &before, &list, self.file, self.errors);
        }
        self.stack.extend(list);
    }
    fn leave_attrs(&mut self, attrs: List<Attr>) {
        let n = self.ast.list(attrs).len();
        self.stack.truncate(self.stack.len() - n);
    }
}

/// One attribute list at a node of `def` (`before`: the attributes of the nodes around it inside
/// the definition).
fn check_list(prog: &Program, def: DefId, before: &[Attr], list: &[Attr], file: u32, errors: &mut Vec<String>) {
    for (ai, a) in list.iter().enumerate() {
        let Some(la) = lint::parse_level_attr(prog, file, a) else { continue };
        // the attributes in force when this one is read
        let mut inner: Vec<Attr> = before.to_vec();
        inner.extend_from_slice(&list[..ai]);
        for (idx, (name, pos)) in la.names.iter().enumerate() {
            if name.contains("::") { continue; }
            if lint::external(prog, file, *pos) { continue; }
            let n = lint::current_name(name);
            if n != name.as_str() && !is_bad_style(name) {
                lint::emit_in(prog, def, &inner, &RENAMED_AND_REMOVED_LINTS, file, *pos, &format!("lint `{}` has been renamed to `{}`", name, n));
            }
            let known = n == "warnings" || is_group(n) || IGNORED.contains(&n) || lint_table::LINTS.iter().any(|l| l.0 == n && !l.4);
            if !known {
                if let Some((_, why)) = lint_table::REMOVED.iter().find(|(r, _)| *r == n) {
                    lint::emit_in(prog, def, &inner, &RENAMED_AND_REMOVED_LINTS, file, *pos, &format!("lint `{}` has been removed: {}", n, why));
                } else {
                    lint::emit_in(prog, def, &inner, &UNKNOWN_LINTS, file, *pos, &format!("unknown lint: `{}`", n));
                }
                continue;
            }
            // an `allow` / `warn` / `expect` below a `forbid` of the same lint
            if la.level != Level::Forbid && la.level != Level::Deny {
                let mut overruled = false;
                let mut from_group = false;
                for l in lint_table::LINTS.iter() {
                    if !(l.0 == n || l.3.contains(&n)) { continue; }
                    let tmp = Lint { name: l.0, default: to_level(l.1), default_2024: l.2.map_or(to_level(l.1), |x| to_level(x.1)), groups: &[] };
                    let r = lint::resolve(prog, def, &inner, &tmp, Some(la.key));
                    if r.level == Level::Forbid {
                        overruled = true;
                        from_group |= r.forbid_from_group;
                    }
                }
                if overruled {
                    let lvl = if la.expect { "expect" } else { match la.level { Level::Allow => "allow", Level::Warn => "warn", _ => "deny" } };
                    let msg = format!("{}({}) incompatible with previous forbid", lvl, name);
                    if from_group {
                        lint::emit_in(prog, def, &inner, &FORBIDDEN_LINT_GROUPS, file, *pos, &msg);
                    } else {
                        errors.push(prog.record_error(file, *pos, "typeck", format!("E0453: {}", msg)));
                    }
                }
            }
            if la.expect && n == "unfulfilled_lint_expectations" {
                // this lint cannot be expected: it would silence itself
                lint::emit_in(prog, def, &inner, &UNFULFILLED_LINT_EXPECTATIONS, file, *pos, "this lint expectation is unfulfilled");
            }
            if la.expect && n != "unfulfilled_lint_expectations" {
                EXPECTATIONS.lock().unwrap_or_else(|e| e.into_inner()).push(Expectation { key: (file, a.toks.lo, idx as u32), pos: *pos, name: n.to_string(), file, def, before: inner.clone() });
            }
        }
    }
}

fn is_bad_style(n: &str) -> bool { n == "bad_style" }

/// Reports the expectations no lint was reported under (after every other pass has run).
pub fn finish(prog: &Program) {
    if !lint::enabled() { return; }
    let fulfilled = lint::fulfilled_expectations();
    let exps = std::mem::take(&mut *EXPECTATIONS.lock().unwrap_or_else(|e| e.into_inner()));
    for e in exps {
        if fulfilled.contains(&e.key) { continue; }
        // only lints the check reports completely can be judged unfulfilled
        let judged = if let Some(l) = lint_table::LINTS.iter().find(|l| l.0 == e.name) {
            IMPLEMENTED.contains(&l.0)
        } else {
            // a group: every lint in it must be implemented
            let members: Vec<&str> = lint_table::LINTS.iter().filter(|l| l.3.contains(&e.name.as_str())).map(|l| l.0).collect();
            !members.is_empty() && members.iter().all(|m| IMPLEMENTED.contains(m))
        };
        if !judged { continue; }
        lint::emit_in(prog, e.def, &e.before, &UNFULFILLED_LINT_EXPECTATIONS, e.file, e.pos, "this lint expectation is unfulfilled");
    }
}
