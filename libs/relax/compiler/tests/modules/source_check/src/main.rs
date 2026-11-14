// The module source check in a program (`relax run`), where a file's fn bodies are parsed on
// demand, after its items: `unsafe` and raw pointers inside a body are refused as they are in
// an item, wherever the body is -- a fn, a closure, a method, a fn nested in a fn, a macro's
// expansion. (The single-file test harness parses whole files at once and never showed this.)
// What a builtin derive writes stays out of it.
use std::relax::Module;

const CASES: &[(&str, &str)] = &[
    ("fn body", "pub fn run() -> u64 { let x = 5u64; let p = &x as *const u64; unsafe { *p } }\n"),
    ("closure", "pub fn run() -> u64 { let x = 5u64; let f = |p: &u64| unsafe { *(p as *const u64) }; f(&x) }\n"),
    ("method", "pub struct S(u64);\nimpl S { fn get(&self) -> u64 { unsafe { *(&self.0 as *const u64) } } }\npub fn run() -> u64 { S(5).get() }\n"),
    ("nested fn", "pub fn run() -> u64 {\n    fn inner(x: &u64) -> u64 { unsafe { *(x as *const u64) } }\n    inner(&5)\n}\n"),
    ("macro in a body", "macro_rules! peek { ($e:expr) => { unsafe { *($e as *const u64) } }; }\npub fn run() -> u64 { let x = 5u64; peek!(&x) }\n"),
    ("raw pointer only", "pub fn run() -> u64 { let x = 5u64; let p = &x as *const u64; p as usize as u64 & 0 }\n"),
    ("asm", "pub fn run() -> u64 { unsafe { core::arch::asm!(\"nop\") }; 5 }\n"),
    ("safe", "pub fn run() -> u64 { let x = 5u64; let f = |p: &u64| *p; f(&x) }\n"),
    // (a builtin derive reads an enum's discriminant in an `unsafe` block of its own: not the module's)
    ("derives on an enum", "#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\npub enum E { A, B(u8) }\npub fn run() -> u64 { (E::A < E::B(1)) as u64 + (E::B(2) == E::B(2)) as u64 * 4 }\n"),
    ("the module's own unsafe beside a derive", "macro_rules! Both {\n    () => {\n        #[derive(PartialEq)]\n        pub enum Both { A, B }\n        pub fn peek(x: &u64) -> u64 { unsafe { *(x as *const u64) } }\n    };\n}\nBoth!();\npub fn run() -> u64 { peek(&1) }\n"),
];

fn main() {
    let mut refused = 0;
    for (k, (name, src)) in CASES.iter().enumerate() {
        match Module::load_source(&format!("case{}", k), &format!("case{}.rs", k), src, &[]) {
            Ok(m) => {
                let run: fn() -> u64 = m.get("run").unwrap();
                println!("out {}: loaded, run {:?}", name, std::relax::guard(0, run).ok());
                m.unload();
            }
            Err(e) => {
                refused += 1;
                let what = ["`unsafe`", "raw pointers", "`asm!`"].iter().filter(|w| e.contains(**w)).copied().collect::<Vec<_>>().join(", ");
                println!("out {}: refused ({})", name, what);
            }
        }
    }
    println!("out refused: {} of {}", refused, CASES.len());
}
