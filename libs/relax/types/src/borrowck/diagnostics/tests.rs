//! C-NLL6 acceptance: error code + primary line (and the main label lines) on hand-built MIR
//! whose spans point into real source text.

use super::region_name::{ExternalName, InputInfo, OutputInfo, UniversalNames};
use super::{render_errors, DiagCx};
use crate::borrowck::borrow_set::BorrowSet;
use crate::borrowck::borrowck_errors::{AccessKind, BorrowInfo, BorrowckError, Detail, ErrorKind, InitAction};
use crate::borrowck::check::{mir_borrowck, MirBorrowckResult};
use crate::borrowck::constraints::{ConstraintCategory, OutlivesConstraint, OutlivesConstraintSet, ReturnConstraint};
use crate::borrowck::event::BorrowError;
use crate::borrowck::mir_nll::{compute_regions, LocationMap, RegionInput};
use crate::borrowck::region_infer::values::LivenessValues;
use crate::borrowck::region_infer::{NllRegionVariableOrigin as Origin, RegionDefinition};
use crate::borrowck::type_check::free_region_relations::UniversalRegionRelations;
use crate::borrowck::universal_regions::{RegionClassification as Class, UniversalRegions};
use crate::mir::move_paths::MoveData;
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::{Tcx, TypingEnv};
use crate::types::{RegionId, RegionKind, TyId, TyKind, UniverseIndex};

struct Fx { prog: Program, tcx: Tcx, src: String, file: u32 }
fn fx(src: &str) -> Fx {
    let session = std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("relax-cnll6-{}-{index}.rs", std::process::id()));
    std::fs::write(&path, src).unwrap();
    let mut prog = Program::new(&session);
    prog.load_crate("custom", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
    std::fs::remove_file(path).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let file = (prog.files.len() - 1) as u32;
    Fx { prog, tcx, src: src.to_string(), file }
}
impl Fx {
    /// The span of the `n`th (0-based) occurrence of `needle`.
    fn sp_n(&self, needle: &str, n: usize) -> Span {
        let lo = self.src.match_indices(needle).nth(n).unwrap_or_else(|| panic!("no occurrence {n} of {needle:?}")).0;
        Span { file: self.file, lo: lo as u32, hi: (lo + needle.len()) as u32 }
    }
    fn sp(&self, needle: &str) -> Span { self.sp_n(needle, 0) }
    fn def(&self, name: &str) -> DefId { (0..self.prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| self.prog.name(d) == name).unwrap() }
    fn adt(&self, name: &str) -> TyId { self.tcx.tys.intern(TyKind::Adt(self.def(name), self.tcx.tys.mk_list(&[]), crate::types::RegionArgsId::EMPTY)) }
    fn rv(&self, n: u32) -> RegionId { self.tcx.tys.intern_region(RegionKind::ReVar(n)) }
    fn rf(&self, mutable: bool, t: TyId, var: u32) -> TyId { self.tcx.tys.intern(TyKind::Ref(mutable, t, self.rv(var))) }
}

fn p(i: u32) -> Place { Place::local(Local(i)) }
fn deref(i: u32) -> Place { Place { local: Local(i), projection: vec![PlaceElem::Deref] } }
fn at(b: u32, s: usize) -> Location { Location { block: BasicBlock(b), statement_index: s } }
fn st(kind: StatementKind, span: Span) -> Statement { Statement { kind, source_info: SourceInfo { span, ..SourceInfo::default() } } }
fn blk(stmts: Vec<Statement>, term: TerminatorKind, span: Span) -> BasicBlockData {
    BasicBlockData { statements: stmts, terminator: Some(Terminator { kind: term, source_info: SourceInfo { span, ..SourceInfo::default() } }), is_cleanup: false }
}
fn val(ty: TyId) -> Operand { Operand::Constant(Box::new(ConstOperand { span: Span::default(), user_ty: None, const_: Const::Val(ConstValue::ZeroSized, ty) })) }
fn assign(dst: u32, op: Operand) -> StatementKind { StatementKind::Assign(p(dst), Rvalue::Use(op)) }
fn borrow(dst: u32, source: Place, region: u32, kind: BorrowKind) -> StatementKind { StatementKind::Assign(p(dst), Rvalue::Ref(Region(region), kind, source)) }
fn mutable() -> BorrowKind { BorrowKind::Mut { kind: MutBorrowKind::Default } }
fn pts(v: &[(u32, usize)]) -> Vec<Location> { v.iter().map(|&(b, s)| at(b, s)).collect() }

/// `locals`: (type, user name, decl span). Local 0 is the return place, 1..=arg_count arguments.
fn body(f: &Fx, locals: Vec<(TyId, Option<&str>, Span)>, arg_count: u32, blocks: Vec<BasicBlockData>) -> Body {
    let mut var_debug_info = vec![];
    let local_decls = locals.iter().enumerate().map(|(i, (ty, name, span))| {
        if let Some(n) = name { var_debug_info.push(VarDebugInfo { name: n.to_string(), source_info: SourceInfo::default(), place: p(i as u32) }); }
        LocalDecl { ty: *ty, mutability: Mutability::Mut, source_info: SourceInfo { span: *span, ..SourceInfo::default() }, user_ty: vec![],
            local_info: LocalInfo::Boring,
            kind: if i == 0 { LocalKind::Return } else if (i as u32) <= arg_count { LocalKind::Arg } else if name.is_some() { LocalKind::UserVar } else { LocalKind::Temp } }
    }).collect();
    Body { phase: MirPhase::Analysis(AnalysisPhase::Initial), source: MirSource { owner: f.def("f"), kind: BodyKind::Fn },
        basic_blocks: blocks, local_decls, source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
        span: Span::default(), user_type_annotations: vec![], upvars: vec![], allocs: Default::default(), var_debug_info, arg_count, spread_arg: None, promoted: vec![], coroutine: None }
}

/// Free regions: 0 is 'static; `locals` are the further local free regions (1..); the existential
/// regions follow, each live at the points given.
struct Regions { locals: usize, scopes: Vec<Vec<Location>>, constraints: Vec<OutlivesConstraint> }
fn regions_of(b: &Body, r: &Regions) -> crate::borrowck::mir_nll::MirRegionResult {
    let map = LocationMap::new(b);
    let nfree = 1 + r.locals;
    let mut definitions = vec![RegionDefinition { origin: Origin::FreeRegion, universe: UniverseIndex(0), external_name: None }; nfree];
    definitions.extend(r.scopes.iter().map(|_| RegionDefinition { origin: Origin::Existential, universe: UniverseIndex(0), external_name: None }));
    let mut liveness = LivenessValues::new(definitions.len(), map.num_points());
    for f in 0..nfree { liveness.add_all_points(f); }
    for (i, points) in r.scopes.iter().enumerate() { for &l in points { liveness.add_point(nfree + i, map.point(l)); } }
    let mut universal = vec![(0usize, Class::Global)];
    universal.extend((1..nfree).map(|i| (i, Class::Local)));
    let mut outlives = OutlivesConstraintSet::default();
    for c in &r.constraints { outlives.push(*c); }
    let input = RegionInput { universe_causes: vec![], polonius: Default::default(), tainted_by_errors: false,
        relations: UniversalRegionRelations::create(UniversalRegions::new(0, None, universal), definitions.len(), &[]),
        definitions, outlives, liveness, type_tests: vec![] };
    let prog = Program::new(&std::sync::Arc::new(crate::Session::default()));
    let tcx = Tcx::new();
    compute_regions(&tcx, &prog, b, input, false)
}
fn constraint(sup: usize, sub: usize, span: Span, category: ConstraintCategory) -> OutlivesConstraint {
    OutlivesConstraint { sup, sub, location: None, span, category, from_closure: false }
}

fn gather(f: &Fx, b: &Body) -> MoveData { MoveData::gather_moves(b, &f.prog, &f.tcx, TypingEnv::fully_monomorphized(&f.prog), &|_| true) }

fn run(f: &Fx, b: &Body, r: &Regions, names: &UniversalNames) -> (MirBorrowckResult, Vec<BorrowError>) {
    let moves = gather(f, b);
    let mut regions = regions_of(b, r);
    let result = mir_borrowck(&f.tcx, &f.prog, b, &mut regions, &moves);
    assert!(result.unsupported.is_empty(), "{:?}", result.unsupported);
    let out = render(f, b, &regions, &moves, names, &result);
    (result, out)
}
fn render(f: &Fx, b: &Body, regions: &crate::borrowck::mir_nll::MirRegionResult, moves: &MoveData, names: &UniversalNames, result: &MirBorrowckResult) -> Vec<BorrowError> {
    let borrows = BorrowSet::build(&f.tcx, b, true, moves);
    let cx = DiagCx::new(&f.tcx, &f.prog, f.def("f"), b, moves, regions, &borrows, names);
    render_errors(&cx, result)
}
fn nr() -> Regions { Regions { locals: 0, scopes: vec![], constraints: vec![] } }
fn label_lines(e: &BorrowError, needle: &str) -> Vec<u32> { e.labels.iter().filter(|l| l.text.contains(needle)).map(|l| l.line).collect() }

#[test]
fn e0499_two_mutable_borrows() {
    let f = fx("fn f() {\n    let mut x = 0;\n    let a = &mut x;\n    let b = &mut x;\n    *a = 1;\n}\nstruct D;\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(true, i, 1), Some("a"), f.sp("a")), (f.rf(true, i, 2), Some("b"), f.sp("b"))], 0, vec![
        blk(vec![st(assign(1, val(i)), f.sp("0;")), st(borrow(2, p(1), 1, mutable()), f.sp("&mut x")), st(borrow(3, p(1), 2, mutable()), f.sp_n("&mut x", 1)),
            st(StatementKind::Assign(deref(2), Rvalue::Use(val(i))), f.sp("*a = 1"))], TerminatorKind::Return, f.sp("}"))]);
    let (result, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![pts(&[(0, 1), (0, 2), (0, 3)]), vec![]], constraints: vec![] }, &UniversalNames::default());
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert_eq!((out[0].code, out[0].line), ("E0499", 4), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot borrow `x` as mutable more than once at a time");
    assert_eq!(label_lines(&out[0], "first mutable borrow occurs here"), [3]);
    assert_eq!(label_lines(&out[0], "borrow later used here"), [5]);
}

#[test]
fn e0502_shared_then_mutable() {
    let f = fx("fn f() {\n    let mut x = 0;\n    let a = &x;\n    let b = &mut x;\n    let c = *a;\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(false, i, 1), Some("a"), f.sp("a")), (f.rf(true, i, 2), Some("b"), f.sp("b")), (i, Some("c"), f.sp("c"))], 0, vec![
        blk(vec![st(assign(1, val(i)), f.sp("0;")), st(borrow(2, p(1), 1, BorrowKind::Shared), f.sp("&x")), st(borrow(3, p(1), 2, mutable()), f.sp("&mut x")),
            st(assign(4, Operand::Copy(deref(2))), f.sp("*a;"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![pts(&[(0, 1), (0, 2), (0, 3)]), vec![]], constraints: vec![] }, &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0502", 4), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot borrow `x` as mutable because it is also borrowed as immutable");
    assert_eq!(label_lines(&out[0], "immutable borrow occurs here"), [3]);
    assert_eq!(label_lines(&out[0], "immutable borrow later used here"), [5]);
}

#[test]
fn e0505_move_out_while_borrowed() {
    let f = fx("fn f() {\n    let x = 0;\n    let r = &x;\n    let y = x;\n    let z = *r;\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(false, i, 1), Some("r"), f.sp("r")), (i, Some("y"), f.sp("y")), (i, Some("z"), f.sp("z"))], 0, vec![
        blk(vec![st(assign(1, val(i)), f.sp("0;")), st(borrow(2, p(1), 1, BorrowKind::Shared), f.sp("&x")), st(assign(3, Operand::Move(p(1))), f.sp("y = x")),
            st(assign(4, Operand::Copy(deref(2))), f.sp("*r"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![pts(&[(0, 1), (0, 2), (0, 3)])], constraints: vec![] }, &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0505", 4), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot move out of `x` because it is borrowed");
    assert_eq!(label_lines(&out[0], "borrow of `x` occurs here"), [3]);
    assert_eq!(label_lines(&out[0], "borrow later used here"), [5]);
}

#[test]
fn e0506_assign_to_borrowed() {
    let f = fx("fn f() {\n    let mut x = 0;\n    let r = &x;\n    x = 1;\n    let z = *r;\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(false, i, 1), Some("r"), f.sp("r")), (i, Some("z"), f.sp("z"))], 0, vec![
        blk(vec![st(assign(1, val(i)), f.sp("0;")), st(borrow(2, p(1), 1, BorrowKind::Shared), f.sp("&x")), st(assign(1, val(i)), f.sp("x = 1")),
            st(assign(3, Operand::Copy(deref(2))), f.sp("*r"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![pts(&[(0, 1), (0, 2), (0, 3)])], constraints: vec![] }, &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0506", 4), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot assign to `x` because it is borrowed");
    assert_eq!(label_lines(&out[0], "`x` is borrowed here"), [3]);
}

#[test]
fn e0382_use_after_move_and_e0381_uninitialized() {
    let f = fx("fn f(s: D) {\n    let t = s;\n    let u = s;\n}\nstruct D;\n");
    let d = f.adt("D");
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (d, Some("s"), f.sp("s")), (d, Some("t"), f.sp("t")), (d, Some("u"), f.sp("u"))], 1, vec![
        blk(vec![st(assign(2, Operand::Move(p(1))), f.sp("s;")), st(assign(3, Operand::Move(p(1))), f.sp_n("s;", 1))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0382", 3), "{}", out[0].render());
    assert_eq!(out[0].message, "use of moved value: `s`");
    assert_eq!(label_lines(&out[0], "value moved here"), [2]);
    assert_eq!(label_lines(&out[0], "value used here after move"), [3]);
    assert_eq!(label_lines(&out[0], "move occurs because `s` has type `D`"), [1]);

    let f = fx("fn f() {\n    let x: i32;\n    let y = x;\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x")), (i, Some("y"), f.sp("y"))], 0, vec![
        blk(vec![st(assign(2, Operand::Copy(p(1))), f.sp_n("x;", 0))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0381", 3), "{}", out[0].render());
    assert_eq!(out[0].message, "used binding `x` isn't initialized");
    assert_eq!(label_lines(&out[0], "binding declared here but left uninitialized"), [2]);
}

#[test]
fn e0384_assign_twice_and_e0596_e0594() {
    let f = fx("fn f() {\n    let x = 1;\n    x = 2;\n}\n");
    let i = f.tcx.tys.i32_;
    let mut b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x"))], 0, vec![
        blk(vec![st(assign(1, val(i)), f.sp("1;")), st(assign(1, val(i)), f.sp("x = 2"))], TerminatorKind::Return, f.sp("}"))]);
    b.local_decls[1].mutability = Mutability::Not;
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0384", 3), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot assign twice to immutable variable `x`");
    assert_eq!(label_lines(&out[0], "first assignment to `x`"), [2]);

    let f = fx("fn f() {\n    let x = 1;\n    let r = &mut x;\n}\n");
    let i = f.tcx.tys.i32_;
    let mut b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(true, i, 1), Some("r"), f.sp("r"))], 0, vec![
        blk(vec![st(assign(1, val(i)), f.sp("1;")), st(borrow(2, p(1), 1, mutable()), f.sp("&mut x"))], TerminatorKind::Return, f.sp("}"))]);
    b.local_decls[1].mutability = Mutability::Not;
    let (_, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![vec![]], constraints: vec![] }, &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0596", 3), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot borrow `x` as mutable, as it is not declared as mutable");

    let f = fx("fn f(r: &i32) {\n    *r = 1;\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (f.tcx.tys.intern(TyKind::Ref(false, i, RegionId::ERASED)), Some("r"), f.sp("r"))], 1, vec![
        blk(vec![st(StatementKind::Assign(deref(1), Rvalue::Use(val(i))), f.sp("*r = 1"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0594", 2), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot assign to `*r`, which is behind a `&` reference");
}

#[test]
fn e0507_e0508_e0509_illegal_moves() {
    let f = fx("fn f(r: &D) {\n    let t = *r;\n}\nstruct D;\n");
    let d = f.adt("D");
    let rd = f.tcx.tys.intern(TyKind::Ref(false, d, RegionId::ERASED));
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (rd, Some("r"), f.sp("r")), (d, Some("t"), f.sp("t"))], 1, vec![
        blk(vec![st(assign(2, Operand::Move(deref(1))), f.sp("*r"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0507", 2), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot move out of `*r` which is behind a shared reference");

    let f = fx("fn f(a: [D; 2]) {\n    let t = a[0];\n}\nstruct D;\n");
    let d = f.adt("D");
    let arr = f.tcx.tys.intern(TyKind::Array(d, f.tcx.tys.const_usize(2)));
    let idx = Place { local: Local(1), projection: vec![PlaceElem::Index(Local(3))] };
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (arr, Some("a"), f.sp("a")), (d, Some("t"), f.sp("t")), (f.tcx.tys.usize_, None, Span::default())], 1, vec![
        blk(vec![st(assign(3, val(f.tcx.tys.usize_)), f.sp("0]")), st(assign(2, Operand::Move(idx)), f.sp("a[0]"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0508", 2), "{}", out[0].render());
    assert!(out[0].message.starts_with("cannot move out of type `[D; 2]`, a non-copy array"), "{}", out[0].message);

    let f = fx("#[lang = \"drop\"] trait Cleanup { fn release(&mut self); }\nstruct W { v: D }\nimpl Cleanup for W { fn release(&mut self) {} }\nfn f(w: W) {\n    let t = w.v;\n}\nstruct D;\n");
    let (w, d) = (f.adt("W"), f.adt("D"));
    let field = Place { local: Local(1), projection: vec![PlaceElem::Field(FieldIdx(0), d)] };
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (w, Some("w"), f.sp("w")), (d, Some("t"), f.sp("t"))], 1, vec![
        blk(vec![st(assign(2, Operand::Move(field)), f.sp("w.v"))], TerminatorKind::Return, f.sp("}"))]);
    let (_, out) = run(&f, &b, &nr(), &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0509", 5), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot move out of type `W`, which implements the `Drop` trait");
}

#[test]
fn e0597_borrowed_value_does_not_live_long_enough() {
    let f = fx("fn f() {\n    let r: &i32;\n    {\n        let x = 5;\n        r = &x;\n    }\n    g(r);\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (f.rf(false, i, 1), Some("r"), f.sp("r")), (i, Some("x"), f.sp_n("x", 0)), (f.rf(false, i, 2), None, Span::default())], 0, vec![
        blk(vec![st(StatementKind::StorageLive(Local(2)), Span::default()), st(assign(2, val(i)), f.sp("5;")), st(borrow(1, p(2), 1, BorrowKind::Shared), f.sp("&x")),
            st(StatementKind::StorageDead(Local(2)), f.sp_n("}", 0)), st(assign(3, Operand::Copy(p(1))), f.sp("g(r)"))], TerminatorKind::Return, f.sp_n("}", 1))]);
    let mut b = b;
    b.local_decls[2].source_info.span = f.sp("x = 5");
    let (_, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![pts(&[(0, 2), (0, 3), (0, 4)]), vec![]], constraints: vec![] }, &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0597", 5), "{}", out[0].render());
    assert_eq!(out[0].message, "`x` does not live long enough");
    assert_eq!(label_lines(&out[0], "`x` dropped here while still borrowed"), [6]);
    assert_eq!(label_lines(&out[0], "borrow later used here"), [7]);
}

#[test]
fn e0716_temporary_dropped_while_borrowed() {
    let f = fx("fn f() {\n    let r: &i32 = &make();\n    g(r);\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (f.rf(false, i, 1), Some("r"), f.sp("r")), (i, None, f.sp("make()")), (f.rf(false, i, 2), None, Span::default())], 0, vec![
        blk(vec![st(StatementKind::StorageLive(Local(2)), Span::default()), st(assign(2, val(i)), f.sp("make()")), st(borrow(1, p(2), 1, BorrowKind::Shared), f.sp("&make()")),
            st(StatementKind::StorageDead(Local(2)), f.sp(";")), st(assign(3, Operand::Copy(p(1))), f.sp("g(r)"))], TerminatorKind::Return, f.sp_n("}", 0))]);
    let (_, out) = run(&f, &b, &Regions { locals: 0, scopes: vec![pts(&[(0, 2), (0, 3), (0, 4)]), vec![]], constraints: vec![] }, &UniversalNames::default());
    assert_eq!((out[0].code, out[0].line), ("E0716", 2), "{}", out[0].render());
    assert_eq!(out[0].message, "temporary value dropped while borrowed");
    assert_eq!(label_lines(&out[0], "borrow later used here"), [3]);
}

#[test]
fn e0515_return_reference_to_local() {
    let f = fx("fn f() -> &'static i32 {\n    let x = 5;\n    &x\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.rf(false, i, 0), None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(false, i, 1), None, Span::default())], 0, vec![
        blk(vec![st(StatementKind::StorageLive(Local(1)), Span::default()), st(assign(1, val(i)), f.sp("5;")), st(borrow(2, p(1), 1, BorrowKind::Shared), f.sp("&x")),
            st(assign(0, Operand::Move(p(2))), f.sp("&x")), st(StatementKind::StorageDead(Local(1)), f.sp_n("}", 0))], TerminatorKind::Return, f.sp_n("}", 0))]);
    let names = UniversalNames { regions: vec![(0, ExternalName::Static)], def_descr: "function", is_fn_def: true, ..Default::default() };
    let r = Regions { locals: 0, scopes: vec![pts(&[(0, 2), (0, 3), (0, 4)])], constraints: vec![constraint(1, 0, f.sp("&x"), ConstraintCategory::Return(ReturnConstraint::Normal))] };
    let (_, out) = run(&f, &b, &r, &names);
    assert_eq!((out[0].code, out[0].line), ("E0515", 3), "{}", out[0].render());
    assert_eq!(out[0].message, "cannot return reference to local variable `x`");
}

#[test]
fn e0373_closure_may_outlive_the_function() {
    // The checker stops on closure aggregates until MIR capture metadata exists, so the error
    // is handed to the renderer as the checker will produce it.
    let f = fx("fn f() -> &'static dyn Fn() {\n    let x = 5;\n    Box::new(|| x)\n}\n");
    let i = f.tcx.tys.i32_;
    let closure = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.rf(false, closure, 0), None, Span::default()), (i, Some("x"), f.sp("x")), (f.rf(false, i, 1), None, Span::default()), (closure, None, Span::default())], 0, vec![
        blk(vec![st(StatementKind::StorageLive(Local(1)), Span::default()), st(assign(1, val(i)), f.sp("5;")), st(borrow(2, p(1), 1, BorrowKind::Shared), f.sp("x)")),
            st(StatementKind::Assign(p(3), Rvalue::Aggregate(Box::new(AggregateKind::Closure(ClosureId { file: 0, expr: 0 }, vec![])), vec![Operand::Move(p(2))])), f.sp("|| x")),
            st(assign(0, Operand::Move(p(3))), f.sp("Box::new(|| x)")), st(StatementKind::StorageDead(Local(1)), f.sp_n("}", 0))], TerminatorKind::Return, f.sp_n("}", 0))]);
    let names = UniversalNames { regions: vec![(0, ExternalName::Static)], def_descr: "function", is_fn_def: true, ..Default::default() };
    let r = Regions { locals: 0, scopes: vec![pts(&[(0, 2), (0, 3), (0, 4), (0, 5)])], constraints: vec![constraint(1, 0, f.sp("Box::new(|| x)"), ConstraintCategory::Return(ReturnConstraint::Normal))] };
    let moves = gather(&f, &b);
    let regions = regions_of(&b, &r);
    let borrows = BorrowSet::build(&f.tcx, &b, true, &moves);
    let borrow = BorrowInfo::from(&borrows[0]);
    let error = BorrowckError::with(ErrorKind::DoesNotLiveLongEnough, f.sp("x)"), Detail::Lifetime { borrow, dropped_place: p(1), location: at(0, 5) });
    let result = MirBorrowckResult { errors: vec![error], used_mut: crate::mir::dataflow::BitSet::new_empty(4), unsupported: vec![], used_mut_upvars: vec![], region_errors: vec![], tainted_by_errors: false, fv_only: vec![], fv_records: vec![], relaxed_records: vec![] };
    let out = render(&f, &b, &regions, &moves, &names, &result);
    assert_eq!((out[0].code, out[0].line), ("E0373", 3), "{}", out[0].render());
    assert_eq!(out[0].message, "closure may outlive the current function, but it borrows `x`, which is owned by the current function");
}

#[test]
fn e0713_e0712_e0626_rendered_from_checker_details() {
    let f = fx("fn f(d: D) {\n    let r = &d.v;\n}\nstruct D { v: i32 }\n");
    let i = f.tcx.tys.i32_;
    let d = f.adt("D");
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (d, Some("d"), f.sp("d")), (f.rf(false, i, 1), Some("r"), f.sp("r"))], 1, vec![
        blk(vec![st(borrow(2, Place { local: Local(1), projection: vec![PlaceElem::Field(FieldIdx(0), i)] }, 1, BorrowKind::Shared), f.sp("&d.v"))], TerminatorKind::Return, f.sp_n("}", 0))]);
    let moves = gather(&f, &b);
    let regions = regions_of(&b, &Regions { locals: 0, scopes: vec![pts(&[(0, 0), (0, 1)])], constraints: vec![] });
    let borrows = BorrowSet::build(&f.tcx, &b, true, &moves);
    let info = BorrowInfo::from(&borrows[0]);
    let none = UniversalNames::default();
    for (kind, code, detail) in [
        (ErrorKind::BorrowAcrossDestructor, "E0713", Detail::Lifetime { borrow: info.clone(), dropped_place: p(1), location: at(0, 1) }),
        (ErrorKind::ThreadLocalEscapes, "E0712", Detail::Lifetime { borrow: info.clone(), dropped_place: p(1), location: at(0, 1) }),
        (ErrorKind::BorrowAcrossYield, "E0626", Detail::BorrowAcrossYield { borrow: info.clone(), yield_location: at(0, 1) }),
    ] {
        let result = MirBorrowckResult { errors: vec![BorrowckError::with(kind, f.sp("&d.v"), detail)], used_mut: crate::mir::dataflow::BitSet::new_empty(3),
            unsupported: vec![], used_mut_upvars: vec![], region_errors: vec![], tainted_by_errors: false, fv_only: vec![], fv_records: vec![], relaxed_records: vec![] };
        let out = render(&f, &b, &regions, &moves, &none, &result);
        assert_eq!((out[0].code, out[0].line), (code, 2), "{}", out[0].render());
    }
    let _ = (AccessKind::Move, InitAction::Use, InputInfo { ty: i, ty_span: Span::default(), ref_span: None }, OutputInfo { ty: i, span: Span::default(), ref_span: None, description: "" });
}

#[test]
fn lifetime_may_not_live_long_enough_with_named_regions() {
    // fn f<'a, 'b>(x: &'a i32) -> &'b i32 { x }
    let f = fx("fn f<'a, 'b>(x: &'a i32) -> &'b i32 {\n    x\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.rf(false, i, 2), None, Span::default()), (f.rf(false, i, 1), Some("x"), f.sp("x"))], 1, vec![
        blk(vec![st(assign(0, Operand::Copy(p(1))), f.sp_n("x", 1))], TerminatorKind::Return, f.sp_n("}", 0))]);
    let names = UniversalNames { regions: vec![(1, ExternalName::Named { name: "'a".into(), span: f.sp("'a"), early: true }), (2, ExternalName::Named { name: "'b".into(), span: f.sp("'b"), early: true })],
        def_descr: "function", is_fn_def: true, ..Default::default() };
    let r = Regions { locals: 2, scopes: vec![], constraints: vec![constraint(1, 2, f.sp_n("x", 1), ConstraintCategory::Return(ReturnConstraint::Normal))] };
    let (result, out) = run(&f, &b, &r, &names);
    assert_eq!(result.region_errors.len(), 1, "{:?}", result.region_errors);
    assert_eq!((out[0].code, out[0].line), ("", 2), "{}", out[0].render());
    assert_eq!(out[0].message, "lifetime may not live long enough");
    assert!(out[0].labels.iter().any(|l| l.text == "lifetime `'a` defined here" && l.line == 1));
    assert!(out[0].labels.iter().any(|l| l.text.contains("function was supposed to return data with lifetime `'b` but it is returning data with lifetime `'a`") && l.line == 2));
}

#[test]
fn e0521_borrowed_data_escapes_and_e0621_explicit_lifetime_required() {
    // fn f(x: &i32) { g(x) } where g needs 'static
    let f = fx("fn f(x: &i32) {\n    g(x);\n}\n");
    let i = f.tcx.tys.i32_;
    let b = body(&f, vec![(f.tcx.tys.unit, None, Span::default()), (f.rf(false, i, 1), Some("x"), f.sp("x"))], 1, vec![
        blk(vec![st(StatementKind::Nop, Span::default())], TerminatorKind::Return, f.sp_n("}", 0))]);
    let names = UniversalNames { regions: vec![(0, ExternalName::Static), (1, ExternalName::Anon)],
        inputs: vec![InputInfo { ty: f.rf(false, i, 1), ty_span: f.sp("&i32"), ref_span: Some(f.sp("&i32")) }],
        def_descr: "function", is_fn_def: true, ..Default::default() };
    let r = Regions { locals: 1, scopes: vec![], constraints: vec![constraint(1, 0, f.sp("g(x)"), ConstraintCategory::CallArgument(None))] };
    let moves = gather(&f, &b);
    let regions = regions_of(&b, &r);
    let result = MirBorrowckResult { errors: vec![], used_mut: crate::mir::dataflow::BitSet::new_empty(2), unsupported: vec![], used_mut_upvars: vec![],
        region_errors: regions.regioncx.errors.clone(), tainted_by_errors: false, fv_only: vec![], fv_records: vec![], relaxed_records: vec![] };
    let out = render(&f, &b, &regions, &moves, &names, &result);
    assert_eq!((out[0].code, out[0].line), ("E0521", 2), "{}", out[0].render());
    assert_eq!(out[0].message, "borrowed data escapes outside of function");
    assert_eq!(label_lines(&out[0], "`x` is a reference that is only valid in the function body"), [1]);

    // fn f<'a>(x: &i32) -> &'a i32 { x }
    let f = fx("fn f<'a>(x: &i32) -> &'a i32 {\n    x\n}\n");
    let b = body(&f, vec![(f.rf(false, i, 2), None, Span::default()), (f.rf(false, i, 1), Some("x"), f.sp("x"))], 1, vec![
        blk(vec![st(assign(0, Operand::Copy(p(1))), f.sp_n("x", 1))], TerminatorKind::Return, f.sp_n("}", 0))]);
    let names = UniversalNames { regions: vec![(1, ExternalName::Anon), (2, ExternalName::Named { name: "'a".into(), span: f.sp("'a"), early: true })],
        inputs: vec![InputInfo { ty: f.rf(false, i, 1), ty_span: f.sp("&i32"), ref_span: Some(f.sp("&i32")) }],
        def_descr: "function", is_fn_def: true, ..Default::default() };
    let r = Regions { locals: 2, scopes: vec![], constraints: vec![constraint(1, 2, f.sp_n("x", 1), ConstraintCategory::Return(ReturnConstraint::Normal))] };
    let (_, out) = run(&f, &b, &r, &names);
    assert_eq!((out[0].code, out[0].line), ("E0621", 2), "{}", out[0].render());
    assert_eq!(out[0].message, "explicit lifetime required in the type of `x`");
}
