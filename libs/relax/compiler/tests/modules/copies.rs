// A twin's O3 build turns the arena memcpy / memmove of a known small size into inline copies
// (sandbox.rs inline_copies). The sorts of copies_kernel.rs move elements of 1 to 72 bytes (the
// sizes with a 4/2/1-byte tail, the largest inline size, and one past it); as a sandboxed
// module, before and after the tier-up, they give what the same source gives as host code.
extern crate self as test_crate;
#[path = "copies_kernel.rs"]
mod copies_kernel;
use std::relax::Module;

#[test]
fn copies() {
    let path = std::path::Path::new(file!()).parent().unwrap().join("copies_kernel.rs");
    let src = std::fs::read_to_string(&path).unwrap();
    let m = Module::load_source("copies", "copies.rs", &src, &[]).unwrap();
    let f: fn(u32) -> u64 = m.get("copies").unwrap();
    let want = (copies_kernel::copies(40), copies_kernel::copies(700));
    let cold = (f(40), f(700));
    // warm the sorts' helpers, then leave the background compiler time to install
    for _ in 0..120 { std::hint::black_box(f(120)); }
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let hot = (f(40), f(700));
    println!("out as host code: {:016x} {:016x}", want.0, want.1);
    println!("out the module, before the tier-up: {}", cold == want);
    println!("out the module, after it: {}", hot == want);
    m.unload();
}
