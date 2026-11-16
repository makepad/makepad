// diff_fast corpus program: macro fragments forwarded to another macro. A forwarded `$p:path` (and
// `$e:expr`, `$l:literal`, `$t:ty`) is one opaque fragment: it matches no literal token of the next
// matcher, not even its own first token, while `ident` and `tt` fragments forward as their tokens.
// A forwarded path also stays usable as an expression inside a built-in macro (assert_eq!, format!).
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

#[derive(Debug, PartialEq, Clone, Copy)]
enum Mode {
    Idle,
    Busy,
}

macro_rules! check_mode {
    ($want:path) => {{
        assert_eq!(Mode::Idle, $want);
        assert_ne!(Mode::Busy, $want);
        format!("{:?} {}", $want, $want == Mode::Idle)
    }};
}

// the written arm wins only for written tokens
macro_rules! which_path {
    (Mode::Idle) => { "written" };
    ($other:path) => { "fragment" };
}
macro_rules! via_path { ($p:path) => { which_path!($p) }; }
macro_rules! via_tt { ($($t:tt)*) => { which_path!($($t)*) }; }
macro_rules! path_then_tt { ($p:path) => { via_tt!($p) }; }

macro_rules! which_lit {
    (1) => { "written" };
    ($e:expr) => { "fragment" };
}
macro_rules! via_expr { ($e:expr) => { which_lit!($e) }; }
macro_rules! via_literal { ($l:literal) => { which_lit!($l) }; }

macro_rules! which_ty {
    (u8) => { "written" };
    ($t:ty) => { "fragment" };
}
macro_rules! via_ty { ($t:ty) => { which_ty!($t) }; }

macro_rules! which_ident {
    (Idle) => { "written" };
    ($i:ident) => { "other" };
}
macro_rules! via_ident { ($i:ident) => { which_ident!($i) }; }

fn work() {
    d!("assert_eq through a path fragment: {}", check_mode!(Mode::Idle));
    d!("path: direct {} forwarded {} as tt {} path then tt {}",
        which_path!(Mode::Idle), via_path!(Mode::Idle), via_tt!(Mode::Idle), path_then_tt!(Mode::Idle));
    d!("path busy: direct {} forwarded {}", which_path!(Mode::Busy), via_path!(Mode::Busy));
    d!("literal: direct {} expr {} literal {} sum {}", which_lit!(1), via_expr!(1), via_literal!(1), via_expr!(1 + 0));
    d!("ty: direct {} forwarded {}", which_ty!(u8), via_ty!(u8));
    d!("ident: direct {} forwarded {} other {}", which_ident!(Idle), via_ident!(Idle), via_ident!(Busy));
}
