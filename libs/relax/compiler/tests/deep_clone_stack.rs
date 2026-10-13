//! Derived Clone builds a memory result in place (COMMERCIAL BLOCKER, Stage VJ effects: a deep
//! Expr tree clone overflowed the 8 MB stack). 20,000 levels on an 8 MB thread pass under rustc
//! (rustc's limit is between 30,000 and 45,000); Relax overflowed at 10,000 before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span { pub start: usize, pub end: usize }
#[derive(Clone, Copy, Debug)]
pub enum BinOp { Add, Mul }
#[derive(Clone, Debug)]
pub enum Stmt { Expr(Expr), Ret(Option<Expr>) }
#[derive(Clone, Debug)]
pub struct FnDecl { pub name: String, pub params: Vec<(String, Option<u8>)>, pub body: Vec<Stmt>, pub span: Span }
#[derive(Clone, Debug)]
pub enum ExprKind {
    Num(f64, bool), Ident(String), Field(Box<Expr>, String), Call(String, Vec<Expr>), Bin(BinOp, Box<Expr>, Box<Expr>),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>), Match(Box<Expr>, Vec<(Option<Vec<Expr>>, Vec<Stmt>)>), Lambda(Box<FnDecl>),
}
#[derive(Clone, Debug)]
pub struct Expr { pub kind: ExprKind, pub span: Span }
#[derive(Clone, Debug)]
pub enum Item { Let { name: String, ann: Option<u8>, value: Expr, span: Span }, Fn(FnDecl) }
fn num(x: f64) -> Expr { Expr { kind: ExprKind::Num(x, false), span: Span { start: 0, end: 1 } } }
#[test]
fn cl2() {
    let e = Expr { kind: ExprKind::Bin(BinOp::Add, Box::new(num(1.0)), Box::new(Expr { kind: ExprKind::Field(Box::new(num(2.0)), "x".into()), span: Span { start: 1, end: 2 } })), span: Span { start: 0, end: 3 } };
    let f = FnDecl { name: "f".into(), params: vec![("a".into(), Some(1))], body: vec![Stmt::Ret(Some(e.clone())), Stmt::Expr(Expr { kind: ExprKind::If(vec![(num(1.0), vec![])], Some(vec![])), span: Span { start: 0, end: 0 } })], span: Span { start: 0, end: 9 } };
    let items = vec![Item::Let { name: "a".into(), ann: None, value: e, span: Span { start: 0, end: 1 } }, Item::Fn(f.clone()), Item::Let { name: "l".into(), ann: Some(2), value: Expr { kind: ExprKind::Lambda(Box::new(f)), span: Span { start: 5, end: 6 } }, span: Span { start: 0, end: 1 } }];
    let mut out = Vec::new();
    for it in &items { out.push(it.clone()); }
    println!("| {:?}", out);
}
#[test]
fn cl3_deep() {
    let depth: usize = std::env::var("DEPTH").ok().and_then(|v| v.parse().ok()).unwrap_or(20000);
    let mut e = num(0.0);
    for i in 0..depth { e = Expr { kind: ExprKind::Bin(BinOp::Add, Box::new(e), Box::new(num(i as f64))), span: Span { start: i, end: i } }; }
    let c = std::thread::Builder::new().stack_size(8 << 20).spawn(move || { let c = e.clone(); std::mem::forget(e); format!("{:?}", c.span) }).unwrap().join();
    println!("| deep {:?}", c.is_ok());
}
