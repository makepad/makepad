// Prints "out <tick> <probe>=<value> ..." every 20 ms; tools/run/fixture/live_kinds.py edits
// src/kinds.rs (or this file) while it runs and waits for the edited probe's value to change.
// It restarts with its state (std::relax::restart_supported): a restarted run prints
// "out restored <tick>" and continues from that tick. Stops after LIVE_KINDS_SECS (default 120).
mod kinds;

fn main() {
    let t0 = std::time::Instant::now();
    let secs: u64 = std::env::var("LIVE_KINDS_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
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
        println!("out {} main_loop={} inlined={} {}", n, 1, kinds::tiny(), line);
        if t0.elapsed().as_secs() > secs {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
