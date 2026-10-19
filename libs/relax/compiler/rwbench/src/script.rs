// The Splash script VM (platform/script, makepad-script) evaluating a compute-heavy script: the
// bytecode loop over arithmetic, locals, array and object access, loops and recursive calls. The
// script is parsed and evaluated once in setup (it defines `bench`); an iteration is one
// `vm.call(bench)`, so the timed work is the interpreter, not the parser.
use crate::fold;
use makepad_script::*;

const SCRIPT: &str = r#"
let fib = fn(n) {
    if n < 2 { return n }
    return fib(n - 1) + fib(n - 2)
}
let sieve = fn(n) {
    let flags = []
    for i in 0..n { flags.push(1) }
    let count = 0
    for i in 2..n {
        if flags[i] == 1 {
            count += 1
            let j = i * 2
            for k in 0..n {
                if j >= n { break }
                flags[j] = 0
                j += i
            }
        }
    }
    return count
}
let bodies = fn(steps) {
    let ps = []
    for i in 0..5 { ps.push({x: i * 1.5, y: i * 0.5 - 1.0, vx: 0.0, vy: 0.1 * i}) }
    for s in 0..steps {
        for i in 0..5 {
            let a = ps[i]
            for j in 0..5 {
                if i != j {
                    let b = ps[j]
                    let dx = b.x - a.x
                    let dy = b.y - a.y
                    let d2 = dx * dx + dy * dy + 0.01
                    a.vx += dx / d2 * 0.001
                    a.vy += dy / d2 * 0.001
                }
            }
        }
        for i in 0..5 {
            let a = ps[i]
            a.x += a.vx
            a.y += a.vy
        }
    }
    let e = 0.0
    for i in 0..5 { e += ps[i].x * 3.0 + ps[i].y }
    return e
}
let bench = fn() {
    return fib(17) * 1000000.0 + sieve(3000) * 1000.0 + bodies(60)
}
bench
"#;

pub fn setup() -> Box<dyn FnMut() -> u64> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let bench = vm.eval(ScriptMod::new("", "", "rwbench.splash", 0, 0, SCRIPT, &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "splash_eval: {errors:?}");
    assert!(bench.as_object().is_some(), "splash_eval: script did not return its entry fn");
    Box::new(move || {
        let v = vm.call(bench, &[]);
        let n = v.as_number().expect("splash_eval: bench returned a number");
        fold(0, n.to_bits())
    })
}
