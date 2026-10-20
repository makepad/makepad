//! Const checking over hand-built MIR (transform/fixtures.rs): each operation a const context may not
//! perform, the flow-sensitive qualifs, secondary errors, cleanup blocks, feature gates.
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

fn check(fx: &Fx, body: &Body, provider: &dyn ConstQualifsProvider) -> Result<ConstCheck, Blocked> {
    mir_const_qualif(&ConstCx::new(&fx.prog, &fx.tcx, fx.env, body, provider))
}

fn codes(c: &ConstCheck) -> Vec<&'static str> {
    c.errors.iter().map(|e| e.code).collect()
}

/// `const X: u8 = <body>`: _0: u8.
fn const_body(fx: &Fx, locals: &[crate::types::TyId], blocks: Vec<BasicBlockData>) -> Body {
    body(fx.def("K"), K::Const, fx.u8(), &[], locals, blocks)
}

#[test]
fn calling_a_const_fn_is_fine_and_a_non_const_fn_is_e0015() {
    let fx = Fx::new("");
    let ok = const_body(&fx, &[], vec![block(vec![], call(zst(fx.fn_ty("cf")), vec![], local(0), 1)), block(vec![], TerminatorKind::Return)]);
    assert!(check(&fx, &ok, &NoProvider).unwrap().errors.is_empty());
    let bad = const_body(&fx, &[], vec![block(vec![], call(zst(fx.fn_ty("nf")), vec![], local(0), 1)), block(vec![], TerminatorKind::Return)]);
    let c = check(&fx, &bad, &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0015"], "{c:?}");
    assert!(c.errors[0].message.contains("cannot call non-const function `mir_build::nf` in constants"), "{}", c.errors[0].message);
}

#[test]
fn a_fn_body_is_checked_only_when_it_is_a_const_fn() {
    let fx = Fx::new("");
    let blocks = || vec![block(vec![], call(zst(fx.fn_ty("nf")), vec![], local(0), 1)), block(vec![], TerminatorKind::Return)];
    // `cf` is a const fn: its body is a const context
    let in_const_fn = body(fx.def("cf"), K::Fn, fx.u8(), &[], &[], blocks());
    assert_eq!(codes(&check(&fx, &in_const_fn, &NoProvider).unwrap()), ["E0015"]);
    // `nf` is not
    let in_fn = body(fx.def("nf"), K::Fn, fx.u8(), &[], &[], blocks());
    assert!(check(&fx, &in_fn, &NoProvider).unwrap().errors.is_empty());
    // a closure body never is
    let in_closure = body(fx.def("cf"), K::Closure(0), fx.u8(), &[], &[], blocks());
    assert!(check(&fx, &in_closure, &NoProvider).unwrap().errors.is_empty());
}

#[test]
fn trait_method_calls_are_never_const_without_the_feature() {
    let fx = Fx::new("");
    let call_m = |fx: &Fx| {
        let m = fx.def("m");
        let ty = fx.tcx.tys.intern(TyKind::FnDef(m, fx.tcx.tys.mk_list(&[fx.u8()]), crate::types::RegionArgsId::EMPTY));
        const_body(fx, &[fx.u8(), fx.ref_ty(false, fx.u8())], vec![
            block(vec![assign(local(1), Rvalue::Use(int(fx.u8(), 1))), assign(local(2), borrow(BorrowKind::Shared, local(1)))], call(zst(ty), vec![mv(2)], local(0), 1)),
            block(vec![], TerminatorKind::Return),
        ])
    };
    let c = check(&fx, &call_m(&fx), &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0015"], "{c:?}");
    let gated = Fx::new("#![feature(const_trait_impl)]");
    let c = check(&gated, &call_m(&gated), &NoProvider).unwrap();
    assert!(c.errors.is_empty(), "{c:?}");
}

#[test]
fn fn_pointer_and_closure_calls_are_e0015() {
    let fx = Fx::new("");
    let fnptr = fx.tcx.tys.intern(TyKind::FnPtr(crate::types::List::EMPTY, fx.u8(), crate::types::FnHeader::RUST, 0));
    let b = const_body(&fx, &[fnptr], vec![
        block(vec![assign(local(1), Rvalue::Use(zst(fnptr)))], call(copy(1), vec![], local(0), 1)),
        block(vec![], TerminatorKind::Return),
    ]);
    let c = check(&fx, &b, &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0015"]);
    assert!(c.errors[0].message.contains("function pointer calls are not allowed in constants"));
}

#[test]
fn live_drops_are_e0493_unless_moved_or_gated() {
    let fx = Fx::new("");
    let d = fx.adt("D");
    // `let _d = D;` live at the end of the initializer
    let dropped = const_body(&fx, &[d], vec![
        block(vec![assign(local(1), aggregate_adt(&fx, "D", vec![])), assign(local(0), Rvalue::Use(int(fx.u8(), 1)))], drop_(local(1), 1)),
        block(vec![], TerminatorKind::Return),
    ]);
    let c = check(&fx, &dropped, &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0493"], "{c:?}");
    assert!(c.errors[0].message.contains("destructor of `D` cannot be evaluated at compile-time"));
    // moved out first: the drop of the moved-from local needs no drop (flow-sensitive)
    let moved = const_body(&fx, &[d, d], vec![
        block(vec![assign(local(1), aggregate_adt(&fx, "D", vec![])), assign(local(2), Rvalue::Use(mv(1))), assign(local(0), Rvalue::Use(int(fx.u8(), 1)))], drop_(local(1), 1)),
        block(vec![], drop_(local(2), 2)),
        block(vec![], TerminatorKind::Return),
    ]);
    let c = check(&fx, &moved, &NoProvider).unwrap();
    // only the second drop (of the moved-to local) is live
    assert_eq!(codes(&c), ["E0493"], "{c:?}");
    // the gate allows every drop
    let gated = Fx::new("#![feature(const_destruct)]");
    let c = check(&gated, &dropped_for(&gated), &NoProvider).unwrap();
    assert!(c.errors.is_empty(), "{c:?}");
}

fn dropped_for(fx: &Fx) -> Body {
    const_body(fx, &[fx.adt("D")], vec![
        block(vec![assign(local(1), aggregate_adt(fx, "D", vec![])), assign(local(0), Rvalue::Use(int(fx.u8(), 1)))], drop_(local(1), 1)),
        block(vec![], TerminatorKind::Return),
    ])
}

#[test]
fn a_drop_in_a_cleanup_block_is_not_checked() {
    let fx = Fx::new("");
    let d = fx.adt("D");
    let b = const_body(&fx, &[d], vec![
        block(vec![assign(local(1), aggregate_adt(&fx, "D", vec![])), assign(local(0), Rvalue::Use(int(fx.u8(), 1)))], TerminatorKind::Return),
        cleanup_block(vec![], drop_(local(1), 2)),
        cleanup_block(vec![], TerminatorKind::UnwindResume),
    ]);
    assert!(check(&fx, &b, &NoProvider).unwrap().errors.is_empty());
}

#[test]
fn escaping_mut_borrows_are_secondary_e0764_and_transient_ones_are_fine() {
    let fx = Fx::new("");
    let arr = fx.array(fx.u8(), 1);
    let rarr = fx.ref_ty(true, arr);
    let blocks = |dead: bool| {
        let mut stmts = vec![
            stmt(StatementKind::StorageLive(Local(1))),
            assign(local(1), Rvalue::Aggregate(Box::new(AggregateKind::Array(fx.u8())), vec![int(fx.u8(), 0)])),
            assign(local(2), borrow(BorrowKind::Mut { kind: MutBorrowKind::Default }, local(1))),
        ];
        if dead {
            stmts.push(stmt(StatementKind::StorageDead(Local(1))));
        }
        vec![block(stmts, TerminatorKind::Return)]
    };
    // the array lives on at the end: E0764 (a const's final value would point to it)
    let escaping = body(fx.def("K"), K::Const, rarr, &[], &[arr, rarr], blocks(false));
    let c = check(&fx, &escaping, &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0764"], "{c:?}");
    // storage-dead on every path to the return: transient
    let transient = body(fx.def("K"), K::Const, rarr, &[], &[arr, rarr], blocks(true));
    assert!(check(&fx, &transient, &NoProvider).unwrap().errors.is_empty());
    // in a `static mut` mutable borrows are allowed
    let in_static_mut = body(fx.def("SM"), K::Static, rarr, &[], &[arr, rarr], blocks(false));
    assert!(check(&fx, &in_static_mut, &NoProvider).unwrap().errors.is_empty());
    // in a const fn all borrows are transient
    let in_const_fn = body(fx.def("cf"), K::Fn, rarr, &[], &[arr, rarr], blocks(false));
    assert!(check(&fx, &in_const_fn, &NoProvider).unwrap().errors.is_empty());
}

#[test]
fn borrowing_interior_mutable_data_is_secondary_e0492() {
    let fx = Fx::new("");
    let cell = fx.adt("Cell");
    let uc = fx.adt("UnsafeCell");
    let b = body(fx.def("K"), K::Const, fx.ref_ty(false, cell), &[], &[cell, uc], vec![block(
        vec![
            assign(local(2), aggregate_adt(&fx, "UnsafeCell", vec![int(fx.u8(), 1)])),
            assign(local(1), aggregate_adt(&fx, "Cell", vec![mv(2)])),
            assign(local(0), borrow(BorrowKind::Shared, local(1))),
        ],
        TerminatorKind::Return,
    )]);
    let c = check(&fx, &b, &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0492"], "{c:?}");
    assert!(c.errors[0].message.contains("constants cannot refer to interior mutable data"));
}

#[test]
fn secondary_errors_are_dropped_when_a_primary_error_exists() {
    let fx = Fx::new("");
    let cell = fx.adt("Cell");
    let uc = fx.adt("UnsafeCell");
    let b = body(fx.def("K"), K::Const, fx.ref_ty(false, cell), &[], &[cell, fx.u8(), uc], vec![
        block(
            vec![
                assign(local(3), aggregate_adt(&fx, "UnsafeCell", vec![int(fx.u8(), 1)])),
                assign(local(1), aggregate_adt(&fx, "Cell", vec![mv(3)])),
                assign(local(0), borrow(BorrowKind::Shared, local(1))),
            ],
            call(zst(fx.fn_ty("nf")), vec![], local(2), 1),
        ),
        block(vec![], TerminatorKind::Return),
    ]);
    let c = check(&fx, &b, &NoProvider).unwrap();
    assert_eq!(codes(&c), ["E0015"], "{c:?}");
}

#[test]
fn thread_locals_pointer_casts_and_pointer_comparisons() {
    let fx = Fx::new("");
    let p = fx.tcx.tys.intern(TyKind::Ptr(false, fx.u8()));
    let usize_ty = fx.tcx.tys.prim(crate::program::Prim::Usize);
    let tls = const_body(&fx, &[p], vec![block(vec![assign(local(1), Rvalue::ThreadLocalRef(fx.def("S")))], TerminatorKind::Return)]);
    assert_eq!(codes(&check(&fx, &tls, &NoProvider).unwrap()), ["E0625"]);
    let cast = body(fx.def("K"), K::Const, usize_ty, &[], &[p], vec![block(
        vec![assign(local(1), Rvalue::Use(zst(p))), assign(local(0), Rvalue::Cast(CastKind::PointerExposeProvenance, copy(1), usize_ty))],
        TerminatorKind::Return,
    )]);
    let c = check(&fx, &cast, &NoProvider).unwrap();
    assert_eq!(c.errors.len(), 1);
    assert!(c.errors[0].message.contains("pointers cannot be cast to integers during const eval"));
    let cmp = body(fx.def("K"), K::Const, fx.tcx.tys.bool_, &[], &[p, p], vec![block(
        vec![assign(local(1), Rvalue::Use(zst(p))), assign(local(2), Rvalue::Use(zst(p))), assign(local(0), Rvalue::BinaryOp(BinOp::Eq, Box::new((copy(1), copy(2)))))],
        TerminatorKind::Return,
    )]);
    let c = check(&fx, &cmp, &NoProvider).unwrap();
    assert_eq!(c.errors.len(), 1);
    assert!(c.errors[0].message.contains("pointers cannot be reliably compared during const eval"));
}

#[test]
fn return_place_qualifs_follow_the_value() {
    let fx = Fx::new("");
    let d = fx.adt("D");
    let cell = fx.adt("Cell");
    // `const X: D = D;`: the return place needs drop
    let b = body(fx.def("K"), K::Const, d, &[], &[], vec![block(vec![assign(local(0), aggregate_adt(&fx, "D", vec![]))], TerminatorKind::Return)]);
    let q = check(&fx, &b, &NoProvider).unwrap().qualifs;
    assert!(q.needs_drop && q.needs_non_const_drop && !q.has_mut_interior, "{q:?}");
    // `const X: Cell = Cell(..)`
    let uc = fx.adt("UnsafeCell");
    let b = body(fx.def("K"), K::Const, cell, &[], &[uc], vec![block(
        vec![assign(local(1), aggregate_adt(&fx, "UnsafeCell", vec![int(fx.u8(), 1)])), assign(local(0), aggregate_adt(&fx, "Cell", vec![mv(1)]))],
        TerminatorKind::Return,
    )]);
    let q = check(&fx, &b, &NoProvider).unwrap().qualifs;
    assert!(q.has_mut_interior && !q.needs_drop, "{q:?}");
    // `const X: Pair = Pair(D, 1)`
    // an enum-like value with the drop-needing field moved out leaves nothing to drop
    let pair = fx.adt("Pair");
    let b = body(fx.def("K"), K::Const, pair, &[], &[d], vec![block(
        vec![assign(local(1), aggregate_adt(&fx, "D", vec![])), assign(local(0), aggregate_adt(&fx, "Pair", vec![mv(1), int(fx.u8(), 1)]))],
        TerminatorKind::Return,
    )]);
    assert!(check(&fx, &b, &NoProvider).unwrap().qualifs.needs_drop);
}

#[test]
fn a_named_constant_contributes_its_own_qualifs() {
    let fx = Fx::new("");
    let cell = fx.adt("Cell");
    let borrow_of_const = |fx: &Fx| {
        body(fx.def("K"), K::Const, fx.ref_ty(false, cell), &[], &[cell], vec![block(
            vec![assign(local(1), Rvalue::Use(item_const(fx.def("CELL"), cell))), assign(local(0), borrow(BorrowKind::Shared, local(1)))],
            TerminatorKind::Return,
        )])
    };
    // CELL's value has interior mutability: borrowing it escapes
    let has = ConstQualifs { has_mut_interior: true, ..Default::default() };
    let c = check(&fx, &borrow_of_const(&fx), &Fixed(vec![(fx.def("CELL"), has)])).unwrap();
    assert_eq!(codes(&c), ["E0492"], "{c:?}");
    // a constant whose value does not (its type could): fine
    let c = check(&fx, &borrow_of_const(&fx), &Fixed(vec![(fx.def("CELL"), ConstQualifs::default())])).unwrap();
    assert!(c.errors.is_empty(), "{c:?}");
    // a provider that cannot answer blocks the body
    assert!(check(&fx, &borrow_of_const(&fx), &NoProvider).is_err());
}

#[test]
fn bodies_with_unresolved_types_are_blocked() {
    let fx = Fx::new("");
    let b = const_body(&fx, &[fx.tcx.tys.error], vec![block(vec![], TerminatorKind::Return)]);
    assert!(check(&fx, &b, &NoProvider).is_err());
}

