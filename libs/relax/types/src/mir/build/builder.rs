//! rustc_mir_build/src/builder/mod.rs: the `Builder`, its state and local/temporary bookkeeping.
use super::scope::Scopes;
use super::place::{PlaceBase, PlaceBuilder};
use super::scope::DropKind;
use super::thir::{BodyTy as BodyKind, LocalVarId, PatKind, ThirBody};
use super::{BuildError, BuildResult, BuildRoot, Cfg, UnsupportedReason};
use super::cfg::BlockAndExtension;
use crate::borrowck::body::{BodyKind as OwnerKind, CheckedBody};
use crate::expr_use::{self, Projection};
use crate::mir::{BasicBlock, BindingForm, BindingMode, BodyKind as MirBodyKind, ByRef, FieldIdx, MirPhase, PlaceElem, Terminator, TerminatorKind, UpvarBorrowKind, UpvarCapture, VarBindingForm, VarBindingIntroduction, VariantIdx};
use crate::types::TyKind;
use crate::upvar::CaptureKind;
use super::cfg::BlockAnd;
use crate::fxhash::HashMap;
use crate::mir::{
    AllocMap, Body, BlockTailInfo, CanonicalUserTypeAnnotation, CapturedPlace, CoroutineInfo, Local, LocalDecl, LocalInfo, LocalKind,
    MirSource, Mutability, Place, SourceInfo, SourceScope, SourceScopeData, Span, VarDebugInfo,
};
use crate::region_scope::ScopeId;
use crate::types::TyId;

/// builder/mod.rs:`LocalsForNode`: a binding has one local, or, when bound in a guarded arm, a reference
/// local for the guard and a by-value/by-ref local for the arm body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalsForNode {
    One(Local),
    ForGuard { ref_for_guard: Local, for_arm_body: Local },
}
/// builder/mod.rs:`GuardFrameLocal` / `GuardFrame`: the bindings of the arm whose guard is being lowered.
#[derive(Clone, Debug)]
pub struct GuardFrameLocal {
    pub id: LocalVarId,
}
#[derive(Clone, Debug)]
pub struct GuardFrame {
    pub locals: Vec<GuardFrameLocal>,
}
/// builder/mod.rs:`ForGuard`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForGuard {
    RefWithinGuard,
    OutsideGuard,
}
/// builder/mod.rs:`BlockFrame`: what the innermost block being lowered is for (statement-expression
/// results, tail expressions and sub-expressions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockFrame {
    /// Evaluating a statement; the result is ignored when `ignores_expr_result`.
    Statement { ignores_expr_result: bool },
    /// Evaluating the tail expression of a block.
    TailExpr { info: BlockTailInfo },
    /// Generic sub-expression.
    SubExpr,
}
/// How a closure body reaches its captures: the environment local `_1`, a reference when the closure is
/// `Fn`/`FnMut`.
#[derive(Clone, Copy, Debug)]
pub struct UpvarEnv {
    pub local: Local,
    pub by_ref: bool,
}
/// builder/mod.rs: the lifetime a temporary lives for (`temp_lifetime`, and the Edition 2024 rescope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TempLifetime {
    pub temp_lifetime: Option<ScopeId>,
    pub backwards_incompatible: Option<ScopeId>,
}

pub struct Builder<'a> {
    pub root: &'a BuildRoot<'a>,
    pub thir: &'a ThirBody,
    pub owner: CheckedBody,
    pub source: MirSource,
    pub body_span: Span,
    pub arg_count: usize,
    pub coroutine: Option<CoroutineInfo>,
    pub cfg: Cfg,
    pub scopes: Scopes,
    pub source_scopes: Vec<SourceScopeData>,
    pub source_scope: SourceScope,
    pub guard_context: Vec<GuardFrame>,
    pub block_context: Vec<BlockFrame>,
    /// Temporaries created for an expression evaluated twice (match guards), keyed by THIR expr.
    pub fixed_temps: HashMap<super::thir::ExprId, Local>,
    pub fixed_temps_scope: Option<ScopeId>,
    pub var_indices: HashMap<LocalVarId, LocalsForNode>,
    pub local_decls: Vec<LocalDecl>,
    pub canonical_user_type_annotations: Vec<CanonicalUserTypeAnnotation>,
    pub upvars: Vec<CapturedPlace>,
    pub upvar_env: Option<UpvarEnv>,
    pub var_debug_info: Vec<VarDebugInfo>,
    pub unit_temp: Option<Place>,
    pub allocs: AllocMap,
}

impl<'a> Builder<'a> {
    /// The body of `owner` from its THIR (rustc `construct_fn` / `construct_const`).
    pub fn build(root: &'a BuildRoot<'a>, owner: CheckedBody, thir: &'a ThirBody) -> BuildResult<Body> {
        let (return_ty, arg_count) = match &thir.body_ty {
            BodyKind::Const(ty) => (*ty, 0),
            BodyKind::Fn { output, .. } => (*output, thir.params.len()),
        };
        let mut builder = Self::new(root, owner, thir, arg_count, return_ty);
        match &thir.body_ty {
            BodyKind::Const(_) => builder.construct_const()?,
            BodyKind::Fn { .. } => builder.construct_fn()?,
        }
        builder.lint_and_remove_uninhabited();
        let mut body = builder.finish()?;
        if let BodyKind::Fn { rust_call: true, .. } = thir.body_ty {
            body.spread_arg = Some(Local(thir.params.len() as u32));
        }
        Ok(body)
    }

    fn new(root: &'a BuildRoot<'a>, owner: CheckedBody, thir: &'a ThirBody, arg_count: usize, return_ty: TyId) -> Self {
        let kind = match owner.kind {
            OwnerKind::Function => MirBodyKind::Fn,
            OwnerKind::Const | OwnerKind::AssocConst => MirBodyKind::Const,
            OwnerKind::Static => MirBodyKind::Static,
            OwnerKind::InlineConst(_) => MirBodyKind::InlineConst,
            OwnerKind::AnonConst(_) => MirBodyKind::AnonConst,
            OwnerKind::Closure(e) => MirBodyKind::Closure(e.0),
        };
        let coroutine = thir.coroutine.as_ref().map(|c| CoroutineInfo { yield_ty: c.yield_ty, resume_ty: c.resume_ty, kind: c.kind });
        let mut builder = Builder {
            root,
            thir,
            owner,
            source: MirSource { owner: owner.owner, kind },
            body_span: thir.span,
            arg_count,
            coroutine,
            cfg: Cfg::default(),
            scopes: Scopes::new(),
            source_scopes: Vec::new(),
            source_scope: SourceScope(0),
            guard_context: Vec::new(),
            block_context: Vec::new(),
            fixed_temps: HashMap::default(),
            fixed_temps_scope: None,
            var_indices: HashMap::default(),
            local_decls: Vec::new(),
            canonical_user_type_annotations: Vec::new(),
            upvars: Vec::new(),
            upvar_env: None,
            var_debug_info: Vec::new(),
            unit_temp: None,
            allocs: AllocMap::default(),
        };
        builder.local_decls.push(LocalDecl {
            ty: return_ty,
            mutability: Mutability::Mut,
            user_ty: Vec::new(),
            source_info: SourceInfo { span: thir.ret_span, scope: SourceScope(0) },
            kind: LocalKind::Return,
            local_info: LocalInfo::Boring,
        });
        let start = builder.cfg.start_new_block();
        debug_assert_eq!(start, BasicBlock(0));
        let scope = builder.new_source_scope(thir.span, None);
        debug_assert_eq!(scope, SourceScope(0));
        builder.source_scopes[0].parent_scope = None;
        builder
    }

    /// builder/mod.rs:`construct_fn`.
    fn construct_fn(&mut self) -> BuildResult<()> {
        let thir = self.thir;
        let Some((call_site_scope, arg_scope)) = thir.fn_scopes else {
            return Err(self.error(thir.span, UnsupportedReason::UnresolvedFact("fn body without call-site/argument scopes")));
        };
        let source_info = self.source_info(thir.span);
        let call_site_s = (call_site_scope, source_info);
        let fn_end = Span { file: thir.span.file, lo: thir.span.hi, hi: thir.span.hi };
        let _: BlockAnd<()> = self.in_scope(call_site_s, |this| {
            let arg_scope_s = (arg_scope, source_info);
            let return_block = this
                .in_breakable_scope(None, Place::local(Local(0)), fn_end, |this| {
                    Ok(Some(this.in_scope(arg_scope_s, |this| this.args_and_body(BasicBlock(0), arg_scope))?))
                })?
                .into_block();
            let source_info = this.source_info(fn_end);
            this.cfg.terminate(return_block, source_info, TerminatorKind::Return);
            this.build_drop_trees()?;
            Ok(return_block.unit())
        })?;
        Ok(())
    }

    /// builder/mod.rs:`construct_const`.
    fn construct_const(&mut self) -> BuildResult<()> {
        let thir = self.thir;
        let block = self.expr_into_dest(Place::local(Local(0)), BasicBlock(0), thir.expr)?.into_block();
        let source_info = self.source_info(thir.span);
        self.cfg.terminate(block, source_info, TerminatorKind::Return);
        self.build_drop_trees()
    }

    /// builder/mod.rs:`args_and_body`: declare the arguments, bind their patterns, lower the body.
    fn args_and_body(&mut self, mut block: BasicBlock, argument_scope: ScopeId) -> BuildResult<BlockAnd<()>> {
        let thir = self.thir;
        let expr_span = thir.thir[thir.expr].span;
        // Allocate locals for the function arguments.
        for param_id in &thir.params {
            let param = &thir.thir[*param_id];
            let span = param.pat.as_ref().map_or(thir.span, |pat| pat.span);
            let source_info = SourceInfo { span, scope: SourceScope(0) };
            // Declare the argument and its debuginfo.
            let arg_local = Local(self.local_decls.len() as u32);
            self.local_decls.push(LocalDecl {
                ty: param.ty,
                mutability: Mutability::Mut,
                user_ty: Vec::new(),
                source_info,
                kind: LocalKind::Arg,
                local_info: LocalInfo::Boring,
            });
            if let Some(name) = param.pat.as_ref().and_then(|pat| pat.simple_ident()) {
                self.var_debug_info.push(VarDebugInfo { name: self.root.cx.prog.syms.str(name).to_string(), source_info, place: Place::local(arg_local) });
            }
        }

        self.insert_upvar_arg()?;

        let mut scope = None;
        // Bind the argument patterns.
        for (index, param_id) in thir.params.iter().enumerate() {
            let param = &thir.thir[*param_id];
            // Function arguments always get the first Local indices after the return place.
            let local = Local(index as u32 + 1);
            let place = Place::local(local);

            // Make sure we drop (parts of) the argument even when not matched on.
            self.schedule_drop(param.pat.as_ref().map_or(expr_span, |pat| pat.span), argument_scope, local, DropKind::Value)?;

            let Some(pat) = param.pat.as_ref() else {
                continue;
            };
            let original_source_scope = self.source_scope;
            let span = pat.span;
            match &pat.kind {
                // Don't introduce extra copies for simple bindings.
                PatKind::Binding { var, mode: BindingMode(ByRef::No, mutability), subpattern: None, .. } => {
                    let decl = &mut self.local_decls[local.0 as usize];
                    decl.mutability = *mutability;
                    decl.source_info.scope = self.source_scope;
                    decl.local_info = if let Some(kind) = param.self_kind {
                        LocalInfo::User(BindingForm::ImplicitSelf(kind))
                    } else {
                        LocalInfo::User(BindingForm::Var(VarBindingForm {
                            binding_mode: BindingMode(ByRef::No, *mutability),
                            opt_ty_info: param.ty_span,
                            opt_match_place: Some((None, span)),
                            pat_span: span,
                            introductions: vec![VarBindingIntroduction { span, is_shorthand: false }],
                        }))
                    };
                    self.var_indices.insert(*var, LocalsForNode::One(local));
                }
                _ => {
                    scope = self.declare_bindings(scope, expr_span, pat, None, Some((Some(&place), span)))?;
                    let place_builder = PlaceBuilder::from(local);
                    block = self.place_into_pattern(block, pat, place_builder, false)?.into_block();
                }
            }
            self.source_scope = original_source_scope;
        }

        // The patterns an async fn's coroutine matches against its captured arguments.
        for (pat, var) in &thir.prologue {
            scope = self.declare_bindings(scope, expr_span, pat, None, Some((None, pat.span)))?;
            let place_builder = PlaceBuilder::from(PlaceBase::Upvar { var: *var });
            block = self.place_into_pattern(block, pat, place_builder, false)?.into_block();
        }

        // Enter the argument pattern bindings source scope, if it exists.
        if let Some(source_scope) = scope {
            self.source_scope = source_scope;
        }

        self.expr_into_dest(Place::local(Local(0)), block, thir.expr)
    }

    /// builder/mod.rs:`insert_upvar_arg`: name the captures of a closure/coroutine body (`_1`'s fields).
    fn insert_upvar_arg(&mut self) -> BuildResult<()> {
        let OwnerKind::Closure(closure) = self.owner.kind else { return Ok(()) };
        let Some(closure_arg) = self.local_decls.get(1) else { return Ok(()) };
        let tys = &self.root.cx.tcx.tys;
        let mut closure_ty = closure_arg.ty;
        let mut by_ref = false;
        if let TyKind::Ref(_, inner, _) = *tys.kind(closure_ty) {
            by_ref = true;
            closure_ty = inner;
        }
        let TyKind::Closure(_, _, args) = *tys.kind(closure_ty) else { return Ok(()) };
        let TyKind::Tuple(capture_tys) = *tys.kind(args.tupled_upvars_ty) else {
            return Err(self.error(self.body_span, UnsupportedReason::UnresolvedFact("closure upvar types are not a tuple")));
        };
        let capture_tys = tys.list(capture_tys).to_vec();
        let Some(captures) = self.root.typeck.min_captures.get(&closure.0) else {
            return Err(self.error(self.body_span, UnsupportedReason::UnresolvedFact("closure without captured places")));
        };
        // An async closure is two bodies: the closure (whose environment holds `captures.captures`; its body
        // just creates the coroutine) and the coroutine, an ordinary coroutine body with its own entry in
        // `min_captures` (the closure's by-ref upvars and the parameters). `captures.coroutine` only
        // describes the AsyncFnOnce by-move shim, which is not built from THIR.
        if captures.captures.len() != capture_tys.len() {
            return Err(self.error(self.body_span, UnsupportedReason::UnresolvedFact("capture count disagrees with the closure's upvar tuple")));
        }
        let env = Local(1);
        self.upvar_env = Some(UpvarEnv { local: env, by_ref });
        let mut upvars = Vec::with_capacity(capture_tys.len());
        for (i, (capture, field_ty)) in captures.captures.iter().zip(&capture_tys).enumerate() {
            let place = &capture.place;
            let cp = self.captured_place(capture, closure)?;
            // The use place of the capture in this body: `(*_1).i` (and `*` when captured by reference).
            let mut projection = if by_ref { vec![PlaceElem::Deref] } else { Vec::new() };
            projection.push(PlaceElem::Field(FieldIdx(i as u32), *field_ty));
            if cp.is_by_ref() {
                projection.push(PlaceElem::Deref);
            }
            self.var_debug_info.push(VarDebugInfo {
                name: cp.name.clone(),
                source_info: SourceInfo { span: Span { file: self.body_span.file, lo: place.span.lo, hi: place.span.hi }, scope: SourceScope(0) },
                place: Place { local: env, projection },
            });
            upvars.push(cp);
        }
        self.upvars = upvars;
        Ok(())
    }

    /// rustc `CapturedPlace` from typeck's minimum capture: the root variable, the Deref/Field path (with a
    /// Downcast before the field of an enum), the captured type, how it is captured and whether it is mutable.
    fn captured_place(&self, capture: &crate::upvar::Capture, _closure: crate::ast::ExprId) -> BuildResult<CapturedPlace> {
        let tcx = self.root.cx.tcx;
        let place = &capture.place;
        let local = &self.root.typeck.locals[place.root as usize];
        let mut projection = Vec::new();
        let mut before = place.root_ty;
        // upvar.rs determine_capture_mutability.
        let mut is_mutbl = local.mutable;
        let mut mutability_known = false;
        for (proj, ty) in &place.projections {
            match proj {
                Projection::Deref => {
                    if !mutability_known {
                        match tcx.tys.kind(before) {
                            TyKind::Ref(true, ..) => is_mutbl = true,
                            TyKind::Ref(false, ..) => {
                                is_mutbl = false;
                                mutability_known = true;
                            }
                            _ => {} // Box
                        }
                    }
                    projection.push(PlaceElem::Deref);
                }
                Projection::Field(variant, field) => {
                    if let TyKind::Adt(def, ..) = tcx.tys.kind(before) {
                        let adt = &tcx.adts[def];
                        if adt.is_enum() {
                            projection.push(PlaceElem::Downcast(Some(adt.variants[*variant as usize].name), VariantIdx(*variant)));
                        }
                    }
                    projection.push(PlaceElem::Field(FieldIdx(*field), *ty));
                }
                Projection::Index | Projection::Subslice => {
                    return Err(self.error(self.body_span, UnsupportedReason::Internal("capture path contains an index projection")));
                }
            }
            before = *ty;
        }
        let capture_kind = match capture.info.kind {
            CaptureKind::ByValue => UpvarCapture::ByValue,
            CaptureKind::ByRef(kind) => UpvarCapture::ByRef(match kind {
                expr_use::BorrowKind::Imm => UpvarBorrowKind::Immutable,
                expr_use::BorrowKind::UniqueImm => UpvarBorrowKind::UniqueImmutable,
                expr_use::BorrowKind::Mut => UpvarBorrowKind::Mutable,
            }),
        };
        let file = self.body_span.file;
        Ok(CapturedPlace {
            name: self.root.cx.prog.syms.str(local.name).to_string(),
            var: place.root,
            root_ty: place.root_ty,
            projection,
            ty: place.ty(),
            capture: capture_kind,
            mutability: if is_mutbl { Mutability::Mut } else { Mutability::Not },
            path_span: Span { file, lo: capture.info.path_span.lo, hi: capture.info.path_span.hi },
            kind_span: Span { file, lo: capture.info.kind_span.lo, hi: capture.info.kind_span.hi },
        })
    }

    /// builder/mod.rs:`lint_and_remove_uninhabited` (the removal half): a call whose destination type is
    /// uninhabited does not return. Only `!` is recognised here (rustc also removes calls returning empty enums
    /// and the like, which needs the module-aware inhabitedness query); the lint is not ported.
    fn lint_and_remove_uninhabited(&mut self) {
        let never = self.root.cx.tcx.tys.never;
        let view = self.local_decls_view();
        for index in 0..self.cfg.basic_blocks.len() {
            let Some(term) = self.cfg.basic_blocks[index].terminator.as_ref() else { continue };
            let TerminatorKind::Call { destination, target: Some(_), .. } = &term.kind else { continue };
            let Some(ty) = destination.ty(&view, &self.root.cx.tcx).map(|t| t.ty) else { continue };
            if ty == never {
                if let Some(Terminator { kind: TerminatorKind::Call { target, .. }, .. }) = self.cfg.basic_blocks[index].terminator.as_mut() {
                    *target = None;
                }
            }
        }
    }

    fn local_decls_view(&self) -> Body {
        // Place typing only reads `local_decls`; a view body avoids a second place-typing implementation.
        Body {
            phase: MirPhase::Built,
            source: self.source.clone(),
            basic_blocks: Vec::new(),
            local_decls: self.local_decls.clone(),
            source_scopes: Vec::new(),
            span: self.body_span,
            user_type_annotations: Vec::new(),
            upvars: Vec::new(),
            allocs: AllocMap::default(),
            var_debug_info: Vec::new(),
            arg_count: self.arg_count as u32,
            spread_arg: None,
            promoted: Vec::new(),
            coroutine: None,
        }
    }

    /// builder/mod.rs:`finish`.
    fn finish(self) -> BuildResult<Body> {
        for (index, block) in self.cfg.basic_blocks.iter().enumerate() {
            if block.terminator.is_none() {
                let _ = index;
                return Err(self.error(self.body_span, UnsupportedReason::Internal("no terminator on a block")));
            }
        }
        Ok(Body {
            phase: MirPhase::Built,
            source: self.source,
            basic_blocks: self.cfg.basic_blocks,
            local_decls: self.local_decls,
            source_scopes: self.source_scopes,
            span: self.body_span,
            user_type_annotations: self.canonical_user_type_annotations,
            upvars: self.upvars,
            allocs: self.allocs,
            var_debug_info: self.var_debug_info,
            arg_count: self.arg_count as u32,
            spread_arg: None,
            promoted: Vec::new(),
            coroutine: self.coroutine,
        })
    }

    /// builder/mod.rs:`BlockContext::currently_in_block_tail`.
    pub fn currently_in_block_tail(&self) -> Option<BlockTailInfo> {
        for frame in self.block_context.iter().rev() {
            match frame {
                BlockFrame::SubExpr => continue,
                BlockFrame::Statement { .. } => break,
                &BlockFrame::TailExpr { info } => return Some(info),
            }
        }
        None
    }
    /// builder/mod.rs:`BlockContext::currently_ignores_tail_results`: whether the block result itself is
    /// discarded (does not skip `SubExpr` frames).
    pub fn currently_ignores_tail_results(&self) -> bool {
        match self.block_context.last() {
            None | Some(BlockFrame::SubExpr) => false,
            Some(BlockFrame::TailExpr { info: BlockTailInfo { tail_result_is_ignored, .. } }) => *tail_result_is_ignored,
            Some(BlockFrame::Statement { ignores_expr_result }) => *ignores_expr_result,
        }
    }
    pub fn error(&self, span: Span, reason: UnsupportedReason) -> BuildError {
        BuildError::new(self.owner, span, reason)
    }
    pub fn source_info(&self, span: Span) -> SourceInfo {
        SourceInfo { span, scope: self.source_scope }
    }
    /// builder/mod.rs:`new_source_scope`.
    pub fn new_source_scope(&mut self, span: Span, parent: Option<SourceScope>) -> SourceScope {
        self.source_scopes.push(SourceScopeData { span, parent_scope: parent.or(Some(self.source_scope)) });
        SourceScope(self.source_scopes.len() as u32 - 1)
    }
    /// builder/mod.rs:`temp`: a fresh temporary local.
    pub fn temp(&mut self, ty: TyId, span: Span) -> Place {
        Place::local(self.new_local(ty, Mutability::Mut, LocalKind::Temp, LocalInfo::Boring, span))
    }
    pub fn new_local(&mut self, ty: TyId, mutability: Mutability, kind: LocalKind, local_info: LocalInfo, span: Span) -> Local {
        let source_info = self.source_info(span);
        self.local_decls.push(LocalDecl { ty, mutability, user_ty: Vec::new(), source_info, kind, local_info });
        Local(self.local_decls.len() as u32 - 1)
    }
    /// builder/matches/mod.rs `var_local_id`: the local of a binding as seen inside/outside its guard.
    pub fn var_local_id(&self, id: LocalVarId, for_guard: ForGuard) -> Local {
        match self.var_indices[&id] {
            LocalsForNode::One(local) => local,
            LocalsForNode::ForGuard { ref_for_guard, for_arm_body } => match for_guard {
                ForGuard::RefWithinGuard => ref_for_guard,
                ForGuard::OutsideGuard => for_arm_body,
            },
        }
    }
    /// builder/mod.rs:`is_bound_var_in_guard`.
    pub fn is_bound_var_in_guard(&self, id: LocalVarId) -> bool {
        self.guard_context.iter().any(|frame| frame.locals.iter().any(|local| local.id == id))
    }
}
