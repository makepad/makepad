//! Port of rustc_mir_build/src/builder/scope.rs (1.97.1): the scope stack, scheduled drops, drop trees,
//! early exits (break/continue/return), unwind and coroutine-drop paths.
//!
//! Deviations (all unstable-only): no const-continuable scopes (`#[loop_match]`), no tail-call drops
//! (`become`), no async drop (`Drop.drop`/`async_fut`), and no lint-level source scopes (borrowck does not
//! read them). The coroutine drop tree exists only for `Yield` edges.
use super::builder::{Builder, BlockFrame};
use super::cfg::{BlockAnd, BlockAndExtension, Cfg};
use super::thir::ExprId;
use super::{BuildResult, UnsupportedReason};
use crate::fxhash::HashMap;
use crate::mir::{
    BackwardIncompatibleDropReason, BasicBlock, Local, Place, Rvalue, SourceInfo, SourceScope, Span, Statement, StatementKind,
    TerminatorKind, UnwindAction, UnwindTerminateReason, AssertKind, Operand,
};
use crate::region_scope::{ScopeData, ScopeId};

/// Index of a node of a [`DropTree`]; `ROOT_NODE` is the exit.
pub type DropIdx = usize;
pub const ROOT_NODE: DropIdx = 0;
/// rustc `DropIdx::MAX`: "no unwind target" (never dereferenced).
pub const NO_DROP: DropIdx = usize::MAX;

#[derive(Debug)]
pub struct Scopes {
    pub scopes: Vec<Scope>,
    /// The current set of breakable scopes. See module comment for more details.
    pub breakable_scopes: Vec<BreakableScope>,
    /// The scope of the innermost `if` whose condition is being lowered (`break_for_else` target).
    pub if_then_scope: Option<IfThenScope>,
    /// Drops that need to be done on unwind paths.
    pub unwind_drops: DropTree,
    /// Drops that need to be done on paths to the `CoroutineDrop` terminator.
    pub coroutine_drops: DropTree,
}

#[derive(Debug)]
pub struct Scope {
    /// The source scope this scope was created in.
    pub source_scope: SourceScope,
    /// the region span of this scope within source code.
    pub region_scope: ScopeId,
    /// Drops that need to run when the scope ends, in schedule order (executed in reverse).
    pub drops: Vec<DropData>,
    pub moved_locals: Vec<Local>,
    /// The drop index that will drop everything in and below this scope on an unwind path.
    pub cached_unwind_block: Option<DropIdx>,
    /// The drop index that will drop everything in and below this scope on a coroutine drop path.
    pub cached_coroutine_drop_block: Option<DropIdx>,
}

#[derive(Clone, Copy, Debug)]
pub struct DropData {
    /// The `Span` where drop obligation was incurred (typically where the place was declared).
    pub source_info: SourceInfo,
    /// The local to drop.
    pub local: Local,
    /// Whether this is a value Drop or a StorageDead.
    pub kind: DropKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropKind {
    Value,
    Storage,
    ForLint,
}

#[derive(Debug)]
pub struct BreakableScope {
    /// Region scope of the loop.
    pub region_scope: ScopeId,
    /// The destination of the loop/block expression itself (i.e., where to put the result of a `break`).
    pub break_destination: Place,
    /// Drops that happen on the `break`/`return` path.
    pub break_drops: DropTree,
    /// Drops that happen on the `continue` path.
    pub continue_drops: Option<DropTree>,
}

#[derive(Debug)]
pub struct IfThenScope {
    /// The if-then scope or arm scope.
    pub region_scope: ScopeId,
    /// Drops that happen on the `else` path.
    pub else_drops: DropTree,
}

/// The target of an early exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakableTarget {
    Continue(ScopeId),
    Break(ScopeId),
    Return,
}

#[derive(Debug)]
pub struct DropTree {
    /// Nodes in the drop tree, containing drop data and a link to the next node.
    pub drop_nodes: Vec<DropNode>,
    /// Map for finding the index of an existing node, given its contents.
    existing_drops_map: HashMap<(DropIdx, Local), DropIdx>,
    /// Edges into the `DropTree` that need to be added once it's lowered.
    pub entry_points: Vec<(DropIdx, BasicBlock)>,
}

#[derive(Debug)]
pub struct DropNode {
    pub data: DropData,
    /// Index of the "next" drop to perform (in drop order, not declaration order).
    pub next: DropIdx,
}

impl Scope {
    pub fn new(source_scope: SourceScope, region_scope: ScopeId) -> Self {
        Scope { source_scope, region_scope, drops: Vec::new(), moved_locals: Vec::new(), cached_unwind_block: None, cached_coroutine_drop_block: None }
    }
    /// scope.rs:`Scope::needs_cleanup`: whether there's anything to do for the cleanup path.
    fn needs_cleanup(&self) -> bool {
        self.drops.iter().any(|drop| match drop.kind {
            DropKind::Value | DropKind::ForLint => true,
            DropKind::Storage => false,
        })
    }
    /// scope.rs:`Scope::invalidate_cache`.
    pub fn invalidate_cache(&mut self) {
        self.cached_unwind_block = None;
        self.cached_coroutine_drop_block = None;
    }
}

/// A trait that determines how [`DropTree`] creates its blocks and links to any entry nodes.
pub(super) trait DropTreeBuilder {
    /// Create a new block for the tree. This should call either `cfg.start_new_block` or `cfg.start_new_cleanup_block`.
    fn make_block(cfg: &mut Cfg) -> BasicBlock;
    /// Links a block outside the drop tree, `from`, to the block `to` inside the drop tree.
    fn link_entry_point(cfg: &mut Cfg, from: BasicBlock, to: BasicBlock) -> Result<(), &'static str>;
}

impl DropTree {
    pub fn new() -> Self {
        let fake_data = DropData {
            source_info: SourceInfo::default(),
            local: Local(u32::MAX),
            kind: DropKind::Storage,
        };
        Self { drop_nodes: vec![DropNode { data: fake_data, next: NO_DROP }], entry_points: Vec::new(), existing_drops_map: HashMap::default() }
    }

    /// Adds a node to the drop tree, consisting of drop data and the index of the "next" drop (in drop order).
    pub fn add_drop(&mut self, data: DropData, next: DropIdx) -> DropIdx {
        let drop_nodes = &mut self.drop_nodes;
        *self.existing_drops_map.entry((next, data.local)).or_insert_with(|| {
            drop_nodes.push(DropNode { data, next });
            drop_nodes.len() - 1
        })
    }

    /// Registers `from` as an entry point to this drop tree, at `to`.
    pub fn add_entry_point(&mut self, from: BasicBlock, to: DropIdx) {
        debug_assert!(to < self.drop_nodes.len());
        self.entry_points.push((to, from));
    }

    /// Builds the MIR for a given drop tree.
    pub(super) fn build_mir<T: DropTreeBuilder>(&mut self, cfg: &mut Cfg, root_node: Option<BasicBlock>) -> Result<Vec<Option<BasicBlock>>, &'static str> {
        let mut blocks = self.assign_blocks::<T>(cfg, root_node)?;
        self.link_blocks(cfg, &mut blocks);
        Ok(blocks)
    }

    /// Assign blocks for all of the drops in the drop tree that need them.
    fn assign_blocks<T: DropTreeBuilder>(&mut self, cfg: &mut Cfg, root_node: Option<BasicBlock>) -> Result<Vec<Option<BasicBlock>>, &'static str> {
        // StorageDead statements can share blocks with each other and also with a Drop terminator. We iterate
        // through the blocks to find which blocks need a terminator.
        #[derive(Clone, Copy)]
        enum Block {
            // This drop is unreachable
            None,
            // This drop is only reachable through the `StorageDead` with the specified index.
            Shares(DropIdx),
            // This drop has more than one way of being reached, or it is a `Drop` terminator.
            Own,
        }

        let mut blocks: Vec<Option<BasicBlock>> = vec![None; self.drop_nodes.len()];
        blocks[ROOT_NODE] = root_node;

        let mut needs_block = vec![Block::None; self.drop_nodes.len()];
        if root_node.is_some() {
            // In some cases (such as drops for `continue`) the root node already has a block. In this case, make
            // sure that we don't override it.
            needs_block[ROOT_NODE] = Block::Own;
        }

        // Sort so that we only need to check the last value.
        let entry_points = &mut self.entry_points;
        entry_points.sort();

        for drop_idx in (0..self.drop_nodes.len()).rev() {
            let drop_node = &self.drop_nodes[drop_idx];
            if entry_points.last().is_some_and(|entry_point| entry_point.0 == drop_idx) {
                let block = *blocks[drop_idx].get_or_insert_with(|| T::make_block(cfg));
                needs_block[drop_idx] = Block::Own;
                while entry_points.last().is_some_and(|entry_point| entry_point.0 == drop_idx) {
                    let entry_block = entry_points.pop().unwrap().1;
                    T::link_entry_point(cfg, entry_block, block)?;
                }
            }
            match needs_block[drop_idx] {
                Block::None => continue,
                Block::Own => {
                    blocks[drop_idx].get_or_insert_with(|| T::make_block(cfg));
                }
                Block::Shares(pred) => {
                    blocks[drop_idx] = blocks[pred];
                }
            }
            if let DropKind::Value = drop_node.data.kind {
                needs_block[drop_node.next] = Block::Own;
            } else if drop_idx != ROOT_NODE {
                match &mut needs_block[drop_node.next] {
                    pred @ Block::None => *pred = Block::Shares(drop_idx),
                    pred @ Block::Shares(_) => *pred = Block::Own,
                    Block::Own => (),
                }
            }
        }

        if !entry_points.is_empty() {
            return Err("drop tree entry points left unassigned");
        }
        Ok(blocks)
    }

    /// Link the blocks of the tree together with the terminators/statements of the drops.
    fn link_blocks(&self, cfg: &mut Cfg, blocks: &[Option<BasicBlock>]) {
        for drop_idx in (0..self.drop_nodes.len()).rev() {
            let drop_node = &self.drop_nodes[drop_idx];
            let Some(block) = blocks[drop_idx] else { continue };
            match drop_node.data.kind {
                DropKind::Value => {
                    let terminator = TerminatorKind::Drop {
                        target: blocks[drop_node.next].unwrap(),
                        // The caller will handle this if needed.
                        unwind: UnwindAction::Terminate(UnwindTerminateReason::InCleanup),
                        place: Place::local(drop_node.data.local),
                        replace: false,
                    };
                    cfg.terminate(block, drop_node.data.source_info, terminator);
                }
                DropKind::ForLint => {
                    let stmt = Statement {
                        source_info: drop_node.data.source_info,
                        kind: StatementKind::BackwardIncompatibleDropHint {
                            place: Place::local(drop_node.data.local),
                            reason: BackwardIncompatibleDropReason::Edition2024,
                        },
                    };
                    cfg.push(block, stmt);
                    let target = blocks[drop_node.next].unwrap();
                    if target != block {
                        // Diagnostics don't use this `Span` but debuginfo might, so we don't want to set it to `DUMMY_SP`.
                        let source_info = SourceInfo { span: Span::default(), ..drop_node.data.source_info };
                        cfg.terminate(block, source_info, TerminatorKind::Goto { target });
                    }
                }
                // Root nodes don't correspond to a drop.
                DropKind::Storage if drop_idx == ROOT_NODE => {}
                DropKind::Storage => {
                    let stmt = Statement { source_info: drop_node.data.source_info, kind: StatementKind::StorageDead(drop_node.data.local) };
                    cfg.push(block, stmt);
                    let target = blocks[drop_node.next].unwrap();
                    if target != block {
                        let source_info = SourceInfo { span: Span::default(), ..drop_node.data.source_info };
                        cfg.terminate(block, source_info, TerminatorKind::Goto { target });
                    }
                }
            }
        }
    }
}

impl Default for Scopes {
    fn default() -> Self {
        Self::new()
    }
}
impl Scopes {
    pub fn new() -> Self {
        Self { scopes: Vec::new(), breakable_scopes: Vec::new(), if_then_scope: None, unwind_drops: DropTree::new(), coroutine_drops: DropTree::new() }
    }

    fn push_scope(&mut self, region_scope: ScopeId, vis_scope: SourceScope) {
        self.scopes.push(Scope::new(vis_scope, region_scope));
    }

    fn pop_scope(&mut self, region_scope: ScopeId) -> Result<Scope, &'static str> {
        let scope = self.scopes.pop().ok_or("pop_scope on an empty scope stack")?;
        if scope.region_scope != region_scope {
            return Err("popped scope is not the requested region scope");
        }
        Ok(scope)
    }

    /// Returns the index of the innermost scope with this region scope.
    fn scope_index(&self, region_scope: ScopeId) -> Result<usize, &'static str> {
        self.scopes.iter().rposition(|scope| scope.region_scope == region_scope).ok_or("region_scope does not enclose")
    }

    /// Returns the topmost active scope, which is known to be alive until the next scope expression.
    fn topmost(&self) -> Result<ScopeId, &'static str> {
        self.scopes.last().map(|scope| scope.region_scope).ok_or("topmost_scope: no scopes present")
    }
}

impl Builder<'_> {
    fn bug<T>(&self, span: Span, what: &'static str) -> BuildResult<T> {
        Err(self.error(span, UnsupportedReason::Internal(what)))
    }
    fn internal(&self, span: Span) -> impl FnOnce(&'static str) -> super::BuildError + '_ {
        move |what| self.error(span, UnsupportedReason::Internal(what))
    }
    pub(crate) fn needs_drop(&self, ty: crate::types::TyId) -> bool {
        let cx = &self.root.cx;
        cx.tcx.needs_drop(cx.prog, cx.env, ty)
    }

    // Adding and removing scopes
    // ==========================

    /// Start a breakable scope, which tracks where `continue`, `break` and `return` should branch to.
    pub fn in_breakable_scope<F>(&mut self, loop_block: Option<BasicBlock>, break_destination: Place, span: Span, f: F) -> BuildResult<BlockAnd<()>>
    where
        F: FnOnce(&mut Self) -> BuildResult<Option<BlockAnd<()>>>,
    {
        let region_scope = self.scopes.topmost().map_err(self.internal(span))?;
        let scope = BreakableScope {
            region_scope,
            break_destination,
            break_drops: DropTree::new(),
            continue_drops: loop_block.map(|_| DropTree::new()),
        };
        self.scopes.breakable_scopes.push(scope);
        let normal_exit_block = f(self)?;
        let breakable_scope = self.scopes.breakable_scopes.pop().unwrap();
        if breakable_scope.region_scope != region_scope {
            return self.bug(span, "breakable scope mismatch");
        }
        let break_block = self.build_exit_tree(breakable_scope.break_drops, region_scope, span, None)?;
        if let Some(drops) = breakable_scope.continue_drops {
            self.build_exit_tree(drops, region_scope, span, loop_block)?;
        }
        Ok(match (normal_exit_block, break_block) {
            (Some(block), None) | (None, Some(block)) => block,
            (None, None) => self.cfg.start_new_block().unit(),
            (Some(normal_block), Some(exit_block)) => {
                let target = self.cfg.start_new_block();
                let source_info = self.source_info(span);
                self.cfg.terminate(normal_block.into_block(), source_info, TerminatorKind::Goto { target });
                self.cfg.terminate(exit_block.into_block(), source_info, TerminatorKind::Goto { target });
                target.unit()
            }
        })
    }

    /// Start an if-then scope which tracks drop for `if` expressions and `if` guards.
    ///
    /// For an if-let chain:
    ///
    /// if let Some(x) = a && let Some(y) = b && let Some(z) = c { ... }
    ///
    /// There are three possible ways the condition can be false and we may have to drop `x`, `x` and `y`, or neither
    /// depending on which binding fails. To handle this correctly we use a `DropTree` in a similar way to a
    /// `loop` expression and 'break' out on all of the 'else' paths.
    ///
    /// Notes:
    /// - We don't need to keep a stack of scopes in the `Builder` because the 'else' paths will only leave the
    ///   innermost scope.
    /// - This is also used for match guards.
    pub fn in_if_then_scope<F>(&mut self, region_scope: ScopeId, span: Span, f: F) -> BuildResult<(BasicBlock, BasicBlock)>
    where
        F: FnOnce(&mut Self) -> BuildResult<BlockAnd<()>>,
    {
        let scope = IfThenScope { region_scope, else_drops: DropTree::new() };
        let previous_scope = self.scopes.if_then_scope.replace(scope);

        let then_block = f(self)?.into_block();

        let if_then_scope = std::mem::replace(&mut self.scopes.if_then_scope, previous_scope).unwrap();
        if if_then_scope.region_scope != region_scope {
            return self.bug(span, "if-then scope mismatch");
        }

        let else_block = match self.build_exit_tree(if_then_scope.else_drops, region_scope, span, None)? {
            Some(else_block_and) => else_block_and.into_block(),
            None => self.cfg.start_new_block(),
        };

        Ok((then_block, else_block))
    }

    /// Convenience wrapper that pushes a scope and then executes `f` to build its contents, popping the scope
    /// afterwards.
    pub fn in_scope<F, R>(&mut self, region_scope: (ScopeId, SourceInfo), f: F) -> BuildResult<BlockAnd<R>>
    where
        F: FnOnce(&mut Self) -> BuildResult<BlockAnd<R>>,
    {
        let source_scope = self.source_scope;
        self.push_scope(region_scope);
        let BlockAnd(block, rv) = f(self)?;
        let block = self.pop_scope(region_scope, block)?.into_block();
        self.source_scope = source_scope;
        Ok(block.and(rv))
    }

    /// Push a scope onto the stack. You can then build code in this scope and call `pop_scope` afterwards.
    pub fn opt_in_scope<R>(
        &mut self,
        opt_region_scope: Option<(ScopeId, SourceInfo)>,
        f: impl FnOnce(&mut Self) -> BuildResult<BlockAnd<R>>,
    ) -> BuildResult<BlockAnd<R>> {
        if let Some(region_scope) = opt_region_scope { self.in_scope(region_scope, f) } else { f(self) }
    }

    pub fn push_scope(&mut self, region_scope: (ScopeId, SourceInfo)) {
        self.scopes.push_scope(region_scope.0, self.source_scope);
    }

    /// Pops a scope, which should have region scope `region_scope`, adding any drops onto the end of `block` that
    /// are needed. This must match 1-to-1 with `push_scope`.
    pub fn pop_scope(&mut self, region_scope: (ScopeId, SourceInfo), mut block: BasicBlock) -> BuildResult<BlockAnd<()>> {
        let span = region_scope.1.span;
        // If we are emitting a `drop` statement, we need to have the cached diverge cleanup pads ready in case that
        // drop panics.
        block = self.leave_top_scope(block)?;
        self.scopes.pop_scope(region_scope.0).map_err(self.internal(span))?;
        Ok(block.unit())
    }

    /// Sets up the drops for breaking from `block` to `target`.
    pub fn break_scope(&mut self, mut block: BasicBlock, value: Option<ExprId>, target: BreakableTarget, source_info: SourceInfo) -> BuildResult<BlockAnd<()>> {
        let span = source_info.span;

        let get_scope_index = |this: &Self, scope: ScopeId| {
            // find the loop-scope by its `region::Scope`.
            this.scopes
                .breakable_scopes
                .iter()
                .rposition(|breakable_scope| breakable_scope.region_scope == scope)
                .ok_or_else(|| this.error(span, UnsupportedReason::Internal("no enclosing breakable scope found")))
        };
        let (break_index, destination) = match target {
            BreakableTarget::Return => {
                let Some(scope) = self.scopes.breakable_scopes.first() else { return self.bug(span, "`return` with no return scope") };
                if scope.break_destination != Place::local(Local(0)) {
                    return self.bug(span, "`return` in item with no return scope");
                }
                (0, Some(scope.break_destination.clone()))
            }
            BreakableTarget::Break(scope) => {
                let break_index = get_scope_index(self, scope)?;
                let scope = &self.scopes.breakable_scopes[break_index];
                (break_index, Some(scope.break_destination.clone()))
            }
            BreakableTarget::Continue(scope) => {
                let break_index = get_scope_index(self, scope)?;
                (break_index, None)
            }
        };

        match (destination, value) {
            (Some(destination), Some(value)) => {
                self.block_context.push(BlockFrame::SubExpr);
                block = self.expr_into_dest(destination, block, value)?.into_block();
                self.block_context.pop();
            }
            (Some(destination), None) => {
                let unit = self.root.cx.tcx.tys.unit;
                self.cfg.push_assign_unit(block, source_info, destination, unit)
            }
            (None, Some(_)) => return self.bug(span, "`return` and `break` with value must have a destination"),
            (None, None) => {}
        }

        let region_scope = self.scopes.breakable_scopes[break_index].region_scope;
        let scope_index = self.scopes.scope_index(region_scope).map_err(self.internal(span))?;
        let is_break = matches!(target, BreakableTarget::Return | BreakableTarget::Break(_));
        if !is_break && self.scopes.breakable_scopes[break_index].continue_drops.is_none() {
            // Unlabelled `continue` within a labelled block: rustc reports a delayed bug. Terminate and go on.
            self.cfg.terminate(block, source_info, TerminatorKind::Unreachable);
            return Ok(self.cfg.start_new_block().unit());
        }
        let drops = if is_break {
            &mut self.scopes.breakable_scopes[break_index].break_drops
        } else {
            self.scopes.breakable_scopes[break_index].continue_drops.as_mut().unwrap()
        };

        let mut drop_idx = ROOT_NODE;
        for scope in &self.scopes.scopes[scope_index + 1..] {
            for drop in &scope.drops {
                drop_idx = drops.add_drop(*drop, drop_idx);
            }
        }
        drops.add_entry_point(block, drop_idx);

        // `build_drop_trees` doesn't have access to our source_info, so we
        // create a dummy terminator now. `TerminatorKind::UnwindResume` is used
        // because MIR type checking will panic if it hasn't been overwritten.
        // (See `<ExitScopes as DropTreeBuilder>::link_entry_point`.)
        self.cfg.terminate(block, source_info, TerminatorKind::UnwindResume);

        Ok(self.cfg.start_new_block().unit())
    }

    /// Sets up the drops for breaking from `block` due to an `if` condition that turned out to be false.
    ///
    /// Must be called in the context of [`Builder::in_if_then_scope`], so that there is an if-then scope to tell
    /// us what the target scope is.
    pub fn break_for_else(&mut self, block: BasicBlock, source_info: SourceInfo) -> BuildResult<()> {
        let Some(if_then_scope) = self.scopes.if_then_scope.as_ref() else { return self.bug(source_info.span, "no if-then scope found") };

        let target = if_then_scope.region_scope;
        let scope_index = self.scopes.scope_index(target).map_err(self.internal(source_info.span))?;

        // Upgrade `if_then_scope` to `&mut`.
        let if_then_scope = self.scopes.if_then_scope.as_mut().expect("upgrading & to &mut");

        let mut drop_idx = ROOT_NODE;
        let drops = &mut if_then_scope.else_drops;
        for scope in &self.scopes.scopes[scope_index + 1..] {
            for drop in &scope.drops {
                drop_idx = drops.add_drop(*drop, drop_idx);
            }
        }
        drops.add_entry_point(block, drop_idx);

        // `build_drop_trees` doesn't have access to our source_info, so we create a dummy terminator now.
        self.cfg.terminate(block, source_info, TerminatorKind::UnwindResume);
        Ok(())
    }

    /// `region::Scope::span`: the source span of a region scope's node (the body span when the node has none).
    pub fn region_scope_span(&self, region_scope: ScopeId) -> Span {
        let file = self.root.typeck.file;
        let ast = &self.root.cx.prog.files[file as usize].ast;
        let range = match self.root.scope_tree.scopes[region_scope.0 as usize].node {
            crate::region_scope::Node::Expr(e) => {
                let e = ast.expr(crate::ast::ExprId(e));
                Some((e.lo, e.hi))
            }
            crate::region_scope::Node::Block(b) => {
                let b = ast.block(crate::ast::BlockId(b));
                Some((b.lo, b.hi))
            }
            _ => None,
        };
        match range {
            Some((lo, hi)) => Span { file, lo, hi },
            None => self.body_span,
        }
    }

    /// The [`SourceInfo`] for the end of a region scope's span, the position drops are attributed to.
    fn scope_end_info(&self, region_scope: ScopeId, source_scope: SourceScope) -> SourceInfo {
        let span = self.region_scope_span(region_scope);
        SourceInfo { span: Span { file: span.file, lo: span.hi.saturating_sub(1), hi: span.hi }, scope: source_scope }
    }

    // Finding scopes
    // ==============

    /// Returns the scope that we are currently in, not the scope to which we are going to jump (`break`...).
    pub fn local_scope(&self) -> ScopeId {
        self.scopes.topmost().expect("local_scope: no scopes present")
    }

    // Scheduling drops
    // ================

    pub fn schedule_drop_storage_and_value(&mut self, span: Span, region_scope: ScopeId, local: Local) -> BuildResult<()> {
        self.schedule_drop(span, region_scope, local, DropKind::Storage)?;
        self.schedule_drop(span, region_scope, local, DropKind::Value)
    }

    /// Indicates that `place` should be dropped on exit from `region_scope`.
    ///
    /// When called with `DropKind::Storage`, `place` shouldn't be the return place, or a function parameter.
    pub fn schedule_drop(&mut self, span: Span, region_scope: ScopeId, local: Local, drop_kind: DropKind) -> BuildResult<()> {
        let needs_drop = match drop_kind {
            DropKind::Value | DropKind::ForLint => {
                if !self.needs_drop(self.local_decls[local.0 as usize].ty) {
                    return Ok(());
                }
                true
            }
            DropKind::Storage => {
                if local.0 as usize <= self.arg_count {
                    return self.bug(span, "`schedule_drop` called with body argument whose storage does not require a drop");
                }
                false
            }
        };

        // When building drops, we try to cache chains of drops to reduce the number of `DropTree::add_drop` calls.
        // This, however, means that whenever we add a drop into a scope which already had some entries in the
        // drop tree built (and thus, cached) we must invalidate all caches which might branch into the scope
        // which had a drop just added to it. This is necessary, because otherwise some other code might use the
        // cache to branch into already built chain of drops, essentially ignoring the newly added drop.
        let invalidate_caches = needs_drop || self.coroutine.is_some();
        let n = self.scopes.scopes.len();
        for i in (0..n).rev() {
            if invalidate_caches {
                self.scopes.scopes[i].invalidate_cache();
            }
            if self.scopes.scopes[i].region_scope == region_scope {
                let info = self.scope_end_info(region_scope, self.scopes.scopes[i].source_scope);
                self.scopes.scopes[i].drops.push(DropData { source_info: info, local, kind: drop_kind });
                return Ok(());
            }
        }
        self.bug(span, "region scope not in scope to drop local")
    }

    /// Schedule emission of a backwards incompatible drop lint hint. Applicable only to temporary values for now.
    pub fn schedule_backwards_incompatible_drop(&mut self, span: Span, region_scope: ScopeId, local: Local) -> BuildResult<()> {
        // Note that we are *not* gating BIDs here on whether they have significant destructor. We need to know all
        // of them so that we can capture potential borrow-checking errors.
        let n = self.scopes.scopes.len();
        for i in (0..n).rev() {
            self.scopes.scopes[i].invalidate_cache();
            if self.scopes.scopes[i].region_scope == region_scope {
                let info = self.scope_end_info(region_scope, self.scopes.scopes[i].source_scope);
                self.scopes.scopes[i].drops.push(DropData { source_info: info, local, kind: DropKind::ForLint });
                return Ok(());
            }
        }
        self.bug(span, "region scope not in scope to drop local for linting")
    }

    /// Indicates that the "local operand" stored in `local` is *moved* at some point during execution (see
    /// `local_scope` for more information about what a "local operand" is -- in short, it's an intermediate
    /// operand created as part of preparing some MIR instruction). We use this information to suppress
    /// redundant drops on the non-unwind paths. This results in less MIR, but also avoids spurious borrow check
    /// errors (c.f. #64391).
    ///
    /// Example: when compiling the call to `foo` here:
    ///
    /// ```ignore (illustrative)
    /// foo(bar(), ...)
    /// ```
    ///
    /// we would evaluate `bar()` to an operand `_X`. We would also schedule `_X` to be dropped when the
    /// expression scope for `foo(bar())` is exited. This is relevant, for example, if the later arguments should
    /// unwind (it would ensure that `_X` gets dropped). However, if no unwind occurs, then `_X` will be
    /// unconditionally consumed by the `foo` call:
    ///
    /// ```ignore (illustrative)
    /// _R = CALL(foo, _X, ...)
    /// ```
    ///
    /// Therefore, we can avoid the useless `DROP(_X)` for the normal path.
    pub fn record_operands_moved(&mut self, operands: &[Operand]) -> BuildResult<()> {
        let local_scope = self.local_scope();
        let Some(scope) = self.scopes.scopes.last_mut() else { return self.bug(Span::default(), "record_operands_moved: no scopes") };
        if scope.region_scope != local_scope {
            return self.bug(Span::default(), "local scope is not the topmost scope!");
        }
        // look for moves of a local variable, like `MOVE(_X)`
        let locals_moved = operands.iter().flat_map(|operand| match operand {
            Operand::Copy(_) | Operand::Constant(_) | Operand::RuntimeChecks(_) => None,
            Operand::Move(place) => place.projection.is_empty().then_some(place.local),
        });
        for local in locals_moved {
            // check if we have a Drop for this operand and -- if so -- add it to the list of moved operands. Note
            // that this local might not have been an operand created for this call, it could come from other
            // places too.
            if scope.drops.iter().any(|drop| drop.local == local && drop.kind == DropKind::Value) {
                scope.moved_locals.push(local);
            }
        }
        Ok(())
    }

    // Other
    // =====

    /// Returns the [DropIdx] for the innermost drop if the function unwound at this point. The `DropIdx` will be
    /// created if it doesn't already exist.
    fn diverge_cleanup(&mut self) -> BuildResult<DropIdx> {
        // It is okay to use dummy span because the getting scope index on the topmost scope must always succeed.
        self.diverge_cleanup_target(self.scopes.topmost().map_err(self.internal(Span::default()))?, Span::default())
    }

    /// This is similar to [diverge_cleanup](Self::diverge_cleanup) except its target is set to a given scope
    /// instead of the current one.
    fn diverge_cleanup_target(&mut self, target_scope: ScopeId, span: Span) -> BuildResult<DropIdx> {
        let target = self.scopes.scope_index(target_scope).map_err(self.internal(span))?;
        let (uncached_scope, mut cached_drop) = self.scopes.scopes[..=target]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(scope_idx, scope)| scope.cached_unwind_block.map(|cached_block| (scope_idx + 1, cached_block)))
            .unwrap_or((0, ROOT_NODE));

        if uncached_scope > target {
            return Ok(cached_drop);
        }

        let is_coroutine = self.coroutine.is_some();
        for scope in &mut self.scopes.scopes[uncached_scope..=target] {
            for drop in &scope.drops {
                if is_coroutine || drop.kind == DropKind::Value {
                    cached_drop = self.scopes.unwind_drops.add_drop(*drop, cached_drop);
                }
            }
            scope.cached_unwind_block = Some(cached_drop);
        }

        Ok(cached_drop)
    }

    /// Prepares to create a path that performs all required cleanup for a terminator that can unwind at the given
    /// basic block.
    ///
    /// This path terminates in Resume. The path isn't created until after all of the non-unwind paths in this
    /// item have been lowered.
    pub fn diverge_from(&mut self, start: BasicBlock) -> BuildResult<()> {
        debug_assert!(
            matches!(
                self.cfg.terminator(start).kind,
                TerminatorKind::Assert { .. } | TerminatorKind::Call { .. } | TerminatorKind::Drop { .. } | TerminatorKind::FalseUnwind { .. }
            ),
            "diverge_from called on block with terminator that cannot unwind."
        );

        let next_drop = self.diverge_cleanup()?;
        self.scopes.unwind_drops.add_entry_point(start, next_drop);
        Ok(())
    }

    /// Returns the [DropIdx] for the innermost drop for dropline (coroutine drop path).
    fn diverge_dropline(&mut self) -> BuildResult<DropIdx> {
        // It is okay to use dummy span because the getting scope index on the topmost scope must always succeed.
        self.diverge_dropline_target(self.scopes.topmost().map_err(self.internal(Span::default()))?, Span::default())
    }

    /// Similar to diverge_cleanup_target, but for dropline (coroutine drop path).
    fn diverge_dropline_target(&mut self, target_scope: ScopeId, span: Span) -> BuildResult<DropIdx> {
        debug_assert!(self.coroutine.is_some(), "diverge_dropline_target is valid only for coroutine");
        let target = self.scopes.scope_index(target_scope).map_err(self.internal(span))?;
        let (uncached_scope, mut cached_drop) = self.scopes.scopes[..=target]
            .iter()
            .enumerate()
            .rev()
            .find_map(|(scope_idx, scope)| scope.cached_coroutine_drop_block.map(|cached_block| (scope_idx + 1, cached_block)))
            .unwrap_or((0, ROOT_NODE));

        if uncached_scope > target {
            return Ok(cached_drop);
        }

        for scope in &mut self.scopes.scopes[uncached_scope..=target] {
            for drop in &scope.drops {
                cached_drop = self.scopes.coroutine_drops.add_drop(*drop, cached_drop);
            }
            scope.cached_coroutine_drop_block = Some(cached_drop);
        }

        Ok(cached_drop)
    }

    /// Sets up a path that performs all required cleanup for dropping a coroutine, starting from the given block
    /// that ends in [TerminatorKind::Yield].
    ///
    /// This path terminates in CoroutineDrop.
    pub fn coroutine_drop_cleanup(&mut self, yield_block: BasicBlock) -> BuildResult<()> {
        debug_assert!(
            matches!(self.cfg.terminator(yield_block).kind, TerminatorKind::Yield { .. }),
            "coroutine_drop_cleanup called on block with non-yield terminator."
        );
        let cached_drop = self.diverge_dropline()?;
        self.scopes.coroutine_drops.add_entry_point(yield_block, cached_drop);
        Ok(())
    }

    /// Utility function for *non*-scope code to build their own drops. Force a drop at this point in the MIR by
    /// creating a new block.
    pub fn build_drop_and_replace(&mut self, block: BasicBlock, span: Span, place: Place, value: Rvalue) -> BuildResult<BlockAnd<()>> {
        let source_info = self.source_info(span);

        // create the new block for the assignment
        let assign = self.cfg.start_new_block();
        self.cfg.push_assign(assign, source_info, place.clone(), value.clone());

        // create the new block for the assignment in the case of unwinding
        let assign_unwind = self.cfg.start_new_cleanup_block();
        self.cfg.push_assign(assign_unwind, source_info, place.clone(), value);

        self.cfg.terminate(
            block,
            source_info,
            TerminatorKind::Drop { place, target: assign, unwind: UnwindAction::Cleanup(assign_unwind), replace: true },
        );
        self.diverge_from(block)?;

        Ok(assign.unit())
    }

    /// Creates an `Assert` terminator and return the success block. If the conditional expression is true,
    /// execution continues at the returned basic block, else a panic occurs.
    pub fn assert(&mut self, block: BasicBlock, cond: Operand, expected: bool, msg: AssertKind, span: Span) -> BuildResult<BasicBlock> {
        let source_info = self.source_info(span);
        let success_block = self.cfg.start_new_block();

        self.cfg.terminate(block, source_info, TerminatorKind::Assert { cond, expected, msg, target: success_block, unwind: UnwindAction::Continue });
        self.diverge_from(block)?;

        Ok(success_block)
    }

    /// Unschedules any drops in the top two scopes.
    ///
    /// This is only needed for `match` arm scopes, because they have one entrance per pattern, but only one exit.
    pub fn clear_match_arm_and_guard_scopes(&mut self, region_scope: ScopeId) -> BuildResult<()> {
        let n = self.scopes.scopes.len();
        if n < 2 {
            return self.bug(Span::default(), "matches with guards should introduce separate scopes for the pattern and guard");
        }
        let tree = &self.root.scope_tree;
        let (arm_region, guard_region) = (self.scopes.scopes[n - 2].region_scope, self.scopes.scopes[n - 1].region_scope);
        let guard = tree.scopes[guard_region.0 as usize];
        if arm_region != region_scope || guard.data != ScopeData::MatchGuard || guard.node != tree.scopes[region_scope.0 as usize].node {
            return self.bug(Span::default(), "match arm and guard scopes disagree");
        }
        for scope in &mut self.scopes.scopes[n - 2..] {
            scope.drops.clear();
            scope.invalidate_cache();
        }
        Ok(())
    }

    /// Leave the top scope: emit its drops at the end of `block`, returning the block control continues in.
    fn leave_top_scope(&mut self, block: BasicBlock) -> BuildResult<BasicBlock> {
        // If we are emitting a `drop` statement, we need to have the cached diverge cleanup pads ready in case
        // that drop panics.
        let needs_cleanup = self.scopes.scopes.last().is_some_and(|scope| scope.needs_cleanup());
        let is_coroutine = self.coroutine.is_some();
        let unwind_to = if needs_cleanup { self.diverge_cleanup()? } else { NO_DROP };

        let Some(scope) = self.scopes.scopes.last() else { return self.bug(Span::default(), "leave_top_scope called with no scopes") };
        let arg_count = self.arg_count;
        build_scope_drops(
            &mut self.cfg,
            &mut self.scopes.unwind_drops,
            &mut self.scopes.coroutine_drops,
            scope,
            block,
            unwind_to,
            is_coroutine && needs_cleanup,
            arg_count,
        )
        .map_err(|what| self.error(Span::default(), UnsupportedReason::Internal(what)))
    }

    // Exit trees and drop trees
    // =========================

    /// Build a drop tree for a breakable scope.
    ///
    /// If `continue_block` is `Some`, then the tree is for `continue` inside a loop. Otherwise this is for
    /// `break` or `return`.
    fn build_exit_tree(&mut self, mut drops: DropTree, else_scope: ScopeId, span: Span, continue_block: Option<BasicBlock>) -> BuildResult<Option<BlockAnd<()>>> {
        let blocks = drops.build_mir::<ExitScopes>(&mut self.cfg, continue_block).map_err(self.internal(span))?;
        let is_coroutine = self.coroutine.is_some();

        // Link the exit drop tree to unwind drop tree.
        if drops.drop_nodes.iter().any(|drop_node| drop_node.data.kind == DropKind::Value) {
            let unwind_target = self.diverge_cleanup_target(else_scope, span)?;
            let mut unwind_indices = vec![unwind_target];
            for (drop_idx, drop_node) in drops.drop_nodes.iter().enumerate().skip(1) {
                match drop_node.data.kind {
                    DropKind::Storage | DropKind::ForLint => {
                        if is_coroutine {
                            let unwind_drop = self.scopes.unwind_drops.add_drop(drop_node.data, unwind_indices[drop_node.next]);
                            unwind_indices.push(unwind_drop);
                        } else {
                            unwind_indices.push(unwind_indices[drop_node.next]);
                        }
                    }
                    DropKind::Value => {
                        let unwind_drop = self.scopes.unwind_drops.add_drop(drop_node.data, unwind_indices[drop_node.next]);
                        self.scopes.unwind_drops.add_entry_point(blocks[drop_idx].unwrap(), unwind_indices[drop_node.next]);
                        unwind_indices.push(unwind_drop);
                    }
                }
            }
        }
        Ok(blocks[ROOT_NODE].map(BasicBlock::unit))
    }

    /// Build the unwind and coroutine drop trees.
    pub fn build_drop_trees(&mut self) -> BuildResult<()> {
        let span = self.body_span;
        if self.coroutine.is_some() {
            self.build_coroutine_drop_trees().map_err(self.internal(span))
        } else {
            Self::build_unwind_tree(&mut self.cfg, &mut self.scopes.unwind_drops, span, &mut None).map_err(self.internal(span))
        }
    }

    fn build_coroutine_drop_trees(&mut self) -> Result<(), &'static str> {
        // Build the drop tree for dropping the coroutine while it's suspended.
        let fn_span = self.body_span;
        let drops = &mut self.scopes.coroutine_drops;
        let cfg = &mut self.cfg;
        let blocks = drops.build_mir::<CoroutineDrop>(cfg, None)?;
        if let Some(root_block) = blocks[ROOT_NODE] {
            cfg.terminate(root_block, SourceInfo { span: fn_span, scope: SourceScope(0) }, TerminatorKind::CoroutineDrop);
        }

        // Build the drop tree for unwinding in the normal control flow paths.
        let resume_block = &mut None;
        let unwind_drops = &mut self.scopes.unwind_drops;
        Self::build_unwind_tree(cfg, unwind_drops, fn_span, resume_block)?;

        // Build the drop tree for unwinding when dropping a suspended coroutine.
        //
        // This is a different tree to the standard unwind paths here to prevent drop elaboration from creating
        // drop flags that would have to be captured by the coroutine. I'm not sure how important this
        // optimization is, but it is here.
        for (drop_idx, drop_node) in drops.drop_nodes.iter().enumerate() {
            if let DropKind::Value = drop_node.data.kind {
                if let Some(bb) = blocks[drop_idx] {
                    debug_assert!(drop_node.next < drops.drop_nodes.len());
                    drops.entry_points.push((drop_node.next, bb));
                }
            }
        }
        Self::build_unwind_tree(cfg, drops, fn_span, resume_block)
    }

    fn build_unwind_tree(cfg: &mut Cfg, drops: &mut DropTree, fn_span: Span, resume_block: &mut Option<BasicBlock>) -> Result<(), &'static str> {
        let blocks = drops.build_mir::<Unwind>(cfg, *resume_block)?;
        if let (None, Some(resume)) = (*resume_block, blocks[ROOT_NODE]) {
            cfg.terminate(resume, SourceInfo { span: fn_span, scope: SourceScope(0) }, TerminatorKind::UnwindResume);
            *resume_block = blocks[ROOT_NODE];
        }
        Ok(())
    }
}

/// Builds drops for `pop_scope` and `leave_top_scope`.
///
/// # Parameters
///
/// * `unwind_drops`, the drop tree data structure storing what needs to be cleaned up if unwind occurs
/// * `scope`, describes the drops that will occur on exiting the scope in regular execution
/// * `block`, the block to branch to once drops are complete (assuming no unwind occurs)
/// * `unwind_to`, describes the drops that would occur at this point in the code if a panic occurred (a subset of
///   the drops in `scope`, since we sometimes elide StorageDead and other instructions on unwinding)
/// * `storage_dead_on_unwind`, if true, then we should emit `StorageDead` even when unwinding
/// * `arg_count`, number of MIR local variables corresponding to fn arguments (used to assert that we don't drop
///   those)
#[allow(clippy::too_many_arguments)]
fn build_scope_drops(
    cfg: &mut Cfg,
    unwind_drops: &mut DropTree,
    _coroutine_drops: &mut DropTree,
    scope: &Scope,
    block: BasicBlock,
    unwind_to: DropIdx,
    storage_dead_on_unwind: bool,
    arg_count: usize,
) -> Result<BasicBlock, &'static str> {
    // Build up the drops in evaluation order. The end result will look like:
    //
    // [SDs, drops[n]] --..> [SDs, drop[1]] -> [SDs, drop[0]] -> [[SDs]]
    //               |                    |                 |
    //               :                    |                 |
    //                                    V                 V
    // [drop[n]] -...-> [drop[1]] ------> [drop[0]] ------> [last_unwind_to]
    //
    // The horizontal arrows represent the execution path when the drops return successfully. The downwards
    // arrows represent the execution path when the drops panic (panicking while unwinding will abort, so there's
    // no need for another set of arrows).
    //
    // For coroutines, we unwind from a drop on a local to its StorageDead statement. For other functions we
    // don't worry about StorageDead. The drops for the unwind path should have already been generated by
    // `diverge_cleanup_gen`.

    // `unwind_to` indicates what needs to be dropped should unwinding occur. This is a subset of what needs to
    // be dropped when exiting the scope. As we unwind the scope, we will also move `unwind_to` backwards to
    // match, so that we can use it should a destructor panic.
    let mut unwind_to = unwind_to;
    let mut block = block;
    let check = |tree: &DropTree, idx: DropIdx, data: &DropData| -> Result<(), &'static str> {
        match tree.drop_nodes.get(idx) {
            Some(node) if node.data.local == data.local && node.data.kind == data.kind => Ok(()),
            _ => Err("unwind drop tree out of sync with the scope's drops"),
        }
    };

    // Next, build up the drops. Here we iterate the vector in reverse and emit drops in the "same" order for
    // each local variable as they were declared.
    for drop_data in scope.drops.iter().rev() {
        let source_info = drop_data.source_info;
        let local = drop_data.local;

        match drop_data.kind {
            DropKind::Value => {
                // `unwind_to` should drop the value that we're about to schedule. If dropping this value
                // panics, then we continue with the *next* value on the unwind path.
                check(unwind_drops, unwind_to, drop_data)?;
                unwind_to = unwind_drops.drop_nodes[unwind_to].next;

                if scope.moved_locals.contains(&local) {
                    // This local was moved out of (record_operands_moved): no need for the drop on the normal path.
                    continue;
                }

                unwind_drops.add_entry_point(block, unwind_to);
                let next = cfg.start_new_block();
                cfg.terminate(
                    block,
                    source_info,
                    TerminatorKind::Drop { place: Place::local(local), target: next, unwind: UnwindAction::Continue, replace: false },
                );
                block = next;
            }
            DropKind::ForLint => {
                // As in the `DropKind::Storage` case below: normally lint-related drops are not emitted for
                // unwind, so we can just leave `unwind_to` unmodified, but in some cases we emit things ALSO on
                // the unwind path, so we need to adjust `unwind_to` in that case.
                if storage_dead_on_unwind {
                    check(unwind_drops, unwind_to, drop_data)?;
                    unwind_to = unwind_drops.drop_nodes[unwind_to].next;
                }

                // If the operand has been moved, and we are not on an unwind path, then don't generate the
                // drop. (We only take this into account for non-unwind paths so as not to disturb the
                // mind-bending unwind path logic.)
                if scope.moved_locals.contains(&local) {
                    continue;
                }

                cfg.push(
                    block,
                    Statement {
                        source_info,
                        kind: StatementKind::BackwardIncompatibleDropHint { place: Place::local(local), reason: BackwardIncompatibleDropReason::Edition2024 },
                    },
                );
            }
            DropKind::Storage => {
                // Ordinarily, storage-dead nodes are not emitted on unwind, so we don't need to adjust
                // `unwind_to` on this path. However, in some specific cases we *do* emit storage-dead nodes
                // on the unwind path, and in that case, we need to adjust `unwind_to` so that the unwind path
                // stays in sync.
                if storage_dead_on_unwind {
                    check(unwind_drops, unwind_to, drop_data)?;
                    unwind_to = unwind_drops.drop_nodes[unwind_to].next;
                }
                if local.0 as usize <= arg_count {
                    return Err("StorageDead scheduled for a body argument");
                }
                cfg.push(block, Statement { source_info, kind: StatementKind::StorageDead(local) });
            }
        }
    }
    Ok(block)
}

/// Builds the exit/continue/else trees: blocks are plain; entry points hold a dummy `UnwindResume` terminator
/// that becomes a `Goto`.
pub(super) struct ExitScopes;

impl DropTreeBuilder for ExitScopes {
    fn make_block(cfg: &mut Cfg) -> BasicBlock {
        cfg.start_new_block()
    }
    fn link_entry_point(cfg: &mut Cfg, from: BasicBlock, to: BasicBlock) -> Result<(), &'static str> {
        // There should be an existing terminator with real source info and a dummy TerminatorKind. Replace it
        // with a proper goto.
        let term = cfg.terminator_mut(from);
        if let TerminatorKind::UnwindResume = term.kind {
            term.kind = TerminatorKind::Goto { target: to };
            Ok(())
        } else {
            Err("unexpected dummy terminator kind")
        }
    }
}

/// The coroutine drop tree: entered from `Yield` terminators.
struct CoroutineDrop;

impl DropTreeBuilder for CoroutineDrop {
    fn make_block(cfg: &mut Cfg) -> BasicBlock {
        cfg.start_new_block()
    }
    fn link_entry_point(cfg: &mut Cfg, from: BasicBlock, to: BasicBlock) -> Result<(), &'static str> {
        let term = cfg.terminator_mut(from);
        if let TerminatorKind::Yield { ref mut drop, .. } = term.kind {
            *drop = Some(to);
            Ok(())
        } else {
            Err("cannot enter coroutine drop tree from this terminator")
        }
    }
}

/// The unwind drop trees: entered from the unwinding edge of calls, asserts and drops.
struct Unwind;

impl DropTreeBuilder for Unwind {
    fn make_block(cfg: &mut Cfg) -> BasicBlock {
        cfg.start_new_cleanup_block()
    }
    fn link_entry_point(cfg: &mut Cfg, from: BasicBlock, to: BasicBlock) -> Result<(), &'static str> {
        let term = cfg.terminator_mut(from);
        match &mut term.kind {
            TerminatorKind::Drop { unwind, .. } => {
                if let UnwindAction::Cleanup(unwind) = *unwind {
                    let source_info = term.source_info;
                    cfg.terminate(unwind, source_info, TerminatorKind::Goto { target: to });
                } else {
                    *unwind = UnwindAction::Cleanup(to);
                }
                Ok(())
            }
            TerminatorKind::FalseUnwind { unwind, .. } | TerminatorKind::Call { unwind, .. } | TerminatorKind::Assert { unwind, .. } => {
                *unwind = UnwindAction::Cleanup(to);
                Ok(())
            }
            _ => Err("cannot unwind from this terminator"),
        }
    }
}
