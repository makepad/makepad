// A design registry like a script_mod! design: `design` registers handlers (closures) on start;
// handlers run only while the file `go` exists ("out val <n> <v>" per tick). They build closures of
// other fns of this file (`adder`'s, through its opaque return type, and `scaled`'s nested one).
// Prints "out tick <n>" every 20 ms; stops when the file `stop` exists or after 90 s.
use std::sync::Mutex;

type Handler = Box<dyn Fn(u64) -> u64 + Send>;
static DESIGN: Mutex<Vec<Handler>> = Mutex::new(Vec::new());

fn adder(k: u64) -> impl Fn(u64) -> u64 {
    let base = k * 10;
    move |x| x + base
}

fn scaled(m: u64) -> impl Fn(u64) -> u64 {
    let f = adder(m);
    move |x| f(x) * 2
}

fn design() {
    let extra = 7u64;
    let mut d = DESIGN.lock().unwrap();
    d.push(Box::new(move |n| {
        let f = adder(n % 5);
        let g = |y: u64| f(y) + extra;
        g(1)
    }));
    d.push(Box::new(move |n| scaled(n % 3)(extra)));
}

#[inline(never)]
fn apply<F: FnOnce() -> u64>(f: F) -> u64 {
    f()
}

// Runs on every tick, so the disk cache holds it; its callee `apply<closure>` first runs after the
// patch: nothing has built that closure's owner body in this process.
fn late(run: bool, n: u64) -> u64 {
    if run { apply(move || n + 1) } else { 0 }
}

fn main() {
    design();
    let t0 = std::time::Instant::now();
    let mut n: u64 = 0;
    loop {
        println!("out tick {}", n);
        if std::path::Path::new("go").exists() {
            let v: u64 = DESIGN.lock().unwrap().iter().map(|h| h(n)).sum();
            println!("out val {} {}", n, v);
        }
        // (its own file: the warm-up run opens `go`, which would put `apply<closure>` in the cache)
        if std::path::Path::new("late").exists() {
            println!("out late {} {}", n, late(true, n));
        } else {
            late(false, n);
        }
        if std::path::Path::new("stop").exists() || t0.elapsed().as_secs() > 90 {
            break;
        }
        n += 1;
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
