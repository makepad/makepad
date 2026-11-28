//! The `#[relax_api]` table this crate lays down under `host-shims` (what the `makepad` loader reads).
#![cfg(feature = "host-shims")]

use relax_api::{entries, HANDLE, OP, PLAIN};

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

#[test]
fn stream_decoder_ops_are_marked_in_relax_api_table() {
    let _ = makepad_hal::clock_ns();
    let ops: Vec<_> = entries().iter()
        .filter(|e| e.module.split("::").next() == Some("makepad_hal") && e.kind == OP)
        .map(|e| (e.key)().to_string())
        .collect();
    for name in [
        "hal_stream_decoder_error", "hal_stream_decoder_open", "hal_stream_decoder_name",
        "hal_stream_decoder_push", "hal_stream_decoder_poll", "hal_stream_decoder_read",
        "hal_stream_decoder_release", "hal_stream_decoder_close",
    ] {
        assert!(ops.iter().any(|key| key.ends_with(name)), "{name} missing from the HAL op table");
    }
}
