// main's first code is lowered before its borrow-check verdict (unverified), so its call to
// step(&mut Vec, &Vec) -- a `&mut` to a sized value next to another pointer-holding argument -- goes to step's unverified-mode
// twin. PRELINK_ONCE=1: one call, then exit (the run that fills the cache and its startup image).
// Otherwise step runs every millisecond for TWIN_SECS (default 4) and the sum is printed.
// step's callee mix and the late fn `late` (first called after LATE_MS, so its code comes from the
// disk cache, not the startup image) name constants: in the warm run nothing lowers on the owner, so
// nobody evaluates MIX_K / LATE_K unless a background build's front asks for them (const_drain).

// TICKS (a `#[thread_local]` static under thread_local!): its getter's background O3 front asked
// the owner for the static's TLS place and bailed on it every time (csg_boolean's CURRENT_CANCEL
// getter ran baseline, PGO6): lowering ahead reads placed statics from FrontShared::tls.

const MIX_K: u64 = 31;
const LATE_K: u64 = 0x9e37_79b9_7f4a_7c15;
const LATE_MS: u64 = 300;

fn mix(a: u64, d: u64) -> u64 {
    a.wrapping_mul(MIX_K).wrapping_add(d)
}

#[inline(never)]
#[allow(clippy::ptr_arg)]
fn step(acc: &mut Vec<u64>, data: &Vec<u64>) {
    for (a, d) in acc.iter_mut().zip(data.iter()) {
        *a = mix(*a, *d);
    }
}

#[inline(never)]
fn late(acc: &mut [u64]) -> u64 {
    let mut h = 0u64;
    for a in acc.iter_mut() {
        *a ^= *a >> 7;
        h = (h ^ *a).wrapping_mul(LATE_K);
    }
    h
}

thread_local! {
    static TICKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[inline(never)]
fn tick() -> u64 {
    let mut s = 0u64;
    for i in 0..256u64 {
        s ^= TICKS.with(|t| {
            t.set(t.get().wrapping_add(i));
            t.get()
        });
    }
    s
}

fn main() {
    let once = std::env::var("PRELINK_ONCE").is_ok();
    let secs: u64 = std::env::var("TWIN_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    let data: Vec<u64> = (0..4096u64).collect();
    let mut acc = vec![1u64; 4096];
    let t0 = std::time::Instant::now();
    let mut n = 0u64;
    let mut h = 0u64;
    loop {
        step(&mut acc, &data);
        n += 1;
        h = h.wrapping_add(tick() & 1);
        if t0.elapsed().as_millis() as u64 >= LATE_MS {
            h ^= late(&mut acc);
            if once {
                break;
            }
        }
        if !once && t0.elapsed().as_secs() >= secs {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let s = acc.iter().fold(h, |h, v| h.wrapping_mul(1099511628211).wrapping_add(*v));
    println!("cache_warm_twin_tier ticks {} sum {}", if once { 1 } else { n.min(1) }, s % 1000);
}
