//! Move paths, MaybeUninit / EverInit dataflow and the move / initialization / mutability
//! checks (rustc_mir_dataflow move_paths + MaybeUninitializedPlaces + EverInitializedPlaces;
//! rustc_borrowck check_if_full_path_is_moved / check_if_path_or_subpath_is_moved /
//! check_if_assigned_path_is_moved / check_movable_place / check_access_permissions).

use crate::types::AliasTyKind;
use crate::types::{AliasTy};
use super::bir::*;
use super::diag::{describe_place, Diag};
use super::tyx::Tx;
use crate::fxhash::HashMap;
use crate::types::{TyId, TyKind};

pub type Bits = Vec<u64>;

pub fn bits_new(n: usize) -> Bits {
    vec![0u64; (n + 63) / 64]
}
#[inline]
pub fn bit_get(b: &Bits, i: usize) -> bool {
    b[i >> 6] >> (i & 63) & 1 != 0
}
#[inline]
pub fn bit_set(b: &mut Bits, i: usize) {
    b[i >> 6] |= 1 << (i & 63);
}
#[inline]
pub fn bit_clear(b: &mut Bits, i: usize) {
    b[i >> 6] &= !(1 << (i & 63));
}
pub fn bits_union(a: &mut Bits, b: &Bits) -> bool {
    let mut ch = false;
    for (x, y) in a.iter_mut().zip(b.iter()) {
        let n = *x | *y;
        if n != *x {
            *x = n;
            ch = true;
        }
    }
    ch
}

pub struct MovePath {
    pub place: Place,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
}

pub struct MoveData {
    pub paths: Vec<MovePath>,
    pub lookup: HashMap<Place, u32>,
    /// an array subslice place -> the element paths it covers (rustc: subslices of arrays move
    /// their elements)
    pub subslices: HashMap<Place, Vec<u32>>,
    /// root path per local
    pub roots: Vec<u32>,
}

/// Type of the place after `n` projections.
pub fn place_ty_n(tx: &Tx, bir: &Bir, p: &Place, n: usize) -> TyId {
    let mut t = bir.locals[p.local as usize].ty;
    for pr in &p.proj[..n] {
        t = proj_ty(tx, t, pr);
    }
    t
}

pub fn proj_ty(tx: &Tx, t: TyId, pr: &Proj) -> TyId {
    match pr {
        Proj::Deref => tx.builtin_deref(t).unwrap_or(t),
        Proj::Field(_, ft) => *ft,
        Proj::Index(_) | Proj::ConstIndex(..) => match tx.kind(t) {
            TyKind::Array(e, _) | TyKind::Slice(e) => e,
            _ => t,
        },
        Proj::Subslice(..) | Proj::Downcast(_) => t,
    }
}

pub fn place_ty(tx: &Tx, bir: &Bir, p: &Place) -> TyId {
    place_ty_n(tx, bir, p, p.proj.len())
}

impl MoveData {
    pub fn new(tx: &Tx, bir: &Bir) -> MoveData {
        let mut md = MoveData { paths: Vec::new(), lookup: HashMap::default(), subslices: HashMap::default(), roots: Vec::new() };
        for l in 0..bir.locals.len() {
            let p = Place::local(l as u32);
            md.roots.push(md.paths.len() as u32);
            md.lookup.insert(p.clone(), md.paths.len() as u32);
            md.paths.push(MovePath { place: p, parent: None, children: Vec::new() });
        }
        for blk in &bir.blocks {
            crate::preempt::point();
            for s in &blk.stmts {
                crate::preempt::point();
                if let StmtKind::Assign(p, rv) = &s.kind {
                    md.add(tx, bir, p);
                    match rv {
                        Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => md.add_op(tx, bir, o),
                        Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                            for o in ops {
                                md.add_op(tx, bir, o);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(t) = &blk.term {
                match &t.kind {
                    TermKind::Call { args, dest, .. } => {
                        md.add(tx, bir, dest);
                        for o in args {
                            md.add_op(tx, bir, o);
                        }
                    }
                    TermKind::Switch(o, _) => md.add_op(tx, bir, o),
                    TermKind::Drop(p, _) => md.add(tx, bir, p),
                    _ => {}
                }
            }
        }
        md
    }

    fn add_op(&mut self, tx: &Tx, bir: &Bir, o: &Operand) {
        if let Operand::Move(p) = o {
            self.add(tx, bir, p);
        }
    }

    /// Creates move paths for the movable prefixes of `p` (stops at a deref of a reference /
    /// raw pointer, an index, a slice element).
    fn add(&mut self, tx: &Tx, bir: &Bir, p: &Place) {
        let mut cur = self.roots[p.local as usize];
        let mut t = bir.locals[p.local as usize].ty;
        for (i, pr) in p.proj.iter().enumerate() {
            let movable = match pr {
                Proj::Deref => tx.box_inner(t).is_some(),
                Proj::Index(_) => false,
                Proj::ConstIndex(..) | Proj::Subslice(..) => matches!(tx.kind(t), TyKind::Array(..)),
                Proj::Field(..) | Proj::Downcast(_) => true,
            };
            if !movable {
                return;
            }
            let q = Place { local: p.local, proj: p.proj[..=i].to_vec() };
            if let (Proj::Subslice(from, to, false), TyKind::Array(_, len)) = (pr, tx.kind(t)) {
                // the elements of an array subslice are its move paths
                if let Some(len) = len.try_to_target_usize(&tx.tcx.tys) {
                    let mut els = Vec::new();
                    for k in *from..*to {
                        let mut e = Place { local: p.local, proj: p.proj[..i].to_vec() };
                        e.proj.push(Proj::ConstIndex(k, false, len as u32));
                        self.add(tx, bir, &e);
                        if let Some(m) = self.exact(&e) {
                            els.push(m);
                        }
                    }
                    self.subslices.insert(q, els);
                }
                return;
            }
            if tx.is_union(t) {
                // a union's fields are the union (rustc: moves / inits of a field are of the
                // whole union)
                self.lookup.entry(q).or_insert(cur);
                return;
            }
            cur = match self.lookup.get(&q) {
                Some(&m) => m,
                None => {
                    let m = self.paths.len() as u32;
                    self.paths.push(MovePath { place: q.clone(), parent: Some(cur), children: Vec::new() });
                    self.paths[cur as usize].children.push(m);
                    self.lookup.insert(q, m);
                    m
                }
            };
            t = proj_ty(tx, t, pr);
        }
    }

    /// The path of `p` itself, if tracked.
    pub fn exact(&self, p: &Place) -> Option<u32> {
        self.lookup.get(p).copied()
    }

    /// The closest tracked prefix of `p` (always exists: the local's root).
    pub fn closest(&self, p: &Place) -> (u32, usize) {
        for n in (0..=p.proj.len()).rev() {
            let q = Place { local: p.local, proj: p.proj[..n].to_vec() };
            if let Some(&m) = self.lookup.get(&q) {
                return (m, n);
            }
        }
        (self.roots[p.local as usize], 0)
    }

    pub fn for_subtree(&self, m: u32, f: &mut dyn FnMut(u32)) {
        f(m);
        for &c in &self.paths[m as usize].children {
            self.for_subtree(c, f);
        }
    }

    pub fn find_in_subtree(&self, m: u32, pred: &dyn Fn(u32) -> bool) -> Option<u32> {
        if pred(m) {
            return Some(m);
        }
        for &c in &self.paths[m as usize].children {
            if let Some(x) = self.find_in_subtree(c, pred) {
                return Some(x);
            }
        }
        None
    }
}

/// Reverse postorder of the blocks reachable from the entry, and predecessors.
pub fn rpo(bir: &Bir) -> Vec<u32> {
    let n = bir.blocks.len();
    let mut seen = vec![false; n];
    let mut post = Vec::with_capacity(n);
    let mut stack: Vec<(u32, usize)> = vec![(0, 0)];
    seen[0] = true;
    while let Some(&mut (b, ref mut i)) = stack.last_mut() {
        crate::preempt::point();
        let succ = successors(bir, b);
        if *i < succ.len() {
            let s = succ[*i];
            *i += 1;
            if !seen[s as usize] {
                seen[s as usize] = true;
                stack.push((s, 0));
            }
        } else {
            post.push(b);
            stack.pop();
        }
    }
    post.reverse();
    post
}

pub fn successors(bir: &Bir, b: u32) -> Vec<u32> {
    match bir.blocks[b as usize].term.as_ref().map(|t| &t.kind) {
        Some(TermKind::Goto(t)) => vec![*t],
        Some(TermKind::Switch(_, ts)) => ts.clone(),
        Some(TermKind::Call { target: Some(t), .. }) => vec![*t],
        Some(TermKind::Drop(_, t)) => vec![*t],
        Some(TermKind::FalseEdge(a, b2)) => vec![*a, *b2],
        Some(TermKind::FalseUnwind(t)) => vec![*t],
        _ => Vec::new(),
    }
}

/// Move / init effects of one statement on the MaybeUninit bits (`uninit`) and EverInit bits
/// per local (`ever`).
pub fn stmt_effect(md: &MoveData, s: &StmtKind, uninit: &mut Bits, ever: &mut Bits) {
    match s {
        StmtKind::Assign(p, rv) => {
            match rv {
                Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => move_op(md, o, uninit),
                Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                    for o in ops {
                        move_op(md, o, uninit);
                    }
                }
                _ => {}
            }
            init_place(md, p, uninit, ever);
        }
        StmtKind::StorageLive(l) | StmtKind::StorageDead(l) => {
            md.for_subtree(md.roots[*l as usize], &mut |m| bit_set(uninit, m as usize));
            bit_clear(ever, *l as usize);
        }
        _ => {}
    }
}

fn move_op(md: &MoveData, o: &Operand, uninit: &mut Bits) {
    if let Operand::Move(p) = o {
        if let Some(m) = md.exact(p) {
            md.for_subtree(m, &mut |x| bit_set(uninit, x as usize));
        } else if let Some(els) = md.subslices.get(p) {
            for m in els {
                md.for_subtree(*m, &mut |x| bit_set(uninit, x as usize));
            }
        }
    }
}

fn init_place(md: &MoveData, p: &Place, uninit: &mut Bits, ever: &mut Bits) {
    if let Some(m) = md.exact(p) {
        md.for_subtree(m, &mut |x| bit_clear(uninit, x as usize));
        // (a union field's path is the union's: writing `x.a` initializes the local `x`)
        if p.proj.is_empty() || m == md.roots[p.local as usize] {
            bit_set(ever, p.local as usize);
        }
    }
}

/// Effects of a terminator before its successors (moves of the arguments; the call's
/// destination is initialized on the return edge, a drop leaves the place uninitialized).
pub fn term_effect(md: &MoveData, t: &TermKind, uninit: &mut Bits, ever: &mut Bits) {
    match t {
        TermKind::Call { args, dest, .. } => {
            for o in args {
                move_op(md, o, uninit);
            }
            init_place(md, dest, uninit, ever);
        }
        TermKind::Switch(o, _) => move_op(md, o, uninit),
        // rustc gather_moves: a Drop is not a move (StorageDead ends the local)
        _ => {}
    }
}

pub struct InitFlow {
    /// per block: (maybe-uninit bits, ever-init bits per local) at entry
    pub entry: Vec<Option<(Bits, Bits)>>,
}

pub fn init_flow(md: &MoveData, bir: &Bir, order: &[u32]) -> InitFlow {
    let n = bir.blocks.len();
    let np = md.paths.len();
    let nl = bir.locals.len();
    let mut entry: Vec<Option<(Bits, Bits)>> = vec![None; n];
    // entry: everything uninitialized except the args (and their subpaths)
    let mut u0 = bits_new(np);
    for i in 0..np {
        bit_set(&mut u0, i);
    }
    let mut e0 = bits_new(nl);
    for a in 1..=bir.n_args as usize {
        if a < nl {
            md.for_subtree(md.roots[a], &mut |m| bit_clear(&mut u0, m as usize));
            bit_set(&mut e0, a);
        }
    }
    entry[0] = Some((u0, e0));
    let mut changed = true;
    let mut iters = 0;
    while changed && iters < 200 {
        changed = false;
        iters += 1;
        for &b in order {
            crate::preempt::point();
            let (mut u, mut e) = match &entry[b as usize] {
                Some(x) => x.clone(),
                None => continue,
            };
            let blk = &bir.blocks[b as usize];
            for s in &blk.stmts {
                crate::preempt::point();
                stmt_effect(md, &s.kind, &mut u, &mut e);
            }
            if let Some(t) = &blk.term {
                term_effect(md, &t.kind, &mut u, &mut e);
            }
            for s in successors(bir, b) {
                match &mut entry[s as usize] {
                    Some((su, se)) => {
                        let c1 = bits_union(su, &u);
                        let c2 = bits_union(se, &e);
                        changed |= c1 || c2;
                    }
                    slot @ None => {
                        *slot = Some((u.clone(), e.clone()));
                        changed = true;
                    }
                }
            }
        }
    }
    InitFlow { entry }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    Use,
    Borrow,
    MatchOn,
    Assign,
}

pub struct MoveCheck<'a> {
    pub tx: &'a Tx<'a>,
    pub bir: &'a Bir,
    pub md: &'a MoveData,
    pub diags: Vec<Diag>,
    reported: Vec<(Place, Span)>,
    reported_moves: Vec<u32>,
    /// Binding declaration span -> (diagnostic, number of attempted mutable borrows).
    buffered_mut_errors: HashMap<(u32, u32), (Diag, usize)>,
    /// rustc MirBorrowckCtxt::used_mut: locals whose `mut` a write or mutable borrow needed
    /// (the `unused_mut` lint's input)
    pub used_mut: Bits,
}

impl<'a> MoveCheck<'a> {
    pub fn new(tx: &'a Tx<'a>, bir: &'a Bir, md: &'a MoveData) -> MoveCheck<'a> {
        MoveCheck { tx, bir, md, diags: Vec::new(), reported: Vec::new(), reported_moves: Vec::new(), buffered_mut_errors: HashMap::default(), used_mut: bits_new(bir.locals.len()) }
    }

    pub fn run(&mut self, flow: &InitFlow, order: &[u32]) {
        for &b in order {
            crate::preempt::point();
            let (mut u, mut e) = match &flow.entry[b as usize] {
                Some(x) => x.clone(),
                None => continue,
            };
            let blk = &self.bir.blocks[b as usize];
            for (si, s) in blk.stmts.iter().enumerate() {
                crate::preempt::point();
                self.visit_stmt(b, si as u32, s, &u, &e);
                stmt_effect(self.md, &s.kind, &mut u, &mut e);
            }
            if let Some(t) = &blk.term {
                self.visit_term(b, t, &u, &e);
            }
        }
        // rustc diagnostics/mod.rs: buffered_mut_errors are emitted once per binding.
        let mut buffered: Vec<_> = self.buffered_mut_errors.drain().collect();
        buffered.sort_by_key(|(span, _)| *span);
        for (_, (mut diag, count)) in buffered {
            if count > 10 {
                diag.note(format!("...and {} other attempted mutable borrows", count - 10));
            }
            self.diags.push(diag);
        }
    }

    fn visit_stmt(&mut self, b: u32, si: u32, s: &Statement, u: &Bits, e: &Bits) {
        match &s.kind {
            StmtKind::Assign(p, rv) => {
                self.consume_rvalue(b, si, rv, s.span, u, e);
                self.mutate_place(b, si, p, s.span, u, e, false);
            }
            // a guard binding read at the guard's end is not a use of its value (moving it in
            // the guard is its own error, E0507)
            StmtKind::FakeRead(FakeReadCause::ForGuardBinding, p) if self.bir.guard_locals.iter().any(|g| g.0 == p.local) => {}
            StmtKind::FakeRead(_, p) => self.path_or_subpath(b, si, p, s.span, UseKind::Use, u),
            _ => {}
        }
    }

    fn visit_term(&mut self, b: u32, t: &Term, u: &Bits, e: &Bits) {
        let si = self.bir.blocks[b as usize].stmts.len() as u32;
        match &t.kind {
            TermKind::Switch(o, _) => self.consume_op(b, si, o, t.span, u),
            TermKind::Call { args, dest, arg_spans, .. } => {
                for (i, o) in args.iter().enumerate() {
                    let sp = arg_spans.get(i).copied().unwrap_or(t.span);
                    self.consume_op(b, si, o, sp, u);
                }
                // the destination is written after the call: its init state is after the moves
                let mut u2 = u.clone();
                let mut e2 = e.clone();
                for o in args {
                    move_op(self.md, o, &mut u2);
                }
                let _ = &mut e2;
                self.mutate_place(b, si, dest, t.span, &u2, &e2, true);
            }
            _ => {}
        }
    }

    fn consume_rvalue(&mut self, b: u32, si: u32, rv: &Rvalue, span: Span, u: &Bits, e: &Bits) {
        match rv {
            Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => self.consume_op(b, si, o, span, u),
            Rvalue::Aggregate(kind, ops) => {
                // a value moved into a closure that may mutate it there needs the binding's `mut`
                // (rustc used_mut_upvars): every local a closure captures by value counts
                if matches!(kind, AggKind::Closure(_)) {
                    for o in ops {
                        if let Operand::Move(p) | Operand::Copy(p) = o { if (p.local as usize) < self.bir.locals.len() { bit_set(&mut self.used_mut, p.local as usize); } }
                    }
                }
                for o in ops {
                    self.consume_op(b, si, o, span, u);
                }
            }
            Rvalue::Op(_, ops) => {
                for o in ops {
                    self.consume_op(b, si, o, span, u);
                }
            }
            Rvalue::Ref(_, bk, p) => {
                // a closure's unique borrow of a guard binding mutates it (rustc E0596 in the guard)
                if bk.is_mut() && (*bk != BorrowKind::Unique || self.bir.guard_locals.iter().any(|g| g.0 == p.local)) {
                    self.check_mutable(p, span, e, true);
                }
                let k = if *bk == BorrowKind::Shallow { UseKind::MatchOn } else { UseKind::Borrow };
                self.path_or_subpath(b, si, p, span, k, u);
            }
            Rvalue::RawPtr(m, p) => {
                if *m {
                    self.check_mutable(p, span, e, true);
                }
                self.path_or_subpath(b, si, p, span, UseKind::Borrow, u);
            }
            Rvalue::Discriminant(p) | Rvalue::Len(p) => self.path_or_subpath(b, si, p, span, UseKind::Use, u),
        }
    }

    fn consume_op(&mut self, b: u32, si: u32, o: &Operand, span: Span, u: &Bits) {
        match o {
            Operand::Copy(p) => self.path_or_subpath(b, si, p, span, UseKind::Use, u),
            Operand::Move(p) => {
                // a pattern binding: rustc reports at the matched expression
                let msp = self.bir.move_sites.iter().find(|x| x.0 == (b, si)).map_or(span, |x| x.1);
                self.check_movable(p, msp);
                self.path_or_subpath(b, si, p, span, UseKind::Use, u);
            }
            Operand::Const(_) => {}
        }
    }

    /// E0507 / E0508 / E0509: moving out of something that cannot be moved out of.
    /// rustc_borrowck/lib.rs:464-509: promotions still need illegal-move
    /// checks (not a second, independent borrow/region-inference pass).
    pub fn check_promoted_moves(&mut self) {
        let bir = self.bir;
        for (bb, block) in bir.blocks.iter().enumerate() {
            crate::preempt::point();
            for (si, stmt) in block.stmts.iter().enumerate() {
                crate::preempt::point();
                let span = bir.move_sites.iter().find(|(l, _)| *l == (bb as Bb, si as u32)).map_or(stmt.span, |(_, s)| *s);
                if let StmtKind::Assign(_, rv) = &stmt.kind {
                    match rv {
                        Rvalue::Use(o) | Rvalue::Repeat(o) | Rvalue::Cast(_, o, _) => {
                            if let Operand::Move(p) = o { self.check_movable(p, span); }
                        }
                        Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                            for o in ops { if let Operand::Move(p) = o { self.check_movable(p, span); } }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(t) = &block.term {
                match &t.kind {
                    TermKind::Call { args, .. } => {
                        for o in args { if let Operand::Move(p) = o { self.check_movable(p, t.span); } }
                    }
                    TermKind::Switch(Operand::Move(p), _) => self.check_movable(p, t.span),
                    _ => {}
                }
            }
        }
    }

    fn check_movable(&mut self, p: &Place, span: Span) {
        // a binding moved inside its match guard (rustc: the guard sees a reference to it)
        if let Some((_, name)) = self.bir.guard_locals.iter().find(|x| x.0 == p.local) {
            if p.proj.is_empty() || p.proj == [Proj::Deref] {
                let mut d = Diag::new("E0507", format!("cannot move out of `{}` in pattern guard", name), span);
                d.label(span, "move occurs because the binding is borrowed in the guard".to_string(), true);
                d.note("variables bound in patterns cannot be moved from until after the end of the pattern guard".to_string());
                self.push_diag(d, p.clone(), span);
                return;
            }
        }
        let mut t = self.bir.locals[p.local as usize].ty;
        for (i, pr) in p.proj.iter().enumerate() {
            match pr {
                Proj::Deref => {
                    if self.tx.box_inner(t).is_none() {
                        let base = Place { local: p.local, proj: p.proj[..=i].to_vec() };
                        let what = describe_place(self.tx, self.bir, p).unwrap_or_else(|| "value".to_string());
                        let (code, kind) = match self.tx.kind(t) {
                            TyKind::Ref(true, _, _) => ("E0507", "a mutable reference"),
                            TyKind::Ref(false, _, _) => ("E0507", "a shared reference"),
                            TyKind::Ptr(..) => ("E0507", "a raw pointer"),
                            // a deref of a type this body does not know (an unnormalized
                            // associated type of a generic body): no verdict
                            TyKind::Alias(_, AliasTy { kind: AliasTyKind::Projection { .. }, .. }) | TyKind::Param(_) | TyKind::Alias(_, AliasTy { kind: AliasTyKind::Opaque { .. }, .. }) | TyKind::Error | TyKind::Infer(_) => return,
                            _ => ("E0507", "a raw pointer"),
                        };
                        let upvar = self.upvar_of(&base).or_else(|| if base.local == 1 && base.proj == [Proj::Deref] { self.upvar_of(p) } else { None });
                        // rustc report_cannot_move_from_borrowed_content: through an overloaded
                        // deref / index the message names the container
                        let ov = if i == 0 { self.bir.ov_results.iter().find(|x| x.0 == p.local).cloned() } else { None };
                        let msg = match (upvar, ov) {
                            (_, Some((_, bt, true))) => format!("cannot move out of index of `{}`", self.tx.ty_str(bt)),
                            (_, Some((_, bt, false))) => {
                                let s = self.tx.ty_str(bt);
                                if s.starts_with("Rc<") || s.starts_with("Arc<") {
                                    format!("cannot move out of an `{}`", &s[..s.find('<').unwrap_or(s.len())])
                                } else {
                                    format!("cannot move out of dereference of `{}`", s)
                                }
                            }
                            (upvar, None) => match upvar {
                            Some(name) => {
                                let k = if self.bir.closure_kind == Some(ClosureKind::FnMut) { "FnMut" } else { "Fn" };
                                if what == name {
                                    format!("cannot move out of `{}`, a captured variable in an `{}` closure", name, k)
                                } else {
                                    format!("cannot move out of `{}`, as `{}` is a captured variable in an `{}` closure", what, name, k)
                                }
                            }
                            None => format!("cannot move out of `{}` which is behind {}", what, kind),
                            },
                        };
                        let mut d = Diag::new(code, msg, span);
                        let pt = place_ty(self.tx, self.bir, p);
                        d.label(span, format!("move occurs because `{}` has type `{}`, which does not implement the `Copy` trait", what, self.tx.ty_str(pt)), true);
                        self.push_diag(d, p.clone(), span);
                        return;
                    }
                }
                Proj::Field(..) => {
                    if self.tx.has_drop_impl(t) {
                        let mut d = Diag::new("E0509", format!("cannot move out of type `{}`, which implements the `Drop` trait", self.tx.ty_str(t)), span);
                        d.label(span, "cannot move out of here".to_string(), true);
                        self.push_diag(d, p.clone(), span);
                        return;
                    }
                }
                Proj::ConstIndex(..) | Proj::Subslice(..) => {
                    if let TyKind::Slice(_) = self.tx.kind(t) {
                        let mut d = Diag::new("E0508", format!("cannot move out of type `{}`, a non-copy slice", self.tx.ty_str(t)), span);
                        d.label(span, "cannot move out of here".to_string(), true);
                        self.push_diag(d, p.clone(), span);
                        return;
                    }
                }
                Proj::Index(_) => {
                    let what = match self.tx.kind(t) {
                        TyKind::Slice(_) => "slice",
                        _ => "array",
                    };
                    let mut d = Diag::new("E0508", format!("cannot move out of type `{}`, a non-copy {}", self.tx.ty_str(t), what), span);
                    d.label(span, "cannot move out of here".to_string(), true);
                    self.push_diag(d, p.clone(), span);
                    return;
                }
                Proj::Downcast(_) => {}
            }
            t = proj_ty(self.tx, t, pr);
        }
    }

    fn upvar_of(&self, p: &Place) -> Option<String> {
        // `(*env).k` / `env.k` / `*(*env).k`
        if self.bir.closure_kind.is_none() || p.local != 1 {
            return None;
        }
        for pr in &p.proj {
            if let Proj::Field(k, _) = pr {
                return self.bir.captures.get(*k as usize).map(|c| c.name.clone());
            }
        }
        None
    }

    fn push_diag(&mut self, d: Diag, p: Place, span: Span) {
        if self.reported.iter().any(|(q, s)| *q == p && *s == span) {
            return;
        }
        self.reported.push((p, span));
        self.diags.push(d);
    }

    fn full_path(&mut self, b: u32, si: u32, p: &Place, span: Span, k: UseKind, u: &Bits) -> bool {
        let (m, _n) = self.md.closest(p);
        if bit_get(u, m as usize) {
            self.report_moved(b, si, p, m, span, k);
            return true;
        }
        false
    }

    fn path_or_subpath(&mut self, b: u32, si: u32, p: &Place, span: Span, k: UseKind, u: &Bits) {
        if self.full_path(b, si, p, span, k, u) {
            return;
        }
        // check_if_subslice_element_is_moved
        let els = match self.md.subslices.get(p) {
            Some(v) => Some(v.clone()),
            None => self.subslice_elements(p),
        };
        if let Some(els) = els {
            for m in els {
                if let Some(x) = self.md.find_in_subtree(m, &|x| bit_get(u, x as usize)) {
                    self.report_moved(b, si, p, x, span, k);
                    return;
                }
            }
        }
        if let Some(m) = self.md.exact(p) {
            if let Some(x) = self.md.find_in_subtree(m, &|x| bit_get(u, x as usize)) {
                self.report_moved(b, si, p, x, span, k);
            }
        }
    }

    /// The tracked element paths an array subslice place covers when the subslice itself was
    /// never moved (a borrow `[ref y @ .., _]` after a move of one element).
    fn subslice_elements(&self, p: &Place) -> Option<Vec<u32>> {
        let (last, base) = p.proj.split_last()?;
        let Proj::Subslice(from, to, false) = last else { return None };
        let bp = Place { local: p.local, proj: base.to_vec() };
        let TyKind::Array(_, len) = self.tx.kind(place_ty(self.tx, self.bir, &bp)) else { return None };
        let len = len.try_to_target_usize(&self.tx.tcx.tys)?;
        let mut els = Vec::new();
        for k in *from..*to {
            let mut e = bp.clone();
            e.proj.push(Proj::ConstIndex(k, false, len as u32));
            if let Some(m) = self.md.exact(&e) {
                els.push(m);
            }
        }
        Some(els)
    }

    fn mutate_place(&mut self, b: u32, si: u32, p: &Place, span: Span, u: &Bits, e: &Bits, call_dest: bool) {
        // check_if_assigned_path_is_moved
        for i in (0..p.proj.len()).rev() {
            let base = Place { local: p.local, proj: p.proj[..i].to_vec() };
            match p.proj[i] {
                Proj::Index(_) | Proj::ConstIndex(..) | Proj::Downcast(_) => {}
                Proj::Deref => {
                    self.full_path(b, si, &base, span, UseKind::Use, u);
                    break;
                }
                Proj::Subslice(..) => {}
                Proj::Field(..) => {
                    let bt = place_ty(self.tx, self.bir, &base);
                    if self.tx.has_drop_impl(bt) {
                        self.path_or_subpath(b, si, &base, span, UseKind::Assign, u);
                        break;
                    }
                    if self.tx.is_union(bt) {
                        // a union field write initializes the union after a move out of it; with
                        // no earlier move it is a partial assignment of an uninitialized binding
                        // (rustc check_parent_of_field: E0381)
                        let mut shortest: Option<(Place, u32)> = None;
                        for n in (0..=i).rev() {
                            let q = Place { local: p.local, proj: p.proj[..n].to_vec() };
                            if n < i && p.proj[n] == Proj::Deref {
                                break;
                            }
                            if let Some(m) = self.md.exact(&q) {
                                if bit_get(u, m as usize) {
                                    shortest = Some((q, m));
                                }
                            }
                        }
                        // (not a union literal `U { a: x }`: it is built by a write of its field)
                        let lit = self.bir.union_inits.contains(&(b as Bb, si));
                        if let (Some((q, m)), false) = (shortest, lit) {
                            if self.find_moves(b, si, m).is_empty() {
                                self.report_moved(b, si, &q, m, span, UseKind::Assign);
                            }
                        }
                        break;
                    }
                    if matches!(self.tx.kind(bt), TyKind::Adt(..) | TyKind::Tuple(_)) {
                        // the shortest uninitialized prefix without crossing a deref
                        let mut shortest: Option<(Place, u32)> = None;
                        for n in (0..=i).rev() {
                            let q = Place { local: p.local, proj: p.proj[..n].to_vec() };
                            if n < i && p.proj[n] == Proj::Deref {
                                break;
                            }
                            if let Some(m) = self.md.exact(&q) {
                                if bit_get(u, m as usize) {
                                    shortest = Some((q, m));
                                }
                            }
                        }
                        if let Some((q, m)) = shortest {
                            self.report_moved(b, si, &q, m, span, UseKind::Assign);
                            break;
                        }
                    }
                }
            }
        }
        let _ = call_dest;
        self.check_mutate(p, span, e);
    }

    /// Relaxed Rust "mut" (relaxed-rust.md A9; relaxed.rs): the place is not mutable only because
    /// a user binding lacks `mut` and the crate opted in -- accept it as if `mut` were written,
    /// recording the note and the write-back (`mut ` before the binding's name). Upvars, `self`,
    /// `ref` bindings and places behind references keep the error.
    fn relaxed_mut(&self, errp: &Place, span: Span, code: &'static str) -> bool {
        if !errp.proj.is_empty() && !matches!(self.tx.kind(self.bir.locals[errp.local as usize].ty), TyKind::Adt(..) | TyKind::Tuple(..) | TyKind::Array(..)) {
            return false;
        }
        let prog = self.tx.prog;
        let file = self.bir.file as usize;
        let Some(&k) = prog.file_crate.get(file) else { return false };
        if !prog.crates[k as usize].relaxed(prog).iter().any(|f| f == "mut") {
            return false;
        }
        if self.upvar_of(errp).is_some() || (errp.local == 1 && self.bir.closure_kind.is_some()) {
            return false;
        }
        let ld = &self.bir.locals[errp.local as usize];
        let Some(name) = ld.name.clone() else { return false };
        if name == "self" || ld.is_ref_binding || !matches!(ld.kind, LocalKind::User | LocalKind::Arg) {
            return false;
        }
        // the binding's name in its declaration span (a whole word, not after `mut `)
        let src = &prog.files[file].src;
        let (lo, hi) = (ld.span.lo as usize, (ld.span.hi as usize).min(src.len()));
        let word = |i: usize| i + name.len() <= src.len() && &src[i..i + name.len()] == name.as_bytes() && (i == 0 || !(src[i - 1] == b'_' || src[i - 1].is_ascii_alphanumeric())) && (i + name.len() >= src.len() || !(src[i + name.len()] == b'_' || src[i + name.len()].is_ascii_alphanumeric()));
        let Some(pos) = (lo..hi).find(|&i| word(i)) else { return false };
        if pos >= 4 && &src[pos - 4..pos] == b"mut " {
            return false;
        }
        let at = |p: u32| {
            let (l, c) = prog.files[file].line_col(p);
            format!("{}:{}:{}", prog.file_paths[file], l, c)
        };
        crate::relaxed::note(crate::relaxed::Note {
            at: at(span.lo),
            feature: "mut",
            code,
            msg: format!("accepted only with relaxed `mut`: `{}` is not declared as mutable; will write: `mut {}` at {}", name, name, at(pos as u32)),
            fix: Some((prog.file_paths[file].clone(), pos as u32, "mut ".to_string())),
        });
        true
    }

    /// check_access_permissions for a write: E0384 (immutable local assigned twice), E0594.
    fn check_mutate(&mut self, p: &Place, span: Span, e: &Bits) {
        // a reassignment (or a write into its fields) needs the local's `mut`; its first
        // initialization does not
        if !p.proj.is_empty() || bit_get(e, p.local as usize) { self.note_used_mut(p); }
        if let Err(errp) = self.is_mutable(p, false) {
            if !bit_get(e, p.local as usize) {
                return;
            }
            if self.relaxed_mut(&errp, span, if p.proj.is_empty() { "E0384" } else { "E0594" }) {
                return;
            }
            if p.proj.is_empty() {
                let name = self.bir.locals[p.local as usize].name.clone().unwrap_or_else(|| "_".to_string());
                let mut d = Diag::new("E0384", format!("cannot assign twice to immutable variable `{}`", name), span);
                d.label(span, "cannot assign twice to immutable variable".to_string(), true);
                if let Some(s) = self.first_init_span(p.local) {
                    d.label(s, format!("first assignment to `{}`", name), false);
                }
                d.help(format!("consider making this binding mutable: `mut {}`", name));
                self.push_diag(d, p.clone(), span);
            } else {
                let d = self.mutability_error(p, &errp, span, false);
                self.push_diag(d, p.clone(), span);
            }
        }
    }

    fn check_mutable(&mut self, p: &Place, span: Span, e: &Bits, borrow: bool) {
        self.note_used_mut(p);
        // a binding mutated inside its match guard (rustc: the guard sees `&binding`)
        if let Some((_, name)) = self.bir.guard_locals.iter().find(|x| x.0 == p.local) {
            // (edition < 2021 closures capture the whole binding by unique borrow)
            if p.proj.first() == Some(&Proj::Deref) || (borrow && p.proj.is_empty()) {
                let (code, verb) = if borrow { ("E0596", format!("cannot borrow `{}` as mutable", name)) } else { ("E0594", format!("cannot assign to `{}`", name)) };
                let mut d = Diag::new(code, format!("{}, as it is immutable for the pattern guard", verb), span);
                d.label(span, if borrow { "cannot borrow as mutable".to_string() } else { "cannot assign".to_string() }, true);
                d.note("variables bound in patterns are immutable until the end of the pattern guard".to_string());
                self.push_diag(d, p.clone(), span);
                return;
            }
        }
        if let Err(errp) = self.is_mutable(p, false) {
            if !bit_get(e, p.local as usize) && self.bir.locals[p.local as usize].kind != LocalKind::Arg {
                return;
            }
            if self.relaxed_mut(&errp, span, if borrow { "E0596" } else { "E0594" }) {
                return;
            }
            // rustc lib.rs access_place suppresses repeated accesses by place and use span,
            // before the mutable-borrow attempt is counted.
            if self.reported.iter().any(|(q, s)| q == p && *s == span) {
                return;
            }
            let mut d = self.mutability_error(p, &errp, span, borrow);
            let decl = &self.bir.locals[errp.local as usize];
            if borrow && errp.proj.is_empty() && decl.can_be_made_mutable {
                // rustc diagnostics/mutability_errors.rs:222-257. Keep the first use as
                // primary until a second attempt shows that the binding is the problem.
                let binding = decl.span;
                let key = (binding.lo, binding.hi);
                let count = if let Some((previous, count)) = self.buffered_mut_errors.remove(&key) {
                    d = previous;
                    let count = count + 1;
                    if count == 2 {
                        d.span = binding;
                        for (_, _, primary) in &mut d.labels {
                            *primary = false;
                        }
                        d.label(binding, "not mutable".to_string(), true);
                    }
                    // Match rustc's label limit; all attempts are still counted.
                    if count < 10 {
                        d.label(span, "cannot borrow as mutable".to_string(), false);
                    }
                    count
                } else {
                    1
                };
                self.reported.push((p.clone(), span));
                self.buffered_mut_errors.insert(key, (d, count));
            } else {
                self.push_diag(d, p.clone(), span);
            }
        }
    }

    fn first_init_span(&self, l: Local) -> Option<Span> {
        for blk in &self.bir.blocks {
            crate::preempt::point();
            for s in &blk.stmts {
                crate::preempt::point();
                if let StmtKind::Assign(p, _) = &s.kind {
                    if p.local == l && p.proj.is_empty() {
                        return Some(s.span);
                    }
                }
            }
            if let Some(Term { kind: TermKind::Call { dest, .. }, span }) = &blk.term {
                if dest.local == l && dest.proj.is_empty() {
                    return Some(*span);
                }
            }
        }
        None
    }

    fn mutability_error(&self, p: &Place, errp: &Place, span: Span, borrow: bool) -> Diag {
        let what = describe_place(self.tx, self.bir, p).unwrap_or_else(|| "value".to_string());
        let (code, verb) = if borrow { ("E0596", format!("cannot borrow `{}` as mutable", what)) } else { ("E0594", format!("cannot assign to `{}`", what)) };
        // why: the place that is not mutable
        let reason = if let Some(name) = self.upvar_of(errp).or_else(|| if errp.local == 1 && self.bir.closure_kind.is_some() { Some(String::new()) } else { None }) {
            let _ = name;
            match self.bir.closure_kind {
                Some(ClosureKind::Fn) => ", as it is a captured variable in a `Fn` closure".to_string(),
                _ => ", as it is not declared as mutable".to_string(),
            }
        } else if errp.proj.is_empty() {
            if self.bir.locals[errp.local as usize].name.is_some() {
                ", as it is not declared as mutable".to_string()
            } else {
                String::new()
            }
        } else {
            let bt = place_ty_n(self.tx, self.bir, errp, errp.proj.len() - 1);
            match self.tx.kind(bt) {
                TyKind::Ref(false, _, _) => format!(", which is behind a `&` reference"),
                TyKind::Ptr(false, _) => format!(", which is behind a `*const` pointer"),
                _ => String::new(),
            }
        };
        let mut d = Diag::new(code, format!("{}{}", verb, reason), span);
        d.label(span, if borrow { "cannot borrow as mutable".to_string() } else { "cannot assign".to_string() }, true);
        d
    }

    /// rustc add_used_mut: the local whose `mut` writing / mutably borrowing `p` relies on --
    /// none when the path goes through a `&mut` reference or a raw pointer (their referent's
    /// mutability is the reference's, not the binding's).
    fn note_used_mut(&mut self, p: &Place) {
        for (i, pr) in p.proj.iter().enumerate() {
            if matches!(pr, Proj::Deref) && matches!(self.tx.kind(place_ty_n(self.tx, self.bir, p, i)), TyKind::Ref(..) | TyKind::Ptr(..)) {
                return;
            }
        }
        if (p.local as usize) < self.bir.locals.len() { bit_set(&mut self.used_mut, p.local as usize); }
    }

    /// rustc is_mutable: Ok when `p` may be written / borrowed mutably.
    pub fn is_mutable(&self, p: &Place, local_ok: bool) -> Result<(), Place> {
        if p.proj.is_empty() {
            let l = &self.bir.locals[p.local as usize];
            return if l.mutable || local_ok { Ok(()) } else { Err(p.clone()) };
        }
        let n = p.proj.len();
        let base = Place { local: p.local, proj: p.proj[..n - 1].to_vec() };
        match p.proj[n - 1] {
            Proj::Deref => {
                let bt = place_ty(self.tx, self.bir, &base);
                match self.tx.kind(bt) {
                    TyKind::Ref(false, _, _) => Err(p.clone()),
                    TyKind::Ref(true, _, _) => {
                        // a by-ref upvar keeps the caller's mode; otherwise unique path suffices
                        let by_ref_upvar = self.upvar_field(&base).map_or(false, |k| self.bir.captures[k].kind != 0);
                        self.is_mutable(&base, if by_ref_upvar { local_ok } else { true })
                    }
                    TyKind::Ptr(false, _) => Err(p.clone()),
                    TyKind::Ptr(true, _) => Ok(()),
                    _ => self.is_mutable(&base, local_ok),
                }
            }
            _ => {
                if let Some(k) = self.upvar_field(p) {
                    let c = &self.bir.captures[k];
                    let mutable = self.capture_mutable(k);
                    if !mutable && !local_ok {
                        return Err(p.clone());
                    }
                    let _ = c;
                    self.is_mutable(&base, local_ok)
                } else {
                    self.is_mutable(&base, local_ok)
                }
            }
        }
    }

    fn capture_mutable(&self, k: usize) -> bool {
        self.bir.captures.get(k).map_or(true, |c| c.mutable)
    }

    /// `(*env).k` / `env.k` (the upvar field itself): its index.
    fn upvar_field(&self, p: &Place) -> Option<usize> {
        if self.bir.closure_kind.is_none() || p.local != 1 {
            return None;
        }
        match p.proj.as_slice() {
            [Proj::Deref, Proj::Field(k, _)] => Some(*k as usize),
            [Proj::Field(k, _)] if self.bir.closure_kind == Some(ClosureKind::FnOnce) => Some(*k as usize),
            _ => None,
        }
    }

    /// Finds the moves of path `m` (or its ancestors / descendants) that reach (b, si)
    /// backwards without an init in between; reports E0382 / E0381.
    fn report_moved(&mut self, b: u32, si: u32, p: &Place, m: u32, span: Span, k: UseKind) {
        let moves = self.find_moves(b, si, m);
        let what = describe_place(self.tx, self.bir, p).unwrap_or_else(|| "value".to_string());
        let mp_place = self.md.paths[m as usize].place.clone();
        let mp_what = describe_place(self.tx, self.bir, &mp_place).unwrap_or_else(|| "value".to_string());
        if moves.is_empty() {
            // never initialized
            if self.reported_moves.contains(&(mp_place.local)) {
                return;
            }
            self.reported_moves.push(mp_place.local);
            let msg = if k == UseKind::Assign {
                format!("partially assigned binding `{}` isn't fully initialized", mp_what)
            } else {
                format!("used binding `{}` isn't initialized", mp_what)
            };
            let mut d = Diag::new("E0381", msg, span);
            let decl = self.bir.locals[mp_place.local as usize].span;
            if decl.hi > decl.lo {
                d.label(decl, "binding declared here but left uninitialized".to_string(), false);
            }
            d.label(span, if k == UseKind::Assign { format!("`{}` partially assigned here but it isn't fully initialized", what) } else { format!("`{}` used here but it isn't initialized", mp_what) }, true);
            self.push_diag(d, p.clone(), span);
            return;
        }
        let partial = mp_place.proj.len() > p.proj.len() && p.is_prefix_of(&mp_place);
        let verb = match k {
            UseKind::Borrow => "borrow of",
            UseKind::Assign => "assign to part of",
            UseKind::MatchOn => "use of",
            UseKind::Use => "use of",
        };
        let msg = if partial {
            format!("{} partially moved value: `{}`", verb, what)
        } else if k == UseKind::Assign {
            format!("assign to part of moved value: `{}`", mp_what)
        } else {
            format!("{} moved value: `{}`", verb, mp_what)
        };
        let mut d = Diag::new("E0382", msg, span);
        let pt = place_ty(self.tx, self.bir, &mp_place);
        for (mv_span, in_loop) in &moves {
            let lab = if *in_loop {
                format!("value moved here, in previous iteration of loop")
            } else if partial {
                format!("value partially moved here")
            } else {
                "value moved here".to_string()
            };
            d.label(*mv_span, lab, false);
        }
        let used = match k {
            UseKind::Borrow => "value borrowed here after move",
            UseKind::Assign => "value partially assigned here after move",
            _ => "value used here after move",
        };
        let used = if partial && k != UseKind::Assign { if k == UseKind::Borrow { "value borrowed here after partial move" } else { "value used here after partial move" } } else { used };
        d.label(span, used.to_string(), true);
        if !self.tx.is_copy(pt) {
            d.note(format!("move occurs because `{}` has type `{}`, which does not implement the `Copy` trait", mp_what, self.tx.ty_str(pt)));
        }
        self.push_diag(d, p.clone(), span);
    }

    /// Spans of moves of `m`'s ancestors-or-self / descendants reaching (b, si) backwards
    /// (stopping at inits of those paths); `in_loop` when the move is at or after the use.
    fn find_moves(&self, b: u32, si: u32, m: u32) -> Vec<(Span, bool)> {
        let md = self.md;
        let target = md.paths[m as usize].place.clone();
        let related = |p: &Place| p.is_prefix_of(&target) || target.is_prefix_of(p);
        // a move of a tracked path related to the target, or of an array subslice whose elements are
        let moved_rel = |q: &Place| (related(q) && md.exact(q).is_some()) || md.subslices.get(q).map_or(false, |els| els.iter().any(|e| related(&md.paths[*e as usize].place)));
        let mut out = Vec::new();
        let preds = predecessors(self.bir);
        let mut seen = vec![false; self.bir.blocks.len()];
        // (block, start index exclusive upper bound, passed the use?)
        let mut stack: Vec<(u32, usize, bool)> = vec![(b, si as usize, false)];
        let mut first = true;
        while let Some((bb, upto, wrapped)) = stack.pop() {
            crate::preempt::point();
            if !first {
                if seen[bb as usize] {
                    continue;
                }
                seen[bb as usize] = true;
            }
            first = false;
            let blk = &self.bir.blocks[bb as usize];
            let mut stop = false;
            // terminator first (if within range)
            let n = blk.stmts.len();
            let mut i = upto.min(n + 1);
            // walk statements i-1 down to 0; the terminator counts at index n
            if i == n + 1 {
                if let Some(t) = &blk.term {
                    match &t.kind {
                        TermKind::Call { args, dest, arg_spans, .. } => {
                            if dest.is_prefix_of(&target) && md.exact(dest).is_some() {
                                stop = true;
                            }
                            for (ai, o) in args.iter().enumerate() {
                                if let Operand::Move(q) = o {
                                    if moved_rel(q) {
                                        out.push((arg_spans.get(ai).copied().unwrap_or(t.span), wrapped));
                                    }
                                }
                            }
                        }
                        TermKind::Drop(..) => {}
                        _ => {}
                    }
                }
                i = n;
            }
            if !stop {
                while i > 0 {
                    i -= 1;
                    let s = &blk.stmts[i];
                    match &s.kind {
                        StmtKind::Assign(p, rv) => {
                            if p.is_prefix_of(&target) && md.exact(p).is_some() {
                                stop = true;
                                // moves in the rvalue happen before the init
                            }
                            let mut ops: Vec<&Operand> = Vec::new();
                            match rv {
                                Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => ops.push(o),
                                Rvalue::Aggregate(_, v) | Rvalue::Op(_, v) => ops.extend(v.iter()),
                                _ => {}
                            }
                            if !stop {
                                for o in ops {
                                    if let Operand::Move(q) = o {
                                        if moved_rel(q) {
                                            out.push((s.span, wrapped));
                                        }
                                    }
                                }
                            }
                            if stop {
                                break;
                            }
                        }
                        StmtKind::StorageDead(l) | StmtKind::StorageLive(l) if *l == target.local => {
                            stop = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            if stop {
                continue;
            }
            for &pb in &preds[bb as usize] {
                let nb = self.bir.blocks[pb as usize].stmts.len() + 1;
                let w = wrapped || pb == b || seen[pb as usize];
                stack.push((pb, nb, w || pb == b));
            }
        }
        out.sort_by_key(|x| (x.0.lo, x.0.hi));
        out.dedup_by_key(|x| (x.0.lo, x.0.hi));
        out
    }
}

pub fn predecessors(bir: &Bir) -> Vec<Vec<u32>> {
    let mut p = vec![Vec::new(); bir.blocks.len()];
    for b in 0..bir.blocks.len() {
        crate::preempt::point();
        for s in successors(bir, b as u32) {
            p[s as usize].push(b as u32);
        }
    }
    p
}

/// MaybeInitializedPlaces: per block entry, the move paths that may be initialized.
pub fn maybe_init_flow(md: &MoveData, bir: &Bir, order: &[u32]) -> Vec<Option<Bits>> {
    let n = bir.blocks.len();
    let np = md.paths.len();
    let mut entry: Vec<Option<Bits>> = vec![None; n];
    let mut e0 = bits_new(np);
    for a in 1..=bir.n_args as usize {
        if a < bir.locals.len() {
            md.for_subtree(md.roots[a], &mut |m| bit_set(&mut e0, m as usize));
        }
    }
    entry[0] = Some(e0);
    let mut changed = true;
    let mut iters = 0;
    while changed && iters < 200 {
        changed = false;
        iters += 1;
        for &b in order {
            crate::preempt::point();
            let mut st = match &entry[b as usize] {
                Some(x) => x.clone(),
                None => continue,
            };
            let blk = &bir.blocks[b as usize];
            for s in &blk.stmts {
                crate::preempt::point();
                init_stmt_effect(md, &s.kind, &mut st);
            }
            if let Some(t) = &blk.term {
                init_term_effect(md, &t.kind, &mut st);
            }
            for s in successors(bir, b) {
                match &mut entry[s as usize] {
                    Some(e) => changed |= bits_union(e, &st),
                    slot @ None => {
                        *slot = Some(st.clone());
                        changed = true;
                    }
                }
            }
        }
    }
    entry
}

fn uninit_op(md: &MoveData, o: &Operand, st: &mut Bits) {
    if let Operand::Move(p) = o {
        if let Some(m) = md.exact(p) {
            md.for_subtree(m, &mut |x| bit_clear(st, x as usize));
        }
    }
}

fn init_at(md: &MoveData, p: &Place, st: &mut Bits) {
    if let Some(m) = md.exact(p) {
        md.for_subtree(m, &mut |x| bit_set(st, x as usize));
    }
}

pub fn init_stmt_effect(md: &MoveData, s: &StmtKind, st: &mut Bits) {
    match s {
        StmtKind::Assign(p, rv) => {
            match rv {
                Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => uninit_op(md, o, st),
                Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                    for o in ops {
                        uninit_op(md, o, st);
                    }
                }
                _ => {}
            }
            init_at(md, p, st);
        }
        StmtKind::StorageLive(l) | StmtKind::StorageDead(l) => {
            md.for_subtree(md.roots[*l as usize], &mut |m| bit_clear(st, m as usize));
        }
        _ => {}
    }
}

pub fn init_term_effect(md: &MoveData, t: &TermKind, st: &mut Bits) {
    match t {
        TermKind::Call { args, dest, .. } => {
            for o in args {
                uninit_op(md, o, st);
            }
            init_at(md, dest, st);
        }
        TermKind::Switch(o, _) => uninit_op(md, o, st),
        _ => {}
    }
}

/// Whether any part of `p` may be initialized under `st`.
pub fn maybe_init(md: &MoveData, p: &Place, st: &Bits) -> bool {
    let (m, n) = md.closest(p);
    if n < p.proj.len() {
        return bit_get(st, m as usize);
    }
    md.find_in_subtree(m, &|x| bit_get(st, x as usize)).is_some()
}
