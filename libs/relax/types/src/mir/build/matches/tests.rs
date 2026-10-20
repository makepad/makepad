//! Unit fixtures of match lowering: THIR patterns of real sources are lowered against a hand-set-up
//! `Builder` (no expression lowering is needed for decision trees and bindings).
use super::*;
use crate::borrowck::body::{BodyKind, CheckedBody};
use crate::fxhash::HashMap;
use crate::mir::build::pattern::tests::{named, program, top_patterns};
use crate::mir::build::scope::Scopes;
use crate::mir::build::thir::{Cx, Pat, Thir, ThirBody};
use crate::mir::build::{BuildCx, BuildRoot, Cfg};
use crate::mir::{AllocMap, Body, LocalDecl, MirPhase, MirSource, PlaceElem, SourceScopeData};
use crate::tcx::{EvalCx, EvalState, TypingEnv};
use crate::typeck;

/// A source's function `name` (whose first parameter is the scrutinee), its patterns (the ones not
/// in parameter position, in source order), and a builder with `_0: ()` and `_1: <first parameter>`.
fn run<R>(source: &str, name: &str, f: impl FnOnce(&mut Builder<'_>, &[Pat], &typeck::Body) -> R) -> R {
    let (prog, tcx) = program(source);
    let def = named(&prog, name);
    let body = tcx.typeck(&prog, def);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let env = TypingEnv::fully_monomorphized(&prog);
    let state = EvalState::new();
    let eval = EvalCx::new(&prog, &tcx, env, &state);
    let root = BuildRoot::new(BuildCx { prog: &prog, tcx: &tcx, env, eval: &eval }, &body).unwrap();
    let owner = CheckedBody { owner: def, kind: BodyKind::Function };
    let pats: Vec<Pat> = {
        let mut cx = Cx::new(&root, owner);
        top_patterns(&prog, &body).into_iter().map(|p| cx.lower_pattern(p).unwrap_or_else(|e| panic!("{e:?}"))).collect()
    };
    let unit = tcx.tys.intern(crate::types::TyKind::Tuple(crate::types::List::EMPTY));
    // expression 0: a stand-in guard
    let mut arenas = Thir::default();
    arenas.alloc_expr(crate::mir::build::thir::Expr { kind: ExprKind::Tuple { fields: Box::new([]) }, ty: unit, temp_scope: None, bid: None, span: Span::default() });
    let thir = ThirBody::for_test(arenas, Vec::new(), ExprId(0), tcx.tys.unit);
    let mut builder = Builder {
        root: &root,
        thir: &thir,
        owner,
        source: MirSource { owner: def, kind: crate::mir::BodyKind::Fn },
        body_span: Span::default(),
        arg_count: 1,
        coroutine: None,
        cfg: Cfg::default(),
        scopes: Scopes::default(),
        source_scopes: vec![SourceScopeData { span: Span::default(), parent_scope: None }],
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
    let decl = |ty, kind| LocalDecl { ty, mutability: Mutability::Mut, user_ty: Vec::new(), source_info: SourceInfo::default(), kind, local_info: LocalInfo::Boring };
    builder.local_decls.push(decl(unit, LocalKind::Return));
    builder.local_decls.push(decl(body.locals[0].ty, LocalKind::Arg));
    builder.cfg.start_new_block();
    f(&mut builder, &pats, &body)
}

/// The body the builder holds, with every block that has no terminator returning.
fn finish(builder: &mut Builder<'_>, name: &str) -> (Body, String) {
    for block in builder.cfg.basic_blocks.iter_mut() {
        if block.terminator.is_none() {
            block.terminator = Some(crate::mir::Terminator { source_info: SourceInfo::default(), kind: TerminatorKind::Return });
        }
    }
    let body = Body {
        phase: MirPhase::Built,
        source: builder.source.clone(),
        basic_blocks: builder.cfg.basic_blocks.clone(),
        local_decls: builder.local_decls.clone(),
        source_scopes: builder.source_scopes.clone(),
        span: Span::default(),
        user_type_annotations: builder.canonical_user_type_annotations.clone(),
        upvars: Vec::new(),
        allocs: builder.allocs.clone(),
        var_debug_info: builder.var_debug_info.clone(),
        arg_count: 1,
        spread_arg: None,
        promoted: Vec::new(),
        coroutine: None,
    };
    let text = crate::mir::pretty::print_body(builder.root.cx.prog, builder.root.cx.tcx, name, &body);
    (body, text)
}

/// Decision tree of the patterns of `source` matched against `_1`; returns the printed body.
fn tree(source: &str) -> String {
    run(source, "f", |b, pats, _| {
        let scrutinee = PlaceBuilder::from(Local(1));
        let patterns = pats.iter().map(|p| (p, HasMatchGuard::No)).collect();
        let built = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), patterns, false).unwrap();
        // the arm bodies are empty: every success block returns
        let _ = built;
        let (body, text) = finish(b, "f");
        let errors = crate::mir::validate::validate(&body, b.root.cx.tcx);
        assert!(errors.is_empty(), "{errors:?}\n{text}");
        text
    })
}

fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

const ENUM: &str = "pub enum E { A(u8), B, C(bool) }\n";

#[test]
fn enum_switch_and_nested_tests() {
    let t = tree(&format!("{ENUM}pub fn f(e: E) {{ match e {{ E::A(0) => {{}} E::A(_) => {{}} E::B => {{}} E::C(true) => {{}} E::C(false) => {{}} }} }}"));
    eprintln!("{t}");
    assert_eq!(count(&t, "discriminant(_1)"), 1, "{t}");
}

#[test]
fn integer_values_and_ranges() {
    let t = tree("pub fn f(x: u8) { match x { 0 => {} 1..=5 => {} 6 => {} 7 => {} 100.. => {} _ => {} } }");
    eprintln!("{t}");
    assert!(count(&t, "switchInt(copy _1)") >= 1, "{t}");
}

#[test]
fn bool_and_tuple_scrutinee() {
    let t = tree("pub fn f(x: (bool, bool, bool)) { match x { (true, _, true) => {} (false, false, _) => {} (_, true, _) => {} (true, _, false) => {} } }");
    eprintln!("{t}");
    assert!(count(&t, "switchInt(copy (_1.0: bool))") >= 1, "{t}");
}

#[test]
fn slice_length_tests() {
    let t = tree("pub fn f(x: &[u8]) { match x { [] => {} [a] => {} [a, b] => {} [a, .., b] => {} [..] => {} } }");
    eprintln!("{t}");
    assert!(count(&t, "PtrMetadata") >= 1, "{t}");
}

#[test]
fn array_pattern_has_no_length_test() {
    let t = tree("pub fn f(x: [u8; 3]) { match x { [0, ..] => {} [_, 1, _] => {} [.., 2] => {} _ => {} } }");
    eprintln!("{t}");
    assert_eq!(count(&t, "PtrMetadata"), 0, "{t}");
}

#[test]
fn or_pattern_binds_on_every_sub_branch() {
    // value_dump.rs:430 (bck-baseline/repro_or_pattern_tuple.rs): `x` must be bound by both alternatives
    run("pub fn f(data: (i32, i32)) { match data { (x, 1) | (x, 2) => {} _ => {} } }", "f", |b, pats, _| {
        let scrutinee = PlaceBuilder::from(Local(1));
        let patterns = pats.iter().map(|p| (p, HasMatchGuard::No)).collect();
        let built = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), patterns, false).unwrap();
        assert_eq!(built.branches.len(), 2);
        let first = &built.branches[0];
        assert_eq!(first.sub_branches.len(), 2, "one sub-branch per alternative");
        for sub in &first.sub_branches {
            assert_eq!(sub.bindings.len(), 1);
            assert_eq!(sub.bindings[0].source.projection.len(), 1, "x is bound from (_1.0)");
            assert_eq!(sub.bindings[0].var_id, first.sub_branches[0].bindings[0].var_id);
        }
        assert_ne!(first.sub_branches[0].success_block, first.sub_branches[1].success_block);
        assert_eq!(built.branches[1].sub_branches.len(), 1);
    });
}

#[test]
fn trivial_or_patterns_merge_into_one_sub_branch() {
    run("pub fn f(x: u8) { match x { 1 | 2 | 3 => {} _ => {} } }", "f", |b, pats, _| {
        let scrutinee = PlaceBuilder::from(Local(1));
        let patterns = pats.iter().map(|p| (p, HasMatchGuard::No)).collect();
        let built = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), patterns, false).unwrap();
        assert_eq!(built.branches[0].sub_branches.len(), 1);
    });
}

#[test]
fn guards_fake_borrow_the_matched_places() {
    run(&format!("{ENUM}pub fn f(e: E) {{ match e {{ E::A(n) if n == 1 => {{}} E::A(_) => {{}} _ => {{}} }} }}"), "f", |b, pats, _| {
        let scrutinee = PlaceBuilder::from(Local(1));
        let patterns = pats.iter().zip([HasMatchGuard::Yes, HasMatchGuard::No, HasMatchGuard::No]).collect();
        let built = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), patterns, false).unwrap();
        let places: Vec<_> = built.fake_borrow_temps.iter().map(|(p, _, k)| (p.projection.len(), *k)).collect();
        // only the scrutinee `_1` is switched on; `(_1 as A).0` is bound by value (no deref prefix)
        assert_eq!(places, [(0, FakeBorrowKind::Shallow)]);
        for (_, temp, _) in &built.fake_borrow_temps {
            assert!(matches!(b.local_decls[temp.0 as usize].local_info, LocalInfo::FakeBorrow));
        }
        // no guard, no fake borrows
        let none = pats.iter().map(|p| (p, HasMatchGuard::No)).collect();
        let b2 = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), none, false).unwrap();
        assert!(b2.fake_borrow_temps.is_empty());
    });
}

#[test]
fn bindings_by_value_and_by_reference_keep_their_sources() {
    run("pub fn f(a: (u8, u8)) { let (ref r, mut m) = a; }", "f", |b, pats, _| {
        let scrutinee = PlaceBuilder::from(Local(1));
        let built = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), vec![(&pats[0], HasMatchGuard::No)], false).unwrap();
        let sub = &built.branches[0].sub_branches[0];
        assert_eq!(sub.bindings.len(), 2);
        assert!(matches!(sub.bindings[0].binding_mode.0, ByRef::Yes(Mutability::Not)));
        assert_eq!(sub.bindings[1].binding_mode, BindingMode(ByRef::No, Mutability::Mut));
    });
}

#[test]
fn declare_bindings_creates_user_locals() {
    run("pub fn f(a: (u8, u8)) { match a { (x, y) if x == y => {} _ => {} } }", "f", |b, pats, _| {
        let before = b.local_decls.len();
        let scope = b.declare_bindings(None, Span::default(), &pats[0], Some(ExprId(0)), None).unwrap();
        assert!(scope.is_some());
        // x and y: each has an arm-body local and, for the guard, a reference local
        assert!(matches!(b.local_decls[before].local_info, LocalInfo::User(BindingForm::Var(_))));
        assert!(matches!(b.local_decls[before + 1].local_info, LocalInfo::User(BindingForm::RefForGuard(_))));
        assert_eq!(b.local_decls.len() - before, 4);
        assert!(b.var_indices.values().all(|l| matches!(l, LocalsForNode::ForGuard { .. })));
        assert_eq!(b.var_debug_info.len(), 4);
    });
}

#[test]
fn guards_fake_borrow_deref_prefixes_of_bound_places() {
    run(&format!("{ENUM}pub fn f(e: &E) {{ match e {{ E::A(n) if *n == 1 => {{}} _ => {{}} }} }}"), "f", |b, pats, _| {
        let scrutinee = PlaceBuilder::from(Local(1));
        let patterns = pats.iter().zip([HasMatchGuard::Yes, HasMatchGuard::No]).collect();
        let built = b.lower_match_tree(BasicBlock(0), Span::default(), &scrutinee, Span::default(), patterns, false).unwrap();
        let places: Vec<_> = built.fake_borrow_temps.iter().map(|(p, _, k)| (p.projection.clone(), *k)).collect();
        // `(*_1)` is switched on; `_1` is its deref prefix; the bound `&(*_1 as A).0` has the same prefix
        assert!(places.contains(&(vec![], FakeBorrowKind::Shallow)), "{places:?}");
        assert!(places.contains(&(vec![PlaceElem::Deref], FakeBorrowKind::Shallow)), "{places:?}");
    });
}
