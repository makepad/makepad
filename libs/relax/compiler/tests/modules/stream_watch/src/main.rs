use std::relax::{Edit, Module, Stream};
use std::time::{Duration, Instant};

fn wait_value(m: Module, want: i32) {
    let start = Instant::now();
    loop {
        if m.get::<fn() -> i32>("value").unwrap()() == want { return; }
        assert!(start.elapsed().as_secs() < 5);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn main() {
    let one = "pub fn value() -> i32 { 1 }\n";
    let two = "pub fn value() -> i32 { 2 }\n";
    let m = Module::load_stream("watch_stream", "memory/watch-stream.rs", one, &[]).unwrap();
    m.stream(one).unwrap();
    assert_eq!(m.stream(two).unwrap(), Stream::Pending);
    wait_value(m, 2); // no further feed: the watcher must drain the pending prefix
    assert_eq!(m.finish_stream(one).unwrap(), Edit::Patched);
    wait_value(m, 1); // finish uses the watcher's new file index
    m.stream(one).unwrap();
    assert_eq!(m.stream(two).unwrap(), Stream::Pending);
    assert!(m.finish_stream("pub fn value() -> i32 { nope() }\n").is_err());
    std::thread::sleep(Duration::from_millis(100));
    wait_value(m, 1); // failed finish also cancels the pending prefix
    // A syntax error stays quiet until the one-second stall. The runner checks the markers.
    eprintln!("stream-watch: feed error");
    m.stream("pub fn value() -> i32 { missing() }\n").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    eprintln!("stream-watch: quiet window ended");
    std::thread::sleep(Duration::from_millis(1100));
    wait_value(m, 1);
    m.unload();
    println!("out module stream watcher: true");
}
