//! A few blocks that outlive their scratch scope pin the burst chunks they were carved in; a trim
//! gives the free granules around them back, and the arena keeps working afterwards.
use relax_base::{memstat, zone_alloc};

#[global_allocator]
static ALLOC: memstat::Counting = memstat::Counting;

fn rss_of_burst() -> usize {
    // Linux: the burst arena's resident bytes from smaps (its mapping is named)
    let Ok(text) = std::fs::read_to_string("/proc/self/smaps") else { return 0 };
    let mut on = false;
    let mut kb = 0usize;
    for line in text.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.contains('-') && first.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            on = line.contains("makepad-run-burst");
        } else if on {
            if let Some(v) = line.strip_prefix("Rss:") {
                kb += v.split_whitespace().next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0);
            }
        }
    }
    kb << 10
}

#[test]
fn survivors_keep_their_bytes_and_the_rest_goes_back() {
    let runtime = relax_base::options::RuntimeOptions::from_env();
    if !runtime.burst_arena || !cfg!(any(target_os = "macos", target_os = "linux", windows)) {
        return;
    }
    let before = zone_alloc::burst_stats();
    let mut kept: Vec<Box<[u64; 8]>> = Vec::new();
    for round in 0..3u64 {
        let survivors = {
            let _phase = zone_alloc::compiler();
            let _scope = zone_alloc::job("test", runtime.burst_arena);
            let mut all: Vec<Box<[u64; 8]>> = Vec::with_capacity(1);
            for i in 0..200_000u64 {
                all.push(Box::new([i ^ round; 8]));
            }
            // one in 2000 outlives the scope (moved out); the rest is freed in it
            let mut s = Vec::new();
            for (i, b) in all.into_iter().enumerate() {
                if i % 2000 == 0 {
                    s.push(b);
                }
            }
            zone_alloc::copy_out(|| s.into_iter().collect::<Vec<_>>())
        };
        let r0 = rss_of_burst();
        zone_alloc::burst_trim();
        let r1 = rss_of_burst();
        if cfg!(target_os = "linux") {
            assert!(r1 + (4 << 20) < r0, "round {round}: burst rss {r0} -> {r1}");
        }
        kept.extend(survivors);
        // the survivors' bytes are intact, and freeing them later works
        for (k, b) in kept.iter().enumerate() {
            let round_of = k as u64 / 100;
            let i = (k as u64 % 100) * 2000;
            assert!(b.iter().all(|&v| v == i ^ round_of), "survivor {k}");
        }
    }
    drop(kept);
    zone_alloc::burst_trim();
    assert_eq!(zone_alloc::burst_stats().0, before.0);
}
