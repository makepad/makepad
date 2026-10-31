// Runs until a signal ends it (tests/signal_exit/run.sh): busy JIT work on one thread, sleeping on another.
fn main() {
    println!("started");
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
    });
    let mut x: u64 = 1;
    loop {
        for i in 0..1_000_000u64 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
        }
        if x == 42 {
            println!("{}", x);
        }
    }
}
