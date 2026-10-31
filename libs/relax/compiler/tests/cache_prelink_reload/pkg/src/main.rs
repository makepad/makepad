// Prints "out tick <n> <caller(n)>" every 20 ms; tests/cache_prelink_reload/run.py edits
// helper.rs while it runs. PRELINK_ONCE=1: one line, then exit (the runs that fill the cache).
// Stops when caller() returns 9991 or after 60 s.
mod helper;

// the caller of the edited fn: its code is not edited, it calls value() through the slot table
#[inline(never)]
fn caller(n: u64) -> u64 {
    helper::value(n) * 10 + 1
}

fn main() {
    let once = std::env::var("PRELINK_ONCE").is_ok();
    let t0 = std::time::Instant::now();
    let mut n: u64 = 0;
    loop {
        let v = caller(n);
        println!("out tick {} {}", n, v);
        if once || v == 9991 || t0.elapsed().as_secs() > 60 {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
