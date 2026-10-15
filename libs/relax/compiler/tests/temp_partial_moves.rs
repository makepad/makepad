// Partial moves out of temporaries (commercial lane's Stage/P(DOOM) find, 2026-10-04): what is not
// moved out of a temporary drops at the end of its statement, once. "| " lines = rustc --test.
use std::sync::atomic::{AtomicUsize, Ordering};
static DROPS: AtomicUsize = AtomicUsize::new(0);
static LOG: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        DROPS.fetch_add(self.0 as usize, Ordering::Relaxed);
        LOG.lock().unwrap().push(self.0);
    }
}
struct P {
    a: D,
    b: D,
}
struct Q {
    p: P,
    c: D,
}
fn drops() -> usize {
    DROPS.swap(0, Ordering::Relaxed)
}
fn order() -> Vec<u32> {
    std::mem::take(&mut *LOG.lock().unwrap())
}
fn make() -> P {
    P { a: D(1), b: D(10) }
}
fn make_q() -> Q {
    Q { p: make(), c: D(100) }
}
fn take(d: D) -> u32 {
    d.0
}

#[test]
fn t01_field_of_call() {
    drops();
    let a = make().a;
    println!("| make().a: rest {}", drops());
    drop(a);
    println!("| then a {}", drops());
}

#[test]
fn t02_field_of_box_deref() {
    drops();
    let a = (*Box::new(make())).a;
    println!("| (*box).a: rest {}", drops());
    drop(a);
    drops();
    let o = Some(Box::new(make()));
    let b = (*o.unwrap()).b;
    println!("| (*o.unwrap()).b: rest {}", drops());
    drop(b);
    drops();
}

#[test]
fn t03_nested_fields() {
    drops();
    order();
    let a = make_q().p.a;
    println!("| make_q().p.a: rest {} order {:?}", drops(), order());
    drop(a);
    drops();
    order();
}

#[test]
fn t04_tuple_and_arg() {
    drops();
    let x = (D(2), D(20)).1;
    println!("| tuple.1: rest {}", drops());
    drop(x);
    drops();
    let n = take(make().b);
    println!("| take(make().b): {} rest {}", n, drops());
}

#[test]
fn t05_in_branches() {
    for c in [true, false] {
        drops();
        let d = if c { make().a } else { D(5) };
        println!("| branch {}: before end {}", c, drops());
        drop(d);
        drops();
    }
}

#[test]
fn t06_match_temp_box() {
    drops();
    let o = Some(Box::new(make()));
    let n = match *o.unwrap() {
        P { a, b } => a.0 + b.0,
    };
    println!("| match *tmp all: {} dropped {}", n, drops());
    let o = Some(Box::new(make()));
    let n = match *o.unwrap() {
        P { a, .. } => take(a),
    };
    println!("| match *tmp part: {} dropped {}", n, drops());
    let o = Some(Box::new(make()));
    let n = match *o.unwrap() {
        P { ref a, .. } if a.0 == 1 => a.0,
        _ => 0,
    };
    println!("| match *tmp ref: {} dropped {}", n, drops());
}
