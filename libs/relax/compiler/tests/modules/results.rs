// Plain-data return values cross the sandbox by value, including memory (sret) results.
#[derive(Clone, Copy)]
pub struct V4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[derive(Clone, Copy)]
pub enum Shade {
    Empty,
    Color(V4),
}

#[derive(Clone, Copy)]
pub struct RefValue {
    pub value: &'static u32,
}

pub struct BoxValue {
    pub value: Box<u32>,
}

#[derive(Clone, Copy)]
pub struct PtrValue {
    pub value: *const u32,
}

pub struct NotCopy {
    pub value: u32,
}

#[test]
fn results() {
    use std::relax::Module;
    let m = Module::load_source("results", "results_module.rs", r#"
use test_crate::{V4, Shade, RefValue, BoxValue, PtrValue, NotCopy};
pub fn color(x: f32) -> [f32; 4] { [x, x + 1.0, x + 2.0, x + 3.0] }
pub fn pair(x: f32) -> (f32, u32) { (x, 42) }
pub fn nested(x: u8) -> [[u8; 3]; 2] { [[x, 2, 3], [4, 5, 6]] }
pub fn v4(x: f32) -> V4 { V4 { x, y: 2.0, z: 3.0, w: 4.0 } }
pub fn shade(x: f32) -> Shade { if x == 0.0 { Shade::Empty } else { Shade::Color(v4(x)) } }
pub fn mixed(x: f32) -> ([f32; 4], (u32, V4)) { (color(x), (42, v4(x))) }
pub fn large(x: u64) -> [u64; 32] { [x; 32] }
pub fn empty() -> [u8; 0] { [] }
pub fn change(x: &mut u32) -> (u32, u32) { let old = *x; *x += 1; (old, *x) }
pub fn fail() -> [f32; 4] { panic!("result panic") }
pub fn reference() -> RefValue { RefValue { value: &42 } }
pub fn boxed() -> BoxValue { BoxValue { value: Box::new(42) } }
pub fn pointer() -> PtrValue { panic!("not called") }
pub fn not_copy() -> NotCopy { NotCopy { value: 42 } }
#[derive(Clone, Copy)]
pub struct Local { pub x: u32 }
pub fn local() -> Local { Local { x: 42 } }
pub fn local_nested() -> [Local; 1] { [local()] }
"#, &["test_crate"]).unwrap();
    let color: fn(f32) -> [f32; 4] = m.get("color").unwrap();
    let pair: fn(f32) -> (f32, u32) = m.get("pair").unwrap();
    let nested: fn(u8) -> [[u8; 3]; 2] = m.get("nested").unwrap();
    let v4: fn(f32) -> V4 = m.get("v4").unwrap();
    let shade: fn(f32) -> Shade = m.get("shade").unwrap();
    let mixed: fn(f32) -> ([f32; 4], (u32, V4)) = m.get("mixed").unwrap();
    let large: fn(u64) -> [u64; 32] = m.get("large").unwrap();
    let empty: fn() -> [u8; 0] = m.get("empty").unwrap();
    let change: fn(&mut u32) -> (u32, u32) = m.get("change").unwrap();
    let fail: fn() -> [f32; 4] = m.get("fail").unwrap();
    println!("out array: {}", color(0.5) == [0.5, 1.5, 2.5, 3.5]);
    println!("out tuple: {}", pair(1.5) == (1.5, 42));
    println!("out nested array: {}", nested(1) == [[1, 2, 3], [4, 5, 6]]);
    let v = v4(1.0);
    println!("out host struct: {}", v.x == 1.0 && v.y == 2.0 && v.z == 3.0 && v.w == 4.0);
    let e = match shade(1.0) { Shade::Color(v) => v.x == 1.0 && v.w == 4.0, _ => false };
    println!("out host enum: {}", e && matches!(shade(0.0), Shade::Empty));
    let mixed = mixed(1.0);
    println!("out mixed: {}", mixed.0 == [1.0, 2.0, 3.0, 4.0] && mixed.1.0 == 42 && mixed.1.1.w == 4.0);
    println!("out large: {}", large(99) == [99; 32]);
    println!("out empty: {}", empty().len() == 0);
    let mut x = 9;
    println!("out mutable argument: {}", change(&mut x) == (9, 10) && x == 10);
    println!("out panic contained: {}", std::relax::guard(1000, || fail()).is_err());
    println!("out after panic: {}", color(2.0) == [2.0, 3.0, 4.0, 5.0]);
    for name in ["reference", "boxed", "pointer", "not_copy", "local", "local_nested"] {
        let err = m.get::<fn() -> u32>(name).err().unwrap();
        println!("out refused {}: {}", name, err.contains("cannot leave a sandboxed module") && err.contains("Copy host structs/enums"));
    }
    // A dependency can be another sandboxed module; its Copy types are not host types.
    let dependent = Module::load_source("dependent", "dependent.rs", "pub fn other() -> results::Local { results::local() }", &["results"]).unwrap();
    println!("out module dependency refused: {}", dependent.get::<fn() -> u32>("other").err().unwrap().contains("cannot leave a sandboxed module"));
    dependent.unload();
    // Exports discovered after an initially empty stream use the same arena result path.
    let streamed = Module::load_stream("result_stream", "result_stream.rs", "", &[]).unwrap();
    streamed.finish_stream("pub fn color() -> [f32; 4] { [1.0; 4] }").unwrap();
    let sf: fn() -> [f32; 4] = streamed.get("color").unwrap();
    let first = sf();
    streamed.finish_stream("pub fn color() -> [f32; 4] { [2.0; 4] }").unwrap();
    println!("out streamed result: {}", first == [1.0; 4] && sf() == [2.0; 4]);
    streamed.unload();
    // More total return bytes than the shadow stack: every call must reclaim its result slot.
    let t = std::time::Instant::now();
    let mut sum = 0.0;
    for k in 0..600000 { sum += color((k % 2) as f32)[3]; }
    eprintln!("results cost: {:.1} ns per array call", t.elapsed().as_nanos() as f64 / 600000.0);
    println!("out repeated: {}", sum == 2100000.0);
    // The host owns the bytes after unload, including an enum payload and a large sret.
    let saved = large(123);
    m.unload();
    println!("out survives unload: {}", saved == [123; 32] && v.w == 4.0);
}
