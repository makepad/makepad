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
    for init in ["&f()", "[&f()]", "(&f(),)", "S { r: &f() }", "T(&f())", "&*(&f())", "{ &f() }", "if true { &f() } else { &0 }", "match true { true => &f(), false => &0 }", "((&f()))", "&((f()))"] {
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
    for init in ["f()", "f().0", "[f()][0].0", "((f()))", "((f().0))"] {
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
    let arguments = t.scope(Node::Block(b.body.unwrap().0), ScopeData::Arguments).unwrap();
    assert_eq!(t.var_scope(local(&b, &p, "arg")), Some(arguments));
}
#[test]
fn region_scope_tail_editions() {
    for ed in [2021, 2024] {
        for tail in ["f()", "((f()))"] {
            let src = format!("fn f() -> u32 {{ 1 }} fn probe() {{ let x = {{ let y = 0; {tail} }}; }}");
            let (p, _, b) = collect(&src, ed);
            let t = region_scope_tree(&p, &b).unwrap();
            let e = expr(&p, &b, "f()");
            let s = scope_of(&t, e);
            if ed == 2024 { assert_eq!(s.node, Node::Expr(e.0)); } else { assert!(matches!(s.node, Node::Statement(_))); }
            let y = t.var_scope(local(&b, &p, "y")).unwrap();
            assert_eq!(t.is_subscope_of(t.temporary_scope(e).unwrap(), y), ed == 2024);
            let ast = &p.files[b.file as usize].ast;
            let outer = ast.expr(expr(&p, &b, tail));
            assert_eq!(t.expr_span(ast, e), (outer.lo, outer.hi));
            assert!(t.scopes.iter().all(|s| !matches!(s.node, Node::Expr(id) if matches!(ast.expr(ExprId(id)).kind, ExprKind::Paren(_)))));
        }
    }
}
#[test]
fn region_scope_if_let_editions_and_boolean_conditions() {
    for ed in [2021, 2024] {
        let (p, _, b) = collect("fn f() -> (u32,) { (1,) } fn probe() { if let (1,) = f() {} else {} }", ed);
        let t = region_scope_tree(&p, &b).unwrap();
        assert_eq!(scope_of(&t, expr(&p, &b, "f()")).data == ScopeData::IfThenRescope, ed == 2024);
    }
    for condition in ["f() && g()", "((f() && g()))"] {
        let src = format!("fn f() -> bool {{ true }} fn g() -> bool {{ false }} fn probe() {{ if {condition} {{}} while f() {{}} }}");
        let (p, _, b) = collect(&src, 2021);
        let t = region_scope_tree(&p, &b).unwrap();
        let g = expr(&p, &b, "g()"); assert_eq!(scope_of(&t, g).node, Node::Expr(g.0));
    }
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
}
#[test]
fn region_scope_closure_body_is_separate() {
    for closure in ["{ let y = &1; p }", "(({ let y = &1; p }))"] {
        let src = format!("fn probe() {{ let x = 0; let f = |p: u32| {closure}; }}");
        let (p, _, b) = collect(&src, 2021);
        let t = region_scope_tree(&p, &b).unwrap();
        let closure_body = expr(&p, &b, "{ let y = &1; p }");
        let arguments = t.scope(Node::Expr(closure_body.0), ScopeData::Arguments).unwrap();
        assert_eq!(t.var_scope(local(&b, &p, "p")), Some(arguments));
        let x = t.var_scope(local(&b, &p, "x")).unwrap();
        let y = t.var_scope(local(&b, &p, "y")).unwrap();
        assert!(!t.is_subscope_of(x, arguments));
        assert!(t.is_subscope_of(y, arguments));
        let ast = &p.files[b.file as usize].ast;
        assert!(t.scopes.iter().all(|s| !matches!(s.node, Node::Expr(id) if matches!(ast.expr(ExprId(id)).kind, ExprKind::Paren(_)))));
    }
}

/// Compare the actual builder's StorageDead events, not a second copy of its scope rules.
/// Call destinations identify the temporary independently of local allocation order.
fn cross_check(source: &str, edition: u16, extended: bool, tail: bool) -> bool {
    use crate::mir::{StatementKind, TerminatorKind};
    let (p, tcx, b) = collect(source, edition);
    let tree = region_scope_tree(&p, &b).unwrap();
    let state = EvalState::new();
    let env = TypingEnv::fully_monomorphized(&p);
    let eval = crate::tcx::EvalCx::new(&p, &tcx, env, &state);
    let root = crate::mir::build::BuildRoot::new(crate::mir::build::BuildCx { prog: &p, tcx: &tcx, env, eval: &eval }, &b).unwrap();
    let owner = crate::borrowck::body::CheckedBody::item(&p, b.def).unwrap();
    let mir = crate::mir::build::build_mir(&root, owner).unwrap_or_else(|e| panic!("{source}: {e:?}"));
    let mut temp = None;
    for bb in &mir.basic_blocks {
        if let Some(term) = &bb.terminator {
            if let TerminatorKind::Call { func: crate::mir::Operand::Constant(c), destination, .. } = &term.kind {
                if let crate::mir::Const::Val(_, ty) = &c.const_ {
                    if let crate::types::TyKind::FnDef(d, ..) = tcx.tys.kind(*ty) { if p.name(*d) == "f" { temp = Some(destination.local); } }
                }
            }
        }
    }
    let temp = temp.expect("f call destination");
    let mut endings = Vec::new();
    for bb in &mir.basic_blocks {
        for stmt in &bb.statements {
            if let StatementKind::StorageDead(l) = stmt.kind { if l == temp { endings.push(stmt.source_info.span); } }
        }
    }
    assert_eq!(endings.len(), 1, "{source}: {:?}", mir.basic_blocks);
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
        // rustc's cleanup span is the end point of the scope's node (builder/scope.rs `end_point`).
        let e = pf.ast.expr(temp_expr);
        assert!(endings[0].lo >= e.hi - 1, "{:?}", endings);
        let Node::Expr(id) = data.node else { panic!("{data:?}") };
        let (_, hi) = tree.expr_span(&pf.ast, ExprId(id));
        assert_eq!((endings[0].lo, endings[0].hi), (hi - 1, hi));
    } else {
        assert!(matches!(data.node, Node::Statement(_)), "{data:?}");
        // rustc's cleanup span is the end point of the statement (its `;`).
        let e = pf.ast.expr(temp_expr);
        assert!(endings[0].lo >= e.hi - 1, "{:?}", endings);
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
        assert!(cross_check("fn f() -> (u32,) { (1,) } fn probe() { let x = &{ ((f().0)) }; }", ed, false, true));
        assert!(cross_check("fn f() -> u32 { 1 } fn probe() { let x = match f() { n => n }; }", ed, false, false));
    }
}
#[test]
fn region_scope_builder_explicit_ref_projection_disagreement() {
    // rustc check/region.rs:521,702 extends the whole ET chain. The MIR builder follows rustc, so
    // the scope tree and the builder agree (the deleted BIR builder's guard missed this case).
    for ed in [2021, 2024] {
        assert!(cross_check("fn f() -> (u32,) { (1,) } fn probe() { let ref x = f().0; }", ed, true, false));
    }
}
#[test]
fn region_scope_destructuring_assignment_rhs() {
    for assignee in ["(x,)", "((x,))", "((x),)"] {
        let src = format!("fn f() -> (u32,) {{ (1,) }} fn probe() {{ let mut x = 0; {assignee} = f(); }}");
        let (p, _, b) = collect(&src, 2021);
        let t = region_scope_tree(&p, &b).unwrap();
        assert!(matches!(scope_of(&t, expr(&p, &b, "f()")).node, Node::Synthetic(_, Synthetic::AssignLet)));
        let ast = &p.files[b.file as usize].ast;
        assert!(t.scopes.iter().all(|s| !matches!(s.node, Node::Expr(id) if matches!(ast.expr(ExprId(id)).kind, ExprKind::Paren(_)))));
    }
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
            let Some(Stmt::Expr(false, tail)) = p.files[b.file as usize].ast.stmts(BlockId(id)).last() else { panic!() };
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
    for item in pf.ast.items.iter() { if let ItemKind::Fn(_, body, _) = item.kind { if let Some(b) = body.get() { root = Some(b); } } }
    let p = Program::new(sess);
    let tcx = Tcx::new();
    let b = Body::empty(crate::program::NO_DEF, vec![], 0, tcx.tys.unit);
    let mut v = Resolve { prog: &p, file: &pf, body: &b, tree: ScopeTree::default(), cx: Context { parent: None, var_parent: None }, extended_super_lets: Default::default() };
    let root = root.unwrap();
    v.enter_body(Node::Block(root.0), false); v.block(root, true);
    let t = v.tree;
    let mut head = None;
    for (i, e) in pf.ast.exprs.iter().enumerate() { if matches!(e.kind, ExprKind::Call(..)) { head = Some(ExprId(i as u32)); } }
    let scope = t.temporary_scope(head.unwrap()).unwrap();
    assert!(matches!(t.scopes[scope.0 as usize].node, Node::Synthetic(_, Synthetic::ForMatch)));
}
#[test]
fn region_scope_const_blocks_repeat_lengths_and_async_fn() {
    let (p, _, b) = collect("async fn probe() { let a = const { &1 }; let b = [0; 2]; async {}.await; }", 2024);
    let t = region_scope_tree(&p, &b).unwrap();
    assert_eq!(t.temporary_scope(expr(&p, &b, "1")), None);
    let count = expr(&p, &b, "2");
    assert_eq!(t.temporary_scope(count), t.scope(Node::Expr(count.0), ScopeData::Node));
    assert!(t.scope(Node::Expr(count.0), ScopeData::Arguments).is_some());
}
#[test]
fn region_scope_nonextended_temp_ends_before_await() {
    let (p, _, b) = collect("fn f() -> u32 { 1 } fn g(x: &u32) {} async fn probe() { g(&f()); async {}.await; }", 2021);
    let t = region_scope_tree(&p, &b).unwrap();
    let s = t.temporary_scope(expr(&p, &b, "f()")).unwrap();
    let await_expr = expr(&p, &b, "async {}.await");
    let await_scope = t.scope(Node::Expr(await_expr.0), ScopeData::Node).unwrap();
    let arguments = t.scope(Node::Block(b.body.unwrap().0), ScopeData::Arguments).unwrap();
    assert!(matches!(t.scopes[s.0 as usize].node, Node::Statement(_)));
    assert!(!t.is_subscope_of(await_scope, s));
    assert!(t.is_subscope_of(await_scope, arguments));
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
