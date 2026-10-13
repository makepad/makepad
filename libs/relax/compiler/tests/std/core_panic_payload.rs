// Panic payloads as real std hands them out: catch_unwind and JoinHandle::join return the
// `&'static str` of a literal-only panic!, a String when formatted, M for panic_any, the payload
// of resume_unwind, and real core's &'static str for its constant messages (unwrap on None).
// (Formatted messages use runtime values: rustc folds literal format arguments into the string.)
use std::any::Any;
use std::panic;

fn describe(p: &Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&'static str>() {
        format!("&str {:?}", s)
    } else if let Some(s) = p.downcast_ref::<String>() {
        format!("String {:?}", s)
    } else if let Some(n) = p.downcast_ref::<u32>() {
        format!("u32 {}", n)
    } else {
        String::from("other")
    }
}

#[derive(Debug)]
struct Custom(u8);

#[test]
fn a01_catch_unwind_payloads() {
    panic::set_hook(Box::new(|_| {}));
    let r = panic::catch_unwind(|| panic!("literal"));
    println!("| literal: {}", describe(&r.unwrap_err()));
    let n = 7;
    let r = panic::catch_unwind(|| panic!("formatted {}", n));
    println!("| formatted: {}", describe(&r.unwrap_err()));
    let r = panic::catch_unwind(|| panic::panic_any(42u32));
    println!("| panic_any u32: {}", describe(&r.unwrap_err()));
    let r = panic::catch_unwind(|| panic::panic_any(Custom(3)));
    let p = r.unwrap_err();
    println!("| panic_any custom: {} {:?}", describe(&p), p.downcast_ref::<Custom>());
    let r = panic::catch_unwind(|| panic::resume_unwind(Box::new(String::from("resumed"))));
    println!("| resume_unwind: {}", describe(&r.unwrap_err()));
    let r = panic::catch_unwind(|| {
        let o: Option<u8> = None;
        o.unwrap()
    });
    println!("| unwrap None: {}", describe(&r.unwrap_err()));
    let r = panic::catch_unwind(|| {
        let v: Vec<u8> = Vec::new();
        v[3]
    });
    println!("| index: {}", describe(&r.unwrap_err()));
    let r = panic::catch_unwind(|| 5);
    println!("| ok: {:?}", r.ok());
    // nested: the inner payload does not leak into the outer one
    let r = panic::catch_unwind(|| {
        let inner = panic::catch_unwind(|| panic!("inner"));
        let n = std::hint::black_box(0);
        println!("| inner: {}", describe(&inner.unwrap_err()));
        panic!("outer {}", n + 1)
    });
    println!("| outer: {}", describe(&r.unwrap_err()));
    let _ = panic::take_hook();
}

#[test]
fn a02_join_payloads() {
    panic::set_hook(Box::new(|_| {}));
    let h = std::thread::spawn(|| panic!("thread literal"));
    println!("| join literal: {}", describe(&h.join().unwrap_err()));
    let w = String::from("formatted");
    let h = std::thread::spawn(move || panic!("thread {}", w));
    println!("| join formatted: {}", describe(&h.join().unwrap_err()));
    let h = std::thread::spawn(|| panic::panic_any(9u32));
    println!("| join panic_any: {}", describe(&h.join().unwrap_err()));
    let h = std::thread::spawn(|| 3u8);
    println!("| join ok: {:?}", h.join().ok());
    let _ = panic::take_hook();
}
