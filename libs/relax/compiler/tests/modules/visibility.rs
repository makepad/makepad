use std::relax::{Edit, Module};

#[inline(never)]
pub fn host_collision() -> i32 {
    let color = 1;
    color
}

#[test]
fn visibility() {
    assert_eq!(host_collision(), 1);
    let base = "pub fn own_collision() -> i32 { let color = 2; color }";
    let direct = "pub fn direct_collision() -> i32 { let color = 3; color }";
    let transitive = "pub fn transitive_collision() -> i32 { let color = 4; color }";
    let sibling = "pub fn sibling_collision() -> i32 { let color = 5; color }";
    let a = Module::load_source("visible_a", "memory/visible_a.rs", base, &["test_crate"]).unwrap();
    let b = Module::load_source("visible_b", "memory/visible_b.rs", direct, &["visible_a"]).unwrap();
    let c = Module::load_source("visible_c", "memory/visible_c.rs", transitive, &["visible_b"]).unwrap();
    let s = Module::load_source("visible_s", "memory/visible_s.rs", sibling, &["test_crate"]).unwrap();
    let af = a.get::<fn() -> i32>("own_collision").unwrap();
    let bf = b.get::<fn() -> i32>("direct_collision").unwrap();
    let cf = c.get::<fn() -> i32>("transitive_collision").unwrap();
    let sf = s.get::<fn() -> i32>("sibling_collision").unwrap();
    assert_eq!((af(), bf(), cf(), sf()), (2, 3, 4, 5));

    // The host dependency and unrelated sibling mention the same added name;
    // only this crate and its direct/transitive dependents may be checked.
    eprintln!("visibility: base begin");
    assert_eq!(a.finish_stream(&format!("{} fn color() -> i32 {{ 9 }}", base)).unwrap(), Edit::Patched);
    eprintln!("visibility: base end");
    assert_eq!((af(), bf(), cf(), sf()), (2, 3, 4, 5));

    // A leaf edit must not check any body in its dependencies or sibling crates.
    eprintln!("visibility: leaf begin");
    assert_eq!(c.finish_stream(&format!("{} fn color() -> i32 {{ 9 }}", transitive)).unwrap(), Edit::Patched);
    eprintln!("visibility: leaf end");
    assert_eq!((af(), bf(), cf(), sf()), (2, 3, 4, 5));
    assert_eq!(host_collision(), 1);
    c.unload(); b.unload(); a.unload(); s.unload();
    println!("out crate visibility: true");
}
