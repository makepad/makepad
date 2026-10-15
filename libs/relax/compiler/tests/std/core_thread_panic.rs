// Panics in spawned threads: join payloads (Builder, panic_any after an inner caught panic, index and
// unwrap messages), thread::scope with a panicking scoped thread (must not hang: the thread's packet
// is released though Relax panics do not unwind, R11), and many threads panicking at once
// (per-thread panic depth: concurrent panics are not nested ones).
use std::any::Any;
use std::panic;
use std::thread;
fn d(p: &Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&'static str>() { format!("&str {:?}", s) }
    else if let Some(s) = p.downcast_ref::<String>() { format!("String {:?}", s) }
    else if let Some(n) = p.downcast_ref::<i64>() { format!("i64 {}", n) }
    else { String::from("other") }
}
#[test]
fn a01() {
    panic::set_hook(Box::new(|_| {}));
    let h = thread::Builder::new().name("w".into()).spawn(|| -> u8 { panic!("named {}", 3) }).unwrap();
    println!("| builder: {}", d(&h.join().unwrap_err()));
    let h = thread::spawn(|| { let _ = panic::catch_unwind(|| panic!("inner")); panic::panic_any(-7i64) });
    println!("| after inner catch: {}", d(&h.join().unwrap_err()));
    let h = thread::spawn(|| { let v: Vec<u8> = vec![]; v[1] });
    println!("| index: {}", d(&h.join().unwrap_err()));
    let h = thread::spawn(|| { let r: Result<u8, String> = Err("e".into()); r.unwrap() });
    println!("| unwrap err: {}", d(&h.join().unwrap_err()));
    let r = panic::catch_unwind(|| thread::scope(|s| { s.spawn(|| panic!("scoped")); }));
    println!("| scope: {}", d(&r.unwrap_err()));
    let hs: Vec<_> = (0..4).map(|i| thread::spawn(move || if i % 2 == 0 { panic!("even {}", i) } else { i })).collect();
    // join all before printing: under Relax the panicking threads' reports go to the same output
    let outs: Vec<String> = hs.into_iter().map(|h| match h.join() { Ok(v) => format!("ok {}", v), Err(p) => format!("err {}", d(&p)) }).collect();
    for o in outs { println!("| {}", o); }
    let hs: Vec<_> = (0..16).map(|i| thread::spawn(move || -> u32 { if i % 3 != 0 { panic!("many {}", i) } else { i } })).collect();
    let outs: Vec<String> = hs.into_iter().map(|h| match h.join() { Ok(v) => format!("many ok {}", v), Err(p) => format!("many {}", d(&p)) }).collect();
    for o in outs { println!("| {}", o); }
    let r = panic::catch_unwind(|| thread::scope(|s| { for i in 0..4 { s.spawn(move || if i == 2 { panic!("one of four") }); } }));
    println!("| scope of four: {}", d(&r.unwrap_err()));
    let _ = panic::take_hook();
}
