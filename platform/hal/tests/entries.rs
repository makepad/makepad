//! The `#[relax_api]` table this crate lays down under `host-shims` (what the `makepad` loader reads).
#![cfg(feature = "host-shims")]

use makepad_relax_api::{entries, HANDLE, OP, PLAIN};

#[test]
fn the_four_ops_and_the_plain_types_are_marked() {
    // links the crate (and with it its link-section entries)
    let _ = makepad_hal::clock_ns();
    let mine: Vec<_> = entries().iter().filter(|e| e.module.split("::").next() == Some("makepad_hal")).collect();
    let count = |k| mine.iter().filter(|e| e.kind == k).count();
    println!("makepad_hal entries: ops {} plain {} handle {}", count(OP), count(PLAIN), count(HANDLE));
    let ops: Vec<String> = mine.iter().filter(|e| e.kind == OP).map(|e| (e.key)().to_string()).collect();
    for name in ["hal_open", "hal_submit", "hal_pump", "hal_wake", "hal_wake_session"] {
        assert!(ops.iter().any(|o| o.ends_with(name)), "{name} missing from {ops:?}");
    }
    assert!(mine.iter().filter(|e| e.kind == PLAIN).all(|e| e.size > 0));
}
