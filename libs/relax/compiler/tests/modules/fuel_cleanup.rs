// A module's cleanups never end the host. With --allow-module-fuel=1000000: a call whose fuel runs
// out in a callee while its frame holds a value to drop, one whose destructor spins until the fuel
// runs out while a panic unwinds, and one whose destructor panics while a panic unwinds (rustc
// aborts the process for the last two, as it did here for all three) each end with an error at
// the host's guard: the module's remaining cleanups are skipped, the host's own cleanups between
// the guard and the module still run, and the module goes on taking calls. A plain panic in the
// module still runs the module's cleanups.
struct HostDrop<'a>(&'a std::cell::Cell<u32>);
impl Drop for HostDrop<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

const SRC: &str = "pub struct Spin(pub u64);
impl Drop for Spin {
    fn drop(&mut self) {
        loop { self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1); }
    }
}
pub struct Panics;
impl Drop for Panics {
    fn drop(&mut self) { panic!(\"a destructor panics while its module unwinds\"); }
}
#[inline(never)]
pub fn spin(n: u64) -> u64 {
    let mut x = 1u64;
    let mut i = 0;
    while i < n { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); i += 1; }
    x
}
pub fn spin_holding(n: u64) -> u64 {
    let held = vec![n; 4];
    spin(n) + held.len() as u64
}
pub fn spin_in_cleanup(n: u64) -> u64 {
    let _s = Spin(n);
    if n > 0 { panic!(\"unwinds into a destructor that never ends\"); }
    n
}
pub fn panic_in_cleanup(n: u64) -> u64 {
    let _p = Panics;
    if n > 0 { panic!(\"unwinds into a destructor that panics\"); }
    n
}
pub fn add(a: u64, b: u64) -> u64 { a + b }
pub struct Counted<'a>(&'a core::cell::Cell<u64>);
impl Drop for Counted<'_> {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}
pub fn plain_panic(n: u64) -> u64 {
    let drops = core::cell::Cell::new(0);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _c = Counted(&drops);
        if n > 0 { panic!(\"unwinds through a destructor that ends\"); }
    }));
    drops.get() * 10 + caught.is_err() as u64
}
";

#[test]
fn fuel_cleanup() {
    use std::relax::Module;
    let m = Module::load_source("cleanup", "cleanup.rs", SRC, &[]).unwrap();
    let spin_holding: fn(u64) -> u64 = m.get("spin_holding").unwrap();
    let spin_in_cleanup: fn(u64) -> u64 = m.get("spin_in_cleanup").unwrap();
    let panic_in_cleanup: fn(u64) -> u64 = m.get("panic_in_cleanup").unwrap();
    let add: fn(u64, u64) -> u64 = m.get("add").unwrap();
    let plain_panic: fn(u64) -> u64 = m.get("plain_panic").unwrap();
    let host_drops = std::cell::Cell::new(0);
    let call = |f: fn(u64) -> u64, n: u64| {
        std::relax::guard(0, || {
            let _h = HostDrop(&host_drops);
            f(n)
        })
    };
    // (CASE=<name> runs one case alone: each is its own process in a negative control)
    let only = std::env::var("CASE").ok();
    let run = |name: &str| only.as_deref().is_none_or(|o| o == name);
    if run("short") {
        println!("out short ok: {}", call(spin_holding, 1000).is_ok());
    }
    if run("holding") {
        println!("out fuel stop holding values: {:?}", call(spin_holding, 100_000_000).err());
    }
    if run("spin") {
        println!("out fuel stop in a cleanup: {:?}", call(spin_in_cleanup, 1).err());
    }
    if run("panic") {
        println!("out panic in a cleanup stopped: {}", call(panic_in_cleanup, 1).is_err());
    }
    if run("plain") {
        // (the module caught its own panic: its cleanup ran once, tens)
        println!("out plain panic's cleanup ran: {:?}", call(plain_panic, 1));
    }
    println!("out host cleanups ran: {}", host_drops.get());
    println!("out module goes on: {:?}", std::relax::guard(0, || add(2, 3)));
}
