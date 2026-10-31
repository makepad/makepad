// ported from N:rustc_middle/src/ty/adjustment.rs and rustc_middle/src/ty/typeck_results.rs
// (adjustments, type_dependent_defs) @8d1a764; 1.97.1 has the same shapes.
//! The adjustments typeck applies to an expression: one list per node, replacing the five tables
//! (`coerce`, `reborrows`, `ov_derefs`, `index_derefs`, `call_derefs`) that each held a slice
//! of it. rustc stores `Vec<Adjustment>` on the adjusted expression; Relax keeps the key its
//! consumers already use (the node that consumes the value) and two sites per node:
//!
//! * `operand`: the autoderef of the node's operand -- the method receiver of a `MethodCall`,
//!   the base of an `Index`, the field base of a `Field`, the callee of a `Call`, the operand
//!   of an overloaded unary `*`. Every step is recorded, built-in or overloaded, in order.
//! * `value`: the node's own value converted to the type its context expects (a coercion site):
//!   `[Deref*, Borrow?, Coerce?]` in application order.
//!
//! Every `Adjustment` carries `target`, the type of the value after the step (rustc
//! `Adjustment::target`), resolved by writeback.
//!
//! Mapping to rustc, exactly (the one place Relax is coarser):
//! * operand site: identical to rustc's autoderef adjustments of the receiver / base / callee
//!   (`Deref(None)` for a reference or raw pointer step, `Deref(Some)` for `Deref::deref`).
//!   rustc's trailing autoref / unsize of a method receiver stays in `MethodRes`
//!   (`autoref`, `unsize`), not in this list.
//! * value site, reference coercions: rustc's `coerce_borrowed_pointer` is
//!   `[Deref(None) (the source reference itself), Deref(..)*, Borrow]`. Relax **omits the leading
//!   built-in `Deref(None)`** that peels the source reference: `&String -> &str` is
//!   `[Deref(Some), Borrow]` (rustc `[Deref(None), Deref(Some), Borrow]`); an equal-type or
//!   `&mut -> &` reborrow is `[Borrow]` (rustc `[Deref(None), Borrow]`); `&&T -> &T` is
//!   `[Deref(None), Borrow]`. A builder that mirrors rustc inserts the leading `Deref(None)` when
//!   the list starts a reference coercion, i.e. whenever the value site contains a `Borrow`
//!   and the source type is a reference (the `target` of the first step is the pointee after
//!   the first listed deref, the source type is `expr_ty`).
//! * value site, unsizing and pointer casts: `[Borrow(source ref -> reborrowed ref),
//!   Coerce(kind)]`, rustc `[Deref(None), Borrow, Pointer(Unsize)]` with the same omission;
//!   `Coerce::{ReifyFn, ClosureFn, UnsafeFn, MutToConst, Unsize, ToDyn, Upcast, RefToRaw}` are
//!   rustc's `Pointer(..)` kinds, `Never` is `NeverToAny`.
//! * `Coerce::DerefRef(n)` is a view over the list (n `Deref`s), never stored.
use super::*;
use crate::types::TyList;

/// An overloaded `Deref::deref` step: the method and its `[Self]` arguments.
pub type OverloadedDeref = (DefId, TyList);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjust {
    /// One autoderef step: built-in for references and raw pointers (`None`), a call of
    /// `Deref::deref` / `deref_mut` otherwise (rustc `Adjust::Deref(Option<OverloadedDeref>)`).
    Deref(Option<OverloadedDeref>),
    /// The final (re)borrow of the adjusted value; `true` for `&mut` / `*mut`
    /// (rustc `Adjust::Borrow(AutoBorrow::Ref | RawPtr)`).
    Borrow(bool),
    /// `Adjust::NeverToAny` and `Adjust::Pointer(..)`; never `Coerce::DerefRef`, which is the
    /// `Deref` entries of the list.
    Coerce(Coerce),
}

/// One step and the type of the value after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adjustment {
    pub kind: Adjust,
    pub target: TyId,
    /// rustc `OverloadedDeref.mutbl`, for `Deref(Some)`: the step is `DerefMut::deref_mut`
    /// (place_op.rs convert_place_derefs_to_mutable). The recorded `(def, args)` stay the
    /// shared `Deref::deref`; `Tcx::mutable_place_op` names the mutable method.
    pub mutbl: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adjustments {
    pub operand: ThinVec<Adjustment>,
    pub value: ThinVec<Adjustment>,
}

impl Adjustments {
    fn is_empty(&self) -> bool { self.operand.is_empty() && self.value.is_empty() }
    pub fn heap_bytes(&self) -> usize { self.operand.heap_bytes() + self.value.heap_bytes() }
}

/// Whether the key addresses the `value` site (the legacy `DEREF_COERCE_KEY` bit) and its node.
fn split_key(key: u32) -> (u32, bool) {
    if key & DEREF_COERCE_KEY != 0 { (key & !DEREF_COERCE_KEY, true) } else { (key, false) }
}

impl Body {
    pub fn adjustments(&self, node: u32) -> Option<&Adjustments> { self.adjustments.get(&node) }

    /// The coercion applied to the value of `node` (rustc `Adjust::NeverToAny` / `Pointer`),
    /// or `DerefRef(n)` for n autoderef steps ending in a borrow (`&String -> &str`).
    pub fn coerce(&self, node: u32) -> Option<Coerce> {
        let value = &self.adjustments.get(&node)?.value;
        let mut derefs = 0;
        for a in value.iter() {
            match a.kind {
                Adjust::Coerce(c) => return Some(c),
                Adjust::Deref(_) => derefs += 1,
                Adjust::Borrow(_) => {}
            }
        }
        (derefs > 0).then_some(Coerce::DerefRef(derefs))
    }

    /// The mutability of the reborrow of the value of `node`, if it is reborrowed.
    pub fn reborrow(&self, node: u32) -> Option<bool> {
        self.adjustments.get(&node)?.value.iter().find_map(|a| match a.kind { Adjust::Borrow(m) => Some(m), _ => None })
    }

    /// The overloaded deref at `step` of the autoderef addressed by `key`: the node of a
    /// receiver / base / callee / `*` operand, or a node's coercion derefs (`node | DEREF_COERCE_KEY`).
    pub fn ov_deref(&self, key: u32, step: usize) -> Option<OverloadedDeref> {
        let (node, value) = split_key(key);
        let adj = self.adjustments.get(&node)?;
        match (if value { &adj.value } else { &adj.operand }).get(step) {
            Some(Adjustment { kind: Adjust::Deref(Some(ov)), .. }) => Some(*ov),
            _ => None,
        }
    }

    /// Autoderef steps from an `Index` base to the indexed type.
    pub fn index_derefs(&self, node: u32) -> usize {
        self.adjustments.get(&node).map_or(0, |a| a.operand.len())
    }

    /// Autoderef steps from a `Call` callee to the callable (none recorded: `None`).
    pub fn call_derefs(&self, node: u32) -> Option<usize> {
        self.adjustments.get(&node).map(|a| a.operand.len()).filter(|&n| n > 0)
    }

    /// Every overloaded deref of the body: (key, step, call), keys as `ov_deref` takes them.
    pub fn overloaded_derefs(&self) -> impl Iterator<Item = (u32, usize, OverloadedDeref)> + '_ {
        self.adjustments.iter().flat_map(|(&node, adj)| {
            let operand = adj.operand.iter().enumerate().filter_map(move |(i, a)| match a.kind { Adjust::Deref(Some(ov)) => Some((node, i, ov)), _ => None });
            let value = adj.value.iter().enumerate().filter_map(move |(i, a)| match a.kind { Adjust::Deref(Some(ov)) => Some((node | DEREF_COERCE_KEY, i, ov)), _ => None });
            operand.chain(value)
        })
    }

    /// rustc `TypeckResults::type_dependent_def`: the method an expression calls without
    /// naming it -- a method call, an overloaded operator or index (`IndexMut::index_mut` when
    /// the place is used mutably), or an overloaded unary `*`.
    pub fn type_dependent_def(&self, node: u32) -> Option<(DefId, TyList)> {
        if let Some(m) = self.methods.get(&node).or_else(|| self.binops.get(&node)) { return Some((m.def, m.args)); }
        if let Some(ov) = self.mutable_index.get(&node).or_else(|| self.ov_index.get(&node)) { return Some(*ov); }
        self.ov_deref(node, 0)
    }

    /// Mutable access to every step (writeback, instantiation): the substitution applies to the
    /// arguments, the coercion payload types and the targets.
    pub fn adjust_payloads_mut(&mut self) -> impl Iterator<Item = &mut Adjustment> {
        self.adjustments.values_mut().flat_map(|a| a.operand.iter_mut().chain(a.value.iter_mut()))
    }
}

impl crate::tcx::Tcx {
    /// `DerefMut::deref_mut` for `Deref::deref` (rustc `OverloadedDeref::method_call` with `mutbl`).
    pub fn mutable_place_op(&self, prog: &Program, shared: DefId) -> Option<DefId> {
        let (item, name) = match prog.syms.str(prog.def(shared).name) { "deref" => (crate::lang_items::LangItem::DerefMut, "deref_mut"), _ => return None };
        let td = self.lang_items.get(item)?;
        let sym = prog.syms.get(name)?;
        prog.traits[prog.def(td).sub as usize].iter().copied().find(|&it| prog.def(it).name == sym)
    }
}

impl Fcx<'_> {
    fn adj_mut(&mut self, node: u32) -> &mut Adjustments { self.b.adjustments.get_or_insert_with(node, Adjustments::default) }

    /// The reference type `source` has after being reborrowed with mutability `mutable`
    /// (the `target` of a `Borrow` step): `&'a T` -> `&'a mut T`, a raw pointer or anything
    /// else keeps its type.
    pub(super) fn borrow_target(&mut self, source: TyId, mutable: bool) -> TyId {
        let source = self.shallow(source);
        match *self.tcx.tys.kind(source) {
            TyKind::Ref(_, inner, region) => self.tcx.tys.intern(TyKind::Ref(mutable, inner, region)),
            _ => source,
        }
    }

    /// Apply the coercion to the value of `node` (the previous one is replaced); `target` is the
    /// type after it.
    pub(super) fn set_coerce(&mut self, node: u32, coerce: Coerce, target: TyId) {
        debug_assert!(!matches!(coerce, Coerce::DerefRef(_)));
        let value = &mut self.adj_mut(node).value;
        value.retain(|a| !matches!(a.kind, Adjust::Coerce(_)));
        value.push(Adjustment { kind: Adjust::Coerce(coerce), target, mutbl: false });
    }

    /// The autoderef steps (`(overloaded call, type after the step)`) then a borrow of
    /// mutability `mutable` of type `borrowed` (an existing borrow stays when `None`):
    /// `&&T -> &T`, `&String -> &str`.
    pub(super) fn set_deref_ref(&mut self, node: u32, derefs: Vec<(Option<OverloadedDeref>, TyId)>, mutable: Option<(bool, TyId)>) {
        let value = &mut self.adj_mut(node).value;
        let old = value.iter().find_map(|a| match a.kind { Adjust::Borrow(m) => Some((m, a.target)), _ => None });
        value.clear();
        for (d, target) in derefs { value.push(Adjustment { kind: Adjust::Deref(d), target, mutbl: false }); }
        if let Some((m, target)) = mutable.or(old) { value.push(Adjustment { kind: Adjust::Borrow(m), target, mutbl: false }); }
    }

    /// Reborrow the value of `node` as `target` (kept in front of a pointer coercion).
    pub(super) fn set_reborrow(&mut self, node: u32, mutable: bool, target: TyId) {
        let value = &mut self.adj_mut(node).value;
        let step = Adjustment { kind: Adjust::Borrow(mutable), target, mutbl: false };
        match value.iter_mut().find(|a| matches!(a.kind, Adjust::Borrow(_))) {
            Some(a) => *a = step,
            None => {
                let at = value.iter().position(|a| matches!(a.kind, Adjust::Coerce(_))).unwrap_or(value.len());
                value.insert(at, step);
            }
        }
    }

    /// The `value` site of `node`, to restore after a coercion attempt fails.
    pub(super) fn save_value_adjustments(&self, node: u32) -> ThinVec<Adjustment> {
        self.b.adjustments.get(&node).map_or_else(ThinVec::new, |a| a.value.clone())
    }

    pub(super) fn restore_value_adjustments(&mut self, node: u32, saved: ThinVec<Adjustment>) {
        if saved.is_empty() {
            if let Some(adj) = self.b.adjustments.get_mut(&node) {
                adj.value.clear();
                if adj.is_empty() { self.b.adjustments.remove(&node); }
            }
        } else {
            self.adj_mut(node).value = saved;
        }
    }

    /// Give `to` the coercion of `from` (the arms of a `CoerceMany` share one adjustment).
    pub(super) fn copy_value_adjustments(&mut self, from: u32, to: u32) {
        let saved = self.save_value_adjustments(from);
        if !saved.is_empty() { self.adj_mut(to).value = saved; }
    }

    /// Record step `step` of the operand autoderef of `node` (built-in or overloaded), leaving
    /// `target` as the type after it. Steps are recorded in order as they are taken; a step
    /// overwrites what an earlier attempt left there.
    pub(super) fn set_operand_deref(&mut self, node: u32, step: usize, deref: Option<OverloadedDeref>, target: TyId) {
        let placeholder = self.tcx.tys.error;
        let operand = &mut self.adj_mut(node).operand;
        while operand.len() <= step { operand.push(Adjustment { kind: Adjust::Deref(None), target: placeholder, mutbl: false }); }
        operand[step] = Adjustment { kind: Adjust::Deref(deref), target, mutbl: false };
    }

    /// The operand autoderef of `node` has exactly `n` steps (all recorded already).
    pub(super) fn set_operand_derefs(&mut self, node: u32, n: usize) {
        if let Some(adj) = self.b.adjustments.get_mut(&node) {
            debug_assert!(adj.operand.len() >= n, "operand autoderef of {node} recorded {} of {n} steps", adj.operand.len());
            adj.operand.truncate(n);
            if adj.is_empty() { self.b.adjustments.remove(&node); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typeck::solve::program;

    const PRELUDE: &str = "#![no_std]\n#[lang = \"sized\"] trait Sized {}\n#[lang = \"deref\"] trait Deref { type Target: ?Sized; fn deref(&self) -> &Self::Target; }\n#[lang = \"unsize\"] trait Unsize<T: ?Sized> {}\n#[lang = \"coerce_unsized\"] trait CoerceUnsized<T: ?Sized> {}\nimpl<'a, 'b: 'a, T: ?Sized + Unsize<U>, U: ?Sized> CoerceUnsized<&'a U> for &'b T {}\nimpl<'a, T: ?Sized + Unsize<U>, U: ?Sized> CoerceUnsized<&'a mut U> for &'a mut T {}\n";

    fn body_of(src: &str, name: &str) -> (Program, std::sync::Arc<Body>) {
        let (prog, tcx) = program(&format!("{PRELUDE}{src}"));
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == name).map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{name}: {:?}", body.errors);
        (prog, body)
    }

    /// The tail expression of the fn body, whose coercion site is the return type.
    fn tail(prog: &Program, body: &Body) -> u32 {
        let ast = &prog.files[body.file as usize].ast;
        let block = ast.block(body.body.expect("a body"));
        let Some(crate::ast::Stmt::Expr(tail, false)) = ast.list(block.stmts).last().copied() else { panic!("no tail expression") };
        tail.0
    }

    #[test]
    fn deref_ref_is_derefs_then_borrow() {
        let (prog, body) = body_of("fn f<'a>(x: &'a &'a u8) -> &'a u8 { x }", "f");
        let e = tail(&prog, &body);
        assert_eq!(body.coerce(e), Some(Coerce::DerefRef(1)));
        assert_eq!(body.reborrow(e), Some(false));
        let kinds: Vec<_> = body.adjustments(e).unwrap().value.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, [Adjust::Deref(None), Adjust::Borrow(false)]);
        // the borrow's target is the coerced value's type, the return type
        assert_eq!(body.adjustments(e).unwrap().value[1].target, body.ret);
    }

    #[test]
    fn overloaded_coercion_deref_keeps_its_call() {
        let (prog, body) = body_of("struct W(u8); impl Deref for W { type Target = u8; fn deref(&self) -> &u8 { &self.0 } } fn f(x: &W) -> &u8 { x }", "f");
        let e = tail(&prog, &body);
        assert_eq!(body.coerce(e), Some(Coerce::DerefRef(1)));
        let (method, args) = body.ov_deref(e | DEREF_COERCE_KEY, 0).expect("overloaded Deref::deref");
        assert_eq!(prog.name(method), "deref");
        assert!(!args.is_empty());
        assert!(body.ov_deref(e, 0).is_none(), "the operand site is a different list");
        assert_eq!(body.overloaded_derefs().count(), 1);
    }

    #[test]
    fn equal_type_mutable_reference_is_a_reborrow_without_coercion() {
        let (prog, body) = body_of("fn f(x: &mut u8) -> &mut u8 { x }", "f");
        let e = tail(&prog, &body);
        assert_eq!(body.coerce(e), None);
        assert_eq!(body.reborrow(e), Some(true));
    }

    #[test]
    fn unsize_keeps_the_borrow_in_front_of_the_pointer_coercion() {
        let (prog, body) = body_of("fn f(x: &[u8; 2]) -> &[u8] { x }", "f");
        let e = tail(&prog, &body);
        assert_eq!(body.coerce(e), Some(Coerce::Unsize));
        let kinds: Vec<_> = body.adjustments(e).unwrap().value.iter().map(|a| a.kind).collect();
        assert_eq!(kinds, [Adjust::Borrow(false), Adjust::Coerce(Coerce::Unsize)]);
    }

    #[test]
    fn unadjusted_expression_has_no_entry() {
        let (prog, body) = body_of("fn f(x: u8) -> u8 { x }", "f");
        let e = tail(&prog, &body);
        assert!(body.adjustments(e).is_none());
        assert_eq!((body.coerce(e), body.reborrow(e)), (None, None));
    }

    #[test]
    fn index_and_call_operand_derefs_are_counted() {
        let (_, body) = body_of("fn f(v: &&[u8], n: usize) -> u8 { v[n] }", "f");
        let index = body.adjustments.iter().find(|(_, a)| !a.operand.is_empty()).expect("an indexed base");
        assert_eq!(body.index_derefs(*index.0), 2);
        // v: &&[u8]: after the first step `&[u8]`, after the second `[u8]`; no step is left unresolved
        let targets: Vec<_> = index.1.operand.iter().map(|a| a.target).collect();
        assert_eq!(targets.len(), 2);
        assert!(targets[0] != targets[1] && targets.iter().all(|&t| t != body.ret));
        let (_, body) = body_of("fn g(f: &fn() -> u8) -> u8 { f() }", "g");
        let call = body.adjustments.iter().find(|(_, a)| !a.operand.is_empty()).expect("a callee");
        assert_eq!(body.call_derefs(*call.0), Some(1));
        assert_eq!(body.index_derefs(u32::MAX), 0);
    }

    #[test]
    fn failed_coercion_leaves_no_adjustment() {
        // `x` is first tried against the expected `&u8` (a deref coercion fails to find one),
        // then reported: the speculative entry must not stay behind
        let (prog, tcx) = program(&format!("{PRELUDE}fn f(x: &u16) -> &u8 {{ x }}"));
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "f").map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(!body.errors.is_empty());
        assert!(body.adjustments.iter().all(|(_, a)| a.value.is_empty()), "{:?}", body.adjustments);
    }
}
