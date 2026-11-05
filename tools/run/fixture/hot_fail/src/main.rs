// Prints "out <tick> value=<n>" every 20 ms. A worker thread calls tick::value, which
// tools/run/fixture/hot_fail.py edits; the main thread takes a restart with its state: a restarted
// run prints "out restored <tick>" and continues from that tick. Each process prints "pid <n>". Exits
// with code 7 once the file "exit-now" exists in the directory it runs in, and stops after 300 s.
mod tick;
use std::sync::atomic::{AtomicU64, Ordering};

extern "C" {
    fn relax_first_present();
}

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
    unsafe { relax_first_present() };
    std::thread::spawn(|| loop {
        VALUE.store(tick::value(), Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(20));
    });
    loop {
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
