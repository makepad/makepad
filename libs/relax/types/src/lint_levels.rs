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
use crate::lint_early::UNUSED_ATTRIBUTES;
use crate::program::{DefId, DefKind, Program, NO_DEF};
use relax_session::sync::Mutex;

pub const UNKNOWN_LINTS: Lint = lint::lint("unknown_lints");
pub const RENAMED_AND_REMOVED_LINTS: Lint = lint::lint("renamed_and_removed_lints");
pub const FORBIDDEN_LINT_GROUPS: Lint = lint::lint("forbidden_lint_groups");
pub const UNFULFILLED_LINT_EXPECTATIONS: Lint = lint::lint("unfulfilled_lint_expectations");

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
    "undropped_manually_drops", "unreachable_code", "unreachable_patterns", "invalid_reference_casting", "mutable_transmutes", "semicolon_in_expressions_from_macros", "overlapping_range_endpoints", "non_contiguous_range_endpoints", "unsafe_attr_outside_unsafe", "unsafe_op_in_unsafe_fn",
    "unused_allocation", "unused_assignments", "non_shorthand_field_patterns", "break_with_label_and_loop", "drop_bounds", "dyn_drop", "missing_copy_implementations", "ambiguous_negative_literals", "function_casts_as_integer", "unused_braces", "unused_import_braces", "unused_imports", "unknown_crate_types", "coherence_leak_check", "uncovered_param_in_projection", "tyvar_behind_raw_pointer", "ambiguous_associated_items", "unnameable_types", "repr_transparent_non_zst_fields", "named_arguments_used_positionally", "unused_qualifications", "async_fn_in_trait", "variant_size_differences", "late_bound_lifetime_arguments", "redundant_lifetimes", "impl_trait_redundant_captures", "redundant_imports", "unused_extern_crates", "unused_labels", "unused_lifetimes",
    "unused_must_use", "unused_mut", "unused_parens", "unused_results", "unused_unsafe", "unused_variables", "while_true",
    "mismatched_lifetime_syntaxes", "elided_lifetimes_in_paths", "explicit_outlives_requirements", 
    "absolute_paths_not_starting_with_crate", "unknown_diagnostic_attributes",
    "dangling_pointers_from_locals", "dangling_pointers_from_temporaries", "const_item_mutation", "unconditional_recursion", "array_into_iter",
    "boxed_slice_into_iter", "if_let_rescope", "unused_comparisons", "invalid_nan_comparisons", "ambiguous_wide_pointer_comparisons",
    "unpredictable_function_pointer_comparisons", "useless_ptr_null_checks", "invalid_null_arguments", "closure_returning_async_block",
    "unnecessary_transmutes", "integer_to_ptr_transmutes", "ptr_to_integer_transmute_in_consts", "unsafe_code", "invalid_value", "internal_eq_trait_method_impls", "non_local_definitions", "const_item_interior_mutations",
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
        check_list_at(prog, md.def, &[], &attrs, md.file, &mut errors, md.parent_mod.is_none());
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
    fn leave_attrs(&mut self, attrs: List<Attr>) { crate::ast_visit::pop_attrs(&mut self.stack, self.ast, attrs); }
}

/// rustc_lint levels.rs `visit_attrs`: the shapes a lint attribute may not have. A bare or
/// name-value `deny` is a malformed attribute; inside the list every item must be a lint path
/// (E0452 otherwise, `reason = "string"` excepted) and a scoped path must name a known tool
/// (E0710).
fn malformed(prog: &Program, file: u32, a: &Attr, errors: &mut Vec<String>) {
    let p = prog.src(file);
    let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
    if hi <= lo { return; }
    let head = crate::cfg::tok_str(p, lo);
    let Some(word) = LEVEL_WORDS.iter().find(|w| w.as_bytes() == head) else { return };
    let at = |i: usize| p.tok(i).lo;
    if lint::external(prog, file, at(lo)) { return; }
    if hi < lo + 2 || crate::cfg::tok_str(p, lo + 1) != b"(" {
        errors.push(prog.record_uncoded_error(file, at(lo), "typeck", format!("malformed `{}` attribute input", word)));
        return;
    }
    // items at the top level of the list, split at commas
    let mut i = lo + 2;
    let mut depth = 0i32;
    let mut item = i;
    let check = |from: usize, to: usize, errors: &mut Vec<String>| {
        if from >= to { return; }
        let toks: Vec<&[u8]> = (from..to).map(|k| crate::cfg::tok_str(p, k)).collect();
        let is_ident = |t: &[u8]| t.first().is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_');
        let mut bad = None;
        if toks[0] == b"reason" && toks.len() >= 2 && toks[1] == b"=" {
            if !(toks.len() == 3 && (toks[2].first() == Some(&b'"') || toks[2].starts_with(b"r\"") || toks[2].starts_with(b"r#"))) { bad = Some(if toks.len() == 3 { "reason must be a string literal" } else { "bad attribute argument" }); }
        } else if toks.iter().any(|t| *t == b"=" || *t == b"(") || !is_ident(toks[0]) {
            bad = Some("bad attribute argument");
        } else {
            // a lint path: `name` or `tool::name`
            let ok = toks.iter().enumerate().all(|(k, t)| if k % 2 == 0 { is_ident(t) } else { *t == b"::" }) && toks.len() % 2 == 1;
            if !ok { bad = Some("bad attribute argument"); }
            else if toks.len() == 3 {
                let tool = String::from_utf8_lossy(toks[0]).into_owned();
                if !["clippy", "rustfmt", "rustdoc", "miri", "rustc"].contains(&tool.as_str()) && !registered_tool(prog, file, &tool) {
                    errors.push(prog.record_error(file, at(from), "typeck", format!("E0710: unknown tool name `{}` found in scoped lint: `{}::{}`", tool, tool, String::from_utf8_lossy(toks[2]))));
                }
            }
        }
        if let Some(_why) = bad {
            errors.push(prog.record_error(file, at(from), "typeck", "E0452: malformed lint attribute input".to_string()));
        }
    };
    while i < hi {
        let t = crate::cfg::tok_str(p, i);
        if t == b"(" || t == b"[" || t == b"{" { depth += 1; }
        else if t == b")" || t == b"]" || t == b"}" {
            if depth == 0 { check(item, i, errors); break; }
            depth -= 1;
        } else if t == b"," && depth == 0 {
            check(item, i, errors);
            item = i + 1;
        }
        i += 1;
    }
}

/// `#![register_tool(name)]` at the root of the crate of `file`.
fn registered_tool(prog: &Program, file: u32, tool: &str) -> bool {
    let k = prog.file_crate.get(file as usize).copied().unwrap_or(u32::MAX);
    let Some(c) = prog.crates.get(k as usize) else { return false };
    let root = prog.mods[c.root_mod as usize].file;
    let ast = &prog.files[root as usize].ast;
    let p = prog.src(root);
    ast.root_attrs.iter().any(|a| {
        let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
        crate::cfg::tok_str(p, lo) == b"register_tool" && (lo..hi).any(|i| crate::cfg::tok_str(p, i) == tool.as_bytes())
    })
}

const LEVEL_WORDS: &[&str] = &["allow", "warn", "deny", "forbid", "expect"];

/// One attribute list at a node of `def` (`before`: the attributes of the nodes around it inside
/// the definition).
/// The lints rustc declares `crate_level_only`: an attribute naming one anywhere but on the crate
/// is ignored (`unused_attributes`).
const CRATE_LEVEL_ONLY: &[&str] = &["ill_formed_attribute_input", "unused_crate_dependencies", "dead_code_pub_in_binary", "unknown_crate_types",
    "macro_expanded_macro_exports_accessed_by_absolute_paths", "rust_2021_prefixes_incompatible_syntax", "text_direction_codepoint_in_comment",
    "text_direction_codepoint_in_literal", "rust_2024_guarded_string_incompatible_syntax", "non_ascii_idents", "uncommon_codepoints",
    "confusable_idents", "mixed_script_confusables"];

fn check_list(prog: &Program, def: DefId, before: &[Attr], list: &[Attr], file: u32, errors: &mut Vec<String>) {
    check_list_at(prog, def, before, list, file, errors, false)
}

fn check_list_at(prog: &Program, def: DefId, before: &[Attr], list: &[Attr], file: u32, errors: &mut Vec<String>, crate_node: bool) {
    for (ai, a) in list.iter().enumerate() {
        malformed(prog, file, a, errors);
        let Some(la) = lint::parse_level_attr(prog, file, a) else { continue };
        // the attributes in force when this one is read
        let mut inner: Vec<Attr> = before.to_vec();
        inner.extend_from_slice(&list[..ai]);
        if !crate_node && !la.expect {
            let hit = la.names.iter().find(|(name, pos)| {
                let n = lint::current_name(name);
                !lint::external(prog, file, *pos) && (CRATE_LEVEL_ONLY.contains(&n)
                    || (is_group(n) && lint_table::LINTS.iter().any(|l| l.3.contains(&n) && CRATE_LEVEL_ONLY.contains(&l.0))))
            });
            if let Some((name, pos)) = hit {
                let lvl = match la.level { Level::Allow => "allow", Level::Warn => "warn", Level::Forbid => "forbid", _ => "deny" };
                lint::emit_in(prog, def, &inner, &UNUSED_ATTRIBUTES, file, *pos, &format!("{}({}) is ignored unless specified at crate level", lvl, name));
            }
        }
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
