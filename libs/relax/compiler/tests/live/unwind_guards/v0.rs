// A panic caught by catch_unwind runs every frame's drops, innermost first, in a live session:
// the frames between the panic and the catch are patched code after v1 / v2.
//  v1: the panicking fn changes (new guard + message)
//  v2: a middle frame gains a guard
use std::panic::{catch_unwind, AssertUnwindSafe};

struct G(&'static str);
impl Drop for G {
    fn drop(&mut self) {
        println!("out drop {}", self.0);
    }
}

fn leaf(n: u32) -> u32 {
    let _g = G("leaf");
    if n > 0 { panic!("leaf {}", n); }
    n
}

fn mid(n: u32) -> u32 {
    let _a = G("mid-a");
    let b = Box::new(G("mid-box"));
    let r = leaf(n);
    drop(b);
    r
}

fn top(n: u32) -> u32 {
    let _t = G("top");
    let v = vec![G("top-vec")];
    let r = mid(n);
    drop(v);
    r
}

#[test]
fn run() {
    std::panic::set_hook(Box::new(|_| {}));
    let r = catch_unwind(AssertUnwindSafe(|| top(3)));
    println!("out caught {}", r.is_err());
    println!("out ok {}", top(0));
}
