// Prints "out <tick> <probe>=<value> ..." every 20 ms; tools/run/fixture/live_kinds.py edits
// src/kinds.rs (or this file) while it runs and waits for the edited probe's value to change.
// It restarts with its state (std::relax::restart_supported): a restarted run prints
// "out restored <tick>" and continues from that tick. Stops after LIVE_KINDS_SECS (default 120)
// of its 20 ms ticks, each process counting its own.
mod kinds;

#[cfg(feature = "window")]
pub use makepad_widgets;
#[cfg(feature = "window")]
use makepad_widgets::*;
#[cfg(feature = "window")]
mod swatch;
#[cfg(feature = "window")]
mod window;
#[cfg(feature = "window")]
use window::App;
#[cfg(feature = "window")]
app_main!(App);

#[cfg(not(feature = "window"))]
fn main() {
    let secs: u64 = std::env::var("LIVE_KINDS_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
    // Counted in ticks, not read off the clock: what a run calls must not hang on where its start
    // falls in a clock second. Two readings within one second compare their nanoseconds, a call
    // two readings in different seconds never make, so a run that began late in a second missed
    // a function the next run then compiled: edit_undone_warm.py, which compares two runs' cache
    // misses, counted it as one miss too many (11-19).
    let ticks = (secs + 1) * 50;
    let mut tick: u64 = 0;
    let mut n: u64 = match std::relax::restored_state() {
        Some(s) => {
            let n = s.trim().parse().unwrap_or(0);
            println!("out restored {}", n);
            n
        }
        None => 0,
    };
    std::relax::restart_supported();
    let mut st = kinds::State::new();
    loop {
        if std::relax::restart_requested() {
            std::relax::restart_now(&n.to_string());
        }
        let line = kinds::report(&mut st);
        println!("out {} main_loop={} inlined={} {} {}", n, 1, kinds::tiny(), line, kinds::report2());
        tick += 1;
        if tick > ticks {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
