//! Panic unwinding probe: every frame between the panic and the catch holds drop guards; the
//! drops must run innermost first, as rustc's unwinder runs them. `unwind_modes.py` runs it
//! plain, under the inspect panic-park (resume panic / restart / return) and live.
//! usage: relax-unwind-probe <panic|oob|restart|return>
use std::cell::Cell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

struct G(&'static str);
impl Drop for G {
    fn drop(&mut self) {
        println!("out drop {}", self.0);
    }
}

thread_local! { static ATTEMPT: Cell<u32> = const { Cell::new(0) }; }

#[inline(never)]
fn leaf(mode: &str, v: &[u32]) -> u32 {
    let _g = G("leaf");
    match mode {
        "oob" => v[v.len() + 1],
        _ => panic!("probe panic"),
    }
}

#[inline(never)]
fn mid(mode: &str, v: &[u32]) -> u32 {
    let _a = G("mid-a");
    let _boxed = Box::new(G("mid-box"));
    let r = leaf(mode, v);
    let _b = G("mid-b-not-reached");
    r
}

// the C-unwind trampoline shape of makepad's run_parked
unsafe extern "C-unwind" fn tramp(p: *mut u8) {
    let f = &mut *(p as *mut &mut dyn FnMut());
    f();
}

#[inline(never)]
fn parked(f: &mut dyn FnMut()) {
    let _g = G("parked-guard");
    let out = catch_unwind(AssertUnwindSafe(|| {
        let _inner = G("parked-body");
        let mut fp: &mut dyn FnMut() = f;
        unsafe { tramp(&mut fp as *mut _ as *mut u8) };
    }));
    if let Err(p) = out {
        println!("out restore");
        resume_unwind(p);
    }
}

#[inline(never)]
fn top(mode: &str) -> u32 {
    let _t = G("top");
    let v = vec![1u32, 2, 3];
    let mut r = 0;
    parked(&mut || r = mid(mode, &v));
    r
}

/// restart / return: the panic is offered at this frame (`restartable work`); the first attempt
/// panics, a restart runs it again (second attempt succeeds), a return gives its value
#[inline(never)]
fn work(x: u64) -> u64 {
    if x == 0 {
        return 0; // the warm-up call: `restartable` needs the fn compiled
    }
    let _w = G("work");
    let n = ATTEMPT.with(|a| { a.set(a.get() + 1); a.get() });
    println!("out work attempt {}", n);
    if n == 1 {
        let _l = G("work-inner");
        let _ = mid("panic", &[1]);
    }
    x + 1000
}

#[inline(never)]
fn outer(mode: &str) {
    let _o = G("outer");
    match mode {
        "restart" | "return" => {
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let r = catch_unwind(|| work(7));
            println!("out work result {:?}", r.map_err(|_| "panic"));
        }
        _ => {
            let r = catch_unwind(|| top(mode));
            println!("out caught {}", r.is_err());
        }
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "panic".into());
    work(0);
    // the inspect driver arms `restartable work` while the probe waits for its go file
    if let (Ok(ready), Ok(go)) = (std::env::var("UNWIND_PROBE_READY"), std::env::var("UNWIND_PROBE_GO")) {
        let _ = std::fs::write(ready, "ready");
        while !std::path::Path::new(&go).exists() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    outer(&mode);
    println!("out done");
}
