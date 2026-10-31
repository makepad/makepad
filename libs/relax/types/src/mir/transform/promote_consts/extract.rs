//! Extraction of validated promotion candidates into promoted bodies:
//! rustc_mir_transform/src/promote_consts.rs `Promoter` and `promote_candidates` (1.97.1).
use super::{Candidate, TempState};
use crate::fxhash::HashMap;
use crate::mir::visit::{MutVisitor, PlaceContext};
use crate::mir::*;
use crate::tcx::Tcx;
use crate::types::{RegionId, TyId, TyKind};

struct Promoter<'a> {
    tcx: &'a Tcx,
    source: &'a mut Body,
    promoted: Body,
    temps: &'a mut Vec<TempState>,
    extra_statements: &'a mut Vec<(Location, Statement)>,

    /// If true, all nested temps are also kept in the source MIR, not moved to the promoted MIR.
    keep_original: bool,

    /// The allocations the promoted's constants name, copied from the source: the source's id to
    /// the promoted's own (allocation ids are local to a body).
    allocs: HashMap<AllocId, AllocId>,
}

/// `LocalDecl::new`: a mutable temporary with no extra information.
fn local_decl(ty: TyId, span: Span, kind: LocalKind) -> LocalDecl {
    LocalDecl { ty, mutability: Mutability::Mut, user_ty: Vec::new(), source_info: SourceInfo { span, scope: SourceScope(0) }, kind, local_info: LocalInfo::Boring }
}

impl Promoter<'_> {
    fn new_block(&mut self) -> BasicBlock {
        let span = self.promoted.span;
        self.promoted.basic_blocks.push(BasicBlockData {
            statements: Vec::new(),
            terminator: Some(Terminator { source_info: SourceInfo { span, scope: SourceScope(0) }, kind: TerminatorKind::Return }),
            is_cleanup: false,
        });
        BasicBlock(self.promoted.basic_blocks.len() as u32 - 1)
    }

    fn assign(&mut self, dest: Local, rvalue: Rvalue, span: Span) {
        let last = self.promoted.basic_blocks.len() - 1;
        self.promoted.basic_blocks[last].statements.push(Statement {
            source_info: SourceInfo { span, scope: SourceScope(0) },
            kind: StatementKind::Assign(Place::local(dest), rvalue),
        });
    }

    fn is_temp_kind(&self, local: Local) -> bool {
        self.source.local_decls[local.0 as usize].kind == LocalKind::Temp
    }

    fn unit_ty(&self) -> TyId {
        self.tcx.tys.intern(TyKind::Tuple(crate::types::List::EMPTY))
    }

    /// Copies the initialization of this temp to the promoted MIR, recursing through temps.
    fn promote_temp(&mut self, temp: Local) -> Local {
        let old_keep_original = self.keep_original;
        let loc = match self.temps[temp.0 as usize] {
            TempState::Defined { location, uses, .. } if uses > 0 => {
                if uses > 1 {
                    self.keep_original = true;
                }
                location
            }
            state => unreachable!("{temp:?} not promotable: {state:?}"),
        };
        if !self.keep_original {
            self.temps[temp.0 as usize] = TempState::PromotedOut;
        }

        let num_stmts = self.source.basic_blocks[loc.block.0 as usize].statements.len();
        let ty = self.source.local_decls[temp.0 as usize].ty;
        let decl_span = self.source.local_decls[temp.0 as usize].source_info.span;
        self.promoted.local_decls.push(local_decl(ty, decl_span, LocalKind::Temp));
        let new_temp = Local(self.promoted.local_decls.len() as u32 - 1);

        // First, take the Rvalue or Call out of the source MIR, or duplicate it, depending on
        // keep_original.
        if loc.statement_index < num_stmts {
            let unit_ty = self.unit_ty();
            let (mut rvalue, source_info) = {
                let statement = &mut self.source.basic_blocks[loc.block.0 as usize].statements[loc.statement_index];
                let span = statement.source_info.span;
                let source_info = statement.source_info;
                let StatementKind::Assign(_, rhs) = &mut statement.kind else { unreachable!("{statement:?} is not an assignment") };

                (
                    if self.keep_original {
                        rhs.clone()
                    } else {
                        let unit = Rvalue::Use(Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_: Const::Val(ConstValue::ZeroSized, unit_ty) })));
                        std::mem::replace(rhs, unit)
                    },
                    source_info,
                )
            };

            self.visit_rvalue(&mut rvalue, loc);
            self.assign(new_temp, rvalue, source_info.span);
        } else {
            let terminator = if self.keep_original {
                self.source.basic_blocks[loc.block.0 as usize].terminator.clone().expect("unfinished block")
            } else {
                let terminator = self.source.basic_blocks[loc.block.0 as usize].terminator.as_mut().expect("unfinished block");
                let target = match &terminator.kind {
                    TerminatorKind::Call { target: Some(target), .. } => *target,
                    kind => unreachable!("{kind:?} not promotable"),
                };
                Terminator { source_info: terminator.source_info, kind: std::mem::replace(&mut terminator.kind, TerminatorKind::Goto { target }) }
            };

            match terminator.kind {
                TerminatorKind::Call { mut func, mut args, call_source, fn_span, arg_spans, .. } => {
                    self.visit_operand(&mut func, loc);
                    for arg in &mut args {
                        self.visit_operand(arg, loc);
                    }

                    let last = self.promoted.basic_blocks.len() - 1;
                    let new_target = self.new_block();

                    self.promoted.basic_blocks[last].terminator = Some(Terminator {
                        kind: TerminatorKind::Call {
                            func,
                            args,
                            unwind: UnwindAction::Continue,
                            destination: Place::local(new_temp),
                            target: Some(new_target),
                            call_source,
                            fn_span,
                            arg_spans,
                        },
                        source_info: SourceInfo { span: terminator.source_info.span, scope: SourceScope(0) },
                    });
                }
                kind => unreachable!("{kind:?} not promotable"),
            };
        };

        self.keep_original = old_keep_original;
        new_temp
    }

    fn promote_candidate(mut self, candidate: Candidate, next_promoted_index: PromotedId) -> Body {
        let def = self.source.source.owner;
        let rvalue = {
            let tcx = self.tcx;
            let loc = candidate.location;
            let blocks = &mut self.source.basic_blocks;
            let local_decls = &mut self.source.local_decls;
            let statement = &mut blocks[loc.block.0 as usize].statements[loc.statement_index];
            let statement_source_info = statement.source_info;
            let StatementKind::Assign(_, Rvalue::Ref(region, borrow_kind, place)) = &mut statement.kind else { unreachable!("candidate is not a borrow") };

            // Use the underlying local for this (necessarily interior) borrow.
            debug_assert!(*region == Region::ERASED);
            let ty = local_decls[place.local.0 as usize].ty;
            let span = statement_source_info.span;

            let mutable = matches!(borrow_kind, BorrowKind::Mut { .. });
            let ref_ty = tcx.tys.intern(TyKind::Ref(mutable, ty, RegionId::ERASED));

            let mut projection = vec![PlaceElem::Deref];
            projection.extend(std::mem::take(&mut place.projection));
            place.projection = projection;

            // Create a temp to hold the promoted reference. This is because `*r` requires `r` to be a
            // local, otherwise we would use the `promoted` directly.
            let mut promoted_ref = local_decl(ref_ty, span, LocalKind::Temp);
            promoted_ref.source_info = statement_source_info;
            local_decls.push(promoted_ref);
            let promoted_ref = Local(local_decls.len() as u32 - 1);
            self.temps.push(TempState::Unpromotable);
            debug_assert_eq!(self.temps.len(), local_decls.len());

            // The promoted body returns the reference; the constant naming it has the same type.
            self.promoted.span = span;
            self.promoted.local_decls[0] = local_decl(ref_ty, span, LocalKind::Return);
            let promoted_operand = ConstOperand {
                span,
                user_ty: None,
                // the owner's identity arguments (erased): implied by `def`
                const_: Const::Unevaluated { def, args: Vec::new(), promoted: Some(next_promoted_index), ty: ref_ty },
            };

            let promoted_ref_statement = Statement {
                source_info: statement_source_info,
                kind: StatementKind::Assign(Place::local(promoted_ref), Rvalue::Use(Operand::Constant(Box::new(promoted_operand)))),
            };
            self.extra_statements.push((loc, promoted_ref_statement));

            Rvalue::Ref(Region::ERASED, *borrow_kind, Place::local(std::mem::replace(&mut place.local, promoted_ref)))
        };

        let mut rvalue = rvalue;
        let start = self.new_block();
        assert_eq!(start, BasicBlock(0));
        self.visit_rvalue(&mut rvalue, Location { block: BasicBlock(0), statement_index: usize::MAX });

        let span = self.promoted.span;
        self.assign(Local(0), rvalue, span);

        self.promoted
    }
}

/// Replaces all temporaries with their promoted counterparts.
impl MutVisitor for Promoter<'_> {
    fn visit_local(&mut self, local: &mut Local, _: PlaceContext, _: Location) {
        if self.is_temp_kind(*local) {
            *local = self.promote_temp(*local);
        }
    }

    /// A constant moved into the promoted keeps the allocations it names, copied into the promoted's
    /// own table.
    fn visit_alloc_id(&mut self, alloc: &mut AllocId) {
        if let Some(&mapped) = self.allocs.get(alloc) {
            *alloc = mapped;
            return;
        }
        let Some(global) = self.source.allocs.global(*alloc).cloned() else { return };
        let mapped = match global {
            GlobalAlloc::Memory(memory) => self.promoted.allocs.insert(memory),
            GlobalAlloc::Function { def, args } => self.promoted.allocs.insert_function(def, args),
            GlobalAlloc::Static(def) => self.promoted.allocs.static_alloc(def),
        };
        self.allocs.insert(*alloc, mapped);
        *alloc = mapped;
    }
}

pub(super) fn promote_candidates(tcx: &Tcx, body: &mut Body, mut temps: Vec<TempState>, candidates: Vec<Candidate>) -> Vec<Body> {
    // Visit candidates in reverse, in case they're nested.

    // eagerly fail fast
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut promotions: Vec<Body> = Vec::new();

    let mut extra_statements = vec![];
    for candidate in candidates.into_iter().rev() {
        let Location { block, statement_index } = candidate.location;
        if let StatementKind::Assign(place, _) = &body.basic_blocks[block.0 as usize].statements[statement_index].kind {
            if place.projection.is_empty() && temps[place.local.0 as usize] == TempState::PromotedOut {
                // Already promoted.
                continue;
            }
        }

        // Declare return place local so that the body is well formed.
        let never = tcx.tys.intern(TyKind::Never);
        let initial_locals = vec![local_decl(never, body.span, LocalKind::Return)];

        let scope_index = body.basic_blocks[block.0 as usize].statements[statement_index].source_info.scope;
        let mut scope = body.source_scopes[scope_index.0 as usize].clone();
        scope.parent_scope = None;

        let index = PromotedId(promotions.len() as u32);
        let promoted = Body {
            phase: MirPhase::Analysis(AnalysisPhase::Initial),
            source: MirSource { owner: body.source.owner, kind: BodyKind::Promoted(body.source.owner, index) },
            basic_blocks: Vec::new(),
            local_decls: initial_locals,
            source_scopes: vec![scope],
            span: body.span,
            // the promoted shares the parent's user type annotations
            user_type_annotations: Vec::new(),
            upvars: Vec::new(),
            allocs: AllocMap::default(),
            var_debug_info: Vec::new(),
            arg_count: 0,
            spread_arg: None,
            promoted: Vec::new(),
            coroutine: None,
        };

        let promoter = Promoter { promoted, tcx, source: body, temps: &mut temps, extra_statements: &mut extra_statements, keep_original: false, allocs: HashMap::default() };

        promotions.push(promoter.promote_candidate(candidate, index));
    }

    // Insert each of `extra_statements` before its indicated location, which has to be done in
    // reverse location order, to not invalidate the rest.
    extra_statements.sort_by_key(|(loc, _)| std::cmp::Reverse((loc.block, loc.statement_index)));
    for (loc, statement) in extra_statements {
        body.basic_blocks[loc.block.0 as usize].statements.insert(loc.statement_index, statement);
    }

    // Eliminate assignments to, and drops of promoted temps.
    let promoted = |index: Local| temps[index.0 as usize] == TempState::PromotedOut;
    for block in &mut body.basic_blocks {
        block.statements.retain(|statement| match &statement.kind {
            StatementKind::Assign(place, _) => {
                if place.projection.is_empty() {
                    !promoted(place.local)
                } else {
                    true
                }
            }
            StatementKind::StorageLive(index) | StatementKind::StorageDead(index) => !promoted(*index),
            _ => true,
        });
        let terminator = block.terminator.as_mut().expect("unfinished block");
        if let TerminatorKind::Drop { place, target, .. } = &terminator.kind {
            if place.projection.is_empty() && promoted(place.local) {
                terminator.kind = TerminatorKind::Goto { target: *target };
            }
        }
    }

    promotions
}
