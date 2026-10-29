// rustc_parse parser/stmt.rs: parse_stmt_nonterminal calls
// parse_stmt_without_recovery(false, ..): trailing expression/let semicolons
// are separate Empty statements; item semicolons belong to the item.
macro_rules! tap_inner {
    ($self_:ident, mut $inner:ident => { $($stmt:stmt)* }) => {
        #[allow(redundant_semicolons)]
        { let mut $inner = $self_; $($stmt)*; $inner }
    };
}

struct R { n: u32 }
impl R { fn bump(&mut self) { self.n += 1; } }
fn go(r: R) -> R {
    tap_inner!(r, mut this => { this.bump(); this.bump(); })
}

macro_rules! count {
    ($($stmt:stmt)*) => { 0 $(+ { let _ = stringify!($stmt); 1 })* };
}

macro_rules! separated {
    ($a:stmt; $b:stmt;) => {{ $a; $b; }};
}

#[test]
fn repeated_statements_keep_expression_semicolons() {
    assert_eq!(go(R { n: 1 }).n, 3);
    assert_eq!(count!(), 0);
    assert_eq!(count!(;), 1);
    assert_eq!(count!(;;;), 3);
    assert_eq!(count!(x.f(); y.g();), 4);
    assert_eq!(count!(let x = 1;), 2);
    assert_eq!(count!(struct Item;), 1);
    assert_eq!(count!(fn item() {}), 1);
    assert_eq!(count!(let x = 1), 1);
    let mut r = R { n: 1 };
    separated!(r.bump(); r.bump(););
    assert_eq!(r.n, 3);
}
