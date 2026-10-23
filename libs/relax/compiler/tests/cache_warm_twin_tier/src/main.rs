// main's first code is lowered before its borrow-check verdict (unverified), so its call to
// step(&mut Vec, &Vec) -- a `&mut` to a sized value next to another pointer-holding argument -- goes to step's unverified-mode
// twin. PRELINK_ONCE=1: one call, then exit (the run that fills the cache and its startup image).
// Otherwise step runs every millisecond for TWIN_SECS (default 4) and the sum is printed.
#[inline(never)]
#[allow(clippy::ptr_arg)]
fn step(acc: &mut Vec<u64>, data: &Vec<u64>) {
    for (a, d) in acc.iter_mut().zip(data.iter()) {
        *a = a.wrapping_mul(31).wrapping_add(*d);
    }
}

fn main() {
    let once = std::env::var("PRELINK_ONCE").is_ok();
    let secs: u64 = std::env::var("TWIN_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let data: Vec<u64> = (0..4096u64).collect();
    let mut acc = vec![1u64; 4096];
    let t0 = std::time::Instant::now();
    let mut n = 0u64;
    loop {
        step(&mut acc, &data);
        n += 1;
        if once || t0.elapsed().as_secs() >= secs {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let s = acc.iter().fold(0u64, |h, v| h.wrapping_mul(1099511628211).wrapping_add(*v));
    println!("cache_warm_twin_tier ticks {} sum {}", if once { 1 } else { n.min(1) }, s % 1000);
}
