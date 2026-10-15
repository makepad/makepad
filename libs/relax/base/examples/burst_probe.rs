//! Synthetic malloc-fragmentation probe (separate processes for ON/OFF):
//! CARGO_INCREMENTAL=0 cargo run --release -p relax-base --example burst_probe
//! MAKEPAD_RUN_BURST_ARENA=0 CARGO_INCREMENTAL=0 cargo run --release -p relax-base --example burst_probe
//! Allocates 240 MB of transient blocks interleaved with same-sized long-lived heap blocks.
//! It intentionally leaves one arena block per class escaping until the last measurement.
//! This measures allocator behavior, not Amp or compiler throughput.

use relax_base::{memstat, zone_alloc};
use std::time::Instant;

#[global_allocator]
static ALLOC: memstat::Counting = memstat::Counting;

fn snapshot(label: &str) {
    println!("{}", memstat::report(label).lines().next().unwrap());
}

fn main() {
    let runtime = relax_base::options::RuntimeOptions::from_env();
    let mut transient = Vec::with_capacity(2_100_000);
    let mut anchors = Vec::with_capacity(17000);
    let mut escaped = Vec::with_capacity(15);
    snapshot("before");
    let start = Instant::now();
    for shift in 4..=18 {
        let size = 1 << shift;
        let count = (16 << 20) / size;
        for batch in (0..count).step_by(128) {
            // Persistent allocations on the ordinary heap, interleaved with compile bursts.
            anchors.push(vec![0x1a_u8; size]);
            let _scope = zone_alloc::scratch(&runtime);
            for _ in batch..(batch + 128).min(count) {
                transient.push(vec![0x6b_u8; size]);
            }
        }
        escaped.push(transient.pop().unwrap());
    }
    let allocated = start.elapsed();
    snapshot("peak");
    let start = Instant::now();
    drop(transient);
    let freed = start.elapsed();
    snapshot("burst freed (15 escaped blocks)");
    for block in &escaped {
        assert!(block.iter().all(|&b| b == 0x6b));
    }
    drop(escaped);
    snapshot("escapes freed");
    println!(
        "allocate_ms={}, free_ms={}",
        allocated.as_millis(),
        freed.as_millis()
    );
    std::hint::black_box(&anchors);
    drop(anchors);
    memstat::trim();
    snapshot("anchors freed and malloc trimmed");
}
