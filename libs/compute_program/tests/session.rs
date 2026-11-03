//! The session core on the CPU: ops, segments, steering, outputs and limits.
use makepad_compute_program::*;

fn config(params: Value) -> SessionConfig {
    SessionConfig { params, ..SessionConfig::default() }
}

fn open(source: &str, config: SessionConfig) -> Session {
    Session::open(&Program::new("test.splash", source), config)
}

fn run(source: &str, config: SessionConfig) -> Result<Value, ProgramError> {
    open(source, config).wait_end()
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

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

#[test]
fn a_kernel_and_library_reductions_run() {
    let v = run(AXPY, config(Value::object(&[("n", n(1000.0))]))).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.numbers().unwrap(), vec![4.0, 4.0, 4000.0, 6000.0]);
}

/// Kernels, repeats, sweeps, reduce kernels and every library op, on odd sizes.
const OPS: &str = r#"
use mod.pod.*
use mod.kernel.*
let wave = {x: output(f32) element: fn(i) { x[i] = float((i * 37) % 97) * 0.37 - 15.0 + float(i % 7) * 0.01 }}
let bump = {x: output(f32) round: param(0, 0, 1000000) element: fn(i) { x[i] = x[i] * 0.5 + float(round) }}
// Each layer of a column takes the layer below's value (an up sweep: a prefix along z).
let climb = {v: output(f32) layer: param(0, 0, 4096) nx: param(1, 1, 4096)
             element: fn(i) { let l = int(layer) if l > 0 { v[i + l * int(nx)] = v[i + l * int(nx)] + v[i + (l - 1) * int(nx)] } }}
let moments = {x: input(f32) reduce_sum: fn(i) { let v = x[i] return vec2(v, v * v) }}
let ints = {k: output(u32) element: fn(i) { k[i] = (i * 7) % 5 }}
{run: fn(s, p) {
    let count = p.n
    let x = s.buffer(f32, count)
    s.run(wave, count, {}, {x: x})
    s.repeat(bump, 5, count, {}, {x: x})
    let block = s.buffer(f32, 8)
    s.sum(x, block, 0)
    s.min(x, block, 1)
    s.max(x, block, 2)
    s.dot(x, x, block, 3)
    s.reduce(moments, count, {}, {x: x}, block, 4)
    let k = s.buffer(u32, count)
    s.run(ints, count, {}, {k: k})
    let scan = s.buffer(u32, count + 1)
    s.scan(k, scan)
    let col = s.buffer(f32, [5, 1, 9])
    s.fill(col, 1.0)
    s.sweep(climb, 9, 5, "up", {nx: 5}, {v: col})
    let fine = s.buffer(f32, 5 * 5 * 3)
    var fv = []
    for i in 0..75 { fv.push(i * 0.25) }
    s.write(fine, fv)
    let coarse = s.buffer(f32, 3 * 3 * 2)
    let grid = {grid: "vertex", dims: [4, 4, 2]}
    s.restrict(fine, coarse, grid)
    let back = s.buffer(f32, 75)
    s.prolong(coarse, back, grid)
    let cells = s.buffer(f32, 4 * 4 * 2)
    var cv = []
    for i in 0..32 { cv.push(i) }
    s.write(cells, cv)
    let half = s.buffer(f32, 4)
    s.restrict(cells, half, {grid: "cell", dims: [4, 4, 2]})
    return {block: s.read(block), scan: s.read(scan), col: s.read(col, 40, 5), coarse: s.read(coarse), back: s.read(back), half: s.read(half)}
}}
"#;

#[test]
fn every_op_gives_the_same_bits_on_any_worker_count_and_on_the_interpreter() {
    let params = Value::object(&[("n", n(70001.0))]);
    let native1 = run(OPS, SessionConfig { threads: 1, ..config(params.clone()) }).unwrap_or_else(|e| panic!("{e}"));
    let native8 = run(OPS, SessionConfig { threads: 8, ..config(params.clone()) }).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(native1, native8);
    let small = Value::object(&[("n", n(1001.0))]);
    let interp = run(OPS, SessionConfig { backend: Backend::Interp, ..config(small.clone()) }).unwrap_or_else(|e| panic!("{e}"));
    let native = run(OPS, SessionConfig { threads: 3, ..config(small) }).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(interp, native);
    // Spot checks: the exclusive scan's total, a column after the sweep, the cell average.
    let scan = native1.get("scan").and_then(|v| v.numbers()).unwrap();
    let total: f64 = (0..70001u64).map(|i| ((i * 7) % 5) as f64).sum();
    assert_eq!(scan[70001], total);
    assert_eq!(native1.get("col").and_then(|v| v.numbers()).unwrap(), vec![9.0; 5]);
    let half = native1.get("half").and_then(|v| v.numbers()).unwrap();
    assert_eq!(half[0], (0.0 + 1.0 + 4.0 + 5.0 + 16.0 + 17.0 + 20.0 + 21.0) / 8.0);
}

#[test]
fn messages_steer_the_program_and_outputs_carry_their_rev() {
    let source = r#"
use mod.pod.*
{run: fn(s, p) {
    let b = s.buffer(f32, 4)
    var total = 0
    loop {
        let m = s.wait_inbox({add: nil, done: nil})
        if m.msgs.add != nil {
            total = total + m.msgs.add
            s.fill(b, total)
            s.publish("level", b, {precision: "u8", range: [0.0, 10.0]})
            s.publish("half", b, {precision: "f16"})
            s.state("total", b)
            s.value("total", total)
            s.progress(total / 10.0, "adding")
        }
        if m.msgs.done != nil {
            return total
        }
    }
}}
"#;
    let session = open(source, config(Value::Nil));
    session.send("add", n(2.5));
    let mut seen = Updates::default();
    while seen.output("level").is_none() || seen.value("total").is_none() {
        let u = session.wait();
        seen.outputs.extend(u.outputs);
        seen.values.extend(u.values);
    }
    let level = seen.output("level").unwrap();
    assert_eq!(level.rev, 1);
    match &level.data {
        OutputData::U8 { levels, .. } => assert_eq!(levels.to_vec(), vec![64u8; 4]),
        other => panic!("{other:?}"),
    }
    match &seen.output("half").unwrap().data {
        OutputData::F16(h) => assert_eq!(h.to_vec(), vec![0x4100u16; 4]),
        other => panic!("{other:?}"),
    }
    assert_eq!(seen.value("total"), Some(&n(2.5)));
    session.send("add", n(1.0));
    session.send("done", true.into());
    let total = session.wait_end().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(total, n(3.5));
}

#[test]
fn an_interrupt_stops_device_work_and_the_program_takes_its_inbox() {
    // A long kernel the interrupt stops; the read after it returns nil until the inbox is taken.
    let source = r#"
use mod.pod.*
use mod.kernel.*
let spin = {x: output(f32) element: fn(i) { var a = float(i) for k in 0..20000 { a = a * 0.999 + 1.0 } x[i] = a }}
{run: fn(s, p) {
    let x = s.buffer(f32, 1048576)
    s.value("ready", true)
    s.run(spin, 1048576, {}, {x: x})
    let r = s.read(x, 0, 1)
    let m = s.inbox()
    return {read: r, interrupted: m.interrupted, why: m.msgs.why}
}}
"#;
    let session = open(source, config(Value::Nil));
    while session.wait().value("ready").is_none() {}
    session.interrupt("why", "regrow".into());
    let v = session.wait_end().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.get("read"), Some(&Value::Nil));
    assert_eq!(v.get("interrupted"), Some(&Value::Bool(true)));
    assert_eq!(v.get("why"), Some(&Value::String("regrow".into())));
}

#[test]
fn cancel_ends_a_looping_driver_and_a_waiting_one() {
    let looping = open("{run: fn(s, p) { var k = 0 loop { k = k + 1 } }}", config(Value::Nil));
    looping.cancel();
    assert_eq!(looping.wait_end(), Err(ProgramError::Cancelled));
    let waiting = open("{run: fn(s, p) { s.wait_inbox() return 1 }}", config(Value::Nil));
    waiting.cancel();
    assert_eq!(waiting.wait_end(), Err(ProgramError::Cancelled));
}

#[test]
fn a_buffer_past_the_budget_is_a_refusal_the_program_answers() {
    let source = r#"
use mod.pod.*
{run: fn(s, p) {
    let big = s.buffer(f32, [1024, 1024, 64])
    if big.refused {
        let small = s.buffer(f32, [1024, 1024, 1])
        return {wanted: big.wanted, available: big.available, len: small.len}
    }
    return nil
}}
"#;
    let limits = Limits { buffer_bytes: 16 << 20, ..Limits::default() };
    let v = run(source, SessionConfig { limits, ..config(Value::Nil) }).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.get("wanted"), Some(&n((1024.0 * 1024.0 * 64.0) * 4.0)));
    assert_eq!(v.get("available"), Some(&n((16 << 20) as f64)));
    assert_eq!(v.get("len"), Some(&n(1024.0 * 1024.0)));
}

#[test]
fn hostile_programs_end_on_their_budgets() {
    let small = Limits { fuel: 2_000_000, heap_bytes: 16 << 20, work: 50_000_000, ..Limits::default() };
    let with = |limits: Limits| SessionConfig { limits, ..config(Value::Nil) };
    // A runaway driver loop.
    assert_eq!(run("{run: fn(s, p) { var k = 0 loop { k = k + 1 } }}", with(small)), Err(ProgramError::OverBudget("instructions")));
    // A heap bomb.
    assert_eq!(run("{run: fn(s, p) { var a = [] loop { a.push([1, 2, 3, 4, 5, 6, 7, 8]) } }}", with(Limits { fuel: 1 << 30, ..small })), Err(ProgramError::OverBudget("heap")));
    // Runaway recursion.
    assert_eq!(run("fn deeper(k) { return deeper(k + 1) }\n{run: fn(s, p) { return deeper(0) }}", with(small)), Err(ProgramError::OverBudget("calls")));
    // 2^29 elements over a tiny buffer: the work budget stops it.
    let huge = r#"
use mod.pod.*
use mod.kernel.*
let k = {x: input(f32) y: output(f32) element: fn(i) { y[i * 0] = y[i * 0] + x[i] }}
{run: fn(s, p) {
    let x = s.buffer(f32, 4)
    let y = s.buffer(f32, 1)
    s.run(k, 536870912, {}, {x: x, y: y})
    return s.read(y)
}}
"#;
    assert_eq!(run(huge, with(small)), Err(ProgramError::OverBudget("work")));
    // A `try` does not keep a program past its budget.
    let caught = r#"
use mod.pod.*
use mod.kernel.*
let k = {x: input(f32) y: output(f32) element: fn(i) { y[i * 0] = y[i * 0] + x[i] }}
{run: fn(s, p) {
    let x = s.buffer(f32, 4)
    let y = s.buffer(f32, 1)
    try { s.run(k, 536870912, {}, {x: x, y: y}) s.read(y) } catch 0
    return 7
}}
"#;
    assert_eq!(run(caught, with(small)), Err(ProgramError::OverBudget("work")));
}

#[test]
fn out_of_range_kernel_accesses_stay_in_their_buffer() {
    // Reads and writes far past the end are clamped to the bound buffer; the neighbour is intact.
    let source = r#"
use mod.pod.*
use mod.kernel.*
let wild = {x: input(f32) y: output(f32) element: fn(i) { y[i * 1009 + 777777] = x[i * 997 - 5] + 1.0 }}
{run: fn(s, p) {
    let x = s.buffer(f32, 8)
    let y = s.buffer(f32, 8)
    let other = s.buffer(f32, 8)
    s.fill(other, 5.0)
    s.run(wild, 4096, {}, {x: x, y: y})
    return {other: s.read(other), y: s.read(y)}
}}
"#;
    let v = run(source, config(Value::Nil)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v.get("other").and_then(|o| o.numbers()), Some(vec![5.0; 8]));
    assert!(v.get("y").and_then(|o| o.numbers()).unwrap().iter().all(|y| *y == 0.0 || *y == 1.0));
}

#[test]
fn bad_arguments_are_script_errors_at_the_call() {
    let source = r#"
use mod.pod.*
use mod.kernel.*
let k = {y: output(f32) element: fn(i) { y[i] = 1.0 }}
{run: fn(s, p) {
    let y = s.buffer(f32, 4)
    s.run(k, 8, {}, {y: y})
    return 1
}}
"#;
    match run(source, config(Value::Nil)) {
        Err(ProgramError::Program(m)) => assert!(m.contains("`y` has 4 words; 8 elements write 8"), "{m}"),
        other => panic!("{other:?}"),
    }
    let freed = "use mod.pod.*\n{run: fn(s, p) { let b = s.buffer(f32, 4) s.free(b) s.fill(b, 1.0) return 1 }}";
    match run(freed, config(Value::Nil)) {
        Err(ProgramError::Program(m)) => assert!(m.contains("the buffer was freed"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_long_driver_loop_keeps_its_heap_flat() {
    // Each read makes a 4096-number array: 1000 of them are far past the heap cap, so the
    // program runs only if the boundaries collect them.
    let source = r#"
use mod.pod.*
use mod.kernel.*
let inc = {x: output(f32) a: param(0.0) element: fn(i) { x[i] = x[i] + a }}
{run: fn(s, p) {
    let x = s.buffer(f32, 4096)
    var last = nil
    for k in 0..1000 {
        s.run(inc, 4096, {a: 1.0}, {x: x})
        last = s.read(x)
    }
    return last[4095]
}}
"#;
    let limits = Limits { heap_bytes: 16 << 20, ..Limits::default() };
    assert_eq!(run(source, SessionConfig { limits, ..config(Value::Nil) }), Ok(n(1000.0)));
}
