//! rustc_mir_build/src/thir/cx/{mod,block,expr}.rs (1.97.1): build the THIR of one body owner from the
//! AST and the typeck facts. A view over (Ast, typeck::Body): no second typeck, no source text.
//!
//! Where Relax's typeck records differ from rustc's TypeckResults:
//! - Adjustments live in one list per node with two sites (`typeck/adjustments.rs`): `operand` (the
//!   autoderef of a receiver / base / callee / `*` operand, applied by the consuming node) and `value`
//!   (the coercion of the node's own value, applied by `mirror_expr`). The leading builtin `Deref(None)`
//!   of a reference coercion is not stored; it is inserted here, as that file documents.
//! - Method autoref, the `&mut` receiver reborrow, overloaded operators, indexing, `for`, `?` and the
//!   builtin macros come from side tables (`methods`, `binops`, `ov_index`, `for_into`/`for_next`,
//!   `try_branch`/`try_from_residual`) and are expanded here to the calls rustc's HIR has.
//! - The desugarings (`for`, `while`, `?`, `matches!`, `assert!`, the print and panic families) use the
//!   scopes of `region_scope.rs`' synthetic nodes; their bindings (`iter`, `val`, `residual`) are
//!   synthetic locals with ids from `Body.locals.len()` up and a drop scope in `Thir::var_scopes`.
//!
//! Not built (`BuildError`): async fn (the region tree has one body for the wrapper and the coroutine,
//! so the wrapper has no scopes of its own) and async closures, `yield` (typeck rejects it), `box`, asm. They match what the BIR checker leaves unchecked.
use super::{
    Arm, ArmId, Block, BlockId, BlockSafety, ClosureExpr, ClosureExprKind, Expr, ExprId, ExprKind, LocalVarId, Param, ParamId, Pat, PatKind, Stmt, StmtId, StmtKind,
    Thir, ThirBody, BodyTy, CoroutineThir,
};
use crate::ast;
use crate::borrowck::body::{BodyKind as OwnerKind, CheckedBody};
use crate::consts::GenericArg;
use crate::mir::build::{BuildError, BuildResult, BuildRoot, UnsupportedReason};
use crate::mir::{BindingMode, ByRef, Mutability, Span};
use crate::program::{DefId, DefKind, Program};
use crate::region_scope::{Node, ScopeData, ScopeId};
use crate::tcx::Tcx;
use crate::typeck;
use crate::types::{RegionArgsId, RegionId, TyId, TyKind, TypeFlags};

mod block;
mod desugar;
mod expr;
#[cfg(test)]
pub(crate) mod tests;

/// A loop or labeled block a `break` / `continue` can target.
#[derive(Clone, Copy)]
pub(super) struct Breakable {
    pub label: Option<ast::Ident>,
    pub scope: ScopeId,
    /// A labeled block: only a labeled `break` reaches it.
    pub is_block: bool,
}

pub struct Cx<'a> {
    pub root: &'a BuildRoot<'a>,
    pub owner: CheckedBody,
    pub thir: Thir,
    /// Where a pattern with no leaf of its own (`_`, `..`) is placed (derived pattern spans,
    /// `pattern/span.rs`): the callers set it to the start of the enclosing construct.
    pub pat_hint: u32,
    pub(super) breakables: Vec<Breakable>,
    /// The next synthetic local id (`Body.locals.len()` up).
    next_synthetic: u32,
    /// The expression being lowered is an argument of a call: a `&mut` reborrow adjustment of it is a
    /// two-phase borrow (rustc `AllowTwoPhase::Yes` at argument coercion sites).
    pub(super) two_phase: bool,
    /// The source of the `Cast` being lowered: its pointer coercions are "from an `as` cast".
    pub(super) cast_source: Option<ast::ExprId>,
    /// The `_task_context` parameter of the coroutine body being built (`.await` polls with it).
    pub(super) task_context: Option<LocalVarId>,
}

impl<'a> Cx<'a> {
    pub fn new(root: &'a BuildRoot<'a>, owner: CheckedBody) -> Self {
        let next_synthetic = root.typeck.locals.len() as u32;
        Self { root, owner, thir: Thir::default(), pat_hint: 0, breakables: Vec::new(), next_synthetic, two_phase: false, cast_source: None, task_context: None }
    }
    pub fn error(&self, span: Span, reason: UnsupportedReason) -> BuildError {
        BuildError::new(self.owner, span, reason)
    }
    pub(crate) fn unresolved<T>(&self, span: Span, what: &'static str) -> BuildResult<T> {
        Err(self.error(span, UnsupportedReason::UnresolvedFact(what)))
    }
    pub(super) fn not_built<T>(&self, span: Span, what: &'static str) -> BuildResult<T> {
        Err(self.error(span, UnsupportedReason::NotYetBuilt(what)))
    }

    // ---- context
    pub(super) fn tb(&self) -> &'a typeck::Body {
        self.root.typeck
    }
    pub(super) fn prog(&self) -> &'a Program {
        self.root.cx.prog
    }
    pub(super) fn tcx(&self) -> &'a Tcx {
        self.root.cx.tcx
    }
    pub(crate) fn ast(&self) -> &'a ast::Ast {
        &self.prog().files[self.tb().file as usize].ast
    }
    pub(super) fn file(&self) -> &'a crate::parser::ParsedFile {
        &self.prog().files[self.tb().file as usize]
    }
    pub(super) fn span(&self, lo: u32, hi: u32) -> Span {
        Span { file: self.tb().file, lo, hi }
    }
    pub(super) fn espan(&self, e: ast::ExprId) -> Span {
        let ex = self.ast().expr(e);
        self.span(ex.lo, ex.hi)
    }
    pub(super) fn unit_ty(&self) -> TyId {
        self.tcx().tys.unit
    }
    pub(super) fn bool_ty(&self) -> TyId {
        self.tcx().tys.bool_
    }
    pub(super) fn ref_ty(&self, mutable: bool, ty: TyId) -> TyId {
        self.tcx().tys.intern(TyKind::Ref(mutable, ty, RegionId::ERASED))
    }

    /// The type typeck recorded for an expression (before its adjustments).
    pub(super) fn ety(&self, e: ast::ExprId) -> BuildResult<TyId> {
        let tb = self.tb();
        let i = tb.expr_ix(e);
        let span = self.espan(e);
        if i >= tb.expr_ty.len() || !tb.expr_typed.get(i) {
            return self.unresolved(span, "expression type");
        }
        let ty = tb.expr_ty[i];
        let flags = self.tcx().tys.flags(ty);
        if flags.intersects(TypeFlags::HAS_ERROR) || flags.intersects(TypeFlags::HAS_NON_REGION_INFER) {
            return Err(self.error(span, UnsupportedReason::TypeckTainted));
        }
        Ok(ty)
    }
    /// The type of an expression after its value adjustments (rustc `expr_ty_adjusted`).
    pub(super) fn ety_adjusted(&self, e: ast::ExprId) -> BuildResult<TyId> {
        let ty = self.ety(e)?;
        Ok(self.tb().adjustments(e.0).and_then(|a| a.value.last()).map_or(ty, |a| a.target))
    }

    // ---- scopes
    pub(super) fn scope_of(&self, node: Node, data: ScopeData) -> Option<ScopeId> {
        self.root.scope_tree.scope(node, data)
    }
    pub(super) fn node_scope(&self, node: Node, span: Span) -> BuildResult<ScopeId> {
        match self.scope_of(node, ScopeData::Node) {
            Some(scope) => Ok(scope),
            None => self.unresolved(span, "region scope of a node"),
        }
    }
    /// rustc `temp_scope_id`, resolved through the scope tree: the scope the temporaries of `e` die in
    /// (None: static extension).
    pub(super) fn temp_scope(&self, e: ast::ExprId) -> Option<ScopeId> {
        self.scope_of(Node::Expr(e.0), ScopeData::Node)?;
        self.root.scope_tree.temporary_scope(e)
    }

    /// The scope `e`'s temporaries die in from edition 2024, when that differs (the lint
    /// `tail_expr_drop_order`).
    pub(super) fn bid_scope(&self, e: ast::ExprId) -> Option<ScopeId> {
        self.scope_of(Node::Expr(e.0), ScopeData::Node)?;
        self.root.scope_tree.temp_lifetime(e).1
    }

    // ---- arena
    pub(super) fn push(&mut self, expr: Expr) -> ExprId {
        self.thir.alloc_expr(expr)
    }
    /// An expression of `e`'s temporary scope and span.
    pub(super) fn mk(&self, e: ast::ExprId, kind: ExprKind, ty: TyId) -> Expr {
        Expr { kind, ty, temp_scope: self.temp_scope(e), bid: self.bid_scope(e), span: self.espan(e) }
    }
    pub(super) fn mk_at(&self, e: ast::ExprId, span: Span, kind: ExprKind, ty: TyId) -> Expr {
        Expr { kind, ty, temp_scope: self.temp_scope(e), bid: self.bid_scope(e), span }
    }

    // ---- generic arguments
    /// The full argument list of `def` from typeck's compact list (types and consts), with erased
    /// region slots.
    pub(super) fn full_args(&self, def: DefId, compact: &[TyId], span: Span) -> BuildResult<Vec<GenericArg>> {
        let (tcx, prog) = (self.tcx(), self.prog());
        let indices = tcx.generic_indices_of(prog, def);
        if compact.len() != indices.types_consts.len() {
            return self.unresolved(span, "generic argument count");
        }
        let regions = tcx.tys.intern_region_args(&vec![RegionId::ERASED; indices.lifetimes.len()]);
        Ok(tcx.complete_args(prog, def, compact, regions))
    }
    /// A `FnDef` type with erased region slots.
    pub(super) fn fn_def_ty(&self, def: DefId, args: &[TyId]) -> TyId {
        let (tcx, prog) = (self.tcx(), self.prog());
        let lifetimes = tcx.generic_indices_of(prog, def).lifetimes.len();
        let regions: RegionArgsId = tcx.tys.intern_region_args(&vec![RegionId::ERASED; lifetimes]);
        tcx.tys.intern(TyKind::FnDef(def, tcx.tys.mk_list(args), regions))
    }
    /// The full arguments of an ADT type.
    pub(super) fn adt_args(&self, ty: TyId, span: Span) -> BuildResult<(DefId, Vec<GenericArg>)> {
        let tcx = self.tcx();
        let TyKind::Adt(def, args, regions) = *tcx.tys.kind(ty) else { return self.unresolved(span, "ADT type") };
        Ok((def, tcx.complete_args(self.prog(), def, tcx.tys.list(args), regions)))
    }

    // ---- synthetic locals
    /// A binding of a desugaring: a fresh local id whose drop is scheduled in `scope`.
    pub(super) fn synthetic_binding(&mut self, scope: ScopeId) -> LocalVarId {
        let id = self.next_synthetic;
        self.next_synthetic += 1;
        self.thir.var_scopes.insert(id, scope);
        LocalVarId(id)
    }
    pub(super) fn binding_pat(&self, var: LocalVarId, name: &str, ty: TyId, mutable: bool, span: Span) -> Pat {
        let name = self.prog().syms.get(name).unwrap_or(0);
        Pat {
            ty,
            span,
            extra: None,
            kind: PatKind::Binding {
                name,
                mode: BindingMode(ByRef::No, if mutable { Mutability::Mut } else { Mutability::Not }),
                var,
                ty,
                subpattern: None,
                is_primary: true,
                is_shorthand: false,
            },
        }
    }

    /// The liberated output of the function's signature when it names an opaque type (the body's value defines
    /// it), as the borrow checker's universal regions have it.
    fn declared_output(&self, def: crate::program::DefId) -> Option<TyId> {
        let tcx = self.tcx();
        let sig = tcx.fn_item_sig(self.prog(), def)?;
        // (only an `impl Trait` output differs from the typeck's own, normalized signature)
        if !tcx.tys.has_opaque(sig.ret) { return None; }
        let late = |v: crate::types::BoundVar| tcx.tys.intern_region(crate::types::RegionKind::ReLateParam(crate::types::RegionScope::Item(def), crate::types::LateParamKind::Anon(v.0)));
        let mut f = crate::types::fold::BoundVarReplacer::new(late, |_| panic!("signature bound type"));
        Some(f.instantiate(&tcx.tys, sig.ret))
    }

    // ---- bodies
    /// rustc `thir_body`.
    pub fn thir_body(mut self) -> BuildResult<ThirBody> {
        let tb = self.tb();
        let (prog, ast) = (self.prog(), self.ast());
        let tcx = self.tcx();
        let def = prog.def(self.owner.owner);
        let item = ast.item(def.item);
        let item_span = self.span(item.lo, item.hi);
        let span = self.span(0, 0);
        if !tb.errors.is_empty() {
            return Err(self.error(span, UnsupportedReason::TypeckTainted));
        }
        let fn_scopes = |this: &Self, node: Node, at: Span| -> BuildResult<Option<(ScopeId, ScopeId)>> {
            let call_site = this.scope_of(node, ScopeData::CallSite);
            let arguments = this.scope_of(node, ScopeData::Arguments);
            match (call_site, arguments) {
                (Some(c), Some(a)) => Ok(Some((c, a))),
                _ => this.unresolved(at, "function region scopes"),
            }
        };
        let mut coroutine = None;
        let mut prologue: Vec<(Pat, LocalVarId)> = Vec::new();
        let (params, expr, body_ty, body_span, ret_span, scopes) = match self.owner.kind {
            OwnerKind::Function => {
                let ast::ItemKind::Fn(sig, _, _) = item.kind else { return self.unresolved(span, "function item") };
                let sig = ast.fn_sig(sig);
                let Some(body) = tb.body else { return self.unresolved(item_span, "function body") };
                if sig.asyncness {
                    // an async fn is two bodies: this one builds the body coroutine, capturing every
                    // parameter by value; the coroutine's own body is `OwnerKind::Closure` with the
                    // expression id `ASYNC_FN_BODY | body block`
                    let params = self.fn_params(sig, item.lo)?;
                    let b = ast.block(body);
                    let scopes = fn_scopes(&self, Node::Block(body.0), item_span)?;
                    let Some(&opaque) = prog.node_id_to_def_id.get(&(def.file, relax_front::def_collector::NodeId::AsyncReturn(def.item))) else { return self.unresolved(item_span, "async fn return type") };
                    let Some(&coroutine_ty) = tb.hidden_types.get(&opaque) else { return self.unresolved(item_span, "async fn coroutine type") };
                    let body_span = self.span(b.lo, b.hi);
                    let root = self.closure_aggregate_at(crate::tcx::ASYNC_FN_BODY | body.0, None, body_span, coroutine_ty, Some(crate::mir::Movability::Static))?;
                    let expr = self.push(root);
                                let ret_span = self.span(b.lo, b.lo);
                    let output = self.declared_output(self.owner.owner).unwrap_or(tb.ret);
                    return Ok(ThirBody { thir: self.thir, params, expr, body_ty: BodyTy::Fn { output, rust_call: false }, span: item_span, ret_span, coroutine: None, fn_scopes: scopes, prologue: Vec::new() });
                }
                let b = ast.block(body);
                let params = self.fn_params(sig, item.lo)?;
                let block = self.mirror_block(body)?;
                let root = Expr { kind: ExprKind::Block { block }, ty: tb.ret, temp_scope: None, bid: None, span: self.span(b.lo, b.hi) };
                let scopes = fn_scopes(&self, Node::Block(body.0), item_span)?;
                let rust_call = false;
                // the return place has the signature's declared output (an `impl Trait` stays the opaque type, which
                // the body's value defines: rustc `construct_fn` takes `fn_sig.output()`)
                let output = self.declared_output(self.owner.owner).unwrap_or(tb.ret);
                (params, self.push(root), BodyTy::Fn { output, rust_call }, item_span, self.span(b.lo, b.lo), scopes)
            }
            OwnerKind::Const | OwnerKind::Static | OwnerKind::AssocConst => {
                let init = match &item.kind {
                    ast::ItemKind::Const(_, init) | ast::ItemKind::Static(_, _, init) => init.get(),
                    _ => None,
                };
                let init = match prog.const_body(def.file, def.item) {
                    Ok(Some(e)) => e,
                    _ => match init {
                        Some(e) => e,
                        None => return self.unresolved(item_span, "initializer"),
                    },
                };
                let expr = self.mirror_expr(init)?;
                (Vec::new(), expr, BodyTy::Const(tb.ret), item_span, self.espan(init), None)
            }
            OwnerKind::AnonConst(e) => {
                let ty = self.ety(e)?;
                let expr = self.mirror_expr(e)?;
                (Vec::new(), expr, BodyTy::Const(ty), self.espan(e), self.espan(e), None)
            }
            OwnerKind::InlineConst(e) => {
                let ast::ExprKind::ConstBlock(b) = ast.expr(e).kind else { return self.unresolved(self.espan(e), "inline const block") };
                let Some(&ty) = tb.inline_const_types.get(&e.0) else { return self.unresolved(self.espan(e), "inline const type") };
                let block = self.mirror_block(b)?;
                let blk = ast.block(b);
                let root = Expr { kind: ExprKind::Block { block }, ty, temp_scope: None, bid: None, span: self.span(blk.lo, blk.hi) };
                (Vec::new(), self.push(root), BodyTy::Const(ty), self.espan(e), self.espan(e), None)
            }
            OwnerKind::Closure(ce) if ce.0 & crate::tcx::ASYNC_FN_BODY != 0 => {
                // the body coroutine of an async fn: the body block, the parameters captured
                let b = ast::BlockId(ce.0 & !crate::tcx::ASYNC_FN_BODY);
                let blk = ast.block(b);
                let body_span = self.span(blk.lo, blk.hi);
                let scopes = fn_scopes(&self, Node::Block(b.0), item_span)?;
                let Some(&opaque) = prog.node_id_to_def_id.get(&(def.file, relax_front::def_collector::NodeId::AsyncReturn(def.item))) else { return self.unresolved(item_span, "async fn return type") };
                let Some(&coroutine_ty) = tb.hidden_types.get(&opaque) else { return self.unresolved(item_span, "async fn coroutine type") };
                let TyKind::Closure(_, _, closure_args) = *tcx.tys.kind(coroutine_ty) else { return self.unresolved(item_span, "coroutine type") };
                let TyKind::FnPtr(_, output, _, _) = *tcx.tys.kind(closure_args.closure_sig_as_fn_ptr_ty) else { return self.unresolved(item_span, "coroutine signature") };
                // the parameters whose pattern the coroutine matches itself, against the whole captured argument
                if let ast::ItemKind::Fn(sig, _, _) = item.kind {
                    for p in ast.list(ast.fn_sig(sig).params) {
                        let Some(&whole) = tb.pat_local.get(&(p.pat.0 | crate::typeck::ASYNC_ARG_KEY)) else { continue };
                        self.pat_hint = item.lo;
                        let pat = self.lower_pattern(p.pat)?;
                        prologue.push((pat, LocalVarId(whole)));
                    }
                }
                let resume_ty = self.resume_ty(item_span)?;
                let env = self.thir.alloc_param(Param { pat: None, ty: coroutine_ty, ty_span: None, self_kind: None, node: None });
                let task_context = self.synthetic_binding(scopes.expect("scopes").1);
                self.task_context = Some(task_context);
                let pat = self.binding_pat(task_context, "_task_context", resume_ty, true, item_span);
                let resume = self.thir.alloc_param(Param { pat: Some(Box::new(pat)), ty: resume_ty, ty_span: None, self_kind: None, node: None });
                let block = self.mirror_block(b)?;
                let root = Expr { kind: ExprKind::Block { block }, ty: output, temp_scope: None, bid: None, span: body_span };
                let kind = crate::mir::CoroutineKind::Desugared(crate::mir::CoroutineDesugaring::Async, crate::mir::CoroutineSource::Fn);
                coroutine = Some(CoroutineThir { kind, yield_ty: tcx.tys.unit, resume_ty });
                (vec![env, resume], self.push(root), BodyTy::Fn { output, rust_call: false }, item_span, self.span(blk.lo, blk.lo), scopes)
            }
            OwnerKind::Closure(ce) => {
                if let ast::ExprKind::Async(_, b) = ast.expr(ce).kind {
                    // an `async` block: the coroutine takes itself and the resume argument `_task_context`
                    let ce_span = self.espan(ce);
                    let scopes = fn_scopes(&self, Node::Block(b.0), ce_span)?;
                    let coroutine_ty = self.ety(ce)?;
                    let TyKind::Closure(_, _, closure_args) = *tcx.tys.kind(coroutine_ty) else { return self.unresolved(ce_span, "coroutine type") };
                    let TyKind::FnPtr(_, output, _, _) = *tcx.tys.kind(closure_args.closure_sig_as_fn_ptr_ty) else { return self.unresolved(ce_span, "coroutine signature") };
                    let resume_ty = self.resume_ty(ce_span)?;
                    let env = self.thir.alloc_param(Param { pat: None, ty: coroutine_ty, ty_span: None, self_kind: None, node: None });
                    let task_context = self.synthetic_binding(scopes.expect("scopes").1);
                    self.task_context = Some(task_context);
                    let pat = self.binding_pat(task_context, "_task_context", resume_ty, true, ce_span);
                    let resume = self.thir.alloc_param(Param { pat: Some(Box::new(pat)), ty: resume_ty, ty_span: None, self_kind: None, node: None });
                    let block = self.mirror_block(b)?;
                    let blk = ast.block(b);
                    let root = Expr { kind: ExprKind::Block { block }, ty: output, temp_scope: None, bid: None, span: self.span(blk.lo, blk.hi) };
                    let kind = crate::mir::CoroutineKind::Desugared(crate::mir::CoroutineDesugaring::Async, crate::mir::CoroutineSource::Block);
                    coroutine = Some(CoroutineThir { kind, yield_ty: tcx.tys.unit, resume_ty });
                    (vec![env, resume], self.push(root), BodyTy::Fn { output, rust_call: false }, ce_span, self.span(blk.lo, blk.lo), scopes)
                } else {
                let ast::ExprKind::Closure(row) = ast.expr(ce).kind else { return self.not_built(self.espan(ce), "coroutine body") };
                let row = *ast.row(row);
                let ce_span = self.espan(ce);
                let Some(sigs) = tb.closure_sigs.get(&ce.0) else { return self.unresolved(ce_span, "closure signature") };
                let TyKind::FnPtr(inputs, output, _, _) = *tcx.tys.kind(sigs.liberated) else { return self.unresolved(ce_span, "closure signature type") };
                let inputs: Vec<TyId> = tcx.tys.list(inputs).to_vec();
                let mut params = vec![self.closure_env_param(ce)?];
                for (i, p) in ast.list(row.params).iter().enumerate() {
                    self.pat_hint = ce_span.lo;
                    let pat = self.lower_pattern(p.pat)?;
                    let ty = inputs.get(i).copied().unwrap_or(pat.ty);
                    params.push(self.thir.alloc_param(Param { pat: Some(Box::new(pat)), ty, ty_span: None, self_kind: None, node: Some(p.pat.0) }));
                }
                let expr = self.mirror_expr(row.body)?;
                let scopes = fn_scopes(&self, Node::Expr(row.body.0), ce_span)?;
                (params, expr, BodyTy::Fn { output, rust_call: false }, ce_span, self.espan(row.body), scopes)
                }
            }
        };
        Ok(ThirBody { thir: self.thir, params, expr, body_ty, span: body_span, ret_span, coroutine, fn_scopes: scopes, prologue })
    }

    /// The method `name` of the lang-item trait `item`.
    pub(super) fn trait_method(&self, item: crate::lang_items::LangItem, name: &str) -> Option<DefId> {
        let (tcx, prog) = (self.tcx(), self.prog());
        let td = tcx.lang_items.get(item)?;
        prog.traits[prog.def(td).sub as usize].iter().copied().find(|&it| prog.name(it) == name)
    }

    /// `core::future::ResumeTy`, the resume argument type of an async coroutine.
    pub(super) fn resume_ty(&self, span: Span) -> BuildResult<TyId> {
        let tcx = self.tcx();
        let Some(def) = tcx.lang_items.get(crate::lang_items::LangItem::ResumeTy) else { return self.unresolved(span, "ResumeTy lang item") };
        Ok(tcx.tys.intern(TyKind::Adt(def, tcx.tys.mk_list(&[]), RegionArgsId::EMPTY)))
    }

    /// The env parameter of a closure body: the closure by reference or by value, per its kind.
    fn closure_env_param(&mut self, ce: ast::ExprId) -> BuildResult<ParamId> {
        let ty = self.ety(ce)?;
        // rustc: the closure type's kind (fixed from the expected `F: FnOnce` bound when there is one),
        // else what capture analysis inferred
        let kind = match *self.tcx().tys.kind(ty) {
            TyKind::Closure(_, _, args) => self.tcx().tys.to_opt_closure_kind(args.kind_ty()),
            _ => None,
        }
        .or_else(|| self.tb().min_captures.get(&ce.0).map(|c| c.kind));
        let env = match kind {
            Some(crate::upvar::ClosureKind::Fn) => self.ref_ty(false, ty),
            Some(crate::upvar::ClosureKind::FnMut) => self.ref_ty(true, ty),
            Some(crate::upvar::ClosureKind::FnOnce) => ty,
            None => return self.unresolved(self.espan(ce), "closure kind"),
        };
        Ok(self.thir.alloc_param(Param { pat: None, ty: env, ty_span: None, self_kind: None, node: None }))
    }

    /// The parameters of a function: `self` (if any) and the declared patterns.
    fn fn_params(&mut self, sig: &ast::FnSig, hint: u32) -> BuildResult<Vec<ParamId>> {
        let (tb, ast) = (self.tb(), self.ast());
        let mut out = Vec::new();
        let mut next_param = 0usize;
        if let Some(self_param) = sig.self_param {
            let Some(index) = tb.locals.iter().position(|l| l.pat == u32::MAX) else {
                return self.unresolved(self.span(0, 0), "self parameter local");
            };
            let local = &tb.locals[index];
            let span = self.span(0, 0);
            let mut pat = self.binding_pat(LocalVarId(index as u32), "self", local.ty, local.mutable, span);
            // `self`'s name is the local's
            if let PatKind::Binding { name, .. } = &mut pat.kind {
                *name = local.name;
            }
            let self_kind = match self_param {
                ast::SelfParam::Value(false) => Some(crate::mir::ImplicitSelfKind::Imm),
                ast::SelfParam::Value(true) => Some(crate::mir::ImplicitSelfKind::Mut),
                ast::SelfParam::Ref(_, false) => Some(crate::mir::ImplicitSelfKind::RefImm),
                ast::SelfParam::Ref(_, true) => Some(crate::mir::ImplicitSelfKind::RefMut),
                ast::SelfParam::Typed(..) => None,
            };
            let ty = pat.ty;
            out.push(self.thir.alloc_param(Param { pat: Some(Box::new(pat)), ty, ty_span: None, self_kind, node: None }));
            next_param = 1;
        }
        let _ = next_param;
        for p in ast.list(sig.params) {
            self.pat_hint = hint;
            // an async fn's coroutine matches this pattern: the parameter is the whole argument
            let pat = match tb.pat_local.get(&(p.pat.0 | crate::typeck::ASYNC_ARG_KEY)) {
                Some(&whole) => {
                    let local = &tb.locals[whole as usize];
                    let span = self.span(0, 0);
                    self.binding_pat(LocalVarId(whole), "_", local.ty, true, span)
                }
                None => self.lower_pattern(p.pat)?,
            };
            let ty = pat.ty;
            out.push(self.thir.alloc_param(Param { pat: Some(Box::new(pat)), ty, ty_span: None, self_kind: None, node: Some(p.pat.0) }));
        }
        Ok(out)
    }

    pub(super) fn is_upvar(&self, local: u32) -> bool {
        match self.owner.kind {
            // an async fn's coroutine captures its parameters; `closure_locals` lists every local of the fn
            OwnerKind::Closure(ce) if ce.0 & crate::tcx::ASYNC_FN_BODY != 0 => {
                self.tb().min_captures.get(&ce.0).is_some_and(|c| c.captures.iter().any(|capture| capture.place.root == local))
            }
            OwnerKind::Closure(ce) => self.tb().closure_locals.get(&ce.0).is_some_and(|caps| caps.contains(&local)),
            _ => false,
        }
    }

    /// The closure id and aggregate arguments of a closure expression's type: the parent's identity
    /// arguments (types, consts, then region slots) followed by `[kind, sig, tupled upvars]`.
    pub(super) fn closure_parts(&self, ty: TyId, span: Span) -> BuildResult<(crate::mir::ClosureId, Vec<GenericArg>, ClosureExprKind, TyId)> {
        let tcx = self.tcx();
        let TyKind::Closure(file, ce, args) = *tcx.tys.kind(ty) else { return self.unresolved(span, "closure type") };
        let TyKind::FnDef(owner, parent, regions) = *tcx.tys.kind(args.parent_args) else { return self.unresolved(span, "closure parent arguments") };
        // The owner's arguments in declaration order (lifetimes and types interleaved), as every consumer
        // of the closure's arguments instantiates them.
        let mut out = tcx.complete_args(self.root.cx.prog, owner, tcx.tys.list(parent), regions);
        out.push(GenericArg::Type(args.closure_kind_ty));
        out.push(GenericArg::Type(args.closure_sig_as_fn_ptr_ty));
        out.push(GenericArg::Type(args.tupled_upvars_ty));
        let kind = if tcx.is_coroutine(ty) { ClosureExprKind::Coroutine } else { ClosureExprKind::Closure };
        Ok((crate::mir::ClosureId { file, expr: ce }, out, kind, args.tupled_upvars_ty))
    }
}

#[allow(dead_code)]
fn _unused(_: (ArmId, Arm, Block, BlockId, BlockSafety, ClosureExpr, Stmt, StmtId, StmtKind, DefKind)) {}

/// `e` without its parentheses.
pub(super) fn strip_paren(ast: &ast::Ast, mut e: ast::ExprId) -> ast::ExprId {
    while let ast::ExprKind::Paren(x) = &ast.expr(e).kind {
        e = *x;
    }
    e
}
