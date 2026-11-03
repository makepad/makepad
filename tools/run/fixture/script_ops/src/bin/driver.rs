//! A compute program's session under `makepad run`: compute_program's driver (JIT-compiled) runs
//! a program on makepad-script bound to the loader's rustc build, every makepad-script call it
//! makes from JIT code included (the VM's setup and limits, eval, ops, errors, gc). A Fab Force
//! study opens such sessions in the app's process. Prints what rustc's build prints
//! (provided_script.sh compares the two).
use makepad_compute_program::*;

const AXPY: &str = r#"
use mod.pod.*
use mod.kernel.*
let axpy = {label: "axpy" x: input(f32) y: output(f32) a: param(0.0)
            element: fn(i) { y[i] = y[i] + a * x[i] }}
{run: fn(s, p) {
    let x = s.buffer(f32, p.n)
    let y = s.buffer(f32, p.n)
    s.fill(x, 1.5)
    s.fill(y, 1.0)
    s.run(axpy, p.n, {a: 2.0}, {x: x, y: y})
    let block = s.buffer(f32, 2)
    s.sum(y, block, 0)
    s.dot(x, y, block, 1)
    let head = s.read(y, 0, 3)
    let sums = s.read(block)
    return [head[0], head[2], sums[0], sums[1]]
}}
"#;

fn main() {
    let config = SessionConfig { params: Value::object(&[("n", Value::Number(1000.0))]), ..SessionConfig::default() };
    let v = Session::open(&Program::new("axpy.splash", AXPY), config).wait_end();
    // a script error raised by an op (the driver's error path: the thread's trap)
    let bad = Session::open(&Program::new("bad.splash", "use mod.pod.*\n{run: fn(s, p) { s.buffer(f32, -1) }}"), SessionConfig::default()).wait_end();
    println!("driver {:?} bad {}", v.map(|v| v.numbers()), bad.is_err());
}
