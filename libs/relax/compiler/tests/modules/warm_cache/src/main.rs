// A module's load takes the twins of std it reaches from the disk cache (reload.rs
// compile_reachable_in). warm_cache.py runs this twice on one cache: the second run links them
// and builds the module's own code alone, which turns hot and reaches O3 as in the first run.
// Both runs give what the same source gives as host code, before and after the tier-up.
mod kernel;
use std::relax::Module;

fn main() {
    let m = Module::load_source("warm", "warm.rs", include_str!("kernel.rs"), &[]).unwrap();
    let mix: fn(u32) -> u64 = m.get("mix").unwrap();
    let rest: fn(u32) -> u64 = m.get("rest").unwrap();
    let want = (kernel::mix(40), kernel::rest(40), kernel::mix(700), kernel::rest(700));
    let cold = (mix(40), rest(40), mix(700), rest(700));
    // warm them, then leave the background compiler time to install
    for _ in 0..300 {
        std::hint::black_box((mix(200), rest(200)));
    }
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let hot = (mix(40), rest(40), mix(700), rest(700));
    println!("out as host code: {:016x} {:016x} {:016x} {:016x}", want.0, want.1, want.2, want.3);
    println!("out the module, before the tier-up: {}", cold == want);
    println!("out the module, after it: {}", hot == want);
    m.unload();
}
