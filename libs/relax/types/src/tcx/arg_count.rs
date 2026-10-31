//! rustc_hir_analysis hir_ty_lowering/generics.rs `check_generic_arg_count` and the message of
//! errors/wrong_number_of_generic_args.rs: a path segment's generic arguments against the
//! definition's own generic parameters (E0107).
use super::{DefId, DefKind, Program, Tcx};
use crate::ast::{self, GenericArg, GenericArgs, GenericParamKind, ItemKind};

/// rustc GenericArgPosition: a type/trait path, or a value path (fn, method, ctor).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ArgPosition { Type, /** a type-relative associated type segment: its lifetimes are never elided in */ AssocType, Value }

/// `ty::Generics::own_counts` / `own_defaults` of one definition.
#[derive(Clone, Copy, Default, Debug)]
pub struct OwnCounts {
    pub lifetimes: usize,
    /// named type params (no `Self`, no synthetic `impl Trait` params)
    pub types: usize,
    pub synth: usize,
    pub consts: usize,
    pub defaults: usize,
}

impl Tcx {
    /// The definition's own generic parameters, from its AST. Fns count every written lifetime;
    /// the caller passes `early_lifetimes` for the early-bound ones.
    pub fn own_generic_counts(&self, prog: &Program, d: DefId, early_lifetimes: Option<usize>) -> OwnCounts {
        let def = prog.def(d);
        let ast = &prog.files[def.file as usize].ast;
        let mut c = OwnCounts::default();
        let item = ast.item(def.item);
        let Some(g) = ast.kind_generics(&item.kind) else { return c };
        for p in ast.list(g.params) {
            match p.kind {
                GenericParamKind::Lifetime(_) => c.lifetimes += 1,
                GenericParamKind::Type(_, default) => { c.types += 1; c.defaults += default.is_some() as usize; }
                GenericParamKind::Const(_, default) => { c.consts += 1; c.defaults += default.is_some() as usize; }
            }
        }
        if let ItemKind::Fn(sig, _, _) = &item.kind {
            let sig = ast.fn_sig(*sig);
            let mut found = Vec::new();
            for p in ast.list(sig.params) { super::collect_argument_impl_traits(ast, p.ty, &mut found); }
            c.synth = found.len();
        }
        if let Some(n) = early_lifetimes { c.lifetimes = n; }
        c
    }

    /// rustc `def_path_str` of a type or associated item (`Trait::Item`).
    pub fn def_path_str(&self, prog: &Program, d: DefId) -> String {
        let def = prog.def(d);
        if matches!(def.kind, DefKind::AssocTy | DefKind::AssocFn | DefKind::AssocConst) {
            format!("{}::{}", prog.name(def.parent), prog.name(d))
        } else { prog.name(d).to_string() }
    }

    /// rustc `def_descr`.
    pub fn def_descr(&self, prog: &Program, d: DefId) -> &'static str {
        let def = prog.def(d);
        match def.kind {
            DefKind::Struct => "struct", DefKind::Enum => "enum", DefKind::Union => "union",
            DefKind::Trait => "trait", DefKind::TypeAlias => "type alias", DefKind::AssocTy => "associated type",
            DefKind::Fn | DefKind::ForeignFn => "function",
            DefKind::AssocFn => {
                let ast = &prog.files[def.file as usize].ast;
                match &ast.item(def.item).kind {
                    ItemKind::Fn(sig, _, _) if ast.fn_sig(*sig).self_param.is_some() => "method",
                    _ => "associated function",
                }
            }
            DefKind::Variant => "variant",
            DefKind::AssocConst => "associated constant", DefKind::Const => "constant",
            _ => "item",
        }
    }

    /// `check_generic_arg_count` for one path segment (`args`: its written generic arguments).
    /// `infer_args`: rustc `seg.infer_args` (a value path without brackets). `late_bound`: the
    /// definition has late-bound lifetimes and the segment writes lifetime arguments (they are
    /// ignored, `prohibit_explicit_late_bound_lifetimes`). Returns the error messages (lifetimes first).
    pub fn check_generic_arg_count(&self, prog: &Program, d: DefId, args: Option<&GenericArgs>, ast: &ast::Ast,
        counts: OwnCounts, pos: ArgPosition, infer_args: bool, late_bound: bool) -> Vec<String> {
        let (mut lifetime_args, mut generic_args) = (0usize, 0usize);
        let has_brackets = matches!(args, Some(GenericArgs::Angle(_)));
        if let Some(GenericArgs::Angle(v)) = args {
            for a in ast.list(*v) {
                match a {
                    GenericArg::Lifetime(_) => lifetime_args += 1,
                    GenericArg::Type(_) | GenericArg::Const(_) => generic_args += 1,
                    _ => {}
                }
            }
        }
        let descr = self.def_descr(prog, d);
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let was = |n: usize| if n == 1 { "was" } else { "were" };
        let message = |quantifier: &str, expected: usize, kind: &str, provided: usize| {
            if !has_brackets { return format!("E0107: missing generics for {} `{}`", descr, self.def_path_str(prog, d)); }
            format!("E0107: {} takes {}{} {} argument{} but {} {} argument{} {} supplied", descr, quantifier, expected, kind, plural(expected), provided, kind, plural(provided), was(provided))
        };
        // (lifetimes) elided lifetimes of a type path are inserted by lowering (E0106 is the
        // resolver's): only written ones are counted; `infer_lifetimes` for a value path
        let infer_lifetimes = (pos == ArgPosition::Value || infer_args) && lifetime_args == 0;
        let min_lifetimes = if infer_lifetimes { 0 } else { counts.lifetimes };
        let provided_lifetimes = if lifetime_args == 0 && pos == ArgPosition::Type { counts.lifetimes } else { lifetime_args };
        let mut errs = Vec::new();
        if !(min_lifetimes..=counts.lifetimes).contains(&provided_lifetimes) && !late_bound {
            errs.push(message("", counts.lifetimes, "lifetime", provided_lifetimes));
        }
        let expected_max = counts.types + counts.consts;
        let expected_min = if infer_args { 0 } else { expected_max - counts.defaults };
        if !(expected_min..=expected_max).contains(&generic_args) {
            errs.push(if generic_args > expected_max {
                let q = if counts.defaults == 0 { "" } else { "at most " };
                message(q, expected_max, "generic", generic_args)
            } else {
                let q = if counts.defaults == 0 { "" } else { "at least " };
                message(q, expected_min, "generic", generic_args)
            });
        }
        errs
    }
}
