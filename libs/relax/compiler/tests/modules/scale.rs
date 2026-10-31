// Run-time modules at scale (hotreload.md §11): 300 modules loaded at once (each its own crate,
// generated from one template with a different constant), every one called per "frame" through
// its fn pointer for 100 frames, the per-call cost measured, then a third unloaded and the rest
// still called. Run: `makepad test tests/modules/scale.rs` (writes its modules to a temp dir).
pub struct FrameIn {
    pub t: f32,
}

#[test]
fn scale() {
    use std::relax::Module;
    let dir = std::env::temp_dir().join(format!("relax-mod-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = 300;
    let mut mods = Vec::new();
    let mut fns: Vec<fn(&FrameIn) -> f32> = Vec::new();
    let t0 = std::time::Instant::now();
    for k in 0..n {
        let p = dir.join(format!("m{}.rs", k));
        let src = format!("use test_crate::FrameIn;\n\nconst K: f32 = {}.0;\n\nfn shape(x: f32) -> f32 {{\n    (x * K).sin() * 0.5 + 0.5\n}}\n\npub fn frame(i: &FrameIn) -> f32 {{\n    shape(i.t) + K\n}}\n", k);
        std::fs::write(&p, src).unwrap();
        let m = Module::load(&format!("m{}", k), p.to_str().unwrap(), &["test_crate"]).unwrap();
        fns.push(m.get("frame").unwrap());
        mods.push(m);
    }
    let load_ms = t0.elapsed().as_secs_f64() * 1e3;
    let mut sum = 0.0f64;
    let t1 = std::time::Instant::now();
    for f in 0..100 {
        let i = FrameIn { t: f as f32 * 0.01 };
        for g in &fns {
            sum += g(&i) as f64;
        }
    }
    let calls = 100 * n;
    let ns = t1.elapsed().as_nanos() as f64 / calls as f64;
    eprintln!("scale: {} modules loaded in {:.0} ms ({:.2} ms each); {:.1} ns per call", n, load_ms, load_ms / n as f64, ns);
    // the expected sum: sum over k of (100 frames of (shape + k))
    let mut want = 0.0f64;
    for f in 0..100 {
        let t = f as f32 * 0.01;
        for k in 0..n {
            want += (((t * k as f32).sin() * 0.5 + 0.5) + k as f32) as f64;
        }
    }
    println!("out sum ok {}", (sum - want).abs() < 1e-3 * want.abs());
    // unload a third, the rest keep working
    let mut kept = Vec::new();
    for (k, m) in mods.into_iter().enumerate() {
        if k % 3 == 0 {
            m.unload();
        } else {
            kept.push(k);
        }
    }
    let i = FrameIn { t: 0.5 };
    let mut s2 = 0.0f64;
    let mut w2 = 0.0f64;
    for &k in &kept {
        s2 += fns[k](&i) as f64;
        w2 += (((0.5 * k as f32).sin() * 0.5 + 0.5) + k as f32) as f64;
    }
    println!("out after unload ok {}", (s2 - w2).abs() < 1e-3 * w2.abs());
    let _ = std::fs::remove_dir_all(&dir);
}
