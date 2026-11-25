// The reference side of the web layout corpus (run.sh): rustc builds this for the target
// (a host binary: the wasm64 reference) and it prints one line
// per row: `<type>|<size>|<align>|<field>=<offset>,...`.
#![allow(dead_code, static_mut_refs)]
include!("corpus.rs");

thread_local! {
    static OUT: RefCell<String> = RefCell::new(String::new());
}

fn line(s: String) {
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        o.push_str(&s);
        o.push('\n');
    });
}

macro_rules! sz {
    ($($t:ty),* $(,)?) => {
        $( line(format!("{}|{}|{}|", stringify!($t), std::mem::size_of::<$t>(), std::mem::align_of::<$t>())); )*
    };
}

macro_rules! offs {
    ($t:ty: $($f:tt),*) => {
        line(format!("{}|{}|{}|{}", stringify!($t), std::mem::size_of::<$t>(), std::mem::align_of::<$t>(),
            [$(format!("{}={}", stringify!($f), std::mem::offset_of!($t, $f))),*].join(",")));
    };
}

fn table() -> String {
    run();
    OUT.with(|o| o.borrow().clone())
}

fn run() {
    OUT.with(|o| o.borrow_mut().clear());
    web_layout_rows_fn();
}

fn web_layout_rows_fn() {
    web_layout_rows!();
}

fn main() {
    print!("{}", table());
}
