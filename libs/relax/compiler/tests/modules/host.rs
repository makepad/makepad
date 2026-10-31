// Run-time modules (hotreload.md §11, modules.rs, std::relax): a running program loads source
// files it has never seen, calls their exports through checked fn pointers, edits them live and
// unloads them; errors and panics in modules never take the program down.
// Run: `makepad test tests/modules/host.rs` (it finds its modules next to itself).

pub struct FxIn {
    pub t: f32,
    pub kick: f32,
}

/// Part of the host's API the modules call.
pub fn host_scale(x: f32) -> f32 {
    x * 10.0
}

fn dir() -> String {
    let f = file!();
    match f.rfind('/') {
        Some(i) => f[..i].to_string(),
        None => ".".to_string(),
    }
}

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{}/{}", dir(), name)).unwrap()
}

#[test]
fn modules() {
    use std::relax::{Edit, Module};
    let inp = FxIn { t: 1.5, kick: 0.25 };
    // 1. load + call
    let t0 = std::time::Instant::now();
    let m = Module::load("fx_a", &format!("{}/fx_a.rs", dir()), &["test_crate"]).unwrap();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    eprintln!("cold load of fx_a: {:.2} ms", ms);
    let frame: fn(&FxIn) -> f32 = m.get("frame").unwrap();
    println!("out a {}", frame(&inp));
    // 2. a wrongly typed export is refused
    match m.get::<fn(f32) -> f32>("frame") {
        Ok(_) => println!("out wrong type accepted"),
        Err(e) => println!("out wrong type refused: {}", e.contains("is `fn(")),
    }
    match m.get::<fn(&FxIn) -> f32>("nope") {
        Ok(_) => println!("out missing accepted"),
        Err(e) => println!("out missing refused: {}", e.contains("no export")),
    }
    // 3. a body edit applies in place (the fn pointer taken before runs the new code)
    match m.replace(&read("fx_a_v1.rs")) {
        Ok(e) => println!("out edit {:?} -> {}", e, frame(&inp)),
        Err(e) => println!("out edit refused: {}", e),
    }
    // 4. an edit with a type error is refused, the module keeps running
    match m.replace(&read("fx_a_bad.rs")) {
        Ok(e) => println!("out bad edit applied {:?}", e),
        Err(e) => println!("out bad edit refused at line: {} -> {}", e.contains("fx_a.rs:"), frame(&inp)),
    }
    // 5. a module with a type error does not load
    match Module::load("fx_bad", &format!("{}/fx_bad.rs", dir()), &["test_crate"]) {
        Ok(_) => println!("out bad module loaded"),
        Err(e) => println!("out bad module refused: {}", e.contains("fx_bad.rs:")),
    }
    // 6. a panic in a module is caught by the caller
    let p = Module::load("fx_panic", &format!("{}/fx_panic.rs", dir()), &["test_crate"]).unwrap();
    let pf: fn(&FxIn) -> f32 = p.get("frame").unwrap();
    let r = std::panic::catch_unwind(|| pf(&inp));
    println!("out panic caught: {}", r.is_err());
    // 7. two modules at once; unload one
    let b = Module::load("fx_b", &format!("{}/fx_b.rs", dir()), &["test_crate"]).unwrap();
    let fb: fn(&FxIn) -> f32 = b.get("frame").unwrap();
    println!("out a {} b {}", frame(&inp), fb(&inp));
    b.unload();
    match b.get::<fn(&FxIn) -> f32>("frame") {
        Ok(_) => println!("out unloaded handle works"),
        Err(e) => println!("out unloaded handle refused: {}", e.contains("unloaded")),
    }
    // 8. a layout edit asks for a fresh load
    match m.replace(&read("fx_a_layout.rs")) {
        Ok(e) => println!("out layout edit {:?}", e),
        Err(e) => println!("out layout edit refused: {}", e),
    }
    // 9. guarded calls: a panic, a call that never returns (deadline) and unbounded recursion
    //    each end the call with an error; the program keeps running
    let g = std::relax::guard(0, || pf(&inp));
    println!("out guard panic: {}", g.is_err());
    let h = Module::load("fx_hang", &format!("{}/fx_hang.rs", dir()), &["test_crate"]).unwrap();
    let hf: fn(&FxIn) -> f32 = h.get("frame").unwrap();
    let g = std::relax::guard(200, || hf(&inp));
    println!("out guard hang: {}", g.is_err());
    let d = Module::load("fx_deep", &format!("{}/fx_deep.rs", dir()), &["test_crate"]).unwrap();
    let df: fn(&FxIn) -> f32 = d.get("frame").unwrap();
    let g = std::thread::Builder::new().stack_size(1 << 20).spawn(move || std::relax::guard(0, || df(&FxIn { t: 1.0, kick: 0.0 })).is_err()).unwrap().join();
    println!("out guard overflow: {:?}", g);
    println!("out still running {}", frame(&inp));
    // 10. load from text (embedded presets, AI edits): errors name the given path; modules can
    //     use each other (a kit module and an effect on top of it)
    let kit = Module::load_source("kit_k", "presets/kit_k.rs", "pub fn boost(x: f32) -> f32 { x * 3.0 }\n", &[]).unwrap();
    let fx = Module::load_source("fx_k", "presets/fx_k.rs", "use test_crate::FxIn;\npub fn frame(i: &FxIn) -> f32 { kit_k::boost(i.t) }\n", &["test_crate", "kit_k"]).unwrap();
    let kf: fn(&FxIn) -> f32 = fx.get("frame").unwrap();
    println!("out text module {}", kf(&inp));
    match Module::load_source("fx_kbad", "presets/fx_kbad.rs", "pub fn frame(x: f32) -> f32 {\n    x + \"a\"\n}\n", &[]) {
        Ok(_) => println!("out bad text loaded"),
        Err(e) => println!("out bad text refused at its path: {}", e.contains("presets/fx_kbad.rs:2:")),
    }
    let _ = kit;
    match Module::load_source("fx_rr", "presets/fx_rr.rx", "pub fn frame(x: f32) -> f32 {\n    let y = x * 2.0;\n    y + 1.0\n}\n", &[]) {
        Ok(m) => {
            let f: fn(f32) -> f32 = m.get("frame").unwrap();
            println!("out rr module {}", f(2.0));
        }
        Err(e) => println!("out rr module refused: {}", e),
    }
    // 11. escape attempts: every file in escape/ is refused at load with a sandbox error
    let mut names: Vec<String> = std::fs::read_dir(format!("{}/escape", dir())).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut refused = 0;
    for n in &names {
        match Module::load(&format!("esc_{}", n.trim_end_matches(".rs")), &format!("{}/escape/{}", dir(), n), &[]) {
            Ok(_) => println!("out ESCAPE LOADED: {}", n),
            Err(e) => {
                if e.contains("sandbox") {
                    refused += 1;
                } else {
                    println!("out escape {} refused without a sandbox reason: {}", n, e);
                }
            }
        }
    }
    println!("out escapes refused: {} of {}", refused, names.len());
    // 12. the orphan rule: an impl of another crate's trait for another crate's type is refused
    match Module::load("fx_orphan", &format!("{}/fx_orphan.rs", dir()), &[]) {
        Ok(_) => println!("out ORPHAN IMPL LOADED"),
        Err(e) => println!("out orphan impl refused: {}", e.contains("orphan rule")),
    }
    let _ = Edit::Patched;
}
