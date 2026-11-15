// A `makepad run` program parses the fn bodies a later parse can do alone on their first need, a
// module's among them: the sandbox's refusal reads those bodies as it reads the file's items.
// Each module hides its escape in a body (its signatures are plain safe Rust) and is refused at
// load; a safe one loads and runs. Prints one line per module.
const ESCAPES: &[(&str, &str)] = &[
    ("unsafe_block", "pub fn frame(x: f32) -> f32 { let p = &x as *const f32; unsafe { *p } }"),
    ("raw_ptr", "pub fn frame(x: f32) -> f32 { let _p: *const f32 = &x; x }"),
    // a reference to whatever address the module names, handed to the host as a plain `&mut`
    ("forged_ref", "pub fn frame(at: usize) -> u8 { let r: &'static mut u8 = unsafe { &mut *(at as *mut u8) }; *r }"),
    ("method_body", "pub struct S(pub u8); impl S { pub fn first(&self) -> u8 { unsafe { *(self as *const S as *const u8) } } }"),
    ("inner_unsafe_fn", "pub fn frame(x: f32) -> f32 { unsafe fn inner(x: f32) -> f32 { x } let _ = inner; x }"),
    ("inner_no_mangle", "pub fn frame(x: f32) -> f32 { #[no_mangle] fn inner() {} inner(); x }"),
];

fn main() {
    for (name, src) in ESCAPES {
        match std::relax::Module::load_source(name, &format!("{name}.rs"), src, &[]) {
            Err(e) if e.contains("not allowed in a sandboxed module") => println!("{name} refused"),
            Err(e) => println!("{name} failed otherwise: {e}"),
            Ok(_) => println!("{name} LOADED"),
        }
    }
    let safe = std::relax::Module::load_source("safe_body", "safe_body.rs", "pub fn frame(x: f32) -> f32 { let y = x * 2.0; y }", &[]).unwrap();
    let frame: fn(f32) -> f32 = safe.get("frame").unwrap();
    println!("safe_body {}", frame(21.0));
}
