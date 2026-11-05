// Prints "out <tick> value=<n>" every 20 ms. A worker thread calls tick::value, which
// tools/run/fixture/hot_fail.py edits; the main thread takes a restart with its state: a restarted
// run prints "out restored <tick>" and continues from that tick. Each process prints "pid <n>". Exits
// with code 7 once the file "exit-now" exists in the directory it runs in, and stops after 300 s.
// Once the file "panic-now" exists, a thread of its own panics there (the program's own panic).
// A terminal program: it never presents a frame, so nothing but its restarted run's restore tells
// the JIT the restart completed.
mod tick;
use std::sync::atomic::{AtomicU64, Ordering};

static VALUE: AtomicU64 = AtomicU64::new(0);

fn main() {
    let t0 = std::time::Instant::now();
    println!("pid {}", std::process::id());
    let mut n: u64 = match std::relax::restored_state() {
        Some(s) => {
            let n = s.trim().parse().unwrap_or(0);
            println!("out restored {}", n);
            n
        }
        None => 0,
    };
    std::relax::restart_supported();
    // (a call that panics does not end the ticks: every later call meets what the first met)
    std::thread::spawn(|| loop {
        if let Ok(v) = std::panic::catch_unwind(tick::value) {
            VALUE.store(v, Ordering::Relaxed);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    });
    let mut panicked = false;
    loop {
        if !panicked && std::path::Path::new("panic-now").exists() {
            panicked = true;
            std::thread::spawn(|| panic!("hot_fail: the program's own panic"));
        }
        if std::relax::restart_requested() {
            std::relax::restart_now(&n.to_string());
        }
        if std::path::Path::new("exit-now").exists() {
            std::process::exit(7);
        }
        println!("out {} value={}", n, VALUE.load(Ordering::Relaxed));
        n += 1;
        if t0.elapsed().as_secs() > 300 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
