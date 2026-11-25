// `work` is called ~300 times a second for LUKE_MS (its decayed cost stays well under the O3
// threshold: it must not tier up then), then as fast as it goes until the file RSR_STOP names exists
// (run.sh makes it once the O3 trace shows work's O3 code installed) or HOT_MS passed, where it must reach O3:
// a fn lukewarm early is still counted when it turns hot (cold race: DrawVars::set_uniform, PGO6
// 10-23; count stubs used to be retired after slow windows). The "hot" line marks the switch.
// `work` is #[inline(never)]: no caller's O3 code holds a copy of it, only its own slot counts.

const LUKE_MS: u64 = 7000;
/// the hot phase's end when no O3 code comes (a failure): run.sh stops it as soon as it does
const HOT_MS: u64 = 10_000;

#[inline(never)]
fn work(x: u64) -> u64 {
    x.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(17) ^ (x >> 7)
}

fn main() {
    let t0 = std::time::Instant::now();
    let mut h = 1u64;
    let mut calls = 0u64;
    while t0.elapsed().as_millis() as u64 <= LUKE_MS {
        h = work(h);
        calls += 1;
        std::thread::sleep(std::time::Duration::from_micros(3300));
    }
    println!("retired_stub_rearm hot");
    let t1 = std::time::Instant::now();
    let stop = std::env::var_os("RSR_STOP").map(std::path::PathBuf::from);
    while t1.elapsed().as_millis() as u64 <= HOT_MS && !stop.as_ref().is_some_and(|p| p.exists()) {
        for _ in 0..100_000 {
            h = work(h);
        }
        calls += 100_000;
    }
    println!("retired_stub_rearm calls {} h {}", calls.min(1), h % 7);
}
