//! makepad inspect's tier clarity against a live program (tools/run/fixture/inspect_tiers.sh): a
//! small fn (`leaf`) inlined into a hot caller (`outer`) that tiers up to O3, and a bounds panic
//! inside another inlined fn (`pick`) once the trigger file exists.

#[inline]
pub fn leaf(x: u32) -> u32 {
    let y = x << 1;
    let z = y ^ (x >> 3);
    z ^ 7
}

#[inline]
pub fn pick(v: &[u32], i: usize) -> u32 {
    v[i]
}

#[inline(never)]
pub fn outer(n: u32, at: usize) -> u32 {
    let table = [1u32, 2, 3, 4];
    let mut s = 0u32;
    let mut i = 0u32;
    while i < n {
        s = s.wrapping_add(leaf(i));
        i += 1;
    }
    s.wrapping_add(pick(&table, at))
}

fn main() {
    let trigger = std::env::var("PROBE_TRIGGER").unwrap_or_default();
    let secs: u64 = std::env::var("PROBE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    let mut total = 0u32;
    for _ in 0..secs * 50 {
        let mut at = 1usize;
        if !trigger.is_empty() && std::path::Path::new(&trigger).exists() {
            let _ = std::fs::remove_file(&trigger);
            at = 9;
        }
        for _ in 0..200 {
            total = total.wrapping_add(outer(1000, at));
            at = 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    println!("total {}", total);
}
