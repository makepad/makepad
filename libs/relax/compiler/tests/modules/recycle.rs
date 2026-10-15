// Sandbox code recycling (hotreload.md §12.5): 400 load/edit/unload cycles of AI-written-style
// effects; dead module code is rebuilt away (recycles), live modules keep working across
// recycles, and code memory stays bounded. Prints code/image/RSS numbers.
fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok())).unwrap_or(0)
}

fn src(k: u32, scale: u32) -> String {
    format!("pub fn frame(t: f32) -> f32 {{\n    let mut v: Vec<f32> = Vec::new();\n    for i in 0..{n} {{\n        v.push((t * i as f32 * {s}.0).sin());\n    }}\n    let mut acc = 0.0;\n    for x in v.iter() {{\n        acc += *x * {s}.0;\n    }}\n    acc + {k}.0\n}}\n", n = 32 + k % 7, s = scale, k = k)
}

#[test]
fn recycle() {
    use std::relax::Module;
    // a long-lived module, used across every recycle
    let keep = Module::load_source("keep", "keep.rs", &src(1000, 1), &[]).unwrap();
    let kf: fn(f32) -> f32 = keep.get("frame").unwrap();
    let want = kf(0.5);
    for _ in 0..10000 { assert_eq!(kf(0.5), want); }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let r0 = rss_kb();
    let s0 = std::relax::sandbox_stats(false).unwrap();
    let mut ok = true;
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
    // force one more so the numbers below are after a recycle
    let _ = std::relax::sandbox_stats(true);
    // Recycling replaces O3 code with freshly compiled confined code and must
    // rearm counters against the new space, never a stale count-stub target.
    for _ in 0..10000 { assert_eq!(kf(0.5), want); }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let s1 = std::relax::sandbox_stats(false).unwrap();
    ok &= kf(0.5) == want;
    eprintln!("recycle: before {:?} after {:?}; rss {} -> {} MB", s0, s1, r0 / 1024, rss_kb() / 1024);
    println!("out edits applied and keep stable: {}", ok);
    println!("out recycled: {}", s1[0] > s0[0]);
    println!("out code bounded: {}", s1[1] < 32 << 20);
}
