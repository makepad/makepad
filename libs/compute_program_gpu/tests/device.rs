//! Programs on the GPU device ([`spawn`]: its own thread on this machine's GPU): element kernels
//! give the CPU's bits at sizes that take one and several dispatches, shader kernels run as
//! dispatched, buffers fill, copy and cross between the devices, and a cancel frees the device
//! with its passes still queued.

use makepad_compute_program::gpu::GpuLink;
use makepad_compute_program::*;
use makepad_compute_program_gpu::spawn;
use std::sync::OnceLock;
use std::time::Instant;

/// One device for every test of the binary (the process's GPU).
fn gpu() -> GpuLink {
    static GPU: OnceLock<GpuLink> = OnceLock::new();
    GPU.get_or_init(|| spawn().expect("a GPU with compute")).clone()
}

fn run(source: &str, params: Value, gpu: Option<GpuLink>) -> Result<Value, ProgramError> {
    Session::open(&Program::new("test.splash", source), SessionConfig { params, gpu, ..SessionConfig::default() }).wait_end()
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

/// An element kernel on either device: the same source, its buffers made where `p.gpu` says.
const WAVE: &str = r#"
use mod.pod.*
use mod.kernel.*
let wave = {x: input(f32) y: output(f32) a: param(0.0)
            element: fn(i) { y[i] = y[i] * 0.5 + a * x[i] + float((i * 37) % 97) * 0.37 - sin(float(i) * 0.01) }}
{run: fn(s, p) {
    let on = if p.gpu { {device: "gpu"} } else { {device: "cpu"} }
    let xs = s.buffer(f32, p.n)
    s.fill(xs, 0.25)
    let x = s.buffer(f32, p.n, on)
    s.copy(xs, x)
    let y = s.buffer(f32, p.n, on)
    s.fill(y, 1.0)
    for r in 0..3 {
        s.run(wave, p.n, {a: 1.5 + r}, {x: x, y: y})
    }
    return s.read(y, p.n - 5, 5)
}}
"#;

#[test]
fn an_element_kernel_gives_the_cpus_bits() {
    // one dispatch, and many (past the first dispatch's 65536 elements)
    for count in [1000.0, 300_000.0] {
        let params = |gpu: bool| Value::object(&[("n", n(count)), ("gpu", Value::Bool(gpu))]);
        let cpu = run(WAVE, params(false), None).unwrap_or_else(|e| panic!("cpu: {e}"));
        let on_gpu = run(WAVE, params(true), Some(gpu())).unwrap_or_else(|e| panic!("gpu: {e}"));
        let bits = |v: &Value| v.numbers().unwrap().iter().map(|x| (*x as f32).to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&on_gpu), bits(&cpu), "{count} elements: the GPU's bits are the CPU's");
    }
}

/// A shader kernel (a workgroup's shared memory and a barrier: each group reverses its 64
/// values), and the library ops and copies around it.
const REVERSE: &str = r#"
use mod.pod.*
use mod.math.*
use mod.shader
let reverse = {
    label: "reverse"
    workgroup_size: 64
    scale: shader.uniform(f32)
    src: shader.buffer(f32)
    dst: shader.buffer_rw(f32)
    tile: shader.workgroup(f32, 64)
    main: fn() {
        let t = local_index()
        let g = group_id().x * 64u
        self.tile[t] = self.src[g + t]
        barrier()
        self.dst[g + t] = self.tile[63u - t] * self.scale
    }
}
{run: fn(s, p) {
    let gpu = {device: "gpu"}
    let a = s.buffer(f32, 256, gpu)
    let b = s.buffer(f32, 256, gpu)
    s.write(a, [1, 2, 3], 0)
    s.write(a, [5], 255)
    s.dispatch(reverse, 4, {scale: 2.0}, {src: a, dst: b})
    let c = s.buffer(f32, 256, gpu)
    s.copy(b, c)
    s.fill(b, 7.0)
    let back = s.buffer(f32, 256)
    s.copy(c, back)
    let r = s.read(back, 61, 3)
    let tail = s.read(c, 192, 1)
    let filled = s.read(b, 100, 1)
    s.free(a)
    return [r[0], r[1], r[2], tail[0], filled[0], s.device().gpu.buffer_bytes > 0]
}}
"#;

#[test]
fn a_shader_kernel_runs_as_dispatched() {
    let v = run(REVERSE, Value::Nil, Some(gpu())).unwrap_or_else(|e| panic!("{e}"));
    let Value::Array(items) = &v else { panic!("{v:?}") };
    let nums: Vec<f64> = items[..5].iter().map(|x| x.as_number().unwrap()).collect();
    // group 0 reversed: element 63 - t; a[0..3] = 1, 2, 3 land at 63, 62, 61
    assert_eq!(nums, vec![6.0, 4.0, 2.0, 10.0, 7.0]);
    assert_eq!(items[5], Value::Bool(true));
}

const LONG: &str = r#"
use mod.pod.*
use mod.kernel.*
let spin = {y: output(f32) element: fn(i) {
    var v = y[i]
    for k in 0..20000 { v = v * 0.999 + 0.001 }
    y[i] = v
}}
{run: fn(s, p) {
    let y = s.buffer(f32, 1000000, {device: "gpu"})
    for r in 0..20 {
        s.run(spin, 1000000, {}, {y: y})
    }
    s.read(y, 0, 1)
    for r in 0..400 {
        s.run(spin, 1000000, {}, {y: y})
    }
    s.flush()
    s.value("queued", 1)
    return s.read(y, 0, 1)
}}
"#;

#[test]
fn a_cancel_frees_the_device() {
    let wave = || run(WAVE, Value::object(&[("n", n(1000.0)), ("gpu", Value::Bool(true))]), Some(gpu()));
    let first = wave().unwrap_or_else(|e| panic!("{e}"));
    // (420 runs of a million elements a 20000-step loop: past the default work budget)
    let limits = Limits { work: 1 << 62, ..Limits::default() };
    let session = Session::open(&Program::new("long.splash", LONG), SessionConfig { gpu: Some(gpu()), limits, ..SessionConfig::default() });
    // its kernel ran (20 runs), and the device has 400 more (a few hundred ms of GPU work on a
    // large GPU, seconds on a small one)
    loop {
        let u = session.wait();
        if u.value("queued").is_some() {
            break;
        }
        if let Some(end) = u.end {
            panic!("the long program ended before its runs were queued: {end:?}");
        }
    }
    let t = Instant::now();
    session.cancel();
    assert_eq!(session.wait_end(), Err(ProgramError::Cancelled));
    let ended = t.elapsed().as_secs_f64() * 1e3;
    // the device takes what comes next once the passes in flight finish: the rest are gone
    let next = wave().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(next, first);
    eprintln!("cancel: the program ended in {ended:.1} ms, the next one's answer in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
}

/// A fetch on the GPU reads where it stands in the program; the device runs the work sent after
/// it while the program waits for it.
const FETCH: &str = r#"
use mod.pod.*
use mod.kernel.*
let inc = {x: output(f32) a: param(0.0) element: fn(i) { x[i] = x[i] + a }}
{run: fn(s, p) {
    let x = s.buffer(f32, 100000, {device: "gpu"})
    s.fill(x, 1.0)
    let fetches = []
    for r in 0..4 {
        s.run(inc, 100000, {a: 1.0}, {x: x})
        fetches.push(s.fetch(x, 99999, 1))
    }
    s.run(inc, 100000, {a: 100.0}, {x: x})
    let got = []
    for r in 0..4 {
        got.push(s.take(fetches[r])[0])
    }
    got.push(s.read(x, 0, 1)[0])
    return got
}}
"#;

#[test]
fn a_fetch_reads_where_it_stands_while_later_work_runs() {
    let v = run(FETCH, Value::Nil, Some(gpu())).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(v, Value::Array([2.0, 3.0, 4.0, 5.0, 105.0].into_iter().map(n).collect()));
}

#[test]
fn a_gpu_op_without_a_device_is_an_error() {
    let e = run(WAVE, Value::object(&[("n", n(10.0)), ("gpu", Value::Bool(true))]), None).unwrap_err();
    assert!(e.to_string().contains("no GPU device"), "{e}");
}
