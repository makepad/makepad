// A sandboxed module's const-context bodies are checked before any of them is evaluated: each
// module below is refused at load, by the sandbox's refusal or the const check, and no body of a
// refused module ran (MARK, a file each body would write, stays absent). A safe module loads and
// runs afterwards: a refused load leaves nothing behind that breaks the next one. One line each.
const CASES: &[(&str, &str)] = &[
    ("static_init", "pub static N: usize = { let _ = std::fs::write(MARK, b\"x\"); 1 }; pub fn frame() -> usize { N }"),
    ("const_init", "pub const N: usize = { let _ = std::fs::write(MARK, b\"x\"); 1 }; pub fn frame() -> usize { N }"),
    ("assoc_const_init", "pub struct S; impl S { pub const N: usize = { let _ = std::fs::write(MARK, b\"x\"); 1 }; } pub fn frame() -> usize { S::N }"),
    ("array_len", "pub struct A { pub a: [u8; { let _ = std::fs::write(MARK, b\"x\"); 1 }] } pub fn frame() -> usize { core::mem::size_of::<A>() }"),
    ("discriminant", "pub enum E { A = { let _ = std::fs::write(MARK, b\"x\"); 1 } } pub fn frame() -> usize { E::A as usize }"),
    ("const_param_default", "pub struct G<const N: usize = { let _ = std::fs::write(MARK, b\"x\"); 1 }>; impl<const N: usize> G<N> { pub fn n(&self) -> usize { N } } pub fn frame() -> usize { let g: G = G; g.n() }"),
    ("repeat_count", "pub fn frame() -> usize { let a = [0u8; { let _ = std::fs::write(MARK, b\"x\"); 1 }]; a.len() }"),
    ("inline_const", "pub fn frame() -> usize { const { let _ = std::fs::write(MARK, b\"x\"); 1 } }"),
    // fn values the refusal would refuse in a fn body, kept in items without a fn signature
    ("const_fn_value", "pub const F: fn() -> u32 = std::process::id; pub fn frame() -> usize { F() as usize }"),
    ("static_fn_value", "pub static F: fn() -> u32 = std::process::id; pub fn frame() -> usize { F() as usize }"),
    ("assoc_const_fn_value", "pub struct S; impl S { pub const F: fn() -> u32 = std::process::id; } pub fn frame() -> usize { (S::F)() as usize }"),
    ("trait_const_default", "pub trait T { const F: fn() -> u32 = std::process::id; } pub struct S; impl T for S {} pub fn frame() -> usize { (<S as T>::F)() as usize }"),
    ("trait_impl_const", "pub trait T { const F: fn() -> u32; } pub struct S; impl T for S { const F: fn() -> u32 = std::process::id; } pub fn frame() -> usize { (<S as T>::F)() as usize }"),
];

fn main() {
    let dir = std::env::temp_dir().join(format!("scb-marks-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, src) in CASES {
        let mark = dir.join(name);
        let src = src.replace("MARK", &format!("{:?}", mark.to_str().unwrap()));
        match std::relax::Module::load_source(name, &format!("{name}.rs"), &src, &[]) {
            Err(e) if e.contains("not allowed in a sandboxed module") || e.contains("not available to sandboxed modules") || e.contains("E0015") => println!("{name} refused"),
            Err(e) => println!("{name} failed otherwise: {}", e.lines().next().unwrap_or("")),
            Ok(_) => println!("{name} LOADED"),
        }
        if mark.exists() {
            println!("{name} RAN");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let safe = std::relax::Module::load_source("safe_const", "safe_const.rs", "pub const N: usize = 6 * 7; pub fn frame() -> usize { N }", &[]).unwrap();
    let frame: fn() -> usize = safe.get("frame").unwrap();
    println!("safe_const {}", frame());
}
