//! A native panic in the compile a JIT call starts (the owner fails finishing it) must reach the
//! program as an ordinary panic of that call, never unwind into the JIT frames (an abort without a
//! message: "failed to initiate panic, error 5"). compile_panic.sh runs it with
//! MAKEPAD_RUN_TEST_COMPILE_PANIC=compile_panic_target.
use std::panic::catch_unwind;

#[inline(never)]
fn compile_panic_target(x: u32) -> u32 {
    x * 3 + 1
}

fn main() {
    for round in 0..2 {
        match catch_unwind(|| compile_panic_target(std::hint::black_box(4))) {
            Ok(v) => println!("round {round}: value {v}"),
            Err(e) => {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                println!("round {round}: caught: {msg}");
            }
        }
    }
    println!("compile_panic done");
}
