// ported from N:rustc_hir_typeck/src/place_op.rs:225-446 @8d1a764 (1.97.1 identical)
//! Mutable place operators. Typeck first resolves every overloaded deref and index as the
//! shared `Deref::deref` / `Index::index`; a place that is used mutably (assigned, `&mut`,
//! a `&mut self` receiver, a mutable reborrow, a `ref mut` scrutinee) needs `DerefMut` /
//! `IndexMut` instead. rustc fixes the places up as it meets each such use
//! (`convert_place_derefs_to_mutable`); Relax collects the uses and fixes them once, before
//! closure analysis, when the types are final.
//!
//! Representation: an overloaded deref step records `mutbl` (rustc `OverloadedDeref.mutbl`;
//! the method is `Deref::deref` or `DerefMut::deref_mut` by it) and an overloaded index
//! records the `IndexMut::index_mut` call in `Body.mutable_index` (rustc replaces the
//! type-dependent def). `Body.ov_index` and the deref steps' `(def, args)` keep naming the
//! shared methods: codegen and the BIR consumers choose their own mutable form and read those.
//!
//! Only the MIR borrowck reads the result, so the pass runs only for crates that will be
//! borrow-checked (`keep_user_types`, the run-path rule).
use super::*;
use crate::lang_items::LangItem;
use crate::fxhash::HashSet;
use super::adjustments::{Adjust, Adjustment};

impl Fcx<'_> {
    pub(super) fn finish_mutable_places(&mut self) {
        if !self.keep_user_types || !self.b.errors.is_empty() { return; }
        let mut roots: Vec<ExprId> = Vec::new();
        let mut receivers: Vec<(ExprId, ExprId, usize)> = Vec::new();
        let mut blocks: HashSet<u32> = self.b.body.into_iter().map(|b| b.0).collect();
        for i in 0..self.b.expr_ty.len() {
            if !self.b.expr_typed.get(i) { continue; }
            let e = self.b.expr_at(i);
            let kind = self.ast().expr(e).kind;
            match kind {
                ExprKind::Assign(lhs, _) => self.mutable_assignees(lhs, &mut roots),
                ExprKind::AssignOp(_, lhs, _) | ExprKind::AddrOf(_, true, lhs) => roots.push(lhs),
                ExprKind::Let(pat, init) if self.has_explicit_mut_ref(pat) => roots.push(init),
                ExprKind::Match(scrutinee, arms) => {
                    if self.ast().list(arms).iter().any(|arm| self.has_explicit_mut_ref(arm.pat)) { roots.push(scrutinee); }
                }
                ExprKind::Mac(row) => {
                    let mac = self.ast().row(row);
                    if mac.args != NO_MAC_ARGS {
                        let args = self.ast().mac_arg(mac.args);
                        if args.pat.is_some_and(|pat| self.has_explicit_mut_ref(pat)) { roots.push(self.ast().list(args.exprs)[0]); }
                        if let Some(asm) = args.asm {
                            for op in self.ast().list(asm.ops) { if let Some(out) = op.output { roots.push(out); } }
                        }
                    }
                }
                _ => {}
            }
            match kind {
                ExprKind::Block(block, _) | ExprKind::Unsafe(block) | ExprKind::Async(_, block)
                | ExprKind::ConstBlock(block) | ExprKind::Loop(block, _) | ExprKind::If(_, block, _) => { blocks.insert(block.0); }
                ExprKind::While(row) => { blocks.insert(self.ast().row(row).body.0); }
                ExprKind::For(row) => { blocks.insert(self.ast().row(row).body.0); }
                _ => {}
            }
            // an adjustment that borrows mutably is `&mut <expr>`
            if self.b.reborrow(e.0) == Some(true) || self.b.coerce(e.0) == Some(Coerce::RefToRaw(true)) { roots.push(e); }
            if let Some(method) = self.b.methods.get(&e.0).filter(|m| m.autoref == 2) {
                let recv = match kind {
                    ExprKind::MethodCall(recv, _) => recv,
                    // write!/writeln! keep the writer as the first parsed macro argument
                    ExprKind::Mac(row) => { let args = self.ast().mac_arg(self.ast().row(row).args); self.ast().list(args.exprs)[0] }
                    _ => continue,
                };
                receivers.push((e, recv, method.derefs as usize));
            }
            if let Some(call) = self.b.call_traits.get(&e.0) {
                if [LangItem::FnMut, LangItem::AsyncFnMut].iter().any(|&item| self.tcx.lang_items.get(item) == Some(call.trait_def)) {
                    if let ExprKind::Call(recv, _) = kind { receivers.push((e, recv, call.derefs)); }
                }
            }
        }
        // let-else blocks have no enclosing expression node of their own
        let mut pending: Vec<BlockId> = blocks.iter().map(|&b| BlockId(b)).collect();
        while let Some(block) = pending.pop() {
            for stmt in self.ast().stmts(block) {
                if let Stmt::Let(row) = *stmt {
                    let decl = self.ast().row(row);
                    if let Some(init) = decl.init { if self.has_explicit_mut_ref(decl.pat) { roots.push(init); } }
                    if let Some(els) = decl.else_ { if blocks.insert(els.0) { pending.push(els); } }
                }
            }
        }
        roots.sort_unstable_by_key(|expr| expr.0);
        roots.dedup();
        for root in roots { self.convert_place_to_mutable(root, false); }
        for (call, recv, derefs) in receivers {
            self.last_lo = self.ast().expr(call).lo;
            let inside_union = self.convert_place_to_mutable(recv, false);
            // the receiver's own autoderef steps live on the call node
            for step in 0..derefs { self.convert_deref_step(call.0, false, step, inside_union); }
        }
    }

    /// has a binding `ref mut x` (explicit: the implicit ones sit behind a reference).
    fn has_explicit_mut_ref(&self, pat: PatId) -> bool {
        match *self.ast().pat(pat) {
            Pat::Ident { by_ref, mutbl, sub, .. } => by_ref && mutbl || sub.is_some_and(|pat| self.has_explicit_mut_ref(pat)),
            Pat::Tuple(pats) | Pat::TupleStruct(_, pats) | Pat::Slice(pats) | Pat::Or(pats) => self.ast().list(pats).iter().any(|&pat| self.has_explicit_mut_ref(pat)),
            Pat::Struct(_, fields, _) => self.ast().list(fields).iter().any(|field| self.has_explicit_mut_ref(field.pat)),
            Pat::Ref(_, pat) | Pat::Paren(pat) | Pat::Box(pat) => self.has_explicit_mut_ref(pat),
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) | Pat::Mac(_) => false,
        }
    }

    /// Destructuring assignment lowers each assignee to its own mutable-place assignment;
    /// aggregates are not mutable places merely because they contain one.
    fn mutable_assignees(&self, expr: ExprId, roots: &mut Vec<ExprId>) {
        match self.ast().expr(expr).kind {
            ExprKind::Paren(inner) => self.mutable_assignees(inner, roots),
            ExprKind::Tuple(fields) | ExprKind::Array(fields) => for &field in self.ast().list(fields) { self.mutable_assignees(field, roots); },
            ExprKind::Call(_, fields) if crate::lit::is_assignee_pattern(self.ast(), expr) => for &field in self.ast().list(fields) { self.mutable_assignees(field, roots); },
            ExprKind::Struct(row) => for field in self.ast().list(self.ast().row(row).fields) { self.mutable_assignees(field.expr, roots); },
            ExprKind::Underscore | ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false) => {}
            _ => roots.push(expr),
        }
    }

    /// place_op.rs:265 convert_place_derefs_to_mutable: from the outermost base to the place,
    /// fix the autoderef steps of each node and its overloaded index / deref. Returns whether
    /// the place is inside a union field (an implicit `DerefMut` of `ManuallyDrop` there is an error).
    fn convert_place_to_mutable(&mut self, expr: ExprId, _unused: bool) -> bool {
        let mut chain = vec![expr];
        loop {
            match self.ast().expr(*chain.last().unwrap()).kind {
                ExprKind::Field(base, _) | ExprKind::TupleField(base, _) | ExprKind::Index(base, _)
                | ExprKind::Unary(UnOp::Deref, base) | ExprKind::Paren(base) => chain.push(base),
                _ => break,
            }
        }
        let mut inside_union = false;
        for expr in chain.into_iter().rev() {
            self.last_lo = self.ast().expr(expr).lo;
            let kind = self.ast().expr(expr).kind;
            if matches!(kind, ExprKind::Unary(UnOp::Deref, _)) { inside_union = false; }
            let ty = self.shallow(self.b.ty(expr));
            if let TyKind::Adt(def, ..) = *self.tcx.tys.kind(ty) { inside_union |= self.tcx.adts.get(&def).is_some_and(|adt| adt.is_union()); }
            let operand_steps = match kind {
                ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Index(..) => self.b.adjustments(expr.0).map_or(0, |a| a.operand.len()),
                _ => 0,
            };
            for step in 0..operand_steps { self.convert_deref_step(expr.0, false, step, inside_union); }
            match kind {
                ExprKind::Index(..) => self.convert_index_to_mutable(expr),
                ExprKind::Unary(UnOp::Deref, _) => self.convert_deref_step(expr.0, false, 0, inside_union),
                _ => {}
            }
            let value_steps = self.b.adjustments(expr.0).map_or(0, |a| a.value.len());
            for step in 0..value_steps { self.convert_deref_step(expr.0, true, step, inside_union); }
        }
        inside_union
    }

    /// One recorded autoderef step: an overloaded one becomes `DerefMut::deref_mut` when the
    /// type has an impl (otherwise the shared call stays and borrowck reports the mutation).
    fn convert_deref_step(&mut self, node: u32, value_site: bool, step: usize, inside_union: bool) {
        let step_ref = self.b.adjustments(node).and_then(|a| (if value_site { &a.value } else { &a.operand }).get(step).copied());
        let Some(adj @ Adjustment { kind: Adjust::Deref(Some((_, args))), mutbl: false, .. }) = step_ref else { return };
        let Some(&source) = self.tcx.tys.list(args).first() else { return };
        let Some(td) = self.tcx.lang_items.get(LangItem::DerefMut) else { return };
        let goal = self.trait_goal(td, source, &[]);
        if !self.infcx.root_goal_may_hold_with_depth(self.prog, self.infcx.recursion_limit(self.prog), goal.clone()) { return; }
        self.register_goal(goal, self.last_lo);
        let adjs = self.b.adjustments.get_mut(&node).unwrap();
        let list = if value_site { &mut adjs.value } else { &mut adjs.operand };
        list[step] = Adjustment { mutbl: true, ..adj };
        // place_op.rs:302-338: writing through a union field must not drop the old value
        let source = self.shallow(source);
        if inside_union && matches!(self.tcx.tys.kind(source), TyKind::Adt(def, ..) if self.tcx.adts.get(def).is_some_and(|adt| adt.is_manually_drop())) {
            self.err(self.ast().expr(ExprId(node)).lo, "not automatically applying `DerefMut` on `ManuallyDrop` union field; writing to this reference calls the destructor for the old value; add an explicit `*` if that is desired, or call `ptr::write` to not run the destructor".into());
        }
    }

    /// place_op.rs:360 convert_place_op_to_mutable (Index): the confirmed `Index<Idx>` arguments
    /// give `IndexMut<Idx>` for the same `Self`.
    fn convert_index_to_mutable(&mut self, expr: ExprId) {
        if self.b.mutable_index.contains_key(&expr.0) { return; }
        let Some(&(_, args)) = self.b.ov_index.get(&expr.0).or_else(|| self.b.std_index.get(&expr.0)) else { return };
        let args = self.tcx.tys.list(args).to_vec();
        let Some(td) = self.tcx.lang_items.get(LangItem::IndexMut) else { return };
        let goal = self.trait_goal(td, args[0], &args[1..]);
        if !self.infcx.root_goal_may_hold_with_depth(self.prog, self.infcx.recursion_limit(self.prog), goal.clone()) { return; }
        let Some(method) = self.trait_fn(td, "index_mut") else { return };
        self.register_goal(goal, self.last_lo);
        self.b.mutable_index.insert(expr.0, (method, self.tcx.tys.mk_list(&args)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typeck::solve::program;

    const PRELUDE: &str = "#![no_std]\n#[lang = \"sized\"] trait Sized {}\n#[lang = \"deref\"] trait Deref { type Target: ?Sized; fn deref(&self) -> &Self::Target; }\n#[lang = \"deref_mut\"] trait DerefMut: Deref { fn deref_mut(&mut self) -> &mut Self::Target; }\n#[lang = \"index\"] trait Index<I> { type Output: ?Sized; fn index(&self, i: I) -> &Self::Output; }\n#[lang = \"index_mut\"] trait IndexMut<I>: Index<I> { fn index_mut(&mut self, i: I) -> &mut Self::Output; }\nstruct V { a: u8 } impl Index<usize> for V { type Output = u8; fn index(&self, _: usize) -> &u8 { &self.a } } impl IndexMut<usize> for V { fn index_mut(&mut self, _: usize) -> &mut u8 { &mut self.a } }\nstruct W { a: u8 } impl Deref for W { type Target = u8; fn deref(&self) -> &u8 { &self.a } } impl DerefMut for W { fn deref_mut(&mut self) -> &mut u8 { &mut self.a } }\nstruct R { a: u8 } impl Deref for R { type Target = u8; fn deref(&self) -> &u8 { &self.a } }\n";

    fn body_of(src: &str) -> (Program, std::sync::Arc<Body>) {
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(true)));
        let (prog, tcx) = program(&format!("{PRELUDE}{src}"));
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "f").map(|(i, _)| DefId(i as u32)).unwrap();
        let body = tcx.typeck(&prog, def);
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        (prog, body)
    }

    fn deref_steps(body: &Body) -> Vec<bool> {
        let mut out: Vec<_> = body.adjustments.iter().flat_map(|(_, a)| a.operand.iter().chain(a.value.iter()))
            .filter(|a| matches!(a.kind, Adjust::Deref(Some(_)))).map(|a| a.mutbl).collect();
        out.sort();
        out
    }

    // (Index lookups go through `core::ops::Index` by path, which the single-crate harness has
    // no `core` for: the index conversion is exercised by `makepad check` on real crates.)
    #[test]
    fn assigned_and_borrowed_derefs_use_deref_mut() {
        // `*w = 3`, a method-free field place through an overloaded deref, and a `&mut *w` reborrow
        let (_, body) = body_of("struct S { w: W } fn f(mut s: S, mut w: W) { *s.w = 3; let _r = &mut *w; }");
        assert_eq!(deref_steps(&body), vec![true, true], "{:?}", body.adjustments);
    }

    #[test]
    fn shared_uses_stay_shared() {
        let (_, body) = body_of("fn f(w: W) -> u8 { let b = *w; b }");
        assert_eq!(deref_steps(&body), vec![false]);
    }

    #[test]
    fn a_type_without_the_mutable_impl_keeps_the_shared_call() {
        // borrowck reports the mutation; typeck must not invent an impl
        let (_, body) = body_of("fn f(mut r: R) { *r = 1; }");
        assert_eq!(deref_steps(&body), vec![false]);
    }

    #[test]
    fn crates_not_borrow_checked_do_no_conversion_work() {
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(Some(false)));
        let (prog, tcx) = program(&format!("{PRELUDE}fn f(mut w: W) {{ *w = 1; }}"));
        let def = prog.defs.iter().enumerate().find(|(_, d)| prog.syms.str(d.name) == "f").map(|(i, _)| DefId(i as u32)).unwrap();
        assert_eq!(deref_steps(&tcx.typeck(&prog, def)), vec![false]);
        crate::borrowck::event::FORCE_USER_TYPES.with(|f| f.set(None));
    }

    #[test]
    fn a_diverging_match_arm_gets_never_to_any() {
        // bytemuck from_bytes: `match r { Ok(t) => t, Err(e) => fail(e) }` with `fn fail() -> !`
        let (prog, body) = body_of("enum Res<T> { Ok(T), Err(u8) } fn fail(_: u8) -> ! { loop {} } fn f(r: Res<&u8>) -> &u8 { match r { Res::Ok(t) => t, Res::Err(e) => fail(e) } }");
        let never = body.adjustments.iter().filter(|(_, a)| a.value.iter().any(|s| s.kind == Adjust::Coerce(Coerce::Never))).count();
        assert_eq!(never, 1, "{:?}", body.adjustments);
        let _ = prog;
    }
}
