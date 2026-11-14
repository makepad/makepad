// No-codegen mode (RELAX_NO_CODEGEN=1, with RELAX_SANDBOX_NOCHECK=1 for the forged pointer): a
// module's fn values. The process writes no machine code there, so a twin taken as a value is its
// slot's precompiled thunk and an indirect call is checked against the thunks given out, where the
// other mode masks it into a table: fn items, closures behind `dyn Fn` and std's formatting (it
// passes fn values) all run; an address the module never got as a value -- here the thunk of a
// host fn -- runs the trap, not that code.
use std::sync::atomic::{AtomicU32, Ordering};

static HOST_RAN: AtomicU32 = AtomicU32::new(0);
fn host_only() {
    HOST_RAN.fetch_add(1, Ordering::SeqCst);
}

const SOURCE: &str = r#"
fn double(x: u64) -> u64 { x * 2 }
fn triple(x: u64) -> u64 { x * 3 }
pub fn pick(k: u64, x: u64) -> u64 {
    let f: fn(u64) -> u64 = if k == 0 { double } else { triple };
    f(x)
}
pub fn boxed(x: u64) -> u64 {
    let fs: Vec<Box<dyn Fn(u64) -> u64>> = vec![Box::new(move |y| y + x), Box::new(|y| y * y)];
    fs.iter().map(|f| f(x)).sum()
}
pub fn text(x: u64) -> String {
    format!("{}-{:?}", x, Some(x))
}
pub fn forged(addr: usize) -> u64 {
    let f: fn() = unsafe { core::mem::transmute(addr) };
    f();
    1
}
"#;

#[test]
fn nocodegen_fns() {
    use std::relax::{guard, Module};
    let m = Module::load_source("fns", "fns.rs", SOURCE, &[]).unwrap();
    let pick: fn(u64, u64) -> u64 = m.get("pick").unwrap();
    let boxed: fn(u64) -> u64 = m.get("boxed").unwrap();
    let text: fn(u64) -> String = m.get("text").unwrap();
    let forged: fn(usize) -> u64 = m.get("forged").unwrap();
    println!("out fn items: {} {}", pick(0, 21), pick(1, 21));
    println!("out closures behind dyn: {}", boxed(5));
    println!("out formatting: {}", text(7));
    let host: fn() = host_only;
    let r = guard(0, || forged(host as usize));
    println!("out a host fn's address traps: {} (host fn ran {} times)", r.is_err(), HOST_RAN.load(Ordering::SeqCst));
    println!("out an arbitrary address traps: {}", guard(0, || forged(0x1234_5678)).is_err());
    println!("out still callable: {}", pick(0, 4));
    m.unload();
}
