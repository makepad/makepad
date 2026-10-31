//! rustc_mir_dataflow/src/framework/direction.rs:56,242; cleanup paths are edges,
//! and successful call/yield assignments are isolated from them.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { Forward, Backward }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchTargetIndex { Normal(usize), Otherwise }

#[derive(Clone, Copy)]
pub enum CallReturnPlaces<'a> { Call(&'a Place), Yield(&'a Place) }

/// rustc_middle/src/mir/terminator.rs:678,725 (Terminator::edges). InlineAsm has no
/// semantic operands in M0 and is rejected rather than treated as an empty edge set.
#[derive(Clone, Copy)]
pub enum TerminatorEdges<'a> {
    None,
    Single(BasicBlock),
    Double(BasicBlock, BasicBlock),
    AssignOnReturn { return_: Option<BasicBlock>, cleanup: Option<BasicBlock>, place: CallReturnPlaces<'a> },
    SwitchInt { targets: &'a SwitchTargets, discr: &'a Operand },
}

impl<'a> TerminatorEdges<'a> {
    pub fn for_terminator(term: &'a Terminator) -> Self {
        match &term.kind {
            TerminatorKind::Goto { target } => Self::Single(*target),
            TerminatorKind::SwitchInt { targets, discr } => Self::SwitchInt { targets, discr },
            TerminatorKind::Drop { target, unwind, .. }
            | TerminatorKind::Assert { target, unwind, .. }
            | TerminatorKind::FalseUnwind { real_target: target, unwind } => {
                match unwind {
                    UnwindAction::Cleanup(cleanup) => Self::Double(*target, *cleanup),
                    _ => Self::Single(*target),
                }
            }
            TerminatorKind::FalseEdge { real_target, imaginary_target } => Self::Double(*real_target, *imaginary_target),
            TerminatorKind::Call { target, unwind, destination, .. } => Self::AssignOnReturn {
                return_: *target,
                cleanup: match unwind { UnwindAction::Cleanup(block) => Some(*block), _ => None },
                place: CallReturnPlaces::Call(destination),
            },
            TerminatorKind::Yield { resume, resume_arg, drop, .. } => Self::AssignOnReturn {
                return_: Some(*resume), cleanup: *drop, place: CallReturnPlaces::Yield(resume_arg),
            },
            TerminatorKind::Return | TerminatorKind::TailCall { .. } | TerminatorKind::UnwindResume
            | TerminatorKind::UnwindTerminate(_) | TerminatorKind::Unreachable | TerminatorKind::CoroutineDrop => Self::None,
            TerminatorKind::InlineAsm(_) => panic!("dataflow requires semantic MIR; inline asm is unsupported"),
        }
    }

    pub(super) fn len(self) -> usize {
        match self {
            Self::None => 0,
            Self::Single(_) => 1,
            Self::Double(..) => 2,
            Self::AssignOnReturn { return_, cleanup, .. } => usize::from(return_.is_some()) + usize::from(cleanup.is_some()),
            Self::SwitchInt { targets, .. } => targets.targets.len() + 1,
        }
    }

    pub(super) fn edge(self, source: BasicBlock, i: usize) -> Edge<'a> {
        assert!(i < self.len());
        let (target, kind) = match self {
            Self::None => unreachable!(),
            Self::Single(target) => (target, EdgeKind::Plain),
            Self::Double(a, b) => (if i == 0 { a } else { b }, EdgeKind::Plain),
            Self::AssignOnReturn { return_, cleanup, place } => {
                if i == 0 && cleanup.is_some() { (cleanup.unwrap(), EdgeKind::Plain) }
                else { (return_.unwrap(), EdgeKind::Return(place)) }
            }
            Self::SwitchInt { targets, discr } => {
                if i == targets.targets.len() { (targets.otherwise, EdgeKind::Switch(targets, discr, SwitchTargetIndex::Otherwise)) }
                else { (targets.targets[i], EdgeKind::Switch(targets, discr, SwitchTargetIndex::Normal(i))) }
            }
        };
        Edge { source, target, kind }
    }
}

#[derive(Clone, Copy)]
enum EdgeKind<'a> { Plain, Return(CallReturnPlaces<'a>), Switch(&'a SwitchTargets, &'a Operand, SwitchTargetIndex) }

#[derive(Clone, Copy)]
pub(super) struct Edge<'a> {
    pub source: BasicBlock,
    pub target: BasicBlock,
    kind: EdgeKind<'a>,
}

pub(super) fn apply_edge_effect(analysis: &dyn Analysis, state: &mut BitSet, edge: Edge<'_>) {
    match edge.kind {
        EdgeKind::Plain => {}
        EdgeKind::Return(place) => {
            // rustc direction.rs:117 passes the resume block for backward Yield.
            let block = if analysis.direction() == Direction::Backward && matches!(place, CallReturnPlaces::Yield(_)) {
                edge.target
            } else { edge.source };
            analysis.apply_call_return_effect(state, block, place);
        }
        EdgeKind::Switch(targets, discr, index) => {
            if analysis.has_switch_int_edge_effect(edge.source, targets, discr) {
                assert_eq!(analysis.direction(), Direction::Forward, "backward switch edge effects are unsupported");
                analysis.apply_switch_int_edge_effect(state, edge.source, targets, discr, index);
            }
        }
    }
}

/// Numbering in analysis order keeps Early before Primary in both directions.
/// rustc framework/mod.rs:417 and direction.rs:145,315.
pub(super) fn location_for_step(body: &Body, direction: Direction, block: BasicBlock, step: usize) -> Location {
    let end = body.basic_blocks[block.0 as usize].statements.len();
    assert!(step < (end + 1) * 2);
    let statement_index = match direction { Direction::Forward => step / 2, Direction::Backward => end - step / 2 };
    Location { block, statement_index }
}

pub(super) fn apply_effect(body: &Body, analysis: &dyn Analysis, state: &mut BitSet, block: BasicBlock, step: usize) {
    let at = location_for_step(body, analysis.direction(), block, step);
    let data = &body.basic_blocks[block.0 as usize];
    if at.statement_index == data.statements.len() {
        let term = data.terminator.as_ref().expect("unfinished MIR block");
        if step % 2 == 0 { analysis.apply_early_terminator_effect(state, term, at); }
        else { analysis.apply_primary_terminator_effect(state, term, at); }
    } else {
        let stmt = &data.statements[at.statement_index];
        if step % 2 == 0 { analysis.apply_early_statement_effect(state, stmt, at); }
        else { analysis.apply_primary_statement_effect(state, stmt, at); }
    }
}

/// Flat edge/predecessor storage; DFS postorder matches rustc's initial work queue.
pub(super) struct Cfg<'a> {
    pub edges: Vec<Edge<'a>>,
    pub pred_offsets: Vec<usize>,
    pub predecessors: Vec<usize>,
    pub reachable: BitSet,
    pub postorder: Vec<BasicBlock>,
}

impl<'a> Cfg<'a> {
    pub fn new(body: &'a Body) -> Self {
        let count = body.basic_blocks.len();
        let mut cfg = Self {
            edges: Vec::new(), pred_offsets: vec![0; count + 1], predecessors: Vec::new(),
            reachable: BitSet::new_empty(count), postorder: Vec::new(),
        };
        let mut offsets = vec![0; count + 1];
        for b in 0..count {
            let term = body.basic_blocks[b].terminator.as_ref().expect("unfinished MIR block");
            let edges = TerminatorEdges::for_terminator(term);
            for i in 0..edges.len() {
                let edge = edges.edge(BasicBlock(b as u32), i);
                assert!((edge.target.0 as usize) < count, "invalid MIR target");
                cfg.pred_offsets[edge.target.0 as usize + 1] += 1;
                cfg.edges.push(edge);
            }
            offsets[b + 1] = cfg.edges.len();
        }
        for b in 0..count { cfg.pred_offsets[b + 1] += cfg.pred_offsets[b]; }
        cfg.predecessors.resize(cfg.edges.len(), 0);
        let mut next = cfg.pred_offsets.clone();
        for i in 0..cfg.edges.len() {
            let b = cfg.edges[i].target.0 as usize;
            cfg.predecessors[next[b]] = i;
            next[b] += 1;
        }
        let mut stack = vec![(BasicBlock(0), offsets[0])];
        cfg.reachable.insert(0);
        while let Some((block, edge_index)) = stack.last_mut() {
            if *edge_index == offsets[block.0 as usize + 1] {
                cfg.postorder.push(*block);
                stack.pop();
            } else {
                let target = cfg.edges[*edge_index].target;
                *edge_index += 1;
                if cfg.reachable.insert(target.0 as usize) { stack.push((target, offsets[target.0 as usize])); }
            }
        }
        cfg
    }
}
