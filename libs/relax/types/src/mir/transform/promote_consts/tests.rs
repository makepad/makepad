//! Promotion over hand-built MIR: which borrows are promoted, what the promoted bodies hold, how the
//! parent is rewritten.
use super::super::fixtures::*;
use super::super::qualifs::{Blocked, ConstQualifs, ConstQualifsProvider, NoProvider};
use super::*;
use crate::mir::BodyKind as K;
use crate::types::TyKind;

struct Fixed(Vec<(DefId, ConstQualifs)>);
impl ConstQualifsProvider for Fixed {
    fn mir_const_qualif(&self, def: DefId) -> Result<ConstQualifs, Blocked> {
        self.0.iter().find(|(d, _)| *d == def).map(|(_, q)| *q).ok_or(Blocked("unknown constant"))
    }
}

fn promote(fx: &Fx, body: &mut Body) -> Vec<Body> {
    promote_with(fx, body, &NoProvider).unwrap()
}

fn promote_with(fx: &Fx, body: &mut Body, provider: &dyn ConstQualifsProvider) -> Result<Vec<Body>, Blocked> {
    promote_temps(&fx.prog, &fx.tcx, fx.env, provider, body)
}

/// `fn nf() -> &'static T { <statements>; return }` : `_0: &T`, `_1: T` (the borrowed temp).
fn borrower(fx: &Fx, ty: crate::types::TyId, stmts: Vec<Statement>) -> Body {
    body(fx.def("nf"), K::Fn, fx.ref_ty(false, ty), &[], &[ty], vec![block(stmts, TerminatorKind::Return)])
}

fn is_promoted_const(stmt: &Statement, index: u32) -> bool {
    matches!(&stmt.kind, StatementKind::Assign(_, Rvalue::Use(Operand::Constant(c))) if matches!(c.const_, Const::Unevaluated { promoted: Some(p), .. } if p.0 == index))
}

#[test]
fn a_borrow_of_a_constant_is_promoted() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    let mut b = borrower(&fx, u8, vec![
        stmt(StatementKind::StorageLive(Local(1))),
        assign(local(1), Rvalue::Use(int(u8, 5))),
        assign(local(0), borrow(BorrowKind::Shared, local(1))),
        stmt(StatementKind::StorageDead(Local(1))),
    ]);
    let promoted = promote(&fx, &mut b);
    assert_eq!(promoted.len(), 1);
    // the promoted body returns `&5`
    let p = &promoted[0];
    assert!(matches!(p.source.kind, K::Promoted(d, PromotedId(0)) if d == fx.def("nf")));
    assert_eq!(p.phase, MirPhase::Analysis(AnalysisPhase::Initial));
    assert_eq!(p.local_decls[0].ty, fx.ref_ty(false, u8));
    assert_eq!(p.basic_blocks.len(), 1);
    assert!(matches!(&p.basic_blocks[0].statements[0].kind, StatementKind::Assign(dest, Rvalue::Use(Operand::Constant(_))) if dest.local.0 == 1));
    assert!(matches!(&p.basic_blocks[0].statements[1].kind, StatementKind::Assign(dest, Rvalue::Ref(_, BorrowKind::Shared, place)) if dest.local.0 == 0 && place.local.0 == 1));
    // the parent reads the promoted through a fresh temporary; the temp's definition and storage are gone
    let stmts = &b.basic_blocks[0].statements;
    assert_eq!(stmts.len(), 2, "{stmts:?}");
    assert!(is_promoted_const(&stmts[0], 0));
    assert!(matches!(&stmts[1].kind, StatementKind::Assign(dest, Rvalue::Ref(_, BorrowKind::Shared, place)) if dest.local.0 == 0 && place.local.0 == 2 && place.projection == [PlaceElem::Deref]));
    assert_eq!(b.local_decls[2].ty, fx.ref_ty(false, u8));
}

#[test]
fn borrows_of_non_constant_values_are_not_promoted() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    // an argument
    let mut arg = body(fx.def("nf"), K::Fn, fx.ref_ty(false, u8), &[u8], &[], vec![block(vec![assign(local(0), borrow(BorrowKind::Shared, local(1)))], TerminatorKind::Return)]);
    assert!(promote(&fx, &mut arg).is_empty());
    // a value with interior mutability
    let cell = fx.adt("Cell");
    let uc = fx.adt("UnsafeCell");
    let mut b = body(fx.def("nf"), K::Fn, fx.ref_ty(false, cell), &[], &[cell, uc], vec![block(
        vec![assign(local(2), aggregate_adt(&fx, "UnsafeCell", vec![int(u8, 1)])), assign(local(1), aggregate_adt(&fx, "Cell", vec![mv(2)])), assign(local(0), borrow(BorrowKind::Shared, local(1)))],
        TerminatorKind::Return,
    )]);
    assert!(promote(&fx, &mut b).is_empty());
    // a value that needs drop
    let d = fx.adt("D");
    let mut b = borrower(&fx, d, vec![assign(local(1), aggregate_adt(&fx, "D", vec![])), assign(local(0), borrow(BorrowKind::Shared, local(1)))]);
    assert!(promote(&fx, &mut b).is_empty());
    // a temporary assigned twice
    let mut b = borrower(&fx, u8, vec![assign(local(1), Rvalue::Use(int(u8, 1))), assign(local(1), Rvalue::Use(int(u8, 2))), assign(local(0), borrow(BorrowKind::Shared, local(1)))]);
    assert!(promote(&fx, &mut b).is_empty());
    // a thread-local pointer
    let p = fx.tcx.tys.intern(TyKind::Ptr(true, u8));
    let mut b = body(fx.def("nf"), K::Fn, fx.ref_ty(false, p), &[], &[p], vec![block(
        vec![assign(local(1), Rvalue::ThreadLocalRef(fx.def("S"))), assign(local(0), borrow(BorrowKind::Shared, local(1)))],
        TerminatorKind::Return,
    )]);
    assert!(promote(&fx, &mut b).is_empty());
}

#[test]
fn only_empty_arrays_are_promoted_mutably() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    let mutable = |fx: &Fx, n: u64| {
        let arr = fx.array(u8, n);
        let elems = (0..n).map(|_| int(u8, 0)).collect();
        body(fx.def("nf"), K::Fn, fx.ref_ty(true, arr), &[], &[arr], vec![block(
            vec![
                assign(local(1), Rvalue::Aggregate(Box::new(AggregateKind::Array(u8)), elems)),
                assign(local(0), borrow(BorrowKind::Mut { kind: MutBorrowKind::Default }, local(1))),
            ],
            TerminatorKind::Return,
        )])
    };
    // `&mut []`
    assert_eq!(promote(&fx, &mut mutable(&fx, 0)).len(), 1);
    // `&mut [0]`
    assert!(promote(&fx, &mut mutable(&fx, 1)).is_empty());
}

#[test]
fn division_by_zero_and_overflow_are_not_promoted() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    let i8 = fx.tcx.tys.prim(crate::program::Prim::I8);
    let div = |fx: &Fx, ty, lhs: u128, rhs: u128| {
        body(fx.def("nf"), K::Fn, fx.ref_ty(false, ty), &[], &[ty], vec![block(
            vec![assign(local(1), Rvalue::BinaryOp(BinOp::Div, Box::new((int(ty, lhs), int(ty, rhs))))), assign(local(0), borrow(BorrowKind::Shared, local(1)))],
            TerminatorKind::Return,
        )])
    };
    assert_eq!(promote(&fx, &mut div(&fx, u8, 4, 2)).len(), 1);
    assert!(promote(&fx, &mut div(&fx, u8, 4, 0)).is_empty());
    // i8::MIN / -1
    assert!(promote(&fx, &mut div(&fx, i8, 0x80, 0xff)).is_empty());
    assert_eq!(promote(&fx, &mut div(&fx, i8, 0x7f, 0xff)).len(), 1);
}

#[test]
fn nested_promotion_extracts_the_tree_once() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    let r = fx.ref_ty(false, u8);
    let rr = fx.ref_ty(false, r);
    // `&&5`
    let mut b = body(fx.def("nf"), K::Fn, rr, &[], &[u8, r], vec![block(
        vec![
            assign(local(1), Rvalue::Use(int(u8, 5))),
            assign(local(2), borrow(BorrowKind::Shared, local(1))),
            assign(local(0), borrow(BorrowKind::Shared, local(2))),
        ],
        TerminatorKind::Return,
    )]);
    let promoted = promote(&fx, &mut b);
    assert_eq!(promoted.len(), 1, "the inner borrow is part of the outer tree");
    let p = &promoted[0];
    // _1 = 5; _2 = &_1; _0 = &_2
    assert_eq!(p.basic_blocks[0].statements.len(), 3);
}

#[test]
fn a_temp_used_twice_stays_in_the_parent() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    let r = fx.ref_ty(false, u8);
    let mut b = body(fx.def("nf"), K::Fn, r, &[], &[u8, r], vec![block(
        vec![
            assign(local(1), Rvalue::Use(int(u8, 5))),
            assign(local(2), borrow(BorrowKind::Shared, local(1))),
            assign(local(0), borrow(BorrowKind::Shared, local(1))),
        ],
        TerminatorKind::Return,
    )]);
    let promoted = promote(&fx, &mut b);
    assert_eq!(promoted.len(), 2);
    // the definition stays (it has two uses); both borrows are replaced
    let assigns_one = b.basic_blocks[0].statements.iter().any(|s| matches!(&s.kind, StatementKind::Assign(p, Rvalue::Use(Operand::Constant(c))) if p.local.0 == 1 && matches!(c.const_, Const::Val(..))));
    assert!(assigns_one, "{:?}", b.basic_blocks[0].statements);
}

fn call_body(fx: &Fx, owner: &str, kind: K, callee: &str, safe_block: bool) -> Body {
    let u8 = fx.u8();
    let mut blocks = vec![
        block(vec![], call(zst(fx.fn_ty(callee)), vec![], local(1), 1)),
        block(vec![assign(local(0), borrow(BorrowKind::Shared, local(1)))], TerminatorKind::Return),
    ];
    if !safe_block {
        // reached only through a branch: not guaranteed to execute
        blocks = vec![
            block(vec![], TerminatorKind::SwitchInt { discr: int(u8, 1), targets: SwitchTargets { values: vec![0], targets: vec![BasicBlock(2)], otherwise: BasicBlock(1) } }),
            block(vec![], call(zst(fx.fn_ty(callee)), vec![], local(1), 3)),
            block(vec![assign(local(0), Rvalue::Use(zst(fx.ref_ty(false, u8))))], TerminatorKind::Return),
            block(vec![assign(local(0), borrow(BorrowKind::Shared, local(1)))], TerminatorKind::Return),
        ];
    }
    body(fx.def(owner), kind, fx.ref_ty(false, u8), &[], &[u8], blocks)
}

#[test]
fn calls_are_promoted_by_attribute_or_in_const_and_static_roots() {
    let fx = Fx::new("");
    // `#[rustc_promotable]` anywhere
    assert_eq!(promote(&fx, &mut call_body(&fx, "nf", K::Fn, "promotable", true)).len(), 1);
    // a const fn only in const/static initializers
    assert!(promote(&fx, &mut call_body(&fx, "nf", K::Fn, "cf", true)).is_empty());
    let promoted = promote(&fx, &mut call_body(&fx, "K", K::Const, "cf", true));
    assert_eq!(promoted.len(), 1);
    assert!(matches!(promoted[0].basic_blocks[0].terminator.as_ref().unwrap().kind, TerminatorKind::Call { .. }));
    assert_eq!(promoted[0].basic_blocks.len(), 2);
    // ... but not in an inline const, not for a non-const fn, and not in a block that may be dead code
    assert!(promote(&fx, &mut call_body(&fx, "K", K::InlineConst, "cf", true)).is_empty());
    assert!(promote(&fx, &mut call_body(&fx, "K", K::Const, "nf", true)).is_empty());
    assert!(promote(&fx, &mut call_body(&fx, "K", K::Const, "cf", false)).is_empty());
}

#[test]
fn promoted_constants_name_their_own_allocations() {
    let fx = Fx::new("");
    let str_ref = fx.ref_ty(false, fx.tcx.tys.prim(crate::program::Prim::Str));
    let mut b = body(fx.def("nf"), K::Fn, fx.ref_ty(false, str_ref), &[], &[str_ref], vec![block(
        vec![
            assign(local(1), Rvalue::Use(zst(str_ref))),
            assign(local(0), borrow(BorrowKind::Shared, local(1))),
        ],
        TerminatorKind::Return,
    )]);
    b.allocs.insert(Allocation { bytes: vec![1], align: 1, init_mask: vec![true], deallocated: false, provenance: vec![], provenance_fragments: vec![], mutability: Mutability::Not });
    let second = b.allocs.insert(Allocation { bytes: vec![2, 3], align: 1, init_mask: vec![true, true], deallocated: false, provenance: vec![], provenance_fragments: vec![], mutability: Mutability::Not });
    if let StatementKind::Assign(_, Rvalue::Use(Operand::Constant(c))) = &mut b.basic_blocks[0].statements[0].kind {
        c.const_ = Const::Val(ConstValue::Slice { alloc: second, meta: 2 }, str_ref);
    }
    let promoted = promote(&fx, &mut b);
    assert_eq!(promoted.len(), 1);
    let p = &promoted[0];
    assert_eq!(p.allocs.entries.len(), 1, "only the allocation the constant names is copied");
    assert!(matches!(&p.basic_blocks[0].statements[0].kind, StatementKind::Assign(_, Rvalue::Use(Operand::Constant(c))) if matches!(c.const_, Const::Val(ConstValue::Slice { alloc: AllocId(0), .. }, _))));
    assert_eq!(p.allocs.get(AllocId(0)).unwrap().bytes, vec![2, 3]);
}

#[test]
fn a_named_constant_with_interior_mutability_blocks_promotion_of_its_borrow() {
    let fx = Fx::new("");
    let cell = fx.adt("Cell");
    let mk = |fx: &Fx| {
        body(fx.def("nf"), K::Fn, fx.ref_ty(false, cell), &[], &[cell], vec![block(
            vec![assign(local(1), Rvalue::Use(item_const(fx.def("CELL"), cell))), assign(local(0), borrow(BorrowKind::Shared, local(1)))],
            TerminatorKind::Return,
        )])
    };
    let has = ConstQualifs { has_mut_interior: true, ..Default::default() };
    assert!(promote_with(&fx, &mut mk(&fx), &Fixed(vec![(fx.def("CELL"), has)])).unwrap().is_empty());
    // qualifs are value-based: a constant whose value has no interior mutability is promotable even
    // though its type could (check_consts/qualifs.rs in_operand returns before the type is looked at)
    assert_eq!(promote_with(&fx, &mut mk(&fx), &Fixed(vec![(fx.def("CELL"), ConstQualifs::default())])).unwrap().len(), 1);
    // an unanswerable constant blocks the body: nothing is rewritten
    let mut b = mk(&fx);
    let before = format!("{:?}", b.basic_blocks);
    assert!(promote_with(&fx, &mut b, &NoProvider).is_err());
    assert_eq!(before, format!("{:?}", b.basic_blocks));
}

#[test]
fn the_pre_borrowck_pipeline_simplifies_checks_and_promotes() {
    let fx = Fx::new("");
    let u8 = fx.u8();
    // `const X: &u8 = &5;` with a dead block after the return (unreachable, ill-typed code)
    let r = fx.ref_ty(false, u8);
    let mut b = body(fx.def("K"), K::Const, r, &[], &[u8], vec![
        block(vec![assign(local(1), Rvalue::Use(int(u8, 5))), assign(local(0), borrow(BorrowKind::Shared, local(1)))], TerminatorKind::Return),
        block(vec![assign(local(0), Rvalue::Use(int(u8, 7)))], TerminatorKind::Return),
    ]);
    let result = super::super::mir_promoted(&fx.prog, &fx.tcx, fx.env, &NoProvider, &mut b).unwrap();
    assert!(result.const_errors.is_empty(), "{:?}", result.const_errors);
    assert_eq!(b.basic_blocks.len(), 1, "the dead block is gone");
    assert_eq!(b.promoted.len(), 1);
    assert_eq!(b.phase, MirPhase::Analysis(AnalysisPhase::Initial));
    // a const initializer calling a non-const fn is reported, still promoted
    let mut bad = call_body(&fx, "K", K::Const, "nf", true);
    let result = super::super::mir_promoted(&fx.prog, &fx.tcx, fx.env, &NoProvider, &mut bad).unwrap();
    assert_eq!(result.const_errors.iter().map(|e| e.code).collect::<Vec<_>>(), ["E0015"]);
}
