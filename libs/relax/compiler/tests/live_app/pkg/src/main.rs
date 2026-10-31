// Prints "out tick <n> <value>" every 20 ms; tests/live_app/run.py edits helper.rs while it runs.
// It supports Relax's state-preserving restart: on a restart request it hands its tick count
// over, and a restarted run continues from it. Stops when value() returns 999 or after 60 s
// (or after LIVE_APP_TICKS ticks: run.py --warm fills the cache that way).
mod helper;

fn main() {
    let t0 = std::time::Instant::now();
    let mut n: u64 = match std::relax::restored_state() {
        Some(s) => {
            let n = s.trim().parse().unwrap_or(0);
            println!("out restored {}", n);
            n
        }
        None => 0,
    };
    std::relax::restart_supported();
    let ticks: u64 = std::env::var("LIVE_APP_TICKS").ok().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
    loop {
        if std::relax::restart_requested() {
            std::relax::restart_now(&n.to_string());
        }
        let v = helper::value(n);
        println!("out tick {} {}", n, v);
        if v == 999 || t0.elapsed().as_secs() > 60 || n + 1 >= ticks {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
