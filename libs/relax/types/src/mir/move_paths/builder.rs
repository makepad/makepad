//! rustc_mir_dataflow/src/move_paths/builder.rs:24,107,363,379,423,448,539,565.
use super::*;
use crate::types::{TyKind};

struct Builder<'a> {
    body: &'a Body, prog: &'a Program, tcx: &'a Tcx, filter: &'a dyn Fn(TyId) -> bool,
    loc: Location, data: MoveData, env: crate::tcx::TypingEnv,
}

pub(super) fn gather_moves(body: &Body, prog: &Program, tcx: &Tcx, env: crate::tcx::TypingEnv, filter: &dyn Fn(TyId) -> bool) -> MoveData {
    let mut builder = Builder {
        body, prog, tcx, filter, env, loc: Location::default(),
        data: MoveData {
            move_paths: Vec::new(), moves: Vec::new(), inits: Vec::new(),
            loc_map: LocationMap::new(body), init_loc_map: LocationMap::new(body),
            path_map: Vec::new(), init_path_map: Vec::new(),
            rev_lookup: MovePathLookup { locals: vec![None; body.local_decls.len()], projections: HashMap::new(),
                deref_chains: vec![Vec::new(); body.local_decls.len()] },
        },
    };
    for (i, decl) in body.local_decls.iter().enumerate() {
        if !matches!(decl.local_info, LocalInfo::DerefTemp) && filter(decl.ty) {
            let path = builder.new_path(None, Place::local(Local(i as u32)));
            builder.data.rev_lookup.locals[i] = Some(path);
        }
    }
    for i in 1..=body.arg_count {
        if let Some(path) = builder.data.rev_lookup.find_local(Local(i)) {
            builder.record_init(path, InitKind::Deep, InitLocation::Argument(Local(i)));
        }
    }
    for (i, block) in body.basic_blocks.iter().enumerate() {
        for (j, stmt) in block.statements.iter().enumerate() {
            builder.loc = Location { block: BasicBlock(i as u32), statement_index: j };
            builder.statement(stmt);
        }
        builder.loc = Location { block: BasicBlock(i as u32), statement_index: block.statements.len() };
        builder.terminator(block.terminator.as_ref().expect("unfinished MIR"));
    }
    builder.data
}

impl Builder<'_> {
    fn new_path(&mut self, parent: Option<usize>, place: Place) -> usize {
        let index = self.data.move_paths.len();
        let next_sibling = parent.and_then(|p| self.data.move_paths[p].first_child);
        self.data.move_paths.push(MovePath { parent, place, first_child: None, next_sibling });
        if let Some(p) = parent { self.data.move_paths[p].first_child = Some(index); }
        self.data.path_map.push(Vec::new());
        self.data.init_path_map.push(Vec::new());
        index
    }
    fn add_path(&mut self, base: usize, key: MoveSubPath, place: Place) -> usize {
        if let Some(&path) = self.data.rev_lookup.projections.get(&(base, key)) { return path; }
        let path = self.new_path(Some(base), place);
        self.data.rev_lookup.projections.insert((base, key), path);
        path
    }
    fn place_ty(&self, place: &Place) -> PlaceTy {
        place.ty_normalized(self.body, self.prog, self.tcx, self.env).expect("invalid MIR projection")
    }
    /// All prefixes are created even if a later projection prevents recording the move.
    /// Union coalescing does not bypass the remaining legality checks.
    fn move_path_for(&mut self, place: &Place, moving: bool) {
        let Some(mut base) = self.data.rev_lookup.find_local(place.local) else { return; };
        let mut union_path = None;
        // Own the chain while adding paths to the reverse lookup.
        let mut projections = Vec::new();
        for (source, len, elem) in self.data.rev_lookup.iter_projections(place) {
            let prefix = Place { local: source.local, projection: source.projection[..len].to_vec() };
            projections.push((prefix, elem.clone()));
        }
        for (i, (prefix, elem)) in projections.iter().enumerate() {
            let ty = self.place_ty(prefix);
            if self.tcx.tys.has_error(ty.ty) { return; }
            let key = match MoveSubPath::of(elem) {
                SubPath::Skip => continue,
                SubPath::Stop => return,
                SubPath::Subslice { from, to } => {
                    for (_, trailing) in &projections[i + 1..] { assert!(MoveSubPath::of(trailing) == SubPath::Skip); }
                    let TyKind::Array(inner, len) = self.tcx.tys.kind(ty.ty) else { panic!("fixed subslice requires array"); };
                    let len = len.try_to_target_usize(&self.tcx.tys).expect("subslice requires fixed array length");
                    if !(self.filter)(*inner) { return; }
                    for offset in from..to {
                        let mut element = prefix.clone();
                        element.projection.push(PlaceElem::ConstantIndex { offset, min_length: len, from_end: false });
                        let path = self.add_path(base, MoveSubPath::ConstantIndex(offset), element);
                        if moving { self.record_move(path); }
                    }
                    return;
                }
                SubPath::One(key) => key,
            };
            match key {
                MoveSubPath::Deref => match self.tcx.tys.kind(ty.ty) {
                    TyKind::Ref(..) | TyKind::Ptr(..) => return,
                    TyKind::Adt(def, _, _) => assert!(self.tcx.adts[def].is_box(), "ADT deref must be Box"),
                    _ => panic!("invalid MIR dereference"),
                },
                MoveSubPath::Field(_) => match self.tcx.tys.kind(ty.ty) {
                    TyKind::Adt(def, _, _) => {
                        if self.tcx.adt_destructor(*def).is_some() { return; }
                        if self.tcx.adts[def].is_union() { union_path.get_or_insert(base); }
                    }
                    TyKind::Closure(..) | TyKind::Tuple(_) => (),
                    _ => panic!("invalid MIR field base"),
                },
                MoveSubPath::ConstantIndex(_) => match self.tcx.tys.kind(ty.ty) {
                    TyKind::Slice(_) => return,
                    TyKind::Array(..) => (),
                    _ => panic!("invalid MIR constant index"),
                },
                MoveSubPath::Downcast(_) => (),
            }
            let child_ty = ty.projection_ty_normalized(self.prog, self.tcx, self.env, elem).expect("invalid MIR projection").ty;
            if !(self.filter)(child_ty) { return; }
            if union_path.is_none() {
                let mut normalized = elem.clone();
                if let PlaceElem::Field(_, field_ty) = &mut normalized { *field_ty = child_ty; }
                let mut child = prefix.clone();
                child.projection.push(normalized);
                base = self.add_path(base, key, child);
            }
        }
        if moving { self.record_move(union_path.unwrap_or(base)); }
    }
    fn record_move(&mut self, path: usize) {
        let index = self.data.moves.len();
        self.data.moves.push(MoveOut { path, source: self.loc });
        self.data.loc_map[self.loc].push(index);
        self.data.path_map[path].push(index);
    }
    fn record_init(&mut self, path: usize, kind: InitKind, location: InitLocation) {
        let index = self.data.inits.len();
        self.data.inits.push(Init { path, kind, location });
        self.data.init_path_map[path].push(index);
        if let InitLocation::Statement(loc) = location { self.data.init_loc_map[loc].push(index); }
    }
    fn init(&mut self, place: &Place, kind: InitKind) {
        let mut place = place.clone();
        if matches!(place.projection.last(), Some(PlaceElem::Field(..))) {
            let mut prefix = place.clone();
            prefix.projection.pop();
            if let TyKind::Adt(def, _, _) = self.tcx.tys.kind(self.place_ty(&prefix).ty) {
                if self.tcx.adts[def].is_union() { place = prefix; }
            }
        }
        if let LookupResult::Exact(path) = self.data.rev_lookup.find(&place) {
            self.record_init(path, kind, InitLocation::Statement(self.loc));
        }
    }
    fn statement(&mut self, stmt: &Statement) {
        match &stmt.kind {
            StatementKind::Assign(place, Rvalue::CopyForDeref(reffed)) => {
                assert!(place.projection.is_empty());
                assert!(matches!(self.body.local_decls[place.local.0 as usize].local_info, LocalInfo::DerefTemp));
                let lookup = &mut self.data.rev_lookup;
                let mut chain = std::mem::take(&mut lookup.deref_chains[reffed.local.0 as usize]);
                chain.push(reffed.clone());
                lookup.locals[place.local.0 as usize] = lookup.locals[chain[0].local.0 as usize];
                lookup.deref_chains[place.local.0 as usize] = chain;
            }
            StatementKind::Assign(place, rvalue) => {
                self.move_path_for(place, false);
                self.init(place, InitKind::Deep);
                self.rvalue(rvalue);
            }
            StatementKind::FakeRead(_, place) => self.move_path_for(place, false),
            StatementKind::StorageDead(local) => {
                if !matches!(self.body.local_decls[local.0 as usize].local_info, LocalInfo::DerefTemp) {
                    self.move_path_for(&Place::local(*local), true);
                }
            }
            StatementKind::SetDiscriminant { .. } => panic!("SetDiscriminant must not exist during move gathering"),
            StatementKind::StorageLive(_) | StatementKind::PlaceMention(_)
            | StatementKind::AscribeUserType(..) | StatementKind::Intrinsic(_)
            | StatementKind::ConstEvalCounter | StatementKind::Nop | StatementKind::BackwardIncompatibleDropHint { .. } => (),
        }
    }
    fn operand(&mut self, operand: &Operand) {
        if let Operand::Move(place) = operand { self.move_path_for(place, true); }
    }
    fn rvalue(&mut self, value: &Rvalue) {
        match value {
            Rvalue::Use(op) | Rvalue::Repeat(op, _) | Rvalue::Cast(_, op, _)
            | Rvalue::UnaryOp(_, op) => self.operand(op),
            Rvalue::BinaryOp(_, ops) => { self.operand(&ops.0); self.operand(&ops.1); }
            Rvalue::Aggregate(_, ops) => { for op in ops { self.operand(op); } }
            Rvalue::CopyForDeref(_) => unreachable!(),
            Rvalue::Ref(..) | Rvalue::RawPtr(..) | Rvalue::Discriminant(_)
            | Rvalue::ThreadLocalRef(_) => (),
        }
    }
    fn terminator(&mut self, term: &Terminator) {
        match &term.kind {
            TerminatorKind::SwitchInt { discr, .. } => self.operand(discr),
            TerminatorKind::Assert { cond, .. } => self.operand(cond),
            TerminatorKind::Call { func, args, destination, target, .. } => {
                self.operand(func);
                for arg in args { self.operand(arg); }
                if target.is_some() {
                    self.move_path_for(destination, false);
                    self.init(destination, InitKind::NonPanicPathOnly);
                }
            }
            TerminatorKind::TailCall { func, args, .. } => { self.operand(func); for arg in args { self.operand(arg); } }
            TerminatorKind::Yield { value, resume_arg, .. } => {
                self.operand(value);
                self.move_path_for(resume_arg, false);
                self.init(resume_arg, InitKind::Deep);
            }
            TerminatorKind::InlineAsm(_) => panic!("move paths require semantic MIR; inline asm is unsupported"),
            TerminatorKind::Goto { .. } | TerminatorKind::FalseEdge { .. } | TerminatorKind::FalseUnwind { .. }
            | TerminatorKind::Return | TerminatorKind::UnwindResume | TerminatorKind::UnwindTerminate(_)
            | TerminatorKind::CoroutineDrop | TerminatorKind::Unreachable | TerminatorKind::Drop { .. } => (),
        }
    }
}
