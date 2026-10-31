// `work` is called ~300 times a second for LUKE_MS (its decayed cost stays well under the O3
// threshold: it must not tier up then), then as fast as it goes for HOT_MS, where it must reach O3:
// a fn lukewarm early is still counted when it turns hot (cold race: DrawVars::set_uniform, PGO6
// 10-23; count stubs used to be retired after slow windows). The "hot" line marks the switch.
// `work` is #[inline(never)]: no caller's O3 code holds a copy of it, only its own slot counts.

const LUKE_MS: u64 = 7000;
const HOT_MS: u64 = 8000;

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
    while t1.elapsed().as_millis() as u64 <= HOT_MS {
        for _ in 0..1000 {
            h = work(h);
        }
        calls += 1000;
    }
    println!("retired_stub_rearm calls {} h {}", calls.min(1), h % 7);
}
