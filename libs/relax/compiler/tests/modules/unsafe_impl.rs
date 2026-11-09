// A sandboxed module may not write an `unsafe impl`. The one the builtin derive(Clone)
// makes for a Copy type (`unsafe impl TrivialClone`) is not the module's own and loads.
use std::relax::Module;

#[test]
fn unsafe_impls() {
    let derived = Module::load_source("derived", "derived_module.rs", r#"
#[derive(Clone, Copy)]
pub struct P { pub x: u32 }
pub fn make() -> u32 { let p = P { x: 7 }; let q = p.clone(); q.x }
"#, &["test_crate"]);
    println!("out derive Clone, Copy loads: {}", derived.as_ref().is_ok_and(|m| m.get::<fn() -> u32>("make").is_ok_and(|f| f() == 7)));
    let refused = |name: &str, src: &str| -> bool {
        match Module::load_source(name, &format!("{name}.rs"), src, &["test_crate"]) {
            Err(e) => e.contains("`unsafe impl` is not allowed in a sandboxed module"),
            Ok(_) => false,
        }
    };
    println!("out hand-written unsafe impl TrivialClone refused: {}", refused("hand_clone",
        "pub struct Q; impl Clone for Q { fn clone(&self) -> Q { Q } } unsafe impl core::clone::TrivialClone for Q {}"));
    println!("out unsafe impl Send refused: {}", refused("hand_send", "pub struct R; unsafe impl Send for R {}"));
}
