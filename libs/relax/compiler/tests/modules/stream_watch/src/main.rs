use std::relax::{Edit, Module, Stream};

fn value(m: Module) -> i32 {
    m.get::<fn() -> i32>("value").unwrap()()
}

fn main() {
    let one = "pub fn value() -> i32 { 1 }\n";
    let two = "pub fn value() -> i32 { 2 }\n";
    let m = Module::load_stream("watch_stream", "memory/watch-stream.rs", one, &[]).unwrap();
    // a held stream's feed is pending by construction (no 16 ms coalescing window to hit)
    let held = m.hold_stream().unwrap();
    assert_eq!(m.stream(two).unwrap(), Stream::Pending);
    assert_eq!(value(m), 1, "a held stream applies nothing");
    assert!(m.stream_settled().is_err(), "a held stream with a pending text does not settle");
    drop(held);
    // no further feed: the watcher drains the pending prefix (stream_settled waits for it)
    m.stream_settled().unwrap();
    assert_eq!(value(m), 2, "the watcher fed the pending prefix");
    assert_eq!(m.finish_stream(one).unwrap(), Edit::Patched);
    assert_eq!(value(m), 1, "finish uses the watcher's new file index");
    let held = m.hold_stream().unwrap();
    assert_eq!(m.stream(two).unwrap(), Stream::Pending);
    assert!(m.finish_stream("pub fn value() -> i32 { nope() }\n").is_err());
    // released after the failed finish: a pending prefix it left would be drained now
    drop(held);
    m.stream_settled().unwrap();
    assert_eq!(value(m), 1, "a failed finish also cancels the pending prefix");
    // A cut prefix is quiet while the stream goes on: its feed keeps the running version, the
    // watcher drains the completed text, and nothing is reported (the runner: no error line
    // before the quiet marker). (The stream ended with the finish: this first feed applies.)
    assert_eq!(m.stream("pub fn value() -> i32 { 3").unwrap(), Stream::Incomplete);
    assert_eq!(value(m), 1, "a cut prefix keeps the running version");
    let held = m.hold_stream().unwrap();
    assert_eq!(m.stream("pub fn value() -> i32 { 3 }\n").unwrap(), Stream::Pending);
    drop(held);
    m.stream_settled().unwrap();
    assert_eq!(value(m), 3, "the watcher fed the completed text");
    // An error is quiet at its feed and reported by the watcher once the stream stalls (a second
    // after its last change and feed): stream_settled returns after that report. (Its feed
    // applies: the watcher's last feed ended over a second ago, past the coalescing window.)
    // The runner's marker goes out while the stream is held, which keeps the stall report back,
    // so the marker comes first however long the program takes to print it: the sleep is a slow
    // print past the stall's second, every run (the marker, the program's first eprintln compiled
    // cold on a loaded machine, once took that long, and the report came first).
    let fed = m.stream("pub fn value() -> i32 { missing() }\n");
    let held = m.hold_stream().unwrap();
    assert_eq!(fed.unwrap(), Stream::Incomplete);
    std::thread::sleep(std::time::Duration::from_millis(1300));
    eprintln!("stream-watch: fed, nothing reported");
    drop(held);
    m.stream_settled().unwrap();
    assert_eq!(value(m), 3, "a stalled error keeps the running version");
    m.unload();
    println!("out module stream watcher: true");
}
