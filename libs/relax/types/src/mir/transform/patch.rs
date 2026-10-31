//! Deferred MIR edits, shared by body drop elaboration and glue construction.
//! rustc_mir_transform/src/patch.rs:12,38,213,238,271.
use crate::mir::*;

pub(crate) struct MirPatch {
    old_blocks: usize,
    old_locals: usize,
    blocks: Vec<BasicBlockData>,
    locals: Vec<LocalDecl>,
    statements: Vec<(Location, StatementKind)>,
    terms: Vec<Option<TerminatorKind>>,
    exits: Vec<(UnwindAction, BasicBlock)>,
}
impl MirPatch {
    pub fn new(body: &Body) -> Self {
        let mut exits = Vec::new();
        for (i, b) in body.basic_blocks.iter().enumerate() {
            if !b.statements.is_empty() || !b.is_cleanup { continue; }
            let action = match b.terminator.as_ref().unwrap().kind {
                TerminatorKind::UnwindResume => UnwindAction::Continue,
                TerminatorKind::Unreachable => UnwindAction::Unreachable,
                TerminatorKind::UnwindTerminate(r) => UnwindAction::Terminate(r),
                _ => continue,
            };
            exits.push((action, BasicBlock(i as u32)));
        }
        Self { old_blocks: body.basic_blocks.len(), old_locals: body.local_decls.len(),
            blocks: Vec::new(), locals: Vec::new(), statements: Vec::new(), terms: vec![None; body.basic_blocks.len()], exits }
    }
    pub fn temp(&mut self, ty: TyId, source_info: SourceInfo) -> Local {
        let local = Local((self.old_locals + self.locals.len()) as u32);
        self.locals.push(LocalDecl { ty, source_info, mutability: Mutability::Mut, kind: LocalKind::Temp,
            user_ty: Vec::new(), local_info: LocalInfo::Boring });
        local
    }
    pub fn local_ty(&self, body: &Body, local: Local) -> TyId {
        let i = local.0 as usize;
        if i < self.old_locals { body.local_decls[i].ty } else { self.locals[i - self.old_locals].ty }
    }
    pub fn block(&mut self, source_info: SourceInfo, is_cleanup: bool, statements: Vec<Statement>, kind: TerminatorKind) -> BasicBlock {
        let bb = BasicBlock((self.old_blocks + self.blocks.len()) as u32);
        self.blocks.push(BasicBlockData { statements, is_cleanup, terminator: Some(Terminator { source_info, kind }) });
        self.terms.push(None);
        bb
    }
    pub fn exit(&mut self, action: UnwindAction, source: SourceInfo) -> BasicBlock {
        if let UnwindAction::Cleanup(bb) = action { return bb; }
        for &(a, bb) in &self.exits { if a == action { return bb; } }
        let kind = match action {
            UnwindAction::Continue => TerminatorKind::UnwindResume,
            UnwindAction::Unreachable => TerminatorKind::Unreachable,
            UnwindAction::Terminate(r) => TerminatorKind::UnwindTerminate(r),
            UnwindAction::Cleanup(_) => unreachable!(),
        };
        let bb = self.block(source, true, Vec::new(), kind);
        self.exits.push((action, bb));
        bb
    }
    pub fn terminator(&mut self, bb: BasicBlock, kind: TerminatorKind) {
        let slot = &mut self.terms[bb.0 as usize];
        assert!(slot.is_none(), "terminator patched twice");
        *slot = Some(kind);
    }
    /// Insertions at the same original location retain their insertion order.
    pub fn statement(&mut self, loc: Location, kind: StatementKind) { self.statements.push((loc, kind)); }
    pub fn apply(self, body: &mut Body) {
        assert_eq!(body.basic_blocks.len(), self.old_blocks);
        assert_eq!(body.local_decls.len(), self.old_locals);
        body.basic_blocks.extend(self.blocks);
        body.local_decls.extend(self.locals);
        let mut stmts = self.statements;
        stmts.sort_by_key(|(loc, _)| (loc.block, loc.statement_index));
        let mut last = BasicBlock(0);
        let mut delta = 0;
        for (loc, kind) in stmts {
            if loc.block != last { last = loc.block; delta = 0; }
            let block = &mut body.basic_blocks[loc.block.0 as usize];
            let i = loc.statement_index + delta;
            let source_info = block.statements.get(i).map(|s| s.source_info)
                .unwrap_or(block.terminator.as_ref().unwrap().source_info);
            block.statements.insert(i, Statement { source_info, kind });
            delta += 1;
        }
        for (i, kind) in self.terms.into_iter().enumerate() {
            let Some(kind) = kind else { continue; };
            let block = &mut body.basic_blocks[i];
            if matches!(kind, TerminatorKind::Unreachable) { block.statements.clear(); }
            block.terminator.as_mut().unwrap().kind = kind;
        }
    }
}
