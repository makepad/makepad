// Append-only module code (MEMRECLAIM): JIT code is never freed or reused, twins' included. 400
// load/edit/unload cycles of effect modules keep a long-lived module stable, and the twin code
// growth per hot patch and per loaded module is printed (sandbox_stats()[0]: twin bytes installed).
fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok())).unwrap_or(0)
}

fn src(k: u32, scale: u32) -> String {
    format!("pub fn frame(t: f32) -> f32 {{\n    let mut v: Vec<f32> = Vec::new();\n    for i in 0..{n} {{\n        v.push((t * i as f32 * {s}.0).sin());\n    }}\n    let mut acc = 0.0;\n    for x in v.iter() {{\n        acc += *x * {s}.0;\n    }}\n    acc + {k}.0\n}}\n", n = 32 + k % 7, s = scale, k = k)
}

#[test]
fn growth() {
    use std::relax::Module;
    let keep = Module::load_source("keep", "keep.rs", &src(1000, 1), &[]).unwrap();
    let kf: fn(f32) -> f32 = keep.get("frame").unwrap();
    let want = kf(0.5);
    for _ in 0..10000 { assert_eq!(kf(0.5), want); }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let r0 = rss_kb();
    // hot patches of one live module
    let m = Module::load_source("patched", "patched.rs", &src(1, 2), &[]).unwrap();
    let f: fn(f32) -> f32 = m.get("frame").unwrap();
    let _ = f(0.25);
    let p0 = std::relax::sandbox_stats()[0];
    let mut ok = true;
    for k in 0..200u32 {
        m.replace(&src(1, 3 + k)).unwrap();
        ok &= f(0.25).is_finite();
    }
    let per_patch = (std::relax::sandbox_stats()[0] - p0) / 200;
    // load / call / edit / unload cycles
    let l0 = std::relax::sandbox_stats()[0];
    for k in 0..400u32 {
        let m = Module::load_source(&format!("fx{}", k), &format!("fx{}.rs", k), &src(k, 2), &[]).unwrap();
        let f: fn(f32) -> f32 = m.get("frame").unwrap();
        let a = f(0.25);
        m.replace(&src(k, 3)).unwrap();
        let b = f(0.25);
        ok &= a != b;
        m.unload();
        ok &= kf(0.5) == want;
    }
    let per_cycle = (std::relax::sandbox_stats()[0] - l0) / 400;
    for _ in 0..10000 { ok &= kf(0.5) == want; }
    eprintln!("growth: {} twin code bytes per hot patch, {} per load+patch+unload cycle; twin code {} KB; rss {} -> {} MB", per_patch, per_cycle, std::relax::sandbox_stats()[0] / 1024, r0 / 1024, rss_kb() / 1024);
    println!("out edits applied and keep stable: {}", ok);
    println!("out code grows: {}", per_patch > 0 && per_cycle > 0);
    m.unload();
}
