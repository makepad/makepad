// Prints "out tick <n>" every 20 ms and sends the worker thread (helper::run) a request per tick;
// while the file `go` exists, the worker answers it with "out song <n> <title>:<end>".
// Stops when the file `stop` exists or after 90 s.
mod helper;

fn main() {
    let t0 = std::time::Instant::now();
    let (tx, rx) = std::sync::mpsc::channel::<(u64, bool)>();
    let worker = std::thread::Builder::new().name("worker".into()).spawn(move || helper::run(rx)).unwrap();
    let mut n: u64 = 0;
    loop {
        println!("out tick {}", n);
        let _ = tx.send((n, std::path::Path::new("go").exists()));
        if std::path::Path::new("stop").exists() || t0.elapsed().as_secs() > 90 {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(tx);
    let _ = worker.join();
}
