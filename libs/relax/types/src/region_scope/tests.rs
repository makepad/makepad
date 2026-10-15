use super::*;
use crate::program::{DefId, Program};
use crate::tcx::{EvalCx, EvalState, Tcx, TypingEnv};

fn collect(source: &str, edition: u16) -> (Program, Tcx, Body) {
    let sess = &std::sync::Arc::new(crate::Session::default());
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("relax-region-scope-{}-{n}.rs", std::process::id()));
    std::fs::write(&path, source).unwrap();
    let mut prog = Program::new(sess);
    prog.load_crate("scope_test", path.to_str().unwrap(), edition, crate::cfg::CfgSet::host(sess, &[]), vec![]).unwrap();
    std::fs::remove_file(path).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut def = None;
    for i in 0..prog.defs.len() { if prog.name(DefId(i as u32)) == "probe" { def = Some(DefId(i as u32)); } }
    let state = EvalState::new();
    let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
    let body = crate::typeck::instantiate_body(&cx, &prog, &tcx, def.unwrap(), &[]);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    (prog, tcx, body)
}
fn expr(prog: &Program, body: &Body, text: &str) -> ExprId {
    let pf = &prog.files[body.file as usize];
    // Include lazy-body arenas through typeck's recorded expression ranges.
    for (lo, n) in [(body.expr_lo, body.expr_n1), (body.expr_lo2, body.expr_ty.len() as u32 - body.expr_n1)] {
        for i in lo..lo.saturating_add(n) {
            let e = pf.ast.expr(ExprId(i));
            if pf.bytes(e.lo, e.hi) == text.as_bytes() { return ExprId(i); }
        }
    }
    panic!("missing expression {text}")
}
fn local(body: &Body, prog: &Program, name: &str) -> u32 {
    for (i, l) in body.locals.iter().enumerate() { if prog.syms.str(l.name) == name { return i as u32; } }
    panic!("missing local {name}")
}
fn scope_of(tree: &ScopeTree, e: ExprId) -> Scope { tree.scopes[tree.temporary_scope(e).unwrap().0 as usize] }

#[test]
fn region_scope_extension_grammar() {
    // Reference destructors.scope.lifetime-extension and rustc check/region.rs:620.
    for init in ["&f()", "[&f()]", "(&f(),)", "S { r: &f() }", "T(&f())", "&*(&f())", "{ &f() }", "if true { &f() } else { &0 }", "match true { true => &f(), false => &0 }"] {
        let src = format!("struct S<'a> {{ r: &'a u32 }} struct T<'a>(&'a u32); fn f() -> u32 {{ 1 }} fn probe() {{ let x = {init}; }}");
        let (p, _, b) = collect(&src, 2021);
        let t = region_scope_tree(&p, &b).unwrap();
        assert_eq!(t.temporary_scope(expr(&p, &b, "f()")), t.var_scope(local(&b, &p, "x")), "{init}");
    }
}
#[test]
fn region_scope_calls_do_not_extend() {
    let (p, _, b) = collect("fn f() -> u32 { 1 } fn identity(x: &u32) -> &u32 { x } fn probe() { let x = identity(&f()); }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert!(matches!(scope_of(&t, expr(&p, &b, "f()")).node, Node::Statement(_)));
    assert_ne!(t.temporary_scope(expr(&p, &b, "f()")), t.var_scope(local(&b, &p, "x")));
}
#[test]
fn region_scope_explicit_ref_and_place_chain() {
    for init in ["f()", "f().0", "[f()][0].0"] {
        let src = format!("fn f() -> (u32,) {{ (1,) }} fn probe() {{ let ref x = {init}; }}");
        let (p, _, b) = collect(&src, 2021);
        let t = region_scope_tree(&p, &b).unwrap();
        let root = if init.starts_with('[') { "[f()]" } else { "f()" };
        assert_eq!(t.temporary_scope(expr(&p, &b, root)), t.var_scope(local(&b, &p, "x")));
        if init.starts_with('[') { assert!(matches!(scope_of(&t, expr(&p, &b, "f()")).node, Node::Statement(_))); }
    }
}
#[test]
fn region_scope_remainders_and_parameters() {
    let (p, _, b) = collect("fn probe(arg: u32) { let a = 1; let b = 2; { let c = 3; } }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    let a = t.var_scope(local(&b, &p, "a")).unwrap();
    let bb = t.var_scope(local(&b, &p, "b")).unwrap();
    let c = t.var_scope(local(&b, &p, "c")).unwrap();
    assert_eq!(t.scopes[a.0 as usize].data, ScopeData::Remainder(0));
    assert_eq!(t.scopes[bb.0 as usize].data, ScopeData::Remainder(1));
    assert!(t.is_subscope_of(c, bb)); assert!(t.is_subscope_of(bb, a)); assert!(!t.is_subscope_of(a, bb));
    assert_eq!(t.var_scope(local(&b, &p, "arg")), Some(t.bodies[0].arguments));
}
#[test]
fn region_scope_tail_editions() {
    for ed in [2021, 2024] {
        let (p, _, b) = collect("fn f() -> u32 { 1 } fn probe() { let x = { let y = 0; f() }; }", ed);
        let t = region_scope_tree(&p, &b).unwrap();
        let e = expr(&p, &b, "f()");
        let s = scope_of(&t, e);
        if ed == 2024 { assert_eq!(s.node, Node::Expr(e.0)); } else { assert!(matches!(s.node, Node::Statement(_))); }
        let y = t.var_scope(local(&b, &p, "y")).unwrap();
        assert_eq!(t.is_subscope_of(t.temporary_scope(e).unwrap(), y), ed == 2024);
    }
}
#[test]
fn region_scope_if_let_editions_and_boolean_conditions() {
    for ed in [2021, 2024] {
        let (p, _, b) = collect("fn f() -> (u32,) { (1,) } fn probe() { if let (1,) = f() {} else {} }", ed);
        let t = region_scope_tree(&p, &b).unwrap();
        assert_eq!(scope_of(&t, expr(&p, &b, "f()")).data == ScopeData::IfThenRescope, ed == 2024);
    }
    let (p, _, b) = collect("fn f() -> bool { true } fn g() -> bool { false } fn probe() { if f() && g() {} while f() {} }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    let g = expr(&p, &b, "g()"); assert_eq!(scope_of(&t, g).node, Node::Expr(g.0));
}
#[test]
fn region_scope_while_let_is_iteration_scoped() {
    for ed in [2021, 2024] {
        let (p, _, b) = collect("fn f() -> (u32,) { (1,) } fn probe() { while let (1,) = f() { break; } }", ed);
        let t = region_scope_tree(&p, &b).unwrap();
        let s = scope_of(&t, expr(&p, &b, "f()"));
        if ed == 2021 { assert!(matches!(s.node, Node::Synthetic(_, Synthetic::Iteration))); }
        else { assert_eq!(s.data, ScopeData::IfThenRescope); }
    }
}
#[test]
fn region_scope_match_scrutinee_and_guard() {
    let (p, _, b) = collect("fn f() -> u32 { 1 } fn g() -> bool { true } fn probe() { match f() { x if g() => x, _ => 0 }; }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert!(matches!(scope_of(&t, expr(&p, &b, "f()")).node, Node::Statement(_)));
    let g = expr(&p, &b, "g()"); assert_eq!(scope_of(&t, g).node, Node::Expr(g.0));
    assert!(matches!(t.scopes[t.var_scope(local(&b, &p, "x")).unwrap().0 as usize].node, Node::Arm(_)));
}
#[test]
fn region_scope_let_else_restores_context() {
    let (p, _, b) = collect("fn probe() { let x = 0; let (1, y) = (2, 3) else { let z = &4; return; }; }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    let y = t.var_scope(local(&b, &p, "y")).unwrap();
    let z = t.var_scope(local(&b, &p, "z")).unwrap();
    assert!(!t.is_subscope_of(z, y));
    assert!(t.is_subscope_of(z, t.var_scope(local(&b, &p, "x")).unwrap()));
    assert_eq!(t.temporary_scope(expr(&p, &b, "4")), Some(z));
}
#[test]
fn region_scope_const_extension() {
    let (p, _, b) = collect("const probe: &u32 = &1;", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert_eq!(t.temporary_scope(expr(&p, &b, "1")), None);
    assert!(t.temporary_scope(expr(&p, &b, "&1")).is_some());
    assert_eq!(t.bodies[0].kind, BodyKind::Constant);
}
#[test]
fn region_scope_nested_awaits_and_positions() {
    let (p, _, b) = collect("fn probe() { let outer = 0; let future = async { let a = &1; { let ended = 2; } async {}.await; let later = 3; let nested = async { async {}.await; }; async {}.await; }; }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert_eq!(t.yields.len(), 3);
    let outer = t.var_scope(local(&b, &p, "outer")).unwrap();
    let a = t.var_scope(local(&b, &p, "a")).unwrap();
    let ended = t.var_scope(local(&b, &p, "ended")).unwrap();
    let later = t.var_scope(local(&b, &p, "later")).unwrap();
    assert_eq!(t.yields_in_scope(outer).len(), 0);
    assert_eq!(t.yields_in_scope(a).len(), 2);
    assert_eq!(t.yields_in_scope(ended).len(), 0);
    assert_eq!(t.yields_in_scope(later).len(), 1);
    assert_eq!(t.yields_in_scope(t.temporary_scope(expr(&p, &b, "1")).unwrap()).len(), 2);
    for y in &t.yields {
        let YieldKind::Await { operand, future_scope } = y.kind else { panic!() };
        let pos = t.expression_position(operand).unwrap();
        assert_eq!(pos.body, y.position.body); assert!(pos.count < y.position.count);
        assert_eq!(t.yields_in_scope(future_scope).len(), 1);
    }
}
#[test]
fn region_scope_closure_body_is_separate() {
    let (p, _, b) = collect("fn probe() { let x = 0; let f = |p: u32| { let y = &1; p }; }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert_eq!(t.bodies.len(), 2);
    assert_eq!(t.bodies[1].kind, BodyKind::Closure);
    assert_eq!(t.var_scope(local(&b, &p, "p")), Some(t.bodies[1].arguments));
    let x = t.var_scope(local(&b, &p, "x")).unwrap();
    let y = t.var_scope(local(&b, &p, "y")).unwrap();
    assert_ne!(t.scopes[x.0 as usize].body, t.scopes[y.0 as usize].body);
}

/// Compare the actual builder's StorageDead events, not a second copy of its scope rules.
/// Call destinations identify the temporary independently of local allocation order.
fn cross_check(source: &str, edition: u16, extended: bool, tail: bool) -> bool {
    use crate::borrowck::bir::{Callee, StmtKind, TermKind};
    let (p, tcx, b) = collect(source, edition);
    let tree = region_scope_tree(&p, &b).unwrap();
    let state = EvalState::new();
    let tx = crate::borrowck::tyx::Tx::new(&p, &tcx, TypingEnv::fully_monomorphized(&p), &state);
    let mut builder = crate::borrowck::build::Build::new(&tx, &b, "probe".into());
    builder.build_body();
    let bir = &builder.out;
    assert!(bir.unsupported.is_none(), "{:?}", bir.unsupported);
    let mut temp = None;
    for bb in &bir.blocks {
        if let Some(term) = &bb.term {
            if let TermKind::Call { callee: Callee::Fn(d, _), dest, .. } = &term.kind {
                if p.name(*d) == "f" { temp = Some(dest.local); }
            }
        }
    }
    let temp = temp.expect("f call destination");
    let mut endings = Vec::new();
    for bb in &bir.blocks {
        for stmt in &bb.stmts {
            if let StmtKind::StorageDead(l) = stmt.kind { if l == temp { endings.push(stmt.span); } }
        }
    }
    assert_eq!(endings.len(), 1, "{source}: {:?}", bir.blocks);
    let temp_expr = expr(&p, &b, "f()");
    let scope = tree.temporary_scope(temp_expr).unwrap();
    let data = tree.scopes[scope.0 as usize];
    let pf = &p.files[b.file as usize];
    if extended {
        assert_eq!(Some(scope), tree.var_scope(local(&b, &p, "x")));
        let Node::Block(id) = data.node else { panic!("{data:?}") };
        return endings[0].lo == pf.ast.block(BlockId(id)).hi - 1;
    } else if tail && edition == 2024 {
        assert_eq!(data.node, Node::Expr(expr(&p, &b, "f().0").0));
        // Tail cleanup happens under the tail's source span.
        let e = pf.ast.expr(temp_expr);
        assert!(endings[0].lo <= e.lo && endings[0].hi >= e.hi, "{:?}", endings);
    } else {
        assert!(matches!(data.node, Node::Statement(_)), "{data:?}");
        // The builder retains the whole statement's expression span for this cleanup.
        let e = pf.ast.expr(temp_expr);
        assert!(endings[0].lo <= e.lo && endings[0].hi >= e.hi, "{:?}", endings);
    }
    true
}
#[test]
fn region_scope_builder_cross_check() {
    for ed in [2021, 2024] {
        for init in ["&f()", "[&f()]", "S { r: &f() }", "T(&f())", "&*(&f())"] {
            let src = format!("struct S<'a> {{ r: &'a u32 }} struct T<'a>(&'a u32); fn f() -> u32 {{ 1 }} fn probe() {{ let x = {init}; }}");
            assert!(cross_check(&src, ed, true, false), "{src}");
        }
        assert!(cross_check("fn f() -> (u32,) { (1,) } fn probe() { let x = &{ f().0 }; }", ed, false, true));
        assert!(cross_check("fn f() -> u32 { 1 } fn probe() { let x = match f() { n => n }; }", ed, false, false));
    }
}
#[test]
fn region_scope_builder_explicit_ref_projection_disagreement() {
    // rustc check/region.rs:521,702 extends the whole ET chain. The old builder's
    // !is_place_expr guard (build.rs:950) misses a projected rvalue here.
    for ed in [2021, 2024] {
        assert!(!cross_check("fn f() -> (u32,) { (1,) } fn probe() { let ref x = f().0; }", ed, true, false));
    }
}
#[test]
fn region_scope_destructuring_assignment_rhs() {
    let (p, _, b) = collect("fn f() -> (u32,) { (1,) } fn probe() { let mut x = 0; (x,) = f(); }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert!(matches!(scope_of(&t, expr(&p, &b, "f()")).node, Node::Synthetic(_, Synthetic::AssignLet)));
}
#[test]
fn region_scope_builtin_matches_and_assert() {
    let (p, _, b) = collect("fn f() -> u32 { 1 } fn g() -> bool { true } fn probe() { let a = matches!(f(), x if g()); assert!(g()); }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    assert!(matches!(scope_of(&t, expr(&p, &b, "f()")).node, Node::Statement(_)));
    let x = t.var_scope(local(&b, &p, "x")).unwrap();
    assert!(matches!(t.scopes[x.0 as usize].node, Node::Synthetic(_, Synthetic::MacroArm)));
}

#[test]
#[ignore = "per-span edition: frontend expansion contexts"]
fn region_scope_cross_edition_macro_and_forwarded_fragment() {
    // The definition's block gets its definition edition; a substituted block keeps
    // its caller edition (rustc_span/src/hygiene.rs:912; check/region.rs:146).
    for (definition, caller) in [(2021, 2024), (2024, 2021)] {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let dir = std::env::temp_dir().join(format!("relax-region-edition-{}-{definition}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dep = dir.join("dep.rs"); let root = dir.join("root.rs");
        std::fs::write(&dep, "pub fn make() -> u32 { 1 } #[macro_export] macro_rules! tail { () => {{ let inner = 0; $crate::make() }}; } #[macro_export] macro_rules! forward { ($b:block) => { $b }; }").unwrap();
        std::fs::write(&root, "fn probe() { let a = dependency::tail!(); let b = dependency::forward!({ let forwarded = 0; dependency::make() }); }").unwrap();
        let mut p = Program::new(sess);
        let dk = p.load_crate("dependency", dep.to_str().unwrap(), definition, crate::cfg::CfgSet::host(sess, &[]), vec![]).unwrap();
        let sym = p.syms.get("dependency").unwrap();
        p.load_crate("caller", root.to_str().unwrap(), caller, crate::cfg::CfgSet::host(sess, &[]), vec![(sym, dk)]).unwrap();
        std::fs::remove_file(dep).unwrap(); std::fs::remove_file(root).unwrap(); std::fs::remove_dir(dir).unwrap();
        p.resolve_imports(); let mut tcx = Tcx::new(); tcx.collect(&p);
        let mut def = None;
        for i in 0..p.defs.len() { if p.name(DefId(i as u32)) == "probe" { def = Some(DefId(i as u32)); } }
        let state = EvalState::new(); let cx = EvalCx::new(&p, &tcx, TypingEnv::fully_monomorphized(&p), &state);
        let b = crate::typeck::instantiate_body(&cx, &p, &tcx, def.unwrap(), &[]);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        let tree = region_scope_tree(&p, &b).unwrap();
        for (name, ed) in [("inner", definition), ("forwarded", caller)] {
            let scope = tree.var_scope(local(&b, &p, name)).unwrap();
            let Node::Block(id) = tree.scopes[scope.0 as usize].node else { panic!() };
            let Some(Stmt::Expr(tail, false)) = p.files[b.file as usize].ast.stmts(BlockId(id)).last() else { panic!() };
            assert_eq!(tree.is_subscope_of(tree.temporary_scope(*tail).unwrap(), scope), ed >= 2024, "{name}: definition {definition}, caller {caller}");
        }
    }
}

#[test]
fn region_scope_for_and_yield_syntax() {
    // Typeck currently rejects yield; exercise the read-only lexical provider directly.
    let sess = &std::sync::Arc::new(crate::Session::default());
    let pf = crate::parser::parse_owned(sess, b"fn probe() { for x in head() { yield x; } }".to_vec(), 2021).unwrap();
    let mut root = None;
    for item in pf.ast.items.iter() { if let ItemKind::Fn(_, Some(b), _) = item.kind { root = Some(b); } }
    let p = Program::new(sess);
    let tcx = Tcx::new();
    let b = Body::empty(crate::program::NO_DEF, vec![], 0, tcx.tys.unit);
    let mut v = Resolve { prog: &p, file: &pf, body: &b, tree: ScopeTree::default(), cx: Context { parent: None, var_parent: None, body: BodyId(0) } };
    let root = root.unwrap();
    v.enter_body(Node::Block(root.0), BodyKind::Coroutine); v.block(root, true);
    let t = v.tree;
    assert_eq!(t.yields.len(), 1);
    assert_eq!(t.yields[0].kind, YieldKind::Yield);
    let mut head = None;
    for (i, e) in pf.ast.exprs.iter().enumerate() { if matches!(e.kind, ExprKind::Call(..)) { head = Some(ExprId(i as u32)); } }
    let scope = t.temporary_scope(head.unwrap()).unwrap();
    assert!(matches!(t.scopes[scope.0 as usize].node, Node::Synthetic(_, Synthetic::ForMatch)));
    assert_eq!(t.yields_in_scope(scope).len(), 1);
    assert!(t.expression_position(head.unwrap()).unwrap().count < t.yields[0].position.count);
}
#[test]
fn region_scope_const_blocks_repeat_lengths_and_async_fn() {
    let (p, _, b) = collect("async fn probe() { let a = const { &1 }; let b = [0; 2]; async {}.await; }", 2024);
    let t = region_scope_tree(&p, &b).unwrap();
    assert_eq!(t.bodies[0].kind, BodyKind::Coroutine);
    assert_eq!(t.temporary_scope(expr(&p, &b, "1")), None);
    let count = t.expression_position(expr(&p, &b, "2")).unwrap();
    assert_eq!(t.bodies[count.body.0 as usize].kind, BodyKind::Constant);
    assert_eq!(t.yields.len(), 1);
    assert_eq!(t.yields[0].position.body, BodyId(0));
}
#[test]
fn region_scope_nonextended_temp_ends_before_await() {
    let (p, _, b) = collect("fn f() -> u32 { 1 } fn g(x: &u32) {} async fn probe() { g(&f()); async {}.await; }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    let s = t.temporary_scope(expr(&p, &b, "f()")).unwrap();
    assert_eq!(t.yields_in_scope(s).len(), 0);
    assert_eq!(t.yields_in_scope(t.bodies[0].arguments).len(), 1);
}
#[test]
fn region_scope_match_let_guard_bindings() {
    let (p, _, b) = collect("fn f() -> (u32,) { (1,) } fn probe() { match 0 { x if let (y,) = f() => y, _ => 0 }; }", 2024);
    let t = region_scope_tree(&p, &b).unwrap();
    let y = t.var_scope(local(&b, &p, "y")).unwrap();
    assert_eq!(t.scopes[y.0 as usize].data, ScopeData::MatchGuard);
    assert_eq!(t.temporary_scope(expr(&p, &b, "f()")), Some(y));
    assert!(t.is_subscope_of(y, t.var_scope(local(&b, &p, "x")).unwrap()));
}

#[test]
fn region_scope_destructuring_assignment_borrow_extension() {
    let (p, _, b) = collect("fn f() -> u32 { 1 } fn probe() { let mut x = &0; (x,) = (&f(),); }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    let s = scope_of(&t, expr(&p, &b, "f()"));
    assert_eq!(s.data, ScopeData::Remainder(0));
    assert!(matches!(s.node, Node::Synthetic(_, Synthetic::AssignBlock)));
}
