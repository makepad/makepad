//! The audio shader suite: every example shader compiles, renders the same
//! bits on every backend and under any host slicing, never allocates on
//! the audio path, and the math and error paths behave.

use makepad_script_audio_aot::{compile_with, AudioShader, AudioShaderRef, Backend, Expression, Instance, Kind, ShaderError, CTX_FRAME};
use makepad_script_audio_aot::{boundary::compile_audio_with, check_with, fuse_with, AudioBindings, Module, SampleView};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The host's buffers for a shader, allocated as a host does (caller-owned).
trait Buffers {
    fn new_ctx(&self, rate: f32) -> Box<[u32]>;
    fn new_state(&self) -> Box<[u32]>;
    fn new_scratch(&self) -> Box<[u32]>;
}

impl Buffers for AudioShader {
    fn new_ctx(&self, rate: f32) -> Box<[u32]> {
        let mut ctx = vec![0u32; self.ctx_words()].into_boxed_slice();
        self.init_ctx(&mut ctx, rate);
        ctx
    }
    fn new_state(&self) -> Box<[u32]> {
        let mut state = vec![0u32; self.state_words()].into_boxed_slice();
        self.reset_state(&mut state);
        state
    }
    fn new_scratch(&self) -> Box<[u32]> {
        vec![0u32; self.scratch_words()].into_boxed_slice()
    }
}

// -- a counting allocator (only counts on the thread that armed it) ----------

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        if ARMED.with(|a| a.get()) {
            ALLOCS.fetch_add(1, Ordering::SeqCst);
        }
        System.realloc(ptr, layout, new)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn counting<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCS.load(Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCS.load(Ordering::SeqCst) - before)
}

// -- helpers -------------------------------------------------------------------

const RATE: f32 = 48000.0;

fn shader_src(name: &str) -> String {
    std::fs::read_to_string(format!("{}/shaders/{}.splash", env!("CARGO_MANIFEST_DIR"), name)).unwrap()
}

const MODULES: &[&str] = &["tone"];

fn with_example_bindings<T>(f: impl FnOnce(&AudioBindings<'_>) -> T) -> T {
    static SOURCES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let sources = SOURCES.get_or_init(|| MODULES.iter().map(|name| shader_src(name)).collect());
    let modules: Vec<_> = MODULES.iter().zip(sources).map(|(path, source)| Module { path, source }).collect();
    f(&AudioBindings { modules: &modules, ..Default::default() })
}

fn build(src: &str, backend: Backend) -> AudioShaderRef {
    match with_example_bindings(|b| compile_audio_with(src, backend, false, b).into_result()) {
        Ok(s) => s,
        Err(errs) => {
            let e = &errs[0];
            let (line, col) = e.line_col(src);
            panic!("compile error at {}:{}: {}\n{}", line, col, e.message, src.lines().nth(line - 1).unwrap_or(""));
        }
    }
}

fn audition_shader(src: &str, backend: Backend, counting: bool) -> AudioShaderRef {
    let shader = with_example_bindings(|b| compile_audio_with(src, backend, counting, b).into_result().unwrap());
    if shader.sample_names().is_empty() {
        return shader;
    }
    let (l, r) = test_input(RATE as usize);
    let data: Vec<_> = l.into_iter().zip(r).flat_map(|(l, r)| [l, r]).collect();
    let samples: Vec<_> = shader.sample_names().iter().map(|name| SampleView { name, channels: 2, frames: RATE as u32, data: &data }).collect();
    with_example_bindings(|b| compile_audio_with(src, backend, counting, &AudioBindings { samples: &samples, ..*b }).into_result().unwrap())
}

const INSTRUMENTS: &[&str] = &["saw_svf", "fm4", "pluck", "wavetable_pad", "fm2_svf", "expressive_lead", "grain_cloud"];
const EFFECTS: &[&str] = &["allpass_reverb", "waveshaper4x", "tempo_tremolo", "scattered_noise", "tone_drive", "channel_strip"];

/// A deterministic test input for effects: a decaying chirp plus clicks.
/// A sine of `turns` from IEEE +, -, * and floor only (no libm), so the test input is
/// the same bits on every host.
fn poly_sin(turns: f32) -> f32 {
    // x in [-0.5, 0.5) turns, then sin(2 pi x) by an odd polynomial on [-0.25, 0.25]
    let x = turns - (turns + 0.5).floor();
    let x = if x > 0.25 { 0.5 - x } else if x < -0.25 { -0.5 - x } else { x };
    let x2 = x * x;
    x * (6.283_185 + x2 * (-41.341_7 + x2 * (81.605_25 + x2 * (-76.705_86 + x2 * 42.058_69))))
}

/// exp(-x) for x in [0, 3] from IEEE +, * only (a truncated series of exp(-x/8), cubed
/// three times).
fn poly_exp_neg(x: f32) -> f32 {
    let y = x * 0.125;
    let e = 1.0 - y + y * y * 0.5 - y * y * y * (1.0 / 6.0) + y * y * y * y * (1.0 / 24.0);
    let e2 = e * e;
    let e4 = e2 * e2;
    e4 * e4
}

fn test_input(n: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0; n];
    let mut r = vec![0.0; n];
    for i in 0..n {
        let t = i as f32 / RATE;
        let env = poly_exp_neg((t % 0.5) * 6.0);
        l[i] = poly_sin(t * 220.0 * (1.0 + t)) * env * 0.7;
        r[i] = if i % 9000 == 0 { 0.9 } else { l[i] * 0.5 };
    }
    (l, r)
}

/// Renders a scripted performance: two notes, a param change, releases.
/// `slices` gives the host's call sizes (cycled).
fn perform(shader: &AudioShaderRef, interp: bool, slices: &[usize], total: usize) -> (Vec<f32>, Vec<f32>) {
    let mut scratch = shader.new_scratch();
    perform_with(shader, slices, total, &mut |ctx, state, ins, outs, n| {
        if interp {
            let [o0, o1] = outs;
            shader.run_interp(ctx, state, &mut scratch, ins[0], ins[1], o0, o1, n);
        } else {
            let [o0, o1] = outs;
            shader.run(ctx, state, &mut scratch, ins[0], ins[1], o0, o1, n);
        }
    })
}

/// One host call: (ctx, state, ins, outs, n).
type Run<'a> = dyn FnMut(&mut [u32], &mut [u32], [&[f32]; 2], [&mut [f32]; 2], usize) + 'a;

/// [`perform`] through any runner of the shader's calls.
fn perform_with(shader: &AudioShaderRef, slices: &[usize], total: usize, run: &mut Run) -> (Vec<f32>, Vec<f32>) {
    perform_events(shader, slices, total, &[0, 12345, 24000, 30000, 52000], |frame, ctx, state| match frame {
        0 => shader.note_on(state, 57.0, 0.9, 0, 7, 0),
        12345 => {
            if let Some(p) = shader.params().first() {
                shader.set_param(ctx, 0, p.min + (p.max - p.min) * 0.3);
            }
            for index in 0..shader.param_count() {
                if !shader.param_meta(index).choices.is_empty() {
                    shader.set_param(ctx, index, shader.param_view(index).max);
                }
            }
        }
        30000 => shader.note_on(state, 64.5, 0.6, frame as u32, 11, 0),
        _ => shader.note_off(state, 0.5),
    }, run)
}

fn perform_events(
    shader: &AudioShaderRef, slices: &[usize], total: usize, mut events: &[usize],
    mut apply: impl FnMut(usize, &mut [u32], &mut [u32]), run: &mut Run,
) -> (Vec<f32>, Vec<f32>) {
    let mut ctx = shader.new_ctx(RATE);
    let mut state = shader.new_state();
    let mut out_l = vec![0.0f32; total];
    let mut out_r = vec![0.0f32; total];
    let (in_l, in_r) = test_input(total);
    let zeros = vec![0.0f32; total];
    let mut frame = 0usize;
    let mut k = 0;
    while frame < total {
        if events.first() == Some(&frame) {
            apply(frame, &mut ctx, &mut state);
            events = &events[1..];
        }
        let next_event = events.first().copied().unwrap_or(total);
        let n = slices[k % slices.len()].min(next_event - frame).min(total - frame);
        k += 1;
        ctx[CTX_FRAME as usize] = frame as u32;
        let (ins_l, ins_r): (&[f32], &[f32]) = if shader.kind == Kind::Effect {
            (&in_l[frame..frame + n], &in_r[frame..frame + n])
        } else {
            (&zeros[frame..frame + n], &zeros[frame..frame + n])
        };
        let (ol, or) = (&mut out_l[frame..frame + n], &mut out_r[frame..frame + n]);
        run(&mut ctx, &mut state, [ins_l, ins_r], [ol, or], n);
        frame += n;
    }
    (out_l, out_r)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

fn first_diff(a: &[f32], b: &[f32]) -> Option<(usize, f32, f32)> {
    a.iter().zip(b).enumerate().find(|(_, (x, y))| x.to_bits() != y.to_bits()).map(|(i, (x, y))| (i, *x, *y))
}

// -- tests -------------------------------------------------------------------

#[test]
fn examples_compile_and_make_sound() {
    for name in INSTRUMENTS.iter().chain(EFFECTS) {
        let src = shader_src(name);
        let s = audition_shader(&src, Backend::Native, false);
        let (l, r) = perform(&s, false, &[128], 60000);
        let peak = l.iter().chain(&r).fold(0.0f32, |m, x| m.max(x.abs()));
        let finite = l.iter().chain(&r).all(|x| x.is_finite());
        assert!(finite, "{} produced non-finite samples", name);
        assert!(peak > 0.01 && peak < 4.0, "{} peak {}", name, peak);
        eprintln!(
            "{:16} {:?} peak {:.3} state {} words, {} AIR vals, {} bytes native",
            name,
            s.backend(),
            peak,
            s.state_words(),
            s.program().vals.len(),
            s.native_code_bytes()
        );
        // The audition a writer (or the AI) runs makes sound too; its numbers.
        let report = with_example_bindings(|b| check_with(&src, b)).unwrap_or_else(|e| panic!("{} does not check: {:?}", name, e));
        assert!(report.peak > 0.01 && report.rms.is_finite(), "{} check peak {} rms {}", name, report.peak, report.rms);
        eprintln!("{:16} check peak {:.3} rms {:.4} {:.1} ops/frame latency {} frames {:?}", name, report.peak, report.rms, report.ops_per_frame, report.latency, report.warnings);
        assert!(report.warnings.is_empty(), "{} checks with warnings: {:?}", name, report.warnings);
    }
}

#[test]
fn renders_are_deterministic_across_compiles() {
    for name in INSTRUMENTS {
        let a = audition_shader(&shader_src(name), Backend::Native, false);
        let b = audition_shader(&shader_src(name), Backend::Native, false);
        assert_eq!(bits(&perform(&a, false, &[128], 20000).0), bits(&perform(&b, false, &[128], 20000).0), "{}", name);
    }
}

#[test]
fn the_audio_path_never_allocates() {
    for name in INSTRUMENTS.iter().chain(EFFECTS) {
        for backend in [Backend::Native, Backend::Interp] {
            let s = audition_shader(&shader_src(name), backend, false);
            let mut inst = Instance::new(s.clone(), RATE);
            let mut l = vec![0.0f32; 128];
            let mut r = vec![0.0f32; 128];
            let (in_l, in_r) = test_input(128);
            let (_, allocs) = counting(|| {
                inst.note_on(60.0, 1.0, 1);
                for block in 0..200 {
                    if block == 100 {
                        inst.note_off(0.5);
                    }
                    inst.set_param("mix", 0.5);
                    if s.kind == Kind::Effect {
                        inst.process(&in_l, &in_r, &mut l, &mut r);
                    } else {
                        inst.render(&mut l, &mut r);
                    }
                }
            });
            assert_eq!(allocs, 0, "{} ({:?}) allocated on the audio path", name, backend);
        }
    }
}

#[test]
fn native_code_runs_on_another_thread() {
    let s = build(&shader_src("fm2_svf"), Backend::Native);
    let here = perform(&s, false, &[128], 8000);
    let s2 = s.clone();
    let there = std::thread::spawn(move || perform(&s2, false, &[128], 8000)).join().unwrap();
    assert_eq!(bits(&here.0), bits(&there.0));
}

/// An effect that outputs f(l) on the left, for checking math intrinsics.
fn math_probe(expr: &str) -> AudioShaderRef {
    build(&format!("fn effect(l, r) {{ let x = l\n vec2({}, 0.0) }}", expr), Backend::Native)
}

fn probe(s: &AudioShaderRef, xs: &[f32], interp: bool) -> Vec<f32> {
    let mut ctx = s.new_ctx(RATE);
    let mut state = s.new_state();
    let mut scratch = s.new_scratch();
    let mut out = vec![0.0f32; xs.len()];
    let mut other = vec![0.0f32; xs.len()];
    let zeros = vec![0.0f32; xs.len()];
    for (k, chunk) in xs.chunks(4096).enumerate() {
        let at = k * 4096;
        let n = chunk.len();
        if interp {
            s.run_interp(&mut ctx, &mut state, &mut scratch, chunk, &zeros[..n], &mut out[at..at + n], &mut other[at..at + n], n);
        } else {
            s.run(&mut ctx, &mut state, &mut scratch, chunk, &zeros[..n], &mut out[at..at + n], &mut other[at..at + n], n);
        }
    }
    out
}

#[test]
fn math_is_accurate_and_backend_exact() {
    let mut xs: Vec<f32> = (0..20000).map(|i| (i as f32 - 10000.0) * 0.00173).collect();
    xs.extend([0.0, -0.0, 1e-30, -1e-30, 88.0, -87.0, 0.5, 1.0, 100.0, -100.0, f32::MAX, f32::MIN_POSITIVE]);
    let cases: &[(&str, fn(f64) -> f64, f64, &dyn Fn(f32) -> bool)] = &[
        ("sin(x)", f64::sin, 3e-7, &|x: f32| x.abs() < 400.0),
        ("cos(x)", f64::cos, 3e-7, &|x: f32| x.abs() < 400.0),
        ("tanh(x)", f64::tanh, 4e-7, &|_| true),
        ("exp(x)", f64::exp, 3e-7, &|x: f32| x.abs() < 80.0),
        ("log(x)", f64::ln, 3e-7, &|x: f32| x > 0.0 && x < 1e30),
        ("sqrt(x)", f64::sqrt, 1e-7, &|x: f32| x >= 0.0),
    ];
    for (expr, reference, tol, domain) in cases {
        let s = math_probe(expr);
        let native = probe(&s, &xs, false);
        let interp = probe(&s, &xs, true);
        assert_eq!(bits(&native), bits(&interp), "{}: backends differ", expr);
        let mut worst = 0.0f64;
        for (x, y) in xs.iter().zip(&native) {
            if !domain(*x) {
                continue;
            }
            let want = reference(*x as f64);
            let err = (*y as f64 - want).abs() / want.abs().max(1.0);
            worst = worst.max(err);
        }
        eprintln!("{:8} worst relative error {:.2e}", expr, worst);
        assert!(worst < *tol, "{}: worst error {:e}", expr, worst);
    }
}

#[test]
fn integer_and_edge_ops_match() {
    // Division by zero, i32::MIN / -1, shifts past 31, saturating casts,
    // NaN compares, wrapping indices.
    let src = r#"
        var buf = [0.0; 7]
        var k = int(0)
        fn effect(l, r) {
            let i = int(l * 1000.0)
            let j = int(r * 3.0) - 1
            let a = i / j + i % j + (i << (j + 40)) + (i >> 33)
            let big = int(l * 1e12)
            let m = (i32_min() / -1) + big
            buf[k * 5 - 3] = l
            k = k + 1
            let nan = sqrt(-1.0 - abs(l))
            let c = if nan < 1.0 { 1.0 } elif nan != nan { 2.0 } else { 3.0 }
            let rd = round(l * 10.5) + trunc(-l * 3.3) + ceil(l) + floor(-l)
            vec2(float(a) + float(m & 255) + c + rd + buf[k - 2], min(l, nan) + max(nan, r))
        }
        fn i32_min() { -2147483647 - 1 }
    "#;
    let s = build(src, Backend::Native);
    let n = 5000;
    let mut ctx = s.new_ctx(RATE);
    let mut st_a = s.new_state();
    let mut st_b = s.new_state();
    let mut scratch = s.new_scratch();
    let l: Vec<f32> = (0..n).map(|i| ((i * 7919) % 2001) as f32 / 1000.0 - 1.0).collect();
    let r: Vec<f32> = (0..n).map(|i| ((i * 104729) % 1001) as f32 / 1000.0).collect();
    let (mut a0, mut a1) = (vec![0.0; n], vec![0.0; n]);
    let (mut b0, mut b1) = (vec![0.0; n], vec![0.0; n]);
    s.run(&mut ctx, &mut st_a, &mut scratch, &l, &r, &mut a0, &mut a1, n);
    s.run_interp(&mut ctx, &mut st_b, &mut scratch, &l, &r, &mut b0, &mut b1, n);
    assert_eq!(bits(&a0), bits(&b0));
    assert_eq!(bits(&a1), bits(&b1));
    assert_eq!(st_a, st_b);
}

#[test]
fn register_pressure_spills_exactly() {
    // 48 values live at once force spills of both register classes.
    let mut src = String::from("var acc = 0.0\nvar n = int(0)\nfn effect(l, r) {\n");
    for i in 0..48 {
        src.push_str(&format!("    let f{} = l * {}.5 + r\n    let i{} = n * {} + {}\n", i, i, i, i + 1, i));
    }
    src.push_str("    let s = 0.0\n");
    for i in 0..48 {
        src.push_str(&format!("    s = s + f{} * float(i{} % 7)\n", 47 - i, i));
    }
    src.push_str("    n = n + 1\n    acc = acc * 0.5 + s\n    vec2(acc, s)\n}\n");
    let s = build(&src, Backend::Native);
    let (l, r) = test_input(3000);
    let mut ctx = s.new_ctx(RATE);
    let (mut sa, mut sb) = (s.new_state(), s.new_state());
    let mut scratch = s.new_scratch();
    let (mut a0, mut a1) = (vec![0.0; 3000], vec![0.0; 3000]);
    let (mut b0, mut b1) = (vec![0.0; 3000], vec![0.0; 3000]);
    s.run(&mut ctx, &mut sa, &mut scratch, &l, &r, &mut a0, &mut a1, 3000);
    s.run_interp(&mut ctx, &mut sb, &mut scratch, &l, &r, &mut b0, &mut b1, 3000);
    assert_eq!(bits(&a0), bits(&b0));
    assert_eq!(bits(&a1), bits(&b1));
}

#[test]
fn control_flow_matches() {
    // Loops with runtime bounds, break/continue, early returns from helpers,
    // match, while, short-circuit with side effects.
    let src = r#"
        var hits = int(0)
        var st = [0.0; 16]
        fn bump() { hits = hits + 1
            true }
        fn find(x) {
            for i in 0..16 {
                if st[i] > x { return float(i) }
            }
            -1.0
        }
        fn effect(l, r) {
            let n = int(abs(l) * 20.0)
            let s = 0.0
            for i in 0..n {
                if i % 3 == 1 { continue }
                if s > 4.0 { break }
                s = s + float(i) * 0.25
            }
            let w = 0
            while w * w < n { w = w + 1 }
            let q = match w % 4 { 0 => 1.5, 1 | 2 => s, _ => -s }
            if l > 0.0 && bump() { st[hits] = l }
            if l < -0.5 || bump() { s = s + 1.0 }
            loop { s = s * 0.5
                if s < 1.0 { break } }
            vec2(s + q + find(l), float(hits))
        }
    "#;
    let s = build(src, Backend::Native);
    let (l, r) = test_input(6000);
    let mut ctx = s.new_ctx(RATE);
    let (mut sa, mut sb) = (s.new_state(), s.new_state());
    let mut scratch = s.new_scratch();
    let (mut a0, mut a1) = (vec![0.0; 6000], vec![0.0; 6000]);
    let (mut b0, mut b1) = (vec![0.0; 6000], vec![0.0; 6000]);
    s.run(&mut ctx, &mut sa, &mut scratch, &l, &r, &mut a0, &mut a1, 4096);
    s.run_interp(&mut ctx, &mut sb, &mut scratch, &l, &r, &mut b0, &mut b1, 4096);
    assert_eq!(bits(&a0[..4096]), bits(&b0[..4096]));
    assert_eq!(bits(&a1[..4096]), bits(&b1[..4096]));
    assert!(a1[4095] > 100.0, "side effects of && / || ran: {}", a1[4095]);
}

fn compile_err(src: &str) -> ShaderError {
    match compile_with(src, Backend::Interp) {
        Ok(_) => panic!("expected an error for:\n{}", src),
        Err(e) => e[0].clone(),
    }
}

#[test]
fn param_errors_point_to_the_offending_field() {
    for (declaration, field, message) in [
        ("param(0, {scale: log})", "scale", "unit, curve, step, label, group, choices, smooth"),
        ("param(0, 0, 1, {curve: log})", "curve: log", "log curve needs min > 0"),
        ("param(0, 0, 1, {choices: [a b]})", "choices: [a b]", "cannot be given with min and max"),
        ("param(0, {unit: cycles})", "unit: cycles", "unknown unit `cycles`"),
        ("param(soft, {choices: [a b]})", "soft", "unknown default choice `soft`"),
        ("param(2, {choices: [a b]})", "2", "index from 0 to 1"),
        ("param(0, {choices: [a]})", "choices: [a]", "at least two"),
        ("param(0, {step: 0})", "step: 0", "step needs a constant number > 0"),
        ("param(0, {smooth: -1})", "smooth: -1", "smooth needs a constant number >= 0"),
        ("param(0, {unit: hz unit: db})", "unit: db", "field `unit` is given twice"),
    ] {
        let src = format!("let knob = {declaration}\nfn voice() {{ knob }}");
        let error = compile_err(&src);
        assert!(error.message.contains(message), "{declaration} gave the wrong diagnostic: {}", error.message);
        assert_eq!(&src[error.start..error.end], field, "{declaration} highlighted the wrong field");
    }
}

#[test]
fn errors_point_at_the_code() {
    for (prefix, decl, message) in [
        ("", "latency()", "one argument"),
        ("", "latency(2, 3)", "one argument"),
        ("latency(0)", "latency(1)", "declared twice"),
        ("", "latency(-1)", "whole number of frames"),
        ("", "latency(5 / 2)", "whole number of frames"),
        ("", "latency(4294967296)", "whole number of frames"),
        ("let knob = param(1)", "latency(knob)", "constant frame count"),
        ("var n = 4", "latency(n * 0)", "constant frame count"),
        ("", "latency(sample_rate)", "constant frame count"),
    ] {
        let src = format!("{prefix}\n{decl}\nfn voice() {{ 1 }}");
        let error = compile_err(&src);
        assert_eq!(&src[error.start..error.end], decl, "{decl} diagnostic span");
        assert!(error.message.contains(message), "{decl} gave {}", error.message);
    }
    let cases: &[(&str, &str, &str)] = &[
        ("foo()\nfn voice() { 1 }", "foo()", "unknown declaration `foo()`"),
        ("mono(1)\nfn voice() { 1 }", "mono(1)", "no arguments"),
        ("mono()\nmono()\nfn voice() { 1 }", "mono()", "twice"),
        ("mono()\nfn effect(l,r) { l }", "mono()", "for instruments"),
        ("fn voice() { foo + 1.0 }", "foo", "unknown name"),
        ("fn voice() { let x = 1.0\n x = \"s\" }", "\"s", "no strings"),
        ("fn voice() { f(1.0) }\nfn f(x) { f(x) }", "f(x)", "itself"),
        ("var b = [0.0; n]\nfn voice() { 0.0 }", "n]", "unknown name"),
        ("fn voice() { break }", "break", "outside a loop"),
        ("fn voice() { cc(pressure) }", "cc(", "0..127"),
        ("fn voice() { cc(1.5) }", "cc(", "0..127"),
        ("fn voice() { cc(-1) }", "cc(", "0..127"),
        ("fn voice() { cc(128) }", "cc(", "0..127"),
        ("var x = pressure\nfn voice() { x }", "pressure", "is an audio input"),
        ("var x = cc(1)\nfn voice() { x }", "cc(", "is an audio input"),
        ("fn nothing() { 1.0 }", "f", "missing entry"),
        ("fn voice() { vec2(1.0, 2.0) + true }", "vec2", "vec2"),
        ("let t = [0.0; 4]\nfn voice() { t[0] = 1.0\n 0.0 }", "t[0] = 1.0", "read-only"),
    ];
    for (src, at, msg) in cases {
        let e = compile_err(src);
        assert!(e.message.contains(msg), "{:?}: message {:?} lacks {:?}", src, e.message, msg);
        let start = src.rfind(at).unwrap();
        if *msg != "missing entry" {
            assert_eq!(e.start, start, "{:?}: error at {} ({:?}), expected {}", src, e.start, &src[e.start..e.end.min(src.len())], start);
        }
    }
}

#[test]
fn shader_reports_its_declared_latency_in_frames() {
    for (decl, frames) in [
        ("", 0),
        ("latency(0)", 0),
        ("latency(4294967295)", u32::MAX),
        ("let ahead = 37\nlatency(ahead)", 37),
    ] {
        let voice = build(&format!("{decl}\nfn voice() {{ 1 }}"), Backend::Interp);
        assert_eq!(voice.latency(), frames, "instrument with {decl:?}");
    }
    let effect = "latency(len(t) * stride + 1)\nlet stride: i32 = 2\nlet t = sample(2)\nfn effect(l,r) { vec2(l,r) }";
    let table = SampleView { name: "t", channels: 2, frames: 3, data: &[0.0; 6] };
    assert_eq!(bound_shader(effect, &[table]).latency(), 7, "bound stereo table counts frames");
    assert_eq!(bound_shader(effect, &[]).latency(), 3, "unbound table has one frame");
    let frames = [Module { path: "frames", source: "let LOOK = 37" }];
    let imported = compile_audio_with("use frames\nlatency(frames.LOOK)\nfn effect(l,r) { vec2(l,r) }", Backend::Interp, false, &AudioBindings { modules: &frames, ..Default::default() });
    assert_eq!(imported.into_result().map(|s| s.latency()).ok(), Some(37), "a module constant counts in a declaration");
}

#[test]
fn hot_swap_state_carry_by_name() {
    let a = build("var phase = 0.0\nvar amp = 1.0\nfn voice() { phase = fract(phase + freq / sample_rate)\n sin(TAU * phase) * amp }", Backend::Native);
    let b = build("var amp = 1.0\nvar phase = 0.0\nvar extra = 5.0\nfn voice() { phase = fract(phase + freq / sample_rate)\n sin(TAU * phase) * amp * 0.5 }", Backend::Native);
    // Same names: phase and amp map across the reordered layout.
    let va = &a.state_vars()[0];
    let vb = b.state_vars().iter().find(|v| v.name == va.name).unwrap();
    assert_eq!(va.sig, vb.sig);
    assert_ne!(va.offset, vb.offset);
}

/// Fundamental by autocorrelation with parabolic peak interpolation.
fn pitch(x: &[f32], rate: f32, lo_hz: f32, hi_hz: f32) -> f32 {
    let min_lag = (rate / hi_hz) as usize;
    let max_lag = (rate / lo_hz) as usize;
    let ac = |lag: usize| -> f64 { x.iter().zip(&x[lag..]).map(|(a, b)| *a as f64 * *b as f64).sum::<f64>() / (x.len() - lag) as f64 };
    let vals: Vec<f64> = (min_lag..=max_lag + 1).map(ac).collect();
    // The first strong peak (avoids octave errors).
    let top = vals.iter().cloned().fold(f64::MIN, f64::max);
    let mut best = 1;
    for k in 1..vals.len() - 1 {
        if vals[k] > vals[k - 1] && vals[k] >= vals[k + 1] && vals[k] > 0.9 * top {
            best = k;
            break;
        }
    }
    let (a, b, c) = (vals[best - 1], vals[best], vals[best + 1]);
    let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
    rate / ((min_lag + best) as f64 + shift) as f32
}

#[test]
fn pluck_stays_in_tune_up_high() {
    let s = build(&shader_src("pluck"), Backend::Native);
    for note in [48.0f32, 60.0, 72.0, 84.0, 93.0] {
        let mut inst = Instance::new(s.clone(), RATE);
        inst.set_param("t60", 10.0);
        inst.note_on(note, 1.0, 5);
        let mut l = vec![0.0f32; 24000];
        let mut r = vec![0.0f32; 24000];
        inst.render(&mut l, &mut r);
        let want = 440.0 * 2f32.powf((note - 69.0) / 12.0);
        let got = pitch(&l[4000..20000], RATE, want * 0.7, want * 1.4);
        let cents = 1200.0 * (got / want).log2();
        eprintln!("pluck note {:>4}: want {:8.2} Hz got {:8.2} Hz ({:+.2} cents)", note, want, got, cents);
        assert!(cents.abs() < 3.0, "note {} is {:+.1} cents off", note, cents);
    }
}

// -- dataflow fusion -----------------------------------------------------------

use makepad_script_audio_aot::{fuse, FuseNode, Port};

#[test]
fn parallel_paths_align_in_frames() {
    use makepad_script_audio_aot::{align, AlignEdge, AlignError, AlignNode, EdgeTiming, NodeTiming};

    let (long_out, mix, wet, dry, source, first, second, short_out, short) = (0, 1, 2, 3, 4, 5, 6, 7, 8);
    let mut nodes = [3, 0, 37, 0, 0, 5, 7, 0, 10].map(|latency| AlignNode { latency, source: false });
    nodes[source].source = true;
    let edge = |from, to| AlignEdge { from, to, deliberate: 0 };
    let mut edges = vec![
        edge(dry, mix), edge(wet, mix), edge(second, source), edge(mix, long_out),
        edge(source, wet), edge(short, short_out), edge(first, second), edge(source, dry),
    ];
    let outputs = [short_out, long_out];
    let base = align(&nodes, &edges, &outputs).unwrap();
    assert_eq!(base.edges[0], EdgeTiming { delay: 37, lateness: 0, aligned: true }, "dry path must wait for the wet path");
    assert_eq!(base.edges[1].delay, 0, "the longest mixer input is already on time");
    assert_eq!(base.nodes[second], NodeTiming { l_in: 5, l_out: 12 }, "serial node latencies must add");
    assert_eq!(base.nodes[source].l_in, 0, "a source starts at frame zero");
    assert_eq!(base.edges[2], EdgeTiming { delay: 0, lateness: 12, aligned: false }, "a source input reports its sender's lateness");
    assert_eq!((base.nodes[short_out].l_in, base.nodes[long_out].l_in), (10, 37), "outputs inherit their input path lengths");
    assert_eq!(base.total, 37, "total uses the latest device input");
    assert_eq!(base.pads, [27, 0], "pads follow the requested device output order");

    edges[0].deliberate = 100;
    let moved = align(&nodes, &edges, &outputs).unwrap();
    assert_eq!(moved.edges[0].delay, base.edges[0].delay + 100, "deliberate frames add to the aligned edge delay");
    assert_eq!(moved.edges[1..], base.edges[1..], "a deliberate delay moves only its own edge");
    assert_eq!(moved.nodes, base.nodes, "a deliberate delay leaves every node's path length intact");
    assert_eq!((moved.total, moved.pads), (base.total, base.pads), "a deliberate delay leaves device timing intact");

    edges.push(edge(wet, source));
    assert!(matches!(align(&nodes, &edges, &outputs), Err(AlignError::Cycle(n)) if n == wet || n == source),
        "the cycle error must identify a node on the source/wet cycle");
}

#[test]
fn fused_instrument_chain_equals_the_nodes_run_one_by_one() {
    let synth = format!("mono()\n{}", shader_src("saw_svf"));
    let drive = shader_src("tone_drive");
    let fused = with_example_bindings(|b| fuse_with(
        &[FuseNode { name: "lead", code: &synth, inputs: vec![] }, FuseNode { name: "drive", code: &drive, inputs: vec![Port::Node(0)] }],
        &[Port::Node(1)],
        Backend::Native,
        b,
    ))
    .unwrap_or_else(|e| panic!("{:?}", e));
    assert!(fused.mono());
    assert!(["lead_cutoff", "drive_drive"].iter().all(|name| fused.params().iter().any(|p| p.name == *name)));
    let n = 20000;
    // Fused: one program per voice.
    let mut f = Instance::new(fused.clone(), RATE);
    f.note_on(45.0, 1.0, 9);
    let (mut fl, mut fr) = (vec![0.0; n], vec![0.0; n]);
    for k in 0..n / 100 {
        f.render(&mut fl[k * 100..k * 100 + 100], &mut fr[k * 100..k * 100 + 100]);
    }
    // Separate: the synth into a buffer, the buffer through the effect.
    let mut a = Instance::new(build(&synth, Backend::Native), RATE);
    let mut b = Instance::new(build(&drive, Backend::Native), RATE);
    a.note_on(45.0, 1.0, 9);
    let (mut al, mut ar) = (vec![0.0; n], vec![0.0; n]);
    a.render(&mut al, &mut ar);
    let (mut bl, mut br) = (vec![0.0; n], vec![0.0; n]);
    b.process(&al, &ar, &mut bl, &mut br);
    if let Some(d) = first_diff(&fl, &bl) {
        panic!("fused and chained differ at {}: {} vs {}", d.0, d.1, d.2);
    }
    assert_eq!(bits(&fr), bits(&br));
    eprintln!("fused lead+drive: {} AIR vals, {} bytes native (vs {} + {})", fused.program().vals.len(), fused.native_code_bytes(),
        a.shader.native_code_bytes(), b.shader.native_code_bytes());
}

#[test]
fn fused_effect_graph_with_fan_in_and_feedback() {
    // y = half(x + y[n-1]) as a graph with a feedback edge, against the
    // same thing written by hand.
    let half = "fn effect(l, r) { vec2(l * 0.5, r * 0.5) }";
    let graph = fuse(&[FuseNode { name: "h", code: half, inputs: vec![Port::Input, Port::Feedback(0)] }], &[Port::Node(0)], Backend::Native).unwrap();
    let by_hand = build("var y = vec2(0.0)\nfn effect(l, r) { y = vec2((l + y.x) * 0.5, (r + y.y) * 0.5)\n y }", Backend::Native);
    let (l, r) = test_input(5000);
    let mut out = [(vec![0.0; 5000], vec![0.0; 5000]), (vec![0.0; 5000], vec![0.0; 5000])];
    for (s, o) in [graph.clone(), by_hand].iter().zip(out.iter_mut()) {
        let mut i = Instance::new(s.clone(), RATE);
        i.process(&l, &r, &mut o.0, &mut o.1);
    }
    assert_eq!(bits(&out[0].0), bits(&out[1].0));
    // Fan-out and fan-in: reverb and drive in parallel on the input, summed.
    let verb = shader_src("allpass_reverb");
    let drive = shader_src("tone_drive");
    let par = with_example_bindings(|b| fuse_with(
        &[
            FuseNode { name: "verb", code: &verb, inputs: vec![Port::Input] },
            FuseNode { name: "drive", code: &drive, inputs: vec![Port::Input] },
        ],
        &[Port::Node(0), Port::Node(1)],
        Backend::Native,
        b,
    ))
    .unwrap();
    let mut i = Instance::new(par.clone(), RATE);
    let (mut pl, mut pr) = (vec![0.0; 5000], vec![0.0; 5000]);
    i.process(&l, &r, &mut pl, &mut pr);
    let mut v = Instance::new(build(&verb, Backend::Native), RATE);
    let mut d = Instance::new(build(&drive, Backend::Native), RATE);
    let (mut sl, mut sr) = (vec![0.0; 5000], vec![0.0; 5000]);
    let (mut vl, mut vr) = (vec![0.0; 5000], vec![0.0; 5000]);
    let (mut dl, mut dr) = (vec![0.0; 5000], vec![0.0; 5000]);
    v.process(&l, &r, &mut vl, &mut vr);
    d.process(&l, &r, &mut dl, &mut dr);
    for k in 0..5000 {
        sl[k] = vl[k] + dl[k];
        sr[k] = vr[k] + dr[k];
    }
    assert_eq!(bits(&pl), bits(&sl));
    assert_eq!(bits(&pr), bits(&sr));
}

#[test]
fn fusion_errors_name_the_node() {
    let good = "fn effect(l, r) { vec2(l, r) }";
    for (voice, effect, node) in [
        ("latency(0)\nfn voice() { 1 }", good, 0),
        ("fn voice() { 1 }", "fn effect(l,r) { l }\nlatency(0)", 1),
    ] {
        let nodes = [
            FuseNode { name: "lead", code: voice, inputs: vec![] },
            FuseNode { name: "fx", code: effect, inputs: vec![Port::Node(0)] },
        ];
        let e = fuse(&nodes, &[Port::Node(1)], Backend::Interp).unwrap_err();
        assert_eq!(e.node, Some(node));
        assert_eq!(&nodes[node].code[e.error.start..e.error.end], "latency(0)");
        assert_eq!(e.error.message, "`latency()` cannot be fused yet");
    }
    let bad = "fn effect(l, r) { vec2(l, nope) }";
    let e = fuse(&[FuseNode { name: "a", code: good, inputs: vec![Port::Input] }, FuseNode { name: "b", code: bad, inputs: vec![Port::Node(0)] }], &[Port::Node(1)], Backend::Interp).unwrap_err();
    assert_eq!(e.node, Some(1));
    assert_eq!(&bad[e.error.start..e.error.end], "nope");
    let e = fuse(&[FuseNode { name: "a", code: good, inputs: vec![Port::Node(0)] }], &[Port::Node(0)], Backend::Interp).unwrap_err();
    assert!(e.error.message.contains("not before it"));
}

#[test]
fn shared_modules_render_inline_and_fuse_in_each_nodes_namespace() {
    let helper = "fn soft(x) { probe(shaped, x)\n x / (1.0 + abs(x)) }";
    let modules = [Module { path: "tone", source: helper }];
    let bindings = AudioBindings { modules: &modules, ..Default::default() };
    let effect = "fn effect(l, r) { vec2(soft(l), soft(r)) }";
    let qualified = format!("use tone\n{}", effect.replace("soft(", "tone.soft("));
    let glob = format!("use tone.*\n{effect}");
    let compile = |src: &str| compile_audio_with(src, Backend::Native, false, &bindings).into_result();
    let inline = compile(&format!("{helper}\n{effect}")).unwrap();
    let stereo_bits = |(left, right): (Vec<f32>, Vec<f32>)| (bits(&left), bits(&right));
    let reference = stereo_bits(perform(&inline, true, &[17], 67));
    for src in [&qualified, &glob] {
        let shader = compile(src).unwrap();
        assert_eq!(stereo_bits(perform(&shader, true, &[17], 67)), reference, "interpreted {src}");
        assert_eq!(stereo_bits(perform(&shader, false, &[17], 67)), reference, "native {src}");
        let mut wasm = WasmRunner::new(&shader);
        let output = perform_with(&shader, &[17], 67, &mut |ctx, state, inputs, outputs, n| wasm.run(ctx, state, inputs, outputs, n));
        assert_eq!(stereo_bits(output), reference, "linked {src}");
    }
    let shadow = compile(&format!("use tone.*\nfn soft(x) {{ 0.25 }}\n{effect}")).unwrap();
    assert_eq!(perform(&shadow, false, &[3], 3), (vec![0.25; 3], vec![0.25; 3]), "local definition wins");
    let duplicate = format!("use tone.*\n{helper}\n{helper}\n{effect}");
    assert!(compile(&duplicate).unwrap_err()[0].message.contains("defined twice"), "both local declarations reach validation");
    let library = [Module { path: "lib:tone@2", source: helper }];
    let revised = format!("use lib(\"tone\", \"2\") as m\n{}", effect.replace("soft(", "m.soft("));
    let shader = compile_audio_with(&revised, Backend::Native, false, &AudioBindings { modules: &library, ..bindings }).into_result().unwrap();
    assert_eq!(stereo_bits(perform(&shader, false, &[17], 67)), reference, "revision and alias");
    let standard = compile("use std.shared as s\nfn voice() { s.knob(0.5) }").unwrap();
    assert_eq!(perform(&standard, true, &[1], 1).0, [1.0]);
    let unsupported = "use std.shared as s\nfn voice() { s.aa_step(0.0, 1.0) }";
    let error = compile(unsupported).unwrap_err().remove(0);
    assert_eq!(error.start, unsupported.find("s.aa_step").unwrap(), "unavailable builtin at the call");

    let src = format!("// effect\n{qualified}");
    let at = src.find("use tone").unwrap();
    let declarations = [Module { path: "tone", source: "mono()\nfn level() { 1 }" }];
    for (decl, name) in [("mono()", "mono"), ("latency(16)", "latency")] {
        let kernel = format!("{decl}\nlet out = output(f32)\nfn element(i) {{ out[i] = 1 }}");
        let errors = makepad_script_compute::kernel::compile(&kernel).err().unwrap();
        assert_eq!(&kernel[errors[0].start..errors[0].end], decl);
        assert!(errors[0].message.contains(&format!("unknown declaration `{name}()`")));
    }
    let invalid_body = [Module { path: "tone", source: "fn soft(x) { missing(x) }" }];
    let invalid_syntax = [Module { path: "tone", source: "\nfn () { 1.0 }" }];
    for (modules, message) in [(&[][..], "no module"), (&invalid_body[..], "missing"), (&invalid_syntax[..], "line 2"), (&declarations[..], "declarations belong to the shader")] {
        let error = compile_audio_with(&src, Backend::Interp, false, &AudioBindings { modules, ..bindings }).into_result().unwrap_err().remove(0);
        assert!(error.message.contains(message), "{}", error.message);
        assert_eq!((error.start, error.end), (at, at + "use tone".len()), "import diagnostic");
    }

    let fused = fuse_with(&[
        FuseNode { name: "first", code: &qualified, inputs: vec![Port::Input] },
        FuseNode { name: "second", code: &glob, inputs: vec![Port::Node(0)] },
    ], &[Port::Node(1)], Backend::Native, &bindings).unwrap();
    assert_eq!((fused.probe_name(0), fused.probe_name(1)), ("first_shaped", "second_shaped"));
    let twice = compile(&format!("{helper}\nfn effect(l,r) {{ vec2(soft(soft(l)), soft(soft(r))) }}")).unwrap();
    assert_eq!(stereo_bits(perform(&fused, false, &[17], 67)), stereo_bits(perform(&twice, true, &[17], 67)));
}

// -- the prelude, diagnostics and check() ---------------------------------------

#[test]
fn the_prelude_works_and_backends_agree() {
    let src = r#"
        var f = Svf{}
        var lad = Ladder{}
        var op = OnePole{}
        var dc = DcBlock{}
        var ap = Allpass1{}
        var tri = Tri{}
        var env = Adsr{}
        var dec = Decay{}
        latency(HALFBAND_LATENCY_4)
        var os = Oversample4{}
        var buf = [0.0; 256]
        var w = int(0)
        var p = 0.0
        fn block() {
            svf_set(f, 1200.0, 2.0)
            ladder_set(lad, 800.0, 0.7)
            onepole_set(op, 3000.0)
        }
        fn voice() {
            let dt = freq / sample_rate
            p = advance(p, dt)
            let x = saw_blep(p, dt) + pulse_blep(p, dt, 0.3) * 0.5 + tri_blep(tri, p, dt) + varsaw(p, 4.0)
            let y = svf_lp(f, x) + svf_bp(f, x) + svf_hp(f, x) + svf_notch(f, x) + svf_ap(f, x)
            let z = ladder(lad, y) + onepole_lp(op, y) + onepole_hp(op, y) + dc_block(dc, y) + allpass1(ap, y, 0.3)
            buf[w] = z
            w = w + 1
            let d = tap(buf, w, 17.5) + tap_cubic(buf, w, 33.25)
            let s = soft_clip(d) + hard_clip(d) + fold(d * 2.0) + asym_clip(d, 0.2)
            let shaped = oversample(os, s, fn(u) { soft_clip(u) })
            let e = adsr(env, 0.01, 0.1, 0.5, 0.1) * decay_env(dec, 0.5)
            width(pan(shaped, 0.3), 1.5) * e * 0.05 + vec2(atan(x) * 0.001, atan2(x, 0.5) * 0.001)
        }
    "#;
    let s = build(src, Backend::Native);
    let (nl, nr) = perform(&s, false, &[128], 30000);
    let (il, ir) = perform(&s, true, &[128], 30000);
    assert_eq!(bits(&nl), bits(&il));
    assert_eq!(bits(&nr), bits(&ir));
    assert!(nl.iter().all(|x| x.is_finite()) && nl.iter().any(|x| x.abs() > 1e-3));
}

#[test]
fn a_shader_item_overrides_the_prelude() {
    let s = build("fn soft_clip(x) { 0.25 }\nfn effect(l, r) { vec2(soft_clip(l), 0.0) }", Backend::Native);
    let mut i = Instance::new(s, RATE);
    let (mut a, mut b) = (vec![0.0; 4], vec![0.0; 4]);
    i.process(&[0.9; 4], &[0.0; 4], &mut a, &mut b);
    assert_eq!(a, vec![0.25; 4]);
}

#[test]
fn near_misses_get_suggestions_and_prelude_errors_point_at_the_call() {
    let e = compile_err("var phase = 0.0\nfn voice() { phse + 1.0 }");
    assert!(e.message.contains("did you mean `phase`"), "{}", e.message);
    let e = compile_err("fn voice() { saw_blp(0.5, 0.01) }");
    assert!(e.message.contains("did you mean `saw_blep`"), "{}", e.message);
    let e = compile_err("var f = Svf{}\nfn voice() { f.cutof }");
    assert!(e.message.contains("it has"), "{}", e.message);
    // A wrong argument to a prelude helper is reported where it is called.
    let src = "fn voice() { svf_lp(1.0, 0.5) }";
    let e = compile_err(src);
    assert!(e.message.starts_with("in `svf_lp`"), "{}", e.message);
    assert_eq!(&src[e.start..e.end], "svf_lp(1.0, 0.5)");
}

#[test]
fn check_reports_health() {
    let good = with_example_bindings(|b| check_with(&shader_src("pluck"), b)).unwrap();
    assert!(good.warnings.is_empty(), "{:?}", good.warnings);
    assert!(good.tail_secs.is_some() && good.peak > 0.05);
    let block = "fn block() { if playing && beat > 1000 { for i in 0..1000000 { acc = fract(acc + 0.125) } } }";
    let costly = format!("var acc = 0.0\n{block}\nfn effect(l,r) {{ vec2(l + acc, r) }}");
    let measured = check_with(&costly, &AudioBindings::default()).unwrap();
    assert!(measured.warnings.iter().any(|w| w.contains("reduce loop sizes")), "check reports the static cost alongside the audition");
    let fused = fuse(&[
        FuseNode { name: "first", code: "fn effect(l,r) { vec2(l,r) }", inputs: vec![Port::Input] },
        FuseNode { name: "second", code: &costly, inputs: vec![Port::Node(0)] },
    ], &[Port::Node(1)], Backend::Native).unwrap();
    let [warning] = fused.warnings() else { panic!("expected one render warning") };
    assert_eq!((warning.node, &costly[warning.start..warning.end]), (1, block), "warning names the contributing node and its block");
    let (mut l, mut r) = ([0.0], [0.0]);
    Instance::new(fused, RATE).process(&[0.25], &[-0.5], &mut l, &mut r);
    assert_eq!((l, r), ([0.25], [-0.5]), "the warned graph plays");
    let endless = makepad_script_audio_aot::check("var p = 0.0\nfn voice() { p = fract(p + freq / sample_rate)\n sin(TAU * p) * 0.3 }").unwrap();
    assert!(endless.warnings.iter().any(|w| w.contains("never end")), "{:?}", endless.warnings);
    let broken = makepad_script_audio_aot::check("fn effect(l, r) { vec2(log(0.0 * l) * 0.0, l * 3.0) }").unwrap();
    assert!(broken.nonfinite > 0 && broken.warnings.len() >= 2, "{:?}", broken.warnings);
    // The audition's three notes (at 0, 0.25 and 0.5 s) all sound together
    // before the release: three held voices of 0.1 peak at 0.3.
    let held = makepad_script_audio_aot::check("fn voice() { gate * 0.1 }").unwrap();
    assert!((held.peak - 0.3).abs() < 1e-6, "the audition played {} notes at once, not 3", held.peak / 0.1);
    let mono = makepad_script_audio_aot::check("mono()\nfn voice() { probe(pitch, note)\n probe(attack, if trigger { note } else { 0.0 })\n gate * 0.1 }").unwrap();
    assert_eq!(mono.peak, 0.1, "the mono audition sounds one voice");
    assert_eq!(&mono.probes[..], &[("pitch".into(), 48.0, 55.0), ("attack".into(), 0.0, 48.0)]);
    // The audition plays with the song's clock running (120 bpm from beat
    // 0), so a beat-locked shader moves: an effect sees 3 s as 6 beats, and
    // the three notes start on beats 0, 0.5 and 1 of the same clock.
    let clocked = makepad_script_audio_aot::check("latency(37)\nfn effect(l, r) { probe(song_beat, beat)\n vec2(l, r) }").unwrap();
    assert_eq!(clocked.latency, 37, "check reports the shader's declared frame latency");
    let (_, lo, hi) = clocked.probes[0];
    assert!(lo == 0.0 && hi > 5.99 && hi < 6.0, "the effect audition's clock ran beats {lo}..{hi}, not 0..6");
    let starts = makepad_script_audio_aot::check("var first = -1.0\nfn voice() { first = if first < 0.0 { beat } else { first }\n probe(first_beat, first)\n gate * 0.1 }").unwrap();
    let (_, lo, hi) = starts.probes[0];
    assert!(lo == 0.0 && hi == 1.0, "the notes started on beats {lo}..{hi}, not 0..1");
}

#[test]
fn audition_peak_cost() {
    use makepad_script_audio_aot::{check, compile_counting, CONTROL_BLOCK};

    let fft_src = "
        var re = [0.0; 4096]
        var im = [0.0; 4096]
        var blocks = int(0)
        fn block() {
            if blocks == 0 { fft(re, im) }
            blocks = (blocks + 1) % 16
        }
        fn effect(l, r) { vec2(l, r) }
    ";
    let loop_src = "
        var acc = 0.0
        var blocks = int(0)
        fn block() {
            if blocks == 0 && voice_index == 2 {
                for i in 0..190000 {
                    if gate == 0.0 { break }
                    acc = fract(acc + 0.125)
                }
            }
            blocks = (blocks + 1) % 16
        }
        fn voice() {
            if gate == 0.0 { stop() }
            let x = gate * (0.0625 + acc)
            vec2(x, -x)
        }
    ";
    let drive = check(include_str!("../../../../libs/score_player/shaders/drive.splash")).unwrap();
    let fft = check(fft_src).unwrap();
    let long_loop = check(loop_src).unwrap();

    assert!(fft.peak_ops_per_block > 10.0 * fft.ops_per_frame, "one 4096-point FFT per sixteen blocks");
    assert!((drive.peak_ops_per_block / drive.ops_per_frame - 1.0).abs() < 0.03, "smooth drive cost");
    assert!(drive.warnings.is_empty() && fft.warnings.is_empty(), "drive and FFT stay below the warning threshold");
    // A control node runs one block() in every 64-frame call: no lump.
    let lfo = check(include_str!("../../../../libs/score_player/shaders/lfo.splash")).unwrap();
    assert_eq!(lfo.kind, Kind::Control);
    assert!(lfo.peak_ops_per_block >= lfo.ops_per_frame && lfo.warnings.is_empty(), "control node peak {} mean {} warnings {:?}",
        lfo.peak_ops_per_block, lfo.ops_per_frame, lfo.warnings);

    // Only voice 2 does the expensive loop. Its first call measures the peak
    // directly and rules out summing simultaneous voices in the report.
    let mut voice = Instance::new(compile_counting(loop_src).unwrap(), RATE);
    voice.set_voice(2);
    voice.note_on(55.0, 0.8, 3);
    voice.render(&mut [0.0; CONTROL_BLOCK as usize], &mut [0.0; CONTROL_BLOCK as usize]);
    let single_voice_peak = voice.ops() as f64 / f64::from(CONTROL_BLOCK);
    assert_eq!(long_loop.peak_ops_per_block.to_bits(), single_voice_peak.to_bits(), "peak of one voice");
    assert!(single_voice_peak > makepad_script_audio_aot::EXPENSIVE_PEAK_OPS_PER_FRAME);
    let [warning] = long_loop.warnings.as_slice() else { panic!("expected one peak warning, got {:?}", long_loop.warnings) };
    assert!(warning.contains(&format!("peak {:.0}", single_voice_peak)) && warning.contains(&format!("mean {:.0}", long_loop.ops_per_frame)));
    assert!(warning.contains("spread the work over blocks") && warning.contains("precompute it in init()"));
}

// -- every backend plays the reference's bits ----------------------------------

/// Every example and shipped library shader, by name (library ones as
/// `lib/<name>`).
fn every_source() -> Vec<(String, String)> {
    let lib_dir = format!("{}/../../../libs/score_player/shaders", env!("CARGO_MANIFEST_DIR"));
    let mut sources: Vec<(String, String)> = INSTRUMENTS.iter().chain(EFFECTS).map(|n| (n.to_string(), shader_src(n))).collect();
    if let Ok(dir) = std::fs::read_dir(&lib_dir) {
        let mut names: Vec<_> = dir.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        names.sort();
        for p in names {
            sources.push((format!("lib/{}", p.file_stem().unwrap().to_string_lossy()), std::fs::read_to_string(&p).unwrap()));
        }
    }
    sources
}

/// Every example and library shader plays the same bits on native code,
/// wasm and the interpreter (the reference), however the host slices its
/// calls. The performance is short and has what a host does to a shader: a
/// note on with the song playing, a param move with a tempo change, a
/// release, and a second note over the first one's tail, each on a frame
/// inside a control block.
#[test]
fn every_shader_plays_the_reference_bits_on_every_backend_and_slicing() {
    let even: &[usize] = &[128];
    let ragged: &[usize] = &[1, 7, 64, 128, 3, 100, 31, 128, 2];
    let whole: &[usize] = &[4096];
    for (name, src) in every_source() {
        let shader = audition_shader(&src, Backend::Native, false);
        if name == "lib/acid" {
            assert!(shader.mono());
            assert_eq!(shader.params().iter().find(|p| p.name == "gain").unwrap().default, 0.5);
        }
        assert_render_agreement(&name, &shader, 9000, &[ragged], &[even, ragged, whole], &[even, ragged]);
    }
}

fn assert_render_agreement(name: &str, shader: &AudioShaderRef, frames: usize, interp: &[&[usize]], native: &[&[usize]], wasm_slices: &[&[usize]]) {
    use makepad_script_compute::ir;
    type CountedRun<'a> = dyn FnMut(&mut [u32], &mut [u32], &[&[f32]], &mut [&mut [f32]], usize) + 'a;
    let mut scratch = shader.new_scratch();
    let mut wasm = WasmRunner::new(shader);
    // The side channels use the same absolute-frame signal at every slicing.
    let sides: Vec<Vec<f32>> = (2..shader.input_channels()).map(|ch|
        (0..frames).map(|i| (1 + (i / 67 + ch * 7) % 31) as f32 / 32.0).collect()
    ).collect();
    let play = |slices: &[usize], run: &mut CountedRun| {
        let main = if shader.kind == Kind::Control { 0 } else { 2 };
        let mut extra = vec![vec![0.0; frames]; shader.output_channels() - main];
        let (left, right) = perform_events(shader, slices, frames, &[0, 2345, 4000, 5003, 7001], |frame, ctx, state| match frame {
            0 => {
                shader.note_on(state, 57.0, 0.9, 0, 7, 0);
                shader.set_time(ctx, 120.0, 0.0, 0, 120.0 / 60.0 / RATE as f64, 4.0, true);
            }
            2345 => {
                if let Some(p) = shader.params().first() {
                    shader.set_param(ctx, 0, p.min + (p.max - p.min) * 0.3);
                }
                shader.set_time(ctx, 150.0, 2345.0 * 120.0 / 60.0 / RATE as f64, 2345, 150.0 / 60.0 / RATE as f64, 3.0, true);
            }
            5003 => shader.note_on(state, 64.5, 0.6, frame as u32, 11, 0),
            _ => shader.note_off(state, 0.5),
        }, &mut |ctx, state, ins, mut outs, n| {
            if sides.is_empty() && extra.is_empty() {
                run(ctx, state, &ins, &mut outs, n);
            } else {
                let at = ctx[CTX_FRAME as usize] as usize;
                let ins: Vec<_> = ins.into_iter().take(main).chain(sides.iter().map(|s| &s[at..at + n])).collect();
                let mut outs: Vec<_> = outs.into_iter().take(main).chain(extra.iter_mut().map(|s| &mut s[at..at + n])).collect();
                run(ctx, state, &ins, &mut outs, n);
            }
        });
        let mut channels = if main == 0 { vec![] } else { vec![left, right] };
        channels.append(&mut extra);
        channels
    };
    let mut interpret = |ctx: &mut [u32], state: &mut [u32], ins: &[&[f32]], outs: &mut [&mut [f32]], n: usize| {
        let mut mem = ir::Mem { ctx, state, shared: ir::Shared::Read(shader.shared_table()), bufs: &[] };
        ir::run(shader.program(), &mut scratch, &mut mem, &mut ir::Io { ins, outs }, n as u32);
    };
    let reference = play(&[128], &mut interpret);
    assert!(reference.iter().flatten().any(|x| *x != 0.0), "{name}: the reference is silent");
    let check = |backend: &str, slices: &[usize], got: Vec<Vec<f32>>| {
        for (channel, (want, got)) in reference.iter().zip(&got).enumerate() {
            if let Some((frame, a, b)) = first_diff(want, got) {
                panic!("{name}: {backend} with calls of {slices:?} frames differs from the interpreter at frame {frame}, channel {channel}: {b} versus {a}");
            }
        }
    };
    for &slices in interp { check("interpreter", slices, play(slices, &mut interpret)); }
    for &slices in native {
        check("native", slices, play(slices, &mut |ctx, state, ins, outs, n| shader.run_ports(ctx, state, &mut scratch, ins, outs, n)));
    }
    for &slices in wasm_slices {
        check("wasm", slices, play(slices, &mut |ctx, state, ins, outs, n| wasm.run_ports(ctx, state, ins, outs, n)));
    }
}

#[test]
fn shader_selftests_check_the_audition() {
    use makepad_script_audio_aot::{selftest, SelftestBackend};
    let sizes: &[&[usize]] = &[&[256], &[1, 7, 128, 33, 64, 3], &[97]];
    let saw_test = include_str!("../shaders/tests/saw_svf.splash");
    let reverb_test = include_str!("../shaders/tests/allpass_reverb.splash");
    for (name, test, check) in [("saw_svf", saw_test, "check_voice"), ("allpass_reverb", reverb_test, "check_effect")] {
        let src = shader_src(name);
        let report = selftest(&src, test, &AudioBindings::default()).unwrap();
        assert!(report.passed(), "{name}: {report:?}");
        assert_eq!(report.check, check);
        let shader = audition_shader(&src, Backend::Native, false);
        assert_eq!(report.backend, SelftestBackend::Native);
        assert_render_agreement(name, &shader, 9000, sizes, sizes, sizes);
    }
    let src = "let a = output(1) let b = output(2) var n = 0.0 fn block() { a = n; b = vec2(n, n + 1.0); n += 1.0 }";
    let control = build(src, Backend::Native);
    assert_eq!((control.kind(), control.input_channels(), control.output_channels()), (Kind::Control, 0, 3));
    let check = makepad_script_audio_aot::check(src).unwrap();
    assert!(check.ops_per_frame > 0.0 && check.peak > 100.0 && check.warnings.is_empty());
    let report = selftest(src, "fn check_control(frame, out) { out.y < 2.0 }", &AudioBindings::default()).unwrap();
    assert_eq!((report.check, report.first_failed_frame), ("check_control", Some(64)));
    let silent = selftest("fn voice() { 0.0 }", saw_test, &AudioBindings::default()).unwrap();
    assert_eq!(silent.first_failed_frame, Some(4800), "a silent saw must fail its level check");

    // The first failure comes after both legato transitions and the release.
    let modules = [Module { path: "levels", source: "fn level(x) { x * 2.0 }" }];
    let samples = [SampleView { name: "bound", channels: 1, frames: 1, data: &[0.125] }];
    let src = "use levels as m
        mono()
        let bound = sample(1)
        let recording = sample(2)
        fn voice() { vec2(note, gate + m.level(bound[0]) + float(len(recording))) }";
    let test = "fn check_voice(frame, out) {
        let pitch = if frame < 12000 { 48 } elif frame < 24000 { 52 } else { 55 }
        let held = if frame < 48000 { 1.0 } else { 0.0 }
        out.x == pitch && m.level(bound[0]) == 0.25 && out.y == held + 48000.25 && frame < 48007
    }";
    let report = selftest(src, test, &AudioBindings { samples: &samples, modules: &modules, ..Default::default() }).unwrap();
    assert_eq!((report.check, report.first_failed_frame), ("check_voice", Some(48007)));
    assert!(!report.passed());

    let bad = "fn check_voice(frame, out) { out.x }";
    let shader = "fn voice() { 1 }";
    let errors = selftest(shader, bad, &AudioBindings::default()).unwrap_err();
    assert!(errors[0].message.contains("bool") && errors[0].start == shader.len() + 1, "{errors:?}");
}

/// A constant `for` in an audio program renders the same bits as the same
/// loop written out by hand: the lowering unrolls it, so expressions of its
/// counter fold exactly as the hand-written literals do. (An unroll rule
/// that kept wavetable_pad's 16-harmonic loop a loop computed `mix(saw, sq,
/// t * 2.0)` in f32 step by step instead of folding it, and its render
/// changed.)
#[test]
fn constant_loops_render_as_written_out() {
    let src = shader_src("wavetable_pad");
    let head = "            for h in 1..17 {\n";
    let start = src.find(head).expect("wavetable_pad's harmonic loop");
    let body_start = start + head.len();
    let end = body_start + src[body_start..].find("\n            }\n").expect("loop end");
    let body = &src[body_start..end];
    let mut written = String::new();
    for h in 1..17 {
        written.push_str(&format!("            {{\n                let h = {h}\n{body}\n            }}\n"));
    }
    let unrolled = format!("{}{}{}", &src[..start], written, &src[end + "\n            }\n".len()..]);
    let total = 48000;
    for backend in [Backend::Native, Backend::Interp] {
        let a = perform(&build(&src, backend), backend == Backend::Interp, &[128], total);
        let b = perform(&build(&unrolled, backend), backend == Backend::Interp, &[128], total);
        assert!(bits(&a.0) == bits(&b.0) && bits(&a.1) == bits(&b.1), "{backend:?}: the loop and its written-out form differ at {:?}", first_diff(&a.0, &b.0));
    }
}

// -- the generated WebAssembly ----------------------------------------------------

/// Runs a shader's generated wasm entry (the compute core's wasm backend,
/// audio ABI) on stitch, a wasm interpreter: one instance on a memory laid
/// out as ctx, state, shared tables, the I/O address table, two input and
/// two output channels of MAX_FRAMES, and the frame scratch. Each call
/// copies the host's ctx, state, inputs and the outputs it mixes into in,
/// and ctx, state and outputs back.
struct WasmRunner {
    store: makepad_stitch::Store,
    mem: makepad_stitch::Mem,
    func: makepad_stitch::Func,
    ctx: usize,
    state: usize,
    io: usize,
    chans: Vec<usize>,
    frame: usize,
    shared: usize,
}

impl WasmRunner {
    fn new(shader: &AudioShader) -> WasmRunner {
        use makepad_stitch as stitch;
        let module = makepad_script_audio_aot::wasm::module(&[shader], Default::default()).expect("audio shader compiles to wasm");
        let engine = stitch::Engine::new();
        let m = stitch::Module::new(&engine, &module).unwrap_or_else(|e| panic!("decode: {:?}", e));
        let mut store = stitch::Store::new(engine);
        let max = makepad_script_audio_aot::MAX_FRAMES as usize;
        // Byte addresses, 16-byte aligned, a guard word gap between regions.
        let mut at = 64usize;
        let mut alloc = |words: usize| {
            let a = at;
            at += (words.max(1) * 4 + 16 + 15) & !15;
            a
        };
        let ctx = alloc(shader.ctx_words());
        let state = alloc(shader.state_words());
        let shared = alloc(shader.shared_table().len());
        let count = shader.program().channel_count();
        let io = alloc(count);
        let chans: Vec<_> = (0..count).map(|_| alloc(max)).collect();
        let frame = alloc(makepad_script_audio_aot::wasm::frame_words(shader));
        let pages = at.div_ceil(65536) as u32;
        let mem = stitch::Mem::new(&mut store, stitch::MemType { limits: stitch::Limits { min: 1, max: None } });
        mem.grow(&mut store, pages - 1).expect("grow memory for the shared table and call buffers");
        let mut linker = stitch::Linker::new();
        linker.define("env", "memory", mem);
        let inst = linker.instantiate(&mut store, &m).unwrap_or_else(|e| panic!("instantiate: {:?}", e));
        let func = inst.exported_func("run0").expect("run0");
        let bytes = mem.bytes_mut(&mut store);
        #[cfg(target_endian = "little")]
        {
            let table = shader.shared_table();
            // SAFETY: the byte slice covers exactly the initialized, immutable table.
            let data = unsafe { std::slice::from_raw_parts(table.as_ptr().cast::<u8>(), std::mem::size_of_val(table)) };
            bytes[shared..shared + data.len()].copy_from_slice(data);
        }
        #[cfg(target_endian = "big")]
        for (k, w) in shader.shared_table().iter().enumerate() {
            bytes[shared + 4 * k..shared + 4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
        for (k, c) in chans.iter().enumerate() {
            bytes[io + 4 * k..io + 4 * k + 4].copy_from_slice(&(*c as u32).to_le_bytes());
        }
        WasmRunner { store, mem, func, ctx, state, io, chans, frame, shared }
    }

    fn put(bytes: &mut [u8], at: usize, words: impl Iterator<Item = u32>) {
        for (k, w) in words.enumerate() {
            bytes[at + 4 * k..at + 4 * k + 4].copy_from_slice(&w.to_le_bytes());
        }
    }

    fn get(bytes: &[u8], at: usize, out: &mut [u32]) {
        for (k, w) in out.iter_mut().enumerate() {
            *w = u32::from_le_bytes(bytes[at + 4 * k..at + 4 * k + 4].try_into().unwrap());
        }
    }

    fn run(&mut self, ctx: &mut [u32], state: &mut [u32], ins: [&[f32]; 2], mut outs: [&mut [f32]; 2], n: usize) {
        self.run_ports(ctx, state, &ins, &mut outs, n)
    }

    fn run_ports(&mut self, ctx: &mut [u32], state: &mut [u32], ins: &[&[f32]], outs: &mut [&mut [f32]], n: usize) {
        use makepad_stitch::Val;
        let bytes = self.mem.bytes_mut(&mut self.store);
        Self::put(bytes, self.ctx, ctx.iter().copied());
        Self::put(bytes, self.state, state.iter().copied());
        for (at, ch) in self.chans.iter().zip(ins.iter().copied().chain(outs.iter().map(|s| &s[..]))) {
            Self::put(bytes, *at, ch[..n].iter().map(|x| x.to_bits()));
        }
        let args = [self.ctx, self.state, self.shared, self.io, n, self.frame].map(|x| Val::I32(x as i32));
        self.func.call(&mut self.store, &args, &mut []).unwrap_or_else(|e| panic!("trap: {:?}", e));
        let bytes = self.mem.bytes(&self.store);
        Self::get(bytes, self.ctx, ctx);
        Self::get(bytes, self.state, state);
        for (c, out) in outs.into_iter().enumerate() {
            let mut w = vec![0u32; n];
            Self::get(bytes, self.chans[ins.len() + c], &mut w);
            for (o, x) in out.iter_mut().zip(w) {
                *o = f32::from_bits(x);
            }
        }
    }
}

#[test]
fn a_shader_takes_as_many_params_as_it_declares() {
    // A big console strip or a kit voice: more knobs than any fixed table.
    const N: usize = 100;
    let mut src: String = (0..N).map(|k| format!("let p{k} = param(0, 0, 1)\n")).collect();
    let sum: Vec<String> = (0..N).map(|k| format!("p{k}")).collect();
    src += &format!("fn voice() {{ vec2({}, p{}) }}", sum.join(" + "), N - 1);
    let shader = build(&src, Backend::Native);
    assert_eq!(shader.param_count(), N, "every declared param is kept");
    let value = |k: usize| (k + 1) as f32 / 128.0;
    let total = 1024;
    let play = |slices: &[usize], run: &mut Run| {
        perform_events(&shader, slices, total, &[0, 100], |frame, ctx, state| {
            if frame == 0 {
                shader.note_on(state, 60.0, 1.0, 0, 1, 0);
            } else {
                for k in 0..N {
                    shader.set_param(ctx, k, value(k));
                }
            }
        }, run)
    };
    let mut scratch = shader.new_scratch();
    let (left, right) = play(&[128], &mut |ctx, state, ins, [l, r], n| shader.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n));
    // Set at frame 100, ramped across the block from 128, reached at 192.
    let want = (0..N).fold(0.0f32, |acc, k| if k == 0 { value(0) } else { acc + value(k) });
    assert!(left[..128].iter().all(|&v| v == 0.0), "params moved before the next block");
    assert!(left[192..].iter().all(|&v| v == want) && right[192..].iter().all(|&v| v == value(N - 1)), "the last params did not reach their targets");
    let ragged = &[1, 7, 128, 33, 64, 3][..];
    let mut wasm = WasmRunner::new(&shader);
    let same = |runner: &str, slices: &[usize], (l, r): (Vec<f32>, Vec<f32>)| {
        assert_eq!((first_diff(&left, &l), first_diff(&right, &r)), (None, None), "{runner} with slices {slices:?}");
    };
    for slices in [&[128][..], ragged] {
        same("interpreter", slices, play(slices, &mut |ctx, state, ins, [l, r], n| shader.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n)));
        same("native", slices, play(slices, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n)));
        same("wasm", slices, play(slices, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n)));
    }
}

#[test]
fn param_metadata_and_control_changes_survive_fusion() {
    use makepad_script_audio_aot::{ParamCurve, ParamMeta, ParamUnit, CONTROL_BLOCK};

    let source = r#"
        let saw = 7
        let mode = param(saw, {choices: ["Soft pulse" saw] label: Wave group: saw})
        fn voice() { vec2(mode, 0.0) }
    "#;
    let effect = r#"
        let quarter = 0.25
        let amount = param(0, 0, 1, {smooth: 0.05 group: "Tone controls" step: quarter
            label: "Tone amount" curve: exp unit: "%"})
        let offset = param(0, {step: quarter})
        fn effect(l, r) { vec2(l + offset, amount) }
    "#;
    let shader = fuse(&[
        FuseNode { name: "lead", code: source, inputs: vec![] },
        FuseNode { name: "tone", code: effect, inputs: vec![Port::Node(0)] },
    ], &[Port::Node(1)], Backend::Native).unwrap();
    let mode = shader.param_view(0);
    assert_eq!((mode.name, mode.default, mode.min, mode.max), ("lead_mode", 1.0, 0.0, 1.0), "fused choice range or word default");
    let choices = shader.param_meta(0);
    assert_eq!(choices.choices, &["Soft pulse", "saw"], "choice spellings and order");
    assert_eq!((choices.label, choices.group), ("Wave", "saw"), "fusion renamed literal metadata");
    assert_eq!((choices.unit, choices.curve, choices.step, choices.smooth), (ParamUnit::None, ParamCurve::Linear, 1.0, -1.0), "choice defaults");

    assert_eq!(shader.param_view(1).name, "tone_amount", "effect metadata follows voice metadata");
    let info = shader.param_meta(1);
    assert_eq!((info.unit, info.curve, info.step, info.smooth), (ParamUnit::Percent, ParamCurve::Exp, 0.25, 0.05), "unit, curve, step or smoothing view");
    assert_eq!((info.label, info.group), ("Tone amount", "Tone controls"), "quoted label or group view");
    assert!(info.choices.is_empty(), "numeric parameter acquired choices");
    let owned = shader.param_metas();
    assert_eq!(owned.len(), shader.param_count(), "one metadata record per parameter");
    assert_eq!(owned[2], ParamMeta { step: 0.25, ..Default::default() }, "omitted metadata defaults");
    let report = makepad_script_audio_aot::check(effect).unwrap();
    assert_eq!(report.param_meta, owned[1..], "check report and fused body disagree on effect metadata");

    let block = CONTROL_BLOCK as usize;
    let total = block + (RATE * 0.3) as usize + 1;
    let play = |slices: &[usize], run: &mut Run| {
        perform_events(&shader, slices, total, &[0, 17, 2 * block + 1], |frame, ctx, state| match frame {
            0 => {
                shader.note_on(state, 60.0, 1.0, 0, 1, 0);
                shader.set_param(ctx, 1, 0.25);
            }
            17 => {
                shader.set_param(ctx, 0, 0.2);
                shader.set_param(ctx, 1, 0.9);
            }
            _ => shader.set_param(ctx, 2, 0.375),
        }, run)
    };
    let mut scratch = shader.new_scratch();
    let (left, right) = play(&[128], &mut |ctx, state, ins, [l, r], n| shader.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n));
    assert!(left[..block].iter().all(|&v| v == 1.0), "choice changed before the next block");
    assert!(left[block..3 * block].iter().all(|&v| v == 0.0), "choice did not jump to its snapped index at block start");
    assert!(left[3 * block..].iter().all(|&v| v == 0.5), "unbounded quarter-step control did not snap and jump");
    assert!(right[..block].iter().all(|&v| v == 0.25), "fresh smoothed parameter did not start at its target");
    let first_block = 0.25 + 0.75 * CONTROL_BLOCK as f32 / (0.05 * RATE);
    assert!((right[2 * block - 1] - first_block).abs() < 1e-6 && right[2 * block] > right[2 * block - 1], "timed smoothing stopped or used the wrong block fraction");
    assert!((1.0 - right[total - 1]).abs() < 0.01, "timed smoothing missed the snapped target by more than 1% after 0.3 s");
    let compare = |runner, (l, r): (Vec<f32>, Vec<f32>)| {
        assert_eq!(first_diff(&left, &l), None, "{runner} changed a discrete sample");
        assert_eq!(first_diff(&right, &r), None, "{runner} changed a smoothed sample");
    };
    let ragged = &[1, 37, 128, 13][..];
    compare("ragged interpreter", play(ragged, &mut |ctx, state, ins, [l, r], n| shader.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n)));
    let mut wasm = WasmRunner::new(&shader);
    for slices in [&[128][..], ragged] {
        compare("native", play(slices, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n)));
        compare("wasm", play(slices, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n)));
    }
}

#[test]
fn probes_report_each_calls_values_and_the_whole_audition() {
    let src = "
        let gain = param(1.0)
        var e = Adsr{}
        var tick = int(0)
        fn block() { probe(start, trigger) }
        fn observe(osc) { probe(x, osc) }
        fn voice() {
            e.level = float(tick % 11) * 0.125
            let osc = (tick % 7) - 3
            probe(env, e.level)
            probe(x, osc)
            observe(osc)
            if pressure > 0.5 { probe(held, pressure) }
            tick = tick + 1
            vec2(e.level * gain, float(osc))
        }";
    let shader = build(src, Backend::Native);
    let names: Vec<_> = (0..shader.probe_count()).map(|k| shader.probe_name(k)).collect();
    assert_eq!(names, ["start", "env", "x", "held"]);
    assert_eq!(shader.probe_base(), makepad_script_audio_aot::CTX_PARAMS as usize + 1);
    assert_eq!(shader.ctx_words(), shader.probe_base() + 12);
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    assert_eq!(shader.backend(), Backend::Native);
    let play = |slices: &[usize], run: &mut Run| {
        let mut calls = Vec::new();
        perform_with(&shader, slices, 144, &mut |ctx, state, ins, [left, right], n| {
            let at = ctx[CTX_FRAME as usize] as usize;
            run(ctx, state, ins, [left, right], n);
            let block = (at..at + n).find(|frame| frame % 64 == 0);
            let expected = block.map(|frame| if frame == 0 { (1.0, 1.0, 1.0) } else { (0.0, 0.0, 0.0) });
            assert_eq!(shader.probe_read(ctx, 0), expected, "control block at {at}");
            for (k, samples) in [(1, &left[..]), (2, &right[..])] {
                let min = samples.iter().copied().fold(f32::INFINITY, f32::min);
                let max = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                assert_eq!(shader.probe_read(ctx, k), Some((samples[n - 1], min, max)), "probe {k} at {at}");
            }
            assert_eq!(shader.probe_read(ctx, 3), None);
            let words = &ctx[shader.probe_base()..];
            assert!(f32::from_bits(words[10]) > f32::from_bits(words[11]), "unreached range at {at}");
            calls.push((at + n, words.to_vec()));
        });
        calls
    };
    let mut scratch = shader.new_scratch();
    let mut wasm = WasmRunner::new(&shader);
    let mut runs = Vec::new();
    for slices in [&[16][..], &[3, 13][..]] {
        let reference = play(slices, &mut |c, s, i, [l, r], n| shader.run_interp(c, s, &mut scratch, i[0], i[1], l, r, n));
        assert_eq!(play(slices, &mut |c, s, i, [l, r], n| shader.run(c, s, &mut scratch, i[0], i[1], l, r, n)), reference);
        assert_eq!(play(slices, &mut |c, s, i, o, n| wasm.run(c, s, i, o, n)), reference);
        runs.push(reference);
    }
    for (end, words) in &runs[0] {
        let (_, ragged) = runs[1].iter().find(|(frame, _)| frame == end).unwrap();
        for last in [3, 6] {
            assert_eq!(words[last], ragged[last], "last at shared frame {end}");
        }
    }
    let report = makepad_script_audio_aot::check(src).unwrap();
    for run in &runs {
        for (k, (name, want_min, want_max)) in report.probes.iter().enumerate() {
            let min = run.iter().map(|(_, w)| f32::from_bits(w[3 * k + 1])).fold(f32::INFINITY, f32::min);
            let max = run.iter().map(|(_, w)| f32::from_bits(w[3 * k + 2])).fold(f32::NEG_INFINITY, f32::max);
            assert_eq!((name.as_str(), *want_min, *want_max), (names[k], min, max));
        }
    }
    assert_eq!(report.probes.len(), names.len());
    let fx = "fn effect(l, r) { probe(env, r)\n vec2(l, r) }";
    let fused = fuse(&[FuseNode { name: "lead", code: src, inputs: vec![] }, FuseNode { name: "fx", code: fx, inputs: vec![Port::Node(0)] }],
        &[Port::Node(1)], Backend::Interp).unwrap();
    assert_eq!((0..fused.probe_count()).map(|k| fused.probe_name(k)).collect::<Vec<_>>(), ["lead_start", "lead_env", "lead_x", "lead_held", "fx_env"]);
    for declaration in ["fn init() { probe(bad, 1) }", "let value = probe(bad, 1)"] {
        let invalid = format!("{declaration}\nfn voice() {{ 0.0 }}");
        let error = compile_err(&invalid);
        assert_eq!(&invalid[error.start..error.end], "probe(bad, 1)");
        assert!(error.message.contains("probe() runs on the audio path"), "{}", error.message);
    }
    assert!(compile_err("fn voice() { probe(bad, vec2(1))\n 0.0 }").message.contains("probe(left, v.x)"));
}

#[test]
fn expression_lands_on_its_sample_on_every_backend() {
    let lead = shader_src("expressive_lead");
    let steps = "fn voice() { vec2(pressure + slide + mod_wheel, release_velocity + bend) }";
    let saw = "mono()\nvar phase = 0.0\nfn voice() { phase = fract(phase + freq / sample_rate)\n vec2(note, phase * 2.0 - 1.0) }";
    for (name, src) in [("expressive_lead", lead.as_str()), ("steps", steps), ("glide", saw)] {
        let shader = build(src, Backend::Native);
        let play = |slices: &[usize], run: &mut Run| {
            perform_events(&shader, slices, 24128, &[0, 9001, 15000, 24000], |frame, ctx, state| match frame {
                0 => shader.note_on(state, 57.0, 0.9, 0, 7, 0),
                15000 if name == "glide" => shader.glide(state, 57.0, 0.9, 2.5 / RATE, RATE),
                24000 if name == "glide" => shader.glide(state, 69.0, 0.9, 100_000.0, RATE),
                9001 if name == "glide" => shader.glide(state, 69.0, 0.9, 0.1, RATE),
                9001 => {
                    shader.set_expression(state, Expression::Bend, 12.0);
                    shader.set_expression(state, Expression::Pressure, 0.8);
                }
                15000 => {
                    shader.set_cc(ctx, 1, 0.7);
                    shader.set_expression(state, Expression::Slide, 0.4);
                }
                24000 => shader.note_off(state, 0.9),
                _ => unreachable!(),
            }, run)
        };
        let mut scratch = shader.new_scratch();
        let reference = |slices: &[usize], scratch: &mut [u32]| {
            play(slices, &mut |ctx, state, ins, [l, r], n| shader.run_interp(ctx, state, scratch, ins[0], ins[1], l, r, n))
        };
        let (left, right) = reference(&[128], &mut scratch);
        if name == "glide" {
            assert_eq!(left[9000], 57.0, "pitch before the glide");
            for (k, &pitch) in left[9001..13801].iter().enumerate() {
                let want = 57.0 + 12.0 * (k + 1) as f32 / 4800.0;
                assert!((pitch - want).abs() < 0.00001, "glide sample {k}: {pitch} vs {want}");
            }
            assert!(left[13799] < 69.0);
            assert_eq!(left[13800], 69.0, "target on sample 4800");
            assert_eq!(left[13801], 69.0, "steady after arrival");
            assert!((left[15000] - 64.2).abs() < 1e-5 && (left[15001] - 59.4).abs() < 1e-5, "fractional glide advances by its step");
            assert_eq!(left[15002], 57.0, "fractional duration ends on the next whole sample");
            assert!(left[24000..].iter().all(|&pitch| (57.0..57.001).contains(&pitch)), "long glides keep their duration");
        }
        if name == "steps" {
            let sum = 0.8f32 + 0.4 + 0.7;
            for (range, l, r) in [
                (0..9001, 0.0f32, 0.0f32), (9001..15000, 0.8, 12.0),
                (15000..24000, sum, 12.0), (24000..24128, sum, 12.0 + 0.9),
            ] {
                for i in range {
                    assert_eq!((left[i].to_bits(), right[i].to_bits()), (l.to_bits(), r.to_bits()), "expression at frame {i}");
                }
            }
        }
        let check = |backend, (l, r): (Vec<f32>, Vec<f32>)| {
            assert_eq!(first_diff(&left, &l), None, "{name} {backend} left");
            assert_eq!(first_diff(&right, &r), None, "{name} {backend} right");
        };
        let ragged = &[1, 7, 128, 33, 64, 3][..];
        check("interpreter", reference(ragged, &mut scratch));
        let mut wasm = WasmRunner::new(&shader);
        for slices in [&[128][..], ragged] {
            check("native", play(slices, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n)));
            check("wasm", play(slices, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n)));
        }
    }
}

#[test]
fn a_shaders_own_name_wins_over_an_input() {
    let shader = build("let pressure = param(0.25, 0, 1)
        fn cc(x) { x }
        fn param(x, options) { x * options.scale }
        fn voice() { cc(param(pressure, {scale: 1.0})) }", Backend::Native);
    let mut inst = Instance::new(shader, RATE);
    inst.note_on(60.0, 1.0, 1);
    for pressure in [0.0, 0.75, 1.0] {
        inst.set_expression(Expression::Pressure, pressure);
        let (mut l, mut r) = ([0.0], [0.0]);
        inst.render(&mut l, &mut r);
        assert_eq!((l, r), ([0.25], [0.25]), "shader pressure with host pressure {pressure}");
    }
}

/// A counting build plays the same samples and counts the same ops on
/// every run: what `check` calls expensive does not depend on the machine.
#[test]
fn a_counting_build_plays_the_same_bits_and_counts_the_same_ops() {
    for name in INSTRUMENTS {
        let src = shader_src(name);
        let plain = audition_shader(&src, Backend::Native, false);
        let play = |s: &AudioShaderRef| {
            let mut i = Instance::new(s.clone(), 48000.0);
            let (mut l, mut r) = (vec![0.0f32; 9000], vec![0.0f32; 9000]);
            i.note_on(50.0, 0.9, 3);
            i.render(&mut l[..6000], &mut r[..6000]);
            i.note_off(0.5);
            i.render(&mut l[6000..], &mut r[6000..]);
            (l.iter().chain(&r).map(|x| x.to_bits()).collect::<Vec<_>>(), i.ops())
        };
        let counting = audition_shader(&src, Backend::Native, true);
        let (want, none) = play(&plain);
        let (got, ops) = play(&counting);
        assert_eq!(none, 0, "{name}: a plain build counts nothing");
        assert!(got == want, "{name}: counting changed the samples");
        assert!(ops > 9000, "{name}: {ops} ops");
        assert_eq!(play(&audition_shader(&src, Backend::Native, true)).1, ops, "{name}: the count is the same every run");
    }
}

/// The song's time reaches a shader on its exact sample, the same bits on
/// every backend and call size: `beat` and `bar` follow the anchor the host
/// set (a new anchor mid-render moves them on its own frame), hold while
/// stopped, `tempo` and `beats_per_bar` read back, a shader's own name wins,
/// and a beat-locked tremolo is at full level on every whole division.
#[test]
fn musical_time_reaches_every_backend_on_its_sample() {
    const TOTAL: usize = 6000;
    const SWITCH: usize = 3001;
    let even: &[usize] = &[128];
    let ragged: &[usize] = &[1, 7, 64, 128, 3, 100, 31, 128, 2];
    let (step_a, step_b) = (120.0 / 60.0 / 48000.0f64, 150.0 / 60.0 / 48000.0f64);
    let beat_b = SWITCH as f64 * step_a;

    let shader = build("fn voice() { vec2(beat, bar) }", Backend::Native);
    let mut scratch = shader.new_scratch();
    let mut wasm = WasmRunner::new(&shader);
    let play = |playing: bool, slices: &[usize], run: &mut Run| {
        perform_events(&shader, slices, TOTAL, &[0, SWITCH], |frame, ctx, state| match frame {
            0 => {
                shader.note_on(state, 57.0, 0.9, 0, 7, 0);
                shader.set_time(ctx, 120.0, if playing { 0.0 } else { 1.5 }, 0, step_a, 4.0, playing);
            }
            _ => shader.set_time(ctx, 150.0, if playing { beat_b } else { 1.5 }, SWITCH as u32, step_b, 3.0, playing),
        }, run)
    };
    let interp = |slices: &[usize], playing: bool, scratch: &mut Box<[u32]>| {
        play(playing, slices, &mut |ctx, state, ins, [l, r], n| shader.run_interp(ctx, state, scratch, ins[0], ins[1], l, r, n))
    };
    let reference = interp(even, true, &mut scratch);
    let backends = [
        ("interpreter", interp(ragged, true, &mut scratch)),
        ("native, even calls", play(true, even, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n))),
        ("native, ragged calls", play(true, ragged, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n))),
        ("wasm, even calls", play(true, even, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n))),
        ("wasm, ragged calls", play(true, ragged, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n))),
    ];
    for (backend, got) in &backends {
        for (side, want, got) in [("beat", &reference.0, &got.0), ("bar", &reference.1, &got.1)] {
            if let Some((frame, a, b)) = first_diff(want, got) {
                panic!("backends and slicing: {backend} differs from the interpreter at frame {frame} ({side}): {backend} {b}, interpreter {a}");
            }
        }
    }

    // The formula, in f32 exactly as the program does it, on both sides of the new anchor.
    for f in [0usize, 1, 127, 128, 1500, 2999, 3000, SWITCH, 3002, 3100, 5999] {
        let (at, frame0, step, per_bar) = if f < SWITCH { (0.0f64, 0usize, step_a, 4.0f32) } else { (beat_b, SWITCH, step_b, 3.0f32) };
        let beat = at as f32 + ((f - frame0) as i32 as f32) * (step as f32);
        assert_eq!(reference.0[f].to_bits(), beat.to_bits(), "the formula: beat at frame {f}: got {}, want {beat}", reference.0[f]);
        assert_eq!(reference.1[f].to_bits(), (beat / per_bar).to_bits(), "the formula: bar at frame {f}: got {}, want {}", reference.1[f], beat / per_bar);
    }

    // Stopped: the beat holds at the anchor on every frame (and so does the bar).
    let stopped = interp(ragged, false, &mut scratch);
    assert!(stopped.0.iter().all(|b| *b == 1.5), "stopped: beat must hold at 1.5, got {:?}", stopped.0.iter().find(|b| **b != 1.5));
    assert!(stopped.1[..SWITCH].iter().all(|b| *b == 1.5 / 4.0) && stopped.1[SWITCH..].iter().all(|b| *b == 1.5 / 3.0), "stopped: bar must hold at beat / beats_per_bar");

    // `tempo` and `beats_per_bar` read back.
    let readback = build("fn voice() { vec2(tempo, beats_per_bar) }", Backend::Native);
    let mut scratch2 = readback.new_scratch();
    let got = perform_events(&readback, even, TOTAL, &[0, SWITCH], |frame, ctx, state| match frame {
        0 => {
            readback.note_on(state, 57.0, 0.9, 0, 7, 0);
            readback.set_time(ctx, 120.0, 0.0, 0, step_a, 4.0, true);
        }
        _ => readback.set_time(ctx, 150.0, beat_b, SWITCH as u32, step_b, 3.0, true),
    }, &mut |ctx, state, ins, [l, r], n| readback.run(ctx, state, &mut scratch2, ins[0], ins[1], l, r, n));
    for (f, tempo, per_bar) in [(0, 120.0f32, 4.0f32), (SWITCH - 1, 120.0, 4.0), (SWITCH, 150.0, 3.0), (TOTAL - 1, 150.0, 3.0)] {
        assert_eq!((got.0[f], got.1[f]), (tempo, per_bar), "tempo and beats_per_bar at frame {f}");
    }

    // A shader's own name wins over the input.
    let own = build("var bar = 0.25\nfn voice() { vec2(bar, bar) }", Backend::Native);
    let mut scratch3 = own.new_scratch();
    let got = perform_events(&own, even, 256, &[0], |_, ctx, state| {
        own.note_on(state, 57.0, 0.9, 0, 7, 0);
        own.set_time(ctx, 120.0, 9.0, 0, step_a, 4.0, true);
    }, &mut |ctx, state, ins, [l, r], n| own.run(ctx, state, &mut scratch3, ins[0], ins[1], l, r, n));
    assert!(got.0.iter().chain(&got.1).all(|x| *x == 0.25), "own name wins: `bar` must read the shader's 0.25");

    // The beat-locked tremolo is at full level on every whole division. The
    // beat advances 1/64 per frame (an exact step), so frame 64 k is beat k.
    let tremolo = build(&shader_src("tempo_tremolo"), Backend::Native);
    let mut scratch4 = tremolo.new_scratch();
    let (in_l, in_r) = test_input(2048);
    let out = perform_events(&tremolo, even, 2048, &[0], |_, ctx, _| tremolo.set_time(ctx, 120.0, 0.0, 0, 1.0 / 64.0, 4.0, true),
        &mut |ctx, state, ins, [l, r], n| tremolo.run(ctx, state, &mut scratch4, ins[0], ins[1], l, r, n));
    let division = tremolo.params().iter().position(|p| p.name == "division").map(|i| tremolo.param_view(i).default).expect("tempo_tremolo has a division param");
    let every = (64.0 * division) as usize;
    assert!(every >= 1 && (every as f32) == 64.0 * division, "tremolo test needs a division that is a whole number of frames");
    for f in (every..2048).step_by(every) {
        assert!((out.0[f] - in_l[f]).abs() < 1e-6 && (out.1[f] - in_r[f]).abs() < 1e-6, "tremolo full level on whole divisions: frame {f}: out {} in {}", out.0[f], in_l[f]);
    }
    assert!((0..2048).any(|f| (out.0[f] - in_l[f]).abs() > 1e-3), "tremolo dips between divisions");
}
#[test]
fn identity_and_seeded_noise_reach_every_backend() {
    let voice = build("fn voice() { vec2(float(voice_index), instance) }", Backend::Native);
    let effect = build("fn effect(l, r) { vec2(rand(), instance + float(voice_index)) }", Backend::Native);
    let even: &[usize] = &[64];
    let ragged: &[usize] = &[3, 1, 37, 8];

    for shader in [&voice, &effect] {
        let mut scratch = shader.new_scratch();
        let mut wasm = WasmRunner::new(shader);
        let mut previous = Vec::new();
        for (seed, identity_bits) in [(1, 12_306_387), (2, 2_172_135)] {
            let identity = identity_bits as f32 / 16_777_216.0;
            let play = |sizes: &[usize], run: &mut Run| {
                perform_events(shader, sizes, 133, &[0], |_, ctx, state| {
                    shader.set_instance(ctx, seed);
                    if shader.kind == Kind::Instrument {
                        shader.note_on(state, 60.0, 1.0, 0, seed, 3);
                    } else {
                        shader.seed_state(state, seed);
                    }
                }, run)
            };
            let mut interp = |ctx: &mut [u32], state: &mut [u32], ins: [&[f32]; 2], [l, r]: [&mut [f32]; 2], n| {
                shader.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n);
            };
            let reference = play(even, &mut interp);
            assert!(reference.1.iter().all(|x| x.to_bits() == identity.to_bits()), "instance for seed {seed}");
            if shader.kind == Kind::Instrument {
                assert!(reference.0.iter().all(|&x| x == 3.0), "voice slot");
            } else {
                assert!(previous.is_empty() || reference.0 != previous, "the two effect streams differ");
                previous.clone_from(&reference.0);
            }
            assert_eq!(play(ragged, &mut interp), reference, "interpreter replay");
            for sizes in [even, ragged] {
                let native = play(sizes, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n));
                assert_eq!(native, reference, "native replay");
                assert_eq!(play(sizes, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n)), reference, "wasm replay");
            }
        }
    }
}
fn bound_shader(src: &str, samples: &[makepad_script_audio_aot::SampleView<'_>]) -> AudioShaderRef {
    use makepad_script_audio_aot::{boundary::compile_audio_with, AudioBindings};
    compile_audio_with(src, Backend::Native, false, &AudioBindings { samples, ..Default::default() }).into_result().unwrap()
}

#[test]
fn sample_tables_play_the_bound_frames() {
    use makepad_script_audio_aot::SampleView;
    let code = "let src = sample(2)\nvar acc = [0.0; len(src)]\nvar pos = 0.0\nfn effect(l,r) { let v = read_cubic(src,pos)\n pos += 0.75\n v }";
    let data = [0.5, -0.25, 0.125, 0.75, -0.75, 0.375, 0.75, -0.5];
    let view = SampleView { name: "src", channels: 2, frames: 4, data: &data };
    let stereo = bound_shader(code, &[view]);
    let unbound = bound_shader(code, &[]);
    assert_eq!(stereo.sample_names(), &["src"]);
    assert_eq!(stereo.state_words() - unbound.state_words(), 3, "len(src) sized the state array at compile time");
    let silent = perform(&unbound, false, &[7], 32);
    assert!(silent.0.iter().chain(&silent.1).all(|x| x.to_bits() == 0));
    let reference = perform(&stereo, true, &[128], 256);
    assert_eq!((reference.0[0], reference.1[0]), (data[0], data[1]));
    assert_eq!((reference.0[4], reference.1[4]), (data[6], data[7]));
    let mut wasm = WasmRunner::new(&stereo);
    for slices in [&[128][..], &[3, 1, 21, 8][..]] {
        for output in [perform(&stereo, true, slices, 256), perform(&stereo, false, slices, 256),
            perform_with(&stereo, slices, 256, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n))] {
            assert_eq!(first_diff(&reference.0, &output.0), None, "left bits");
            assert_eq!(first_diff(&reference.1, &output.1), None, "right bits");
        }
    }
    let mono = bound_shader("let src = sample(1)\nfn effect(l,r) { src[1] }", &[view]);
    let mixed = (data[2] + data[3]) * 0.5;
    assert_eq!(perform(&mono, false, &[1], 1), (vec![mixed], vec![mixed]));
    let duplicated = bound_shader("let src = sample(2)\nfn effect(l,r) { read(src,0.5) }",
        &[SampleView { channels: 1, frames: 2, data: &[0.25, 0.75], ..view }]);
    assert_eq!(perform(&duplicated, false, &[1], 1), (vec![0.5], vec![0.5]));

    let two = bound_shader("let first = sample(1)\nlet next = sample(2)\nfn voice() { len(first) + len(next) }",
        &[SampleView { name: "absent", channels: 3, ..view }]);
    assert_eq!(two.sample_names(), &["first", "next"], "unused tables keep their declaration order");
    for name in ["lib", "code", "mono", "at", "in", "seed"] {
        let error = compile_err(&format!("let {name} = sample(1)\nfn voice() {{ 0.0 }}"));
        assert!(error.message.contains(name), "reserved table {name}: {}", error.message);
    }
    for bad in [SampleView { channels: 3, ..view }, SampleView { frames: 2, ..view }] {
        let error = makepad_script_audio_aot::boundary::compile_audio_with(code, Backend::Interp, false,
            &makepad_script_audio_aot::AudioBindings { samples: &[bad], ..Default::default() });
        assert!(error.error_count() > 0 && error.error(0).message.contains("src"), "bad sample binding");
    }

    let frames = 5_000_000usize;
    let mut recording = vec![0.0; frames * 2];
    recording[frames * 2 - 2..].copy_from_slice(&[0.625, -0.75]);
    let large = bound_shader("let src = sample(2)\nfn effect(l,r) { src[len(src)-1] }",
        &[SampleView { frames: frames as u32, data: &recording, ..view }]);
    drop(recording);
    let mut wasm = WasmRunner::new(&large);
    let out = perform_with(&large, &[128], 256, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n));
    assert_eq!(out, (vec![0.625; 256], vec![-0.75; 256]), "last stereo frame in grown memory");
    let pages = makepad_script_audio_aot::wasm::memory_pages;
    let resident = 2 * 16384 - 4 * (u64::from(makepad_script_audio_aot::MAX_FRAMES) + 1);
    assert_eq!(pages(resident, 0, 4, None).unwrap(), 2, "four buffers and pointers fit exactly");
    assert_eq!(pages(resident, 0, 5, None).unwrap(), 3, "a fifth channel crosses the page boundary");
    let e = pages(1 << 30, 1, 4, None).unwrap_err();
    assert!(e.message.contains("65536 pages"), "{}", e.message);
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[test]
fn native_wide_sample_offsets_read_the_last_frames() {
    use makepad_script_audio_aot::SampleView;
    let n = 2048;
    let mut data = vec![0.0; n];
    data[n - 2..].copy_from_slice(&[-0.625, 0.375]);
    let shader = bound_shader("let src = sample(1)\nfn effect(l,r) { vec2(src[len(src)-1], src[len(src)-2]) }",
        &[SampleView { name: "src", channels: 1, frames: n as u32, data: &data }]);
    let narrow = perform(&shader, false, &[8], 8);
    assert_eq!(shader.backend(), Backend::Native);
    let wide = makepad_script_compute::native::compile_with_offset_limit(shader.program(), 0, 64).unwrap();
    let got = perform_with(&shader, &[8], 8, &mut |ctx, state, ins, outs, n| {
        // This code was compiled for these exact regions; render entries only read the shared table.
        unsafe { wide.run(ctx, state, shader.shared_table().as_ptr().cast_mut(), ins, outs, n as u32) }
    });
    assert_eq!(got, narrow);
    assert_eq!(got, (vec![0.375; 8], vec![-0.625; 8]));

    // Audio entries are scalar. Exercise the same displacement in a four-lane kernel.
    use makepad_script_compute::{kernel, native_vec};
    let kernel = kernel::compile("let t = [0.0; 2048]\nlet out = output(f32)\nfn init() { t[len(t)-2] = -0.625 }\nfn element(i) { out[i] = t[len(t)-2] }").unwrap();
    for limit in [i32::MAX, 64] {
        let (code, lanes) = native_vec::compile_with_offset_limit(kernel.program(), 4, limit).unwrap();
        let mut output = [0.0f32; 4];
        let mut control = 0u32;
        let mut state = 0u32;
        let mut ctx = vec![0; kernel.ctx_words()];
        ctx[kernel::K_COUNT as usize] = lanes;
        let table = [&mut control as *mut u32 as u64, 1, output.as_mut_ptr() as u64, 4];
        // SAFETY: the two entries cover the control word and all four outputs.
        // The kernel owns the shared image initialized for this entry.
        unsafe { code.run_kernel(ctx.as_mut_ptr(), &mut state, kernel.shared_table().as_ptr().cast_mut(), table.as_ptr(), lanes) };
        assert_eq!(output, [-0.625; 4], "vector displacement limit {limit}");
    }
}

fn effect_output(shader: &AudioShaderRef, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![0.0; l.len()], vec![0.0; l.len()]);
    Instance::new(shader.clone(), RATE).process(l, r, &mut left, &mut right);
    (left, right)
}

// Complex spectrum of a real signal, in increasing bin order.
fn spectrum(samples: &[f32]) -> Vec<(f64, f64)> {
    let n = samples.len();
    assert!(n.is_power_of_two(), "spectrum needs a power-of-two sample count, got {n}");
    let mut bins = vec![(0.0, 0.0); n];
    for (i, &v) in samples.iter().enumerate() {
        bins[i.reverse_bits() >> (usize::BITS - n.ilog2())] = (f64::from(v), 0.0);
    }
    for stage in 1..=n.ilog2() {
        let width = 1 << stage;
        let half = width / 2;
        let (step_im, step_re) = (-std::f64::consts::TAU / width as f64).sin_cos();
        for group in bins.chunks_exact_mut(width) {
            let (mut re, mut im) = (1.0, 0.0);
            for i in 0..half {
                let (x, y) = group[i + half];
                let (a, b) = (x * re - y * im, x * im + y * re);
                let (u, v) = group[i];
                group[i] = (u + a, v + b);
                group[i + half] = (u - a, v - b);
                (re, im) = (re * step_re - im * step_im, re * step_im + im * step_re);
            }
        }
    }
    bins.truncate(n / 2 + 1);
    bins
}

fn amplitude_db((re, im): (f64, f64)) -> f64 { 20.0 * re.hypot(im).log10() }

// Squared magnitude of a DFT bin over one complete, settled signal period.
fn period_bin_power(period: &[f32], bin: usize) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &sample) in period.iter().enumerate() {
        let (s, c) = (std::f64::consts::TAU * (i * bin) as f64 / period.len() as f64).sin_cos();
        re += f64::from(sample) * c;
        im += f64::from(sample) * s;
    }
    re * re + im * im
}

fn sweep_rejection_db(
    plain: &AudioShaderRef,
    raised: &AudioShaderRef,
    harmonic_step: usize,
    configure: impl Fn(&mut Instance),
) -> f64 {
    const SETTLE: usize = 9600;
    const WINDOW: usize = 480;
    const FUNDAMENTALS: [usize; 12] = [31, 37, 43, 47, 53, 61, 67, 71, 79, 83, 89, 97];

    let sine: [f32; WINDOW] = std::array::from_fn(|i| (std::f64::consts::TAU * i as f64 / WINDOW as f64).sin() as f32);
    let alias_power = |shader: &AudioShaderRef, input: &[f32], fundamental| {
        let mut inst = Instance::new(shader.clone(), RATE);
        configure(&mut inst);
        let (mut left, mut right) = (vec![0.0; input.len()], vec![0.0; input.len()]);
        inst.process(input, input, &mut left, &mut right);
        let window = &left[SETTLE..];
        // Parseval gives the power in bins 1..239 after removing DC and Nyquist and halving.
        let total = WINDOW as f64 * window.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
        let positive = (total - period_bin_power(window, 0) - period_bin_power(window, WINDOW / 2)) * 0.5;
        let harmonics = (fundamental..WINDOW / 2).step_by(harmonic_step * fundamental).map(|k| period_bin_power(window, k)).sum::<f64>();
        positive - harmonics
    };

    let (mut plain_sum, mut raised_sum) = (0.0, 0.0);
    for fundamental in FUNDAMENTALS {
        let input: Vec<_> = (0..SETTLE + WINDOW).map(|i| sine[(i * fundamental) % WINDOW]).collect();
        plain_sum += alias_power(plain, &input, fundamental);
        raised_sum += alias_power(raised, &input, fundamental);
    }
    10.0 * (plain_sum / raised_sum).log10()
}

#[test]
fn library_drive_cost_and_alias_rejection() {
    let drive = include_str!("../../../../libs/score_player/shaders/drive.splash");
    let report = check_with(drive, &AudioBindings::default()).unwrap();
    eprintln!("drive check {:.6} ops/base frame", report.ops_per_frame);
    assert!(report.ops_per_frame < 3172.83, "drive must cost less than the old 64-tap drive (3172.83 ops/base frame), got {}", report.ops_per_frame);

    let mut base = AudioBindings::default();
    base.oversample.factor = 1;
    let plain = compile_audio_with(drive, Backend::Native, false, &base).into_result().unwrap();
    let raised = build(drive, Backend::Native);
    // At exactly 5 kHz, hard mode loses 1.25 dB from edge phase on the 4x grid at that rational frequency. Its neighbours improve.
    for (name, mode, floor) in [("soft", 0.0, 16.5), ("hard", 1.0, 15.5)] {
        let db = sweep_rejection_db(&plain, &raised, 2, |inst| {
            assert!(inst.set_param("amount", 1.0), "drive must accept the sweep's full amount");
            assert!(inst.set_param("mode", mode), "drive must accept sweep mode {mode}");
        });
        eprintln!("drive {name} power-summed rejection {db:.6} dB");
        assert!(db.is_finite() && db >= floor, "{name} drive power-summed alias rejection must reach {floor} dB, got {db:.6} dB");
    }
}

#[test]
fn waveshaper_rejection_and_region_costs() {
    let waveshaper = include_str!("../shaders/waveshaper4x.splash");
    let kick = include_str!("../../../../libs/score_player/shaders/techno_kick.splash");
    let ops = |src| check_with(src, &AudioBindings::default()).unwrap().ops_per_frame;
    let wave_ops = ops(waveshaper);
    let kick_ops = ops(kick);
    eprintln!("waveshaper4x {wave_ops:.6} ops/base frame, techno_kick {kick_ops:.6} ops/voice/base frame");
    assert!(wave_ops < 2974.77, "waveshaper4x must cost less than 2974.77 ops/base frame, got {wave_ops}");
    assert!(kick_ops < 1775.60, "techno_kick must cost less than 1775.60 ops/voice/base frame, got {kick_ops}");

    // Fully wet at the waveshaper's default drive and bias, evaluated at base rate.
    let plain = build("var dc = OnePole{}
        fn block() { onepole_set(dc, 10.0) }
        fn effect(l,r) {
            let shaped = tanh((l + 0.1) * 4.0) - tanh(0.1 * 4.0)
            onepole_hp(dc, shaped) * (0.75 / tanh(4.0))
        }", Backend::Native);
    let raised = build(waveshaper, Backend::Native);
    // The bias produces both odd and even harmonics, so exclude every multiple.
    let db = sweep_rejection_db(&plain, &raised, 1, |_| {});
    eprintln!("waveshaper4x power-summed rejection {db:.6} dB");
    assert!(db.is_finite() && db >= 18.5, "waveshaper4x power-summed alias rejection must reach 18.5 dB, got {db:.6} dB");
}

#[test]
fn halfband_stages_and_round_trips() {
    // An isolated impulse, then a 3/8-rate sine with an integral number of
    // periods after the longest filter has filled.
    let mut input = [0.0f32; 384];
    input[0] = 1.0;
    for (i, x) in input[128..].iter_mut().enumerate() {
        *x = (std::f64::consts::TAU * (3 * i % 8) as f64 / 8.0).sin() as f32;
    }
    let silence = [0.0; 384];
    let render = |shader: &AudioShaderRef| {
        let mut inst = Instance::new(shader.clone(), RATE);
        let (mut l, mut r) = ([0.0; 384], [0.0; 384]);
        inst.process(&input, &silence, &mut l, &mut r);
        (l, inst.ops() as f64 / input.len() as f64)
    };
    let counted = |src: &str| audition_shader(src, Backend::Native, true);
    let (_, baseline) = render(&counted("fn effect(l,r) { vec2(l,r) }"));

    for (factor, latency, stage, up) in [
        (2usize, 34usize, "Halfband2", "halfband_up2"),
        (4, 38, "HalfbandStage4", "halfband_stage4_up"),
        (8, 40, "HalfbandStage8", "halfband_stage8_up"),
    ] {
        let src = format!("var s = {stage}{{}} fn effect(l,r) {{ {up}(s,l) }}");
        let shader = build(&src, Backend::Native);
        let (even, odd) = effect_output(&shader, &input[..64], &silence[..64]);
        // Undo the interpolation gain, interleave the phases and zero-pad
        // the measured response to resolve the peaks between the tap zeros.
        let mut kernel = vec![0.0; 16384];
        for (dst, (a, b)) in kernel.chunks_exact_mut(2).zip(even.into_iter().zip(odd)) {
            dst.copy_from_slice(&[0.5 * a, 0.5 * b]);
        }
        let pass = (kernel.len() as f64 * 0.42 / factor as f64) as usize;
        let stop = kernel.len() / 2 - pass;
        let bins = spectrum(&kernel);
        let ripple = bins[..=pass].iter().map(|&v| amplitude_db(v).abs()).fold(0.0, f64::max);
        let rejection = bins[stop..].iter().map(|&v| amplitude_db(v)).fold(f64::NEG_INFINITY, f64::max);
        assert!(ripple <= 0.05, "{factor}x stage pass-band deviation {ripple} dB");
        assert!(rejection <= -80.0, "{factor}x stage stop-band peak {rejection} dB");

        let body = if factor == 8 {
            "halfband_up8(u,l,pairs)\nvec2(halfband_down8(d,pairs),r)".to_owned()
        } else {
            format!("vec2(halfband_down{factor}(d,halfband_up{factor}(u,l)),r)")
        };
        let pairs = if factor == 8 { "var pairs = [vec2(0.0); 4]" } else { "" };
        let src = format!("latency(HALFBAND_LATENCY_{factor})
            var u = Halfband{factor}{{}} var d = Halfband{factor}{{}} {pairs}
            fn effect(l,r) {{ {body} }}");
        let shader = counted(&src);
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        assert_eq!(shader.backend(), Backend::Native, "{factor}x native code generation");
        assert_eq!(shader.latency(), latency as u32, "{factor}x declared round-trip latency");
        let (out, cost) = render(&shader);
        let peak = (0..128).max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs())).unwrap();
        assert_eq!(peak, latency, "{factor}x round-trip impulse peak");
        let mut error = 0.0;
        let mut power = 0.0;
        for i in 256..out.len() {
            let delayed = f64::from(input[i - latency]);
            error += (f64::from(out[i]) - delayed).powi(2);
            power += delayed * delayed;
        }
        let null = 10.0 * (error / power).log10();
        assert!(null < -70.0, "{factor}x sine null against a {latency}-frame delay is {null} dB");
        eprintln!("{factor}x stage ripple {ripple:.5} dB, stop {rejection:.2} dB, round-trip null {null:.2} dB, {:.0} ops", cost - baseline);
        if factor == 4 {
            // The previous 64-tap converter added 1214 ops to the mono path.
            assert!(cost - baseline < 1214.0, "4x mono conversion adds {} ops/frame", cost - baseline);
        }
        let even = &[128][..];
        let ragged = &[3, 1, 42, 7, 95][..];
        assert_render_agreement(&format!("{factor}x half-band"), &shader, input.len(), &[ragged], &[even, ragged], &[even, ragged]);
    }
}

#[test]
fn prelude_maximally_flat_orders() {
    // Use whole periods and let the slowest pole settle. Project only onto
    // the input tone, so even the sixth-order stop band is measurable.
    let cycle = |period: usize| -> Vec<f32> {
        (0..period).map(|i| (std::f64::consts::TAU * i as f64 / period as f64).sin() as f32).collect()
    };
    let gain = |out: &[f32], period: usize| {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in out[out.len() - period..].iter().enumerate() {
            let (s, c) = (std::f64::consts::TAU * i as f64 / period as f64).sin_cos();
            re += f64::from(x) * c;
            im += f64::from(x) * s;
        }
        20.0 * (2.0 * re.hypot(im) / period as f64).log10()
    };
    for order in [1, 2, 3, 6] {
        let src = if order == 1 {
            "var a = OnePole{} var b = OnePole{}
             fn block() { butter_real_set(a,46.875) butter_real_set(b,750.0) }
             fn effect(l,r) { vec2(butter_real_lp(a,l), r - butter_real_lp(b,r)) }".to_owned()
        } else {
            let (suffix, lp, hp) = if order % 2 == 0 { ("", "lp", "hp") } else { ("_odd", "lp, a", "hp, b") };
            format!("var lp = [Svf{{}}; {n}] var hp = [Svf{{}}; {n}] var a = OnePole{{}} var b = OnePole{{}}
                fn block() {{ butter_set{suffix}({lp},46.875) butter_set{suffix}({hp},750.0) }}
                fn effect(l,r) {{ vec2(butter_lp{suffix}({lp},l), butter_hp{suffix}({hp},r)) }}", n = order / 2)
        };
        let shader = build(&src, Backend::Native);
        for octave in [0, 1, 4] {
            let (pl, ph) = (1024 >> octave, 64 << octave);
            let frames = 24576 + pl.max(ph);
            let l: Vec<_> = cycle(pl).into_iter().cycle().take(frames).collect();
            let r: Vec<_> = cycle(ph).into_iter().cycle().take(frames).collect();
            let (left, right) = effect_output(&shader, &l, &r);
            let expected = -10.0 * (1.0 + 2.0f64.powi(2 * order * octave)).log10();
            let (low, high) = (gain(&left, pl), gain(&right, ph));
            eprintln!("order {order}, {octave} octaves: low {low:.4}, high {high:.4} dB");
            assert!((low - expected).abs() < 0.1, "order {order} low-pass at +{octave} octaves: {low} vs {expected} dB");
            assert!((high - expected).abs() < 0.1, "order {order} high-pass at -{octave} octaves: {high} vs {expected} dB");
        }
    }
}

#[test]
fn prelude_crossover_recombines() {
    let src = r#"
        var a = Split4{} var b = Split4{} var phase = Svf{}
        fn block() {
            split4_set(a, 1500.0)
            split4_set(b, 4500.0)
            svf_set(phase, 4500.0, 0.707106781)
        }
        fn effect(l,r) {
            let v = split4(a,l)
            let upper = split4(b,v.y)
            OUTPUT
        }
    "#;
    let mut impulse = vec![0.0; 8192];
    impulse[0] = 1.0;
    for three in [false, true] {
        let output = if three { "vec2(svf_ap(phase,v.x), upper.x + upper.y)" } else { "v" };
        let shader = build(&src.replace("OUTPUT", output), Backend::Native);
        let (l, r) = effect_output(&shader, &impulse, &impulse);
        let (lo, hi) = (spectrum(&l), spectrum(&r));
        let mut worst = 0.0f64;
        for (i, (&(a, b), &(c, d))) in lo.iter().zip(&hi).enumerate() {
            let hz = i as f64 * f64::from(RATE) / impulse.len() as f64;
            if (20.0..=20000.0).contains(&hz) {
                let db = amplitude_db((a + c, b + d));
                assert!(db.abs() <= 0.05, "three bands {three}: sum {db} dB at {hz} Hz");
                worst = worst.max(db.abs());
            }
        }
        let at = 256; // 1500 Hz
        let (low, high) = (amplitude_db(lo[at]), amplitude_db(hi[at]));
        assert!((low + 6.0206).abs() < 0.05 && (high + 6.0206).abs() < 0.05,
            "three bands {three}: complementary sides at crossover: {low}, {high} dB");
        eprintln!("three bands {three}: sum ripple {worst:.6} dB, split {low:.4}/{high:.4} dB");
    }
}

#[test]
fn prelude_equaliser_gains() {
    let mut impulse = vec![0.0; 4096];
    impulse[0] = 1.0;
    for (name, at) in [("bell", 128), ("low_shelf", 2), ("high_shelf", 1700)] {
        for db in [-9.0f64, 6.0] {
            let src = format!("var e = Eq{{}} var flat = Eq{{}}
                fn block() {{ eq_{name}_set(e,1500.0,0.707106781,{db})\n eq_{name}_set(flat,1500.0,0.707106781,0) }}
                fn effect(l,r) {{ vec2(eq(e,l),eq(flat,r)) }}");
            let shader = build(&src, Backend::Native);
            let (l, r) = effect_output(&shader, &impulse, &impulse);
            assert!(r.iter().zip(&impulse).all(|(a, b)| (a - b).abs() <= 1e-6), "{name}: 0 dB must pass the impulse unchanged");
            let response = spectrum(&l);
            let measured = amplitude_db(response[at]);
            assert!((measured - db).abs() < 0.05, "{name}: measured {measured} dB for {db} dB");
            let corner = amplitude_db(response[128]);
            let want = if name == "bell" { db } else { db * 0.5 };
            assert!((corner - want).abs() < 0.05, "{name}: corner {corner} vs {want} dB");
            if name != "bell" {
                let unity = amplitude_db(response[if at == 2 { 1700 } else { 2 }]);
                assert!(unity.abs() < 0.05, "{name}: opposite end has gain {unity} dB");
            }
            eprintln!("{name}: {db} dB measured {measured:.4}, corner {corner:.4} dB");
        }
    }
}

#[test]
fn prelude_multimode_advances_once() {
    let src = r#"
        var f = [Svf{}; 4]
        fn block() { for i in 0..4 { svf_set(f[i],1500.0,0.8) } }
        fn effect(l,r) {
            let v = svf_multi(f[0],l)
            let err = abs(v.x-svf_lp(f[1],l)) + abs(v.y-svf_bp(f[2],l)) + abs(v.z-svf_hp(f[3],l))
            vec2(err, v.x + v.y + v.z - l)
        }
    "#;
    let shader = build(src, Backend::Native);
    let (l, r) = test_input(1024);
    let (err, sum) = effect_output(&shader, &l, &r);
    assert!(err.iter().all(|e| *e == 0.0), "simultaneous modes differ from three independent filters");
    let worst = sum.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    assert!(worst <= 2e-7, "the three modes reconstruct the input: peak error {worst}");
    eprintln!("multimode reconstruction peak error {worst:.9}");
}

#[test]
fn prelude_noise_spectra() {
    const FRAMES: usize = 1 << 16;
    const WINDOW: usize = 1 << 14;
    let silence = vec![0.0; FRAMES];
    let noise = build("var p = Pink{} var b = Brown{} fn effect(l,r) { vec2(pink(p),brown(b)) }", Backend::Native);
    let white = build("fn effect(l,r) { vec2(rand(),rand()) }", Backend::Native);
    let (pink, brown) = effect_output(&noise, &silence, &silence);
    let (wp, wb) = effect_output(&white, &silence, &silence);
    let peak = brown.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    assert!(peak <= 1.0, "brown noise exceeded full scale: {peak}");
    let taper: Vec<_> = (0..WINDOW).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / WINDOW as f64).cos()) as f32).collect();
    let power = |signal: &[f32]| {
        let mut sum = vec![0.0; WINDOW / 2 + 1];
        for segment in signal.chunks_exact(WINDOW) {
            let tapered: Vec<_> = segment.iter().zip(&taper).map(|(x, w)| x * w).collect();
            for (v, (re, im)) in sum.iter_mut().zip(spectrum(&tapered)) { *v += re * re + im * im; }
        }
        sum
    };
    for (name, signal, excitation, exponent) in [("pink", pink, wp, 1), ("brown", brown, wb, 2)] {
        let (out, input) = (power(&signal), power(&excitation));
        // Use the very same white stream as a reference in each band. Its
        // finite-record power varies most at the bottom of the spectrum.
        let band = |hz: f64| {
            let (mut energy, mut reference) = (0.0, 0.0);
            let width = 2.0f64.powf(1.0 / 6.0);
            for bin in 1..input.len() {
                let f = bin as f64 * f64::from(RATE) / WINDOW as f64;
                if f >= hz / width && f <= hz * width {
                    energy += out[bin];
                    reference += input[bin] * (hz / f).powi(exponent);
                }
            }
            10.0 * (energy / reference).log10()
        };
        let slope = (band(10000.0) - band(100.0)) / 100.0f64.log2();
        let target = -10.0 * 2.0f64.log10() * f64::from(exponent);
        assert!((slope - target).abs() < 0.5, "{name} slope {slope} dB/octave from 100 Hz to 10 kHz");
        for low in [100.0, 200.0, 400.0, 800.0, 1600.0, 3200.0, 5000.0] {
            let change = band(2.0 * low) - band(low);
            assert!((change - target).abs() < 0.5, "{name} octave from {low} Hz changed by {change} dB");
        }
        if name == "pink" {
            let anchor = band(1000.0);
            let mut worst = 0.0f64;
            for i in 0..=30 {
                let hz = 20.0 * 1000.0f64.powf(f64::from(i) / 30.0);
                let error = band(hz) - anchor + 10.0 * (hz / 1000.0).log10();
                assert!(error.abs() < 0.5, "pink spectrum at {hz} Hz differs from the target by {error} dB");
                worst = worst.max(error.abs());
            }
            eprintln!("pink spectrum peak error {worst:.4} dB from 20 Hz to 20 kHz");
        }
        eprintln!("{name} slope {slope:.4} dB/octave from 100 Hz to 10 kHz");
    }
    eprintln!("brown peak {peak:.5}");
}

#[test]
fn prelude_lfo_follows_the_beat() {
    for shape in 0..5 {
        let src = format!("fn effect(l,r) {{ lfo_beat(0.5,{shape}) }}");
        let s = build(&src, Backend::Native);
        let mut scratch = s.new_scratch();
        let mut render = |slices: &[usize], step: f64, seed: u32| {
            perform_events(&s, slices, 128, &[0], |_, ctx, state| {
                s.seed_state(state, seed);
                s.set_time(ctx, 120.0, -0.5, 0, step, 4.0, step != 0.0);
            }, &mut |ctx, state, ins, [l, r], n| s.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n)).0
        };
        let output = render(&[128], 1.0 / 32.0, 3);
        assert_eq!(output, render(&[3, 1, 9], 1.0 / 32.0, 42), "shape {shape} changed with slicing or seed");
        assert!(output.iter().all(|x| (-1.0..=1.0).contains(x)), "shape {shape} out of range");
        let held = render(&[7], 0.0, 17);
        assert!(held.iter().all(|&x| x == output[0]), "shape {shape} advanced on a stopped beat");
        if shape == 4 {
            for cycle in output.chunks_exact(16) {
                assert!(cycle.iter().all(|x| *x == cycle[0]), "sample-and-hold changed within a cycle");
            }
            for frame in (16..output.len()).step_by(16) {
                assert_ne!(output[frame], output[frame - 1], "sample-and-hold repeated the previous cycle at frame {frame}");
            }
        } else {
            for (frame, &v) in output.iter().enumerate() {
                let p = (frame % 16) as f64 / 16.0;
                let expected = match shape {
                    0 => (std::f64::consts::TAU * p).sin(),
                    1 => 1.0 - 4.0 * (p - 0.5).abs(),
                    2 => 2.0 * p - 1.0,
                    _ => if p < 0.5 { 1.0 } else { -1.0 },
                };
                assert!((f64::from(v) - expected).abs() < 2e-6, "shape {shape} at phase {p}: got {v}, want {expected}");
            }
        }
    }
}

#[test]
fn prelude_player_reads_and_wraps_frames() {
    let data = [0.0, 0.25, -0.5, 0.75, 1.0, -0.25, -0.75, 0.5];
    let stereo: Vec<f32> = data.iter().flat_map(|&x| [x, -x]).collect();
    for channels in [1, 2] {
        let view = SampleView { name: "t", channels, frames: 8, data: if channels == 1 { &data } else { &stereo } };
        let player = |body: &str, speed: f32, start: f32| {
            let src = format!("let t = sample({channels})\nvar p = Player{{}}\nfn effect(l,r) {{
                if r > 0.0 {{ player_start(p,{start}) }}
                {body} }}");
            let shader = bound_shader(&src, &[view]);
            let mut reset = [0.0; 18];
            reset[0] = 1.0;
            effect_output(&shader, &[speed; 18], &reset)
        };
        for speed in [1.0, -1.0] {
            let (l, r) = player("play(p,t,l)", speed, if speed > 0.0 { 0.0 } else { 7.0 });
            for i in 0..l.len() {
                let want = if i >= 8 { 0.0 } else { data[if speed > 0.0 { i } else { 7 - i }] };
                assert_eq!((l[i], r[i]), (want, if channels == 2 { -want } else { want }), "one-shot {channels} channels, speed {speed}, frame {i}");
            }
        }
        let empty_call = if channels == 2 { "play_loop(p,t,l,3,3).y" } else { "play_loop(p,t,l,3,3)" };
        let empty = player(empty_call, 1.0, 3.0);
        assert!(empty.0.iter().chain(&empty.1).all(|v| *v == 0.0), "empty loop must be silent on both channels");
        let half = player("play(p,t,l)", 0.5, 0.0).0;
        // The midpoint is (-before + 9*x + 9*next - after)/16.
        for i in 0..8 {
            let midpoint = (-data[(i + 7) % 8] + 9.0 * data[i] + 9.0 * data[(i + 1) % 8] - data[(i + 2) % 8]) / 16.0;
            assert_eq!(half[2 * i + 1], midpoint, "one-shot cubic midpoint after frame {i}");
        }
        for speed in [1.0, -0.5, 8.5] {
            let (l, r) = player("play_loop(p,t,l,2,6)", speed, 2.0);
            for i in 0..l.len() {
                let at = (speed * i as f32).rem_euclid(4.0);
                let j = at as usize;
                let sample = |k: usize| data[2 + k % 4];
                let want = if at.fract() == 0.0 { sample(j) } else {
                    (-sample(j + 3) + 9.0 * sample(j) + 9.0 * sample(j + 1) - sample(j + 2)) / 16.0
                };
                assert_eq!((l[i], r[i]), (want, if channels == 2 { -want } else { want }),
                    "wrapped cubic read, {channels} channels, speed {speed}, frame {i}");
            }
        }
    }
}

#[test]
fn prelude_blocks_agree_on_every_backend_and_slicing() {
    let src = r#"
        let t = sample(2)
        var lp = [Svf{}; 3] var hp = [Svf{}; 1]
        var oddl = [Svf{}; 1] var oddh = [Svf{}; 2] var a = OnePole{} var b = OnePole{}
        var s = Split4{} var e = [Eq{}; 3] var f = Svf{} var real = OnePole{}
        var p = Pink{} var n = Brown{} var shot = Player{} var repeat = Player{}
        fn block() {
            butter_set(lp,1500.0) butter_set(hp,300.0)
            butter_set_odd(oddl,a,5000.0) butter_set_odd(oddh,b,150.0)
            split4_set(s,1200.0) svf_set(f,600.0,1.2) butter_real_set(real,800.0)
            eq_bell_set(e[0],800.0,1.0,3.0)
            eq_low_shelf_set(e[1],200.0,0.7,-2.0)
            eq_high_shelf_set(e[2],4000.0,0.7,2.0)
        }
        fn effect(l,r) {
            let mod = lfo_beat(0.25,int(floor(beat)) % 5)
            if mod > 0.75 { player_start(shot,2.0) }
            let x = l + mod * 0.1 + pink(p) * 0.05 + play(shot,t,1.5).x
            let y = r + brown(n) * 0.05 + play_loop(repeat,t,-0.5,1,9).y
            let v = split4(s, butter_lp(lp,x) + butter_hp(hp,y))
            let w = svf_multi(f,butter_lp_odd(oddl,a,x) + butter_hp_odd(oddh,b,y))
            vec2(eq(e[0],v.x) + butter_real_lp(real,w.x), eq(e[1],v.y) + eq(e[2],w.y) + w.z)
        }
    "#;
    let s = audition_shader(src, Backend::Native, false);
    let mut scratch = s.new_scratch();
    let mut wasm = WasmRunner::new(&s);
    let play = |slices: &[usize], run: &mut Run| {
        perform_events(&s, slices, 384, &[0], |_, ctx, _| s.set_time(ctx, 120.0, 0.0, 0, 1.0 / 64.0, 4.0, true), run)
    };
    let even = &[128][..];
    let ragged = &[3, 1, 42, 7, 95][..];
    let want = play(even, &mut |ctx, state, ins, [l, r], n| s.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n));
    for slices in [even, ragged] {
        for (name, got) in [
            ("interpreter", play(slices, &mut |ctx, state, ins, [l, r], n| s.run_interp(ctx, state, &mut scratch, ins[0], ins[1], l, r, n))),
            ("native", play(slices, &mut |ctx, state, ins, [l, r], n| s.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n))),
            ("wasm", play(slices, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n))),
        ] {
            assert_eq!(first_diff(&want.0, &got.0), None, "{name} left channel with slices {slices:?}");
            assert_eq!(first_diff(&want.1, &got.1), None, "{name} right channel with slices {slices:?}");
        }
    }
}

#[test]
fn oversample_regions() {
    let mut impulse = [0.0; 192];
    impulse[0] = 1.0;
    let sine = std::array::from_fn::<_, 192, _>(|i| (std::f64::consts::TAU * (3 * i % 8) as f64 / 8.0).sin() as f32);
    let slices: &[&[usize]] = &[&[128], &[3, 1, 42, 7, 95]];
    for (factor, latency, down) in [(2, 34, 17), (4, 38, 19), (8, 40, 20)] {
        let source = |body: &str| format!("latency(HALFBAND_LATENCY_{factor})
            var os = Oversample{factor}{{}}
            fn effect(l,r) {{ oversample(os, vec2(l,r), fn(u) {{ {body} }}) }}");
        let identity = source("u");
        let shader = build(&identity, Backend::Native);
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        assert_eq!(shader.backend(), Backend::Native, "{factor}x native code generation");
        assert_eq!(shader.latency() as usize, latency, "{factor}x declared latency");
        let (left, right) = effect_output(&shader, &impulse, &sine);
        let peak = (0..left.len()).max_by(|&a, &b| left[a].abs().total_cmp(&left[b].abs())).unwrap();
        assert_eq!(peak, latency, "{factor}x impulse peak");
        // This impulse originates inside the region. The local u shadows
        // the parameter, so only the down delay needs to be declared.
        let generated = build(&format!("latency(HALFBAND_DOWN_LATENCY_{factor})
            var os = Oversample{factor}{{}} var first = 1.0
            fn effect(l,r) {{ oversample(os,l,fn(u) {{ let u = first; first = 0.0; u }}) }}"), Backend::Native);
        assert_eq!(generated.latency() as usize, down, "{factor}x declared down latency");
        let generated = effect_output(&generated, &impulse, &sine).0;
        let down_peak = (0..generated.len()).max_by(|&a, &b| generated[a].abs().total_cmp(&generated[b].abs())).unwrap();
        assert_eq!(down_peak, down, "{factor}x generated impulse peak");
        for offset in 1..=down {
            let asymmetry = (generated[down - offset] - generated[down + offset]).abs();
            assert!(asymmetry < 1e-6, "{factor}x down impulse is off centre at offset {offset}: {asymmetry}");
        }
        let mut error = 0.0;
        let mut power = 0.0;
        for i in 96..right.len() {
            let want = f64::from(sine[i - latency]);
            error += (f64::from(right[i]) - want).powi(2);
            power += want * want;
        }
        let null = 10.0 * (error / power).log10();
        assert!(null < -70.0, "{factor}x identity sine null {null} dB");
        assert_render_agreement(&identity, &shader, sine.len(), slices, slices, slices);
        let ops = |code: &str| check_with(code, &AudioBindings::default()).unwrap().ops_per_frame;
        let identity_ops = ops(&identity);
        let extra = ops(&source("vec2(u.x + l, u.y)")) - identity_ops;
        eprintln!("{factor}x identity peak {peak}, null {null:.2} dB, {identity_ops:.2} stereo ops/frame, one extra add costs {extra:.2}");
        assert!((extra - factor as f64).abs() < 0.01, "{factor}x check counted {extra} ops/frame for one extra add per sub-sample");
    }

    let voice = build("latency(HALFBAND_LATENCY_4)
        var os = Oversample4{} var hits = 0.0 var steps = 0.0
        let base = [0.0; oversampling]
        fn voice() {
            let clock = sample_rate
            oversample(os, 0.0, fn(u) {
                let high = [0.0; 2 * oversampling]
                if sample_rate == clock * 4 && oversampling == 4 && len(high) == 8 { steps += 1.0 }
                if trigger { hits += 1.0 }
                u
            })
            vec2(hits, if sample_rate == 48000 && oversampling == 1 && len(base) == 1 { steps } else { -1.0 })
        }", Backend::Native);
    let mut inst = Instance::new(voice, RATE);
    for note in [60.0, 63.0] {
        inst.note_on(note, 1.0, 7);
        let (mut hits, mut steps) = ([0.0; 3], [0.0; 3]);
        inst.render(&mut hits, &mut steps);
        assert_eq!(hits, [1.0; 3], "4x trigger count for note {note}");
        assert_eq!(steps, [4.0, 8.0, 12.0], "4x clock, array sizes, state advances and restored base clock for note {note}");
    }

    let under = "oversample(os,l,fn(u) { u })";
    let unused = "oversample(os,l,fn(u) { u = 1.0; 0.0 })";
    let branch = "oversample(os,l,fn(u) { if false { u } else { 0.0 } })";
    let round_trip = "4x oversample region needs 38 frames of round-trip latency; declare latency(HALFBAND_LATENCY_4)";
    for (latency, body, at, message) in [
        (37, under.to_owned(), under, round_trip),
        (19, branch.to_owned(), branch, round_trip),
        (18, unused.to_owned(), unused, "4x oversample region needs 19 frames of down latency; declare latency(HALFBAND_DOWN_LATENCY_4)"),
        (38, "oversample(os,l,shape)".to_owned(), "shape", "write the oversample body here"),
        (38, "oversample(os,l,fn(u) { vec2(u) })".to_owned(), "fn(u) { vec2(u) }", "must return the input sample's type"),
    ] {
        let src = format!("latency({latency}) var os = Oversample4{{}} fn shape(u) {{ u }} fn effect(l,r) {{ {body} }}");
        let e = compile_err(&src);
        assert_eq!(&src[e.start..e.end], at, "region diagnostic span for {body}");
        assert!(e.message.contains(message), "region diagnostic for {body}: {}", e.message);
    }

    let wave = |pos: f64| (std::f64::consts::TAU * pos / 64.0).sin() as f32;
    let table: Vec<_> = (0..256).map(|i| wave(i as f64)).collect();
    let player = bound_shader("latency(HALFBAND_DOWN_LATENCY_4)
        let t = sample(1) var os = Oversample4{} var high = Player{} var plain = Player{}
        fn effect(l,r) { vec2(oversample(os,l,fn(u) { play(high,t,1.0) }), play(plain,t,1.0)) }",
        &[SampleView { name: "t", channels: 1, frames: table.len() as u32, data: &table }]);
    let silence = [0.0; 192];
    let (high, plain) = effect_output(&player, &silence, &silence);
    let player_error = (96..192).map(|i| (high[i] - plain[i - 19]).abs()).fold(0.0, f32::max);
    eprintln!("4x player peak error at 19 frames {player_error:.6}");
    assert!(player_error < 0.0005, "4x player against plain play delayed by 19 frames: peak error {player_error}");

    let tone = |khz: usize, level: f64| std::array::from_fn::<_, 192, _>(|i|
        (level * (std::f64::consts::TAU * (khz * i % 48) as f64 / 48.0).sin()) as f32);
    let shape = |curve: &str, input: &[f32]| {
        let shader = build(&format!("latency(HALFBAND_LATENCY_4) var os = Oversample4{{}} fn shape(x) {{ {curve} }}
            fn effect(l,r) {{ vec2(oversample(os,l,fn(u) {{ shape(u) }}), shape(l)) }}"), Backend::Native);
        effect_output(&shader, input, &silence)
    };
    // Only the 5 kHz fundamental and its third harmonic are in band.
    let (high, plain) = shape("hard_clip(x)", &tone(5, 2.0));
    let alias_power = |out: &[f32]| (1..24).filter(|&k| k != 5 && k != 15).map(|k| period_bin_power(&out[out.len() - 48..], k)).sum::<f64>();
    eprintln!("4x hard clipper alias rejection {:.2} dB", 10.0 * (alias_power(&plain) / alias_power(&high)).log10());

    // On [-0.75, 0.75] the cubic is monotonic. Its 33 kHz third harmonic
    // folds to 15 kHz at base rate, and is in the 4x cascade's stop band.
    let cubic = "x - x*x*x/3";
    let input = tone(11, 0.75);
    let (high, plain) = shape(cubic, &input);
    let explicit = build(&format!("var up = Halfband4{{}} var down = Halfband4{{}} fn shape(x) {{ {cubic} }}
        fn effect(l,r) {{ let v = halfband_up4(up,l)
            halfband_down4(down,vec4(shape(v.x),shape(v.y),shape(v.z),shape(v.w))) }}"), Backend::Native);
    let reference = effect_output(&explicit, &input, &silence).0;
    assert_eq!(first_diff(&high, &reference), None, "4x cubic against explicit up, shape and down conversion");
    let base_alias = period_bin_power(&plain[144..], 15);
    let expected = 0.75f64.powi(3) / 12.0;
    assert!((base_alias.sqrt() / 24.0 - expected).abs() < 1e-6, "1x cubic third harmonic at 15 kHz");
    let rejection = 10.0 * (base_alias / period_bin_power(&high[144..], 15)).log10();
    eprintln!("4x cubic alias rejection at 15 kHz {rejection:.2} dB");
    assert!(rejection >= 60.0, "4x cubic alias rejection at 15 kHz for an 11 kHz sine: {rejection:.2} dB");
}

#[test]
fn whole_shader_rate() {
    let identity = "fn effect(l,r) { vec2(l,r) }";
    let plain = build(identity, Backend::Native);
    let one = build(&format!("rate(1) {identity}"), Backend::Native);
    assert_eq!(one.program_key(), plain.program_key(), "rate(1) changed the render program");
    assert_eq!(one.new_state(), plain.new_state(), "rate(1) added state");
    assert_eq!(one.latency(), 0, "rate(1) added latency");

    let peak = |xs: &[f32]| (0..xs.len()).max_by(|&i, &j| xs[i].abs().total_cmp(&xs[j].abs())).unwrap();
    let mut impulse = [0.0; 256];
    impulse[0] = 1.0;
    let sine = std::array::from_fn::<_, 256, _>(|i| (std::f64::consts::TAU * (3 * i % 8) as f64 / 8.0).sin() as f32);
    let slices: &[&[usize]] = &[&[64], &[7, 1, 29, 4, 83]];
    let ops = |src: &str| check_with(src, &AudioBindings::default()).unwrap().ops_per_frame;
    for (factor, delay) in [(2, 34), (4, 38), (8, 40)] {
        let src = format!("rate({factor}) {identity}");
        let shader = build(&src, Backend::Native);
        let (left, right) = effect_output(&shader, &impulse, &sine);
        assert_eq!(shader.latency(), delay as u32, "{factor}x effect latency");
        assert_eq!(peak(&left), delay, "{factor}x impulse peak");
        let (mut error, mut power) = (0.0, 0.0);
        for i in 96..right.len() {
            let reference = f64::from(sine[i - delay]);
            error += (f64::from(right[i]) - reference).powi(2);
            power += reference * reference;
        }
        let null = 10.0 * (error / power).log10();
        eprintln!("whole-rate {factor}x identity null {null:.2} dB");
        assert!(null < -70.0, "{factor}x delayed 18 kHz sine null {null:.2} dB");
        if factor == 4 {
            assert_render_agreement("4x effect", &shader, sine.len(), slices, slices, slices);
            eprintln!("whole-rate 4x stereo identity {:.2} ops/frame", ops(&src));
        }
    }
    let declared = build(&format!("latency(5) rate(k*2) let k: i32 = 2 {identity}"), Backend::Interp);
    assert_eq!(declared.latency(), 43, "declared base latency plus 4x conversion");

    let table = SampleView { name: "t", channels: 1, frames: 2, data: &[0.0; 2] };
    let bound = bound_shader(&format!("rate(len(t)) let t = sample(1) {identity}"), &[table]);
    assert_eq!(bound.latency(), 34, "bound table length selects the constant factor");

    // The right channel marks the exact high-rate sample of note-on and glide arrival.
    let voice = "var phase = 0.0 var arrived = 0
        fn voice() {
            phase = fract(phase + freq / sample_rate)
            let mark = trigger || (arrived == 0 && note == 81)
            if note == 81 { arrived = 1 }
            vec2(gate * sin(TAU * phase), if mark { 1.0 } else { 0.0 })
        }";
    let at = 37usize;
    let slide = 1800usize;
    let mut arrivals = Vec::new();
    for factor in [1, 4] {
        let src = format!("rate({factor}) {voice}");
        let shader = build(&src, Backend::Native);
        let delay = if factor == 1 { 0 } else { 19 };
        assert_eq!(shader.latency(), delay as u32, "{factor}x voice down latency");
        let mut scratch = shader.new_scratch();
        let (tone, marks) = perform_events(&shader, slices[1], 2000, &[at, slide], |frame, _, state| {
            if frame == at { shader.note_on(state, 69.0, 1.0, frame as u32, 7, 0); }
            else { shader.glide(state, 81.0, 1.0, 48.0 / RATE, RATE); }
        }, &mut |ctx, state, ins, [l, r], n| shader.run(ctx, state, &mut scratch, ins[0], ins[1], l, r, n));
        let hz = pitch(&tone[128..1664], RATE, 400.0, 480.0);
        assert!((hz - 440.0).abs() < 1.0, "{factor}x sine voice pitch {hz} Hz");
        assert_eq!(peak(&marks[at..at + 80]), delay, "{factor}x note-on marker latency");
        let hit_area: f32 = marks[at..at + 80].iter().sum();
        assert!((hit_area - 1.0 / factor as f32).abs() < 1e-4, "{factor}x trigger must fire on one sub-sample: area {hit_area}");
        arrivals.push(slide + peak(&marks[slide..]) - delay);
        if factor == 4 {
            assert_render_agreement("4x sine voice", &shader, sine.len(), slices, slices, slices);
            eprintln!("whole-rate 4x sine voice {hz:.2} Hz, {:.2} ops/frame", ops(&src));
        }
    }
    assert_eq!(arrivals, [slide + 47; 2], "glide reaches its target on base frame 48 at both rates");

    let mut click = vec![0.0; 2048];
    click[0] = 1.0;
    for factor in [1, 4] {
        let filter = build(&format!("rate({factor}) var f = Svf{{}} var calls = 0.0
            fn block() {{ calls += 1; svf_set(f, 1500, 6) }}
            fn effect(l,r) {{ probe(blocks,calls); svf_bp(f,l) }}"), Backend::Native);
        let response = spectrum(&effect_output(&filter, &click, &click).0);
        let db: Vec<_> = response.into_iter().map(amplitude_db).collect();
        let bin = (0..db.len()).max_by(|&i, &j| db[i].total_cmp(&db[j])).unwrap();
        assert_eq!(bin, 64, "{factor}x filter set in block() must peak at 1500 Hz");
        let mut ctx = filter.new_ctx(RATE);
        filter.run(&mut ctx, &mut filter.new_state(), &mut filter.new_scratch(),
            &click, &click, &mut [0.0; 65], &mut [0.0; 65], 65);
        assert_eq!(filter.probe_read(&ctx, 0), Some((2.0, 1.0, 2.0)), "{factor}x control block cadence");
    }

    let delay = build("var ring = [0.0; 4800 * oversampling] var w = int(0)
        let config = [0.0; 1] fn init() { config[0] = oversampling }
        rate(4) fn effect(l,r) { let y = ring[w]; ring[w] = l; w = (w+1) % len(ring); y * config[0] / 4 }",
        Backend::Native);
    let mut click = vec![0.0; 4900];
    click[0] = 1.0;
    let (delayed, _) = effect_output(&delay, &click, &click);
    assert_eq!(peak(&delayed), 4800 + 38, "4x ring range must be 0.1 seconds plus conversion");
    let reference = build(&format!("rate(4) {identity}"), Backend::Native);
    // Swap while an impulse is still in the filter and move its state offset.
    let edited = build(&format!("var pad = [0.0; 5] rate(4) {identity}"), Backend::Native);
    let mut ctx = reference.new_ctx(RATE);
    let mut before = reference.new_state();
    reference.run(&mut ctx, &mut before, &mut reference.new_scratch(), &impulse, &sine,
        &mut [0.0; 8], &mut [0.0; 8], 8);
    let mut after = edited.new_state();
    edited.carry_state(&mut after, &reference, &before);
    let run_tail = |shader: &AudioShaderRef, state: &mut [u32]| {
        let (mut l, mut r) = ([0.0; 80], [0.0; 80]);
        let mut ctx = shader.new_ctx(RATE);
        ctx[CTX_FRAME as usize] = 8;
        shader.run(&mut ctx, state, &mut shader.new_scratch(), &[0.0; 80], &[0.0; 80], &mut l, &mut r, 80);
        (bits(&l), bits(&r))
    };
    assert_eq!(run_tail(&edited, &mut after), run_tail(&reference, &mut before), "4x cascade history across an edit");
    let (reference, _) = effect_output(&reference, &impulse, &sine);
    assert_eq!(first_diff(&delayed[4800..], &reference[..100]), None, "4x init factor and delayed impulse shape");

    // Both renders use the same seed and draw once per sub-sample.
    // Divide their tapered spectra to remove the finite white record's colour.
    let noise = build("rate(4) let white = param(0) var p = Pink{}
        fn voice() { if white > 0.5 { rand() } else { pink(p) } }", Backend::Native);
    let noise_power = |white| {
        let mut inst = Instance::new(noise.clone(), RATE);
        inst.set_param("white", white);
        inst.note_on(69.0, 1.0, 17);
        let mut out = vec![0.0; 16384];
        inst.render(&mut out, &mut vec![0.0; 16384]);
        for (i, x) in out.iter_mut().enumerate() {
            *x *= (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / 16384.0).cos()) as f32;
        }
        spectrum(&out).into_iter().map(|(re, im)| re * re + im * im).collect::<Vec<_>>()
    };
    let (pink, white) = (noise_power(0.0), noise_power(1.0));
    let band = |lo: usize| {
        let bin = |hz: usize| (hz * 16384).div_ceil(RATE as usize);
        let range = bin(lo)..bin(lo * 2);
        let measured = pink[range.clone()].iter().sum::<f64>();
        let reference = range.map(|i| white[i] * lo as f64 / (i as f64 * f64::from(RATE) / 16384.0)).sum::<f64>();
        10.0 * (measured / reference).log10()
    };
    let target = -10.0 * 2.0f64.log10();
    let mut worst = 0.0f64;
    for lo in [100, 200, 400, 800, 1600, 3200] {
        let slope = band(2 * lo) - band(lo);
        worst = worst.max((slope - target).abs());
        assert!((slope - target).abs() < 0.5, "4x pink octave at {lo} Hz: {slope:.3} dB vs {target:.3}");
    }
    eprintln!("whole-rate 4x pink maximum octave error {worst:.3} dB (100 Hz to 12.8 kHz)");

    for src in ["rate(4) var n = 0 fn voice() { n += 1; probe(tick, n); 0.0 }",
        "latency(19) var os = Oversample4{} var n = 0 fn voice() { oversample(os,0.0,fn(u) { n += 1; probe(tick, n); 0.0 }) }"] {
        let shader = build(src, Backend::Native);
        let mut ctx = shader.new_ctx(RATE);
        shader.run(&mut ctx, &mut shader.new_state(), &mut shader.new_scratch(),
            &[0.0; 3], &[0.0; 3], &mut [0.0; 3], &mut [0.0; 3], 3);
        assert_eq!(shader.probe_read(&ctx, 0), Some((12.0, 4.0, 12.0)), "only final sub-samples enter the probe range: {src}");
    }
    for (prefix, at, body, msg) in [
        ("", "rate(3)", identity, "constant factor"),
        ("", "rate()", identity, "one argument"),
        ("let knob = param(1)", "rate(knob)", identity, "constant factor"),
        ("var k = 2", "rate(0*k+2)", identity, "constant factor"),
        ("rate(1)", "rate(2)", identity, "twice"),
    ] {
        let src = format!("{prefix} {at} {body}");
        let e = compile_err(&src);
        assert_eq!((e.start, e.end), (prefix.len() + 1, prefix.len() + 1 + at.len()), "diagnostic span for {at}");
        assert!(e.message.contains(msg), "diagnostic for {at}: {}", e.message);
    }
    for (factor, allowed) in [(1, true), (4, false)] {
        let src = format!("let k = {factor} rate(k) {identity}");
        let nodes = [FuseNode { name: "a", code: identity, inputs: vec![Port::Input] },
            FuseNode { name: "b", code: &src, inputs: vec![Port::Node(0)] }];
        let fused = fuse(&nodes, &[Port::Node(1)], Backend::Interp);
        if allowed { assert!(fused.is_ok(), "rate(1) node must fuse"); } else {
            let e = fused.unwrap_err();
            assert_eq!(e.node, Some(1), "raised rate fusion node");
            assert_eq!(&src[e.error.start..e.error.end], "rate(k)", "raised rate fusion declaration span");
            assert!(e.error.message.contains("cannot be fused"), "raised rate fusion diagnostic");
        }
    }
}

#[test]
fn raised_rate_conversion_modes() {
    let identity = "fn effect(l,r) { vec2(l,r) }";
    let voice = "var p = 0.0 fn voice() { p = fract(p + freq / sample_rate); 2*p - 1 }";
    let slices: &[&[usize]] = &[&[32], &[1, 7, 2, 13]];
    let mut impulse = [0.0; 64];
    impulse[0] = 1.0;
    let (_, signal) = test_input(impulse.len());
    let mut failures = Vec::new();

    for (factor, delay) in [(2, 17), (4, 19), (8, 20)] {
        for (up, down, latency) in [
            ("filter", "filter", 2 * delay), ("filter", "linear", delay), ("filter", "hold", delay - 1),
            ("linear", "filter", if factor == 2 { delay } else { delay + 1 }), ("linear", "linear", 0), ("linear", "hold", 0),
            ("hold", "filter", delay), ("hold", "linear", 0), ("hold", "hold", 0),
        ] {
            let whole = format!("rate({factor}, {up}, {down}) {identity}");
            let region = format!("latency({latency}) var os = Oversample{factor}{{}}
                fn effect(l,r) {{ oversample(os, vec2(l,r), fn(u) {{ u }}, {up}, {down}) }}");
            for src in [&whole, &region] {
                let shader = build(src, Backend::Native);
                #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
                assert_eq!(shader.backend(), Backend::Native, "native conversion for {src}");
                assert_eq!(shader.latency(), latency, "declared conversion latency for {src}");
                let (left, right) = effect_output(&shader, &impulse, &signal);
                let peak = (0..left.len()).max_by(|&i, &j| left[i].abs().total_cmp(&left[j].abs())).unwrap();
                // Either frame is a peak if two adjacent phases have equal magnitude.
                let at_latency = left[latency as usize].abs();
                if at_latency != left[peak].abs() || at_latency == 0.0 {
                    failures.push(format!("{src}: impulse magnitude {at_latency:.6} at declared frame {latency}, peak {:.6} at frame {peak}", left[peak].abs()));
                }
                if up == down && up != "filter" {
                    for (input, output) in [(&impulse[..], left), (&signal[..], right)] {
                        let mut prev = 0.0;
                        for (i, (&x, y)) in input.iter().zip(output).enumerate() {
                            let want = if up == "hold" { x } else {
                                let sum: f32 = (1..factor).map(|j| prev + (x - prev) * (j as f32 / factor as f32)).sum();
                                (sum + x) / factor as f32
                            };
                            assert_eq!(y.to_bits(), want.to_bits(), "identity frame {i} for {src}");
                            prev = x;
                        }
                    }
                }
                assert_render_agreement(src, &shader, impulse.len(), slices, slices, slices);
            }
        }
    }

    for (mode, latency) in [("hold", 0), ("linear", 0), ("filter", 19)] {
        let src = format!("rate(4, {mode}) {voice}");
        let shader = build(&src, Backend::Native);
        assert_eq!(shader.latency(), latency, "saw down conversion with {mode}");
        assert_render_agreement(&src, &shader, impulse.len(), slices, slices, slices);
    }
    for src in ["var os = Oversample4{} fn effect(l,r) { oversample(os,l,fn(u) { 0.25 },filter,hold) }",
        "rate(4, filter, hold) fn effect(l,r) { 0.25 }"] {
        let unused = build(src, Backend::Native);
        assert_eq!(unused.latency(), 0, "unused input has no up latency for {src}");
        assert!(effect_output(&unused, &impulse, &signal).0.iter().all(|&x| x == 0.25), "unused up filter affects output for {src}");
    }
    let down_only = build("rate(4, linear, filter) fn effect(l,r) { 0.25 }", Backend::Native);
    assert_eq!(down_only.latency(), 19, "unused input contributes no linear up delay to the down filter");

    let under = "oversample(os,l,fn(u) { u },filter,hold)";
    let average = "oversample(os,l,fn(u) { u },filter,linear)";
    let interpolate = "oversample(os,l,fn(u) { u },linear,filter)";
    for (src, at, msg) in [
        (format!("rate(4, cubic) {identity}"), "cubic", "filter, linear or hold"),
        (format!("rate(4, hold, filter) {voice}"), "rate(4, hold, filter)", "an instrument converts down only; write rate(k, mode)"),
        ("var os = Oversample2{} fn effect(l,r) { oversample(os,l,fn(u) { u },hold,1+1) }".into(), "1+1", "filter, linear or hold"),
        (format!("latency(17) var os = Oversample4{{}} fn effect(l,r) {{ {under} }}"), under, "18 frames of up latency; declare latency(HALFBAND_DOWN_LATENCY_4 - 1)"),
        (format!("latency(18) var os = Oversample4{{}} fn effect(l,r) {{ {average} }}"), average, "19 frames of up latency; declare latency(HALFBAND_DOWN_LATENCY_4)"),
        (format!("latency(19) var os = Oversample4{{}} fn effect(l,r) {{ {interpolate} }}"), interpolate, "20 frames of down latency; declare latency(HALFBAND_DOWN_LATENCY_4 + 1)"),
    ] {
        let e = compile_err(&src);
        assert_eq!(&src[e.start..e.end], at, "mode diagnostic span for {src}");
        assert!(e.message.contains(msg), "mode diagnostic for {src}: {}", e.message);
    }
    let plain = build(identity, Backend::Native);
    let one = build(&format!("rate(1, linear, hold) {identity}"), Backend::Native);
    assert_eq!(one.program_key(), plain.program_key(), "base rate modes changed the render program");
    assert_eq!(one.new_state(), plain.new_state(), "base rate modes added state");

    let src = "rate(1, filter, linear) let hold = 0.5 var os = Oversample2{}
        fn effect(l,r) { oversample(os,vec2(l,r),fn(u) { u * hold },hold) }";
    let nodes = [FuseNode { name: "fx", code: src, inputs: vec![Port::Input] }];
    let fused = fuse(&nodes, &[Port::Node(0)], Backend::Native).unwrap();
    let (out, _) = effect_output(&fused, &impulse, &signal);
    assert_eq!(out[0], 0.5, "a fused mode name can also name a value in the body");

    let module = Module { path: "shape", source: "let hold = 0.5
        fn half(s,x) { oversample(s,x,fn(u) { u * hold },hold) }" };
    let bindings = AudioBindings { modules: &[module], ..Default::default() };
    let src = "use shape var os = Oversample2{} fn effect(l,r) { shape.half(os,l) }";
    let shader = compile_audio_with(src, Backend::Native, false, &bindings).into_result().unwrap();
    assert_eq!(effect_output(&shader, &impulse, &signal).0[0], 0.5, "bare mode in an imported helper");
    let src = format!("use shape rate(2, shape.hold) {identity}");
    let errors = compile_audio_with(&src, Backend::Interp, false, &bindings).into_result().unwrap_err();
    assert_eq!(&src[errors[0].start..errors[0].end], "shape.hold", "qualified value used as a mode");
    assert!(errors[0].message.contains("filter, linear or hold"));

    for up in ["filter", "linear", "hold"] {
        for down in ["filter", "linear", "hold"] {
            let src = format!("rate(4, {up}, {down}) {identity}");
            let ops = check_with(&src, &AudioBindings::default()).unwrap().ops_per_frame;
            eprintln!("4x identity {up}/{down} {ops:.2} ops/frame");
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const PORT_EFFECT: &str = "
    let key = input(2)
    let env = output(1)
    let side = input(1)
    let aux = output(2)
    fn send(x) { aux = x }
    fn read_key() { key }
    fn block() { env += 0.5 }
    fn effect(l,r) {
        send(read_key() + vec2(l, side))
        vec2(aux.x + env, aux.y + r)
    }";

#[test]
fn audio_ports_mix_each_declared_channel_on_every_backend() {
    let instrument = build("let key = input(2) let env = output(1) fn voice() { env = key.x * velocity; vec2(env, key.y) }", Backend::Native);
    let slices: &[&[usize]] = &[&[256], &[97], &[1, 13, 64]];
    assert_render_agreement("instrument side and envelope", &instrument, 289, slices, slices, slices);
    let native = build(PORT_EFFECT, Backend::Native);
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    assert_eq!(native.backend(), Backend::Native);
    let interp = build(PORT_EFFECT, Backend::Interp);
    let mut wasm = WasmRunner::new(&native);
    assert_eq!((native.program().input_channels, native.program().output_channels), (5, 5));
    for n in [8, 5] {
        let signals: Vec<Vec<f32>> = (0..5).map(|ch| (0..n).map(|i| (3 * ch + i) as f32).collect()).collect();
        let ins: Vec<_> = signals.iter().map(Vec::as_slice).collect();
        let mut expected = vec![vec![-0.25f32; n + 1]; 5];
        for i in 0..n {
            let x = ins[2][i] + ins[0][i];
            let y = ins[3][i] + ins[4][i];
            for (out, val) in expected.iter_mut().zip([x + 0.5, y + ins[1][i], 0.5, x, y]) {
                out[i] += val;
            }
        }
        for (label, shader) in [("interpreter", &interp), ("native", &native), ("wasm", &native)] {
            let mut ctx = vec![0; shader.ctx_words()];
            shader.init_ctx(&mut ctx, RATE);
            let mut state = shader.new_state();
            let mut out = vec![vec![-0.25f32; n + 1]; 5];
            let mut outs: Vec<_> = out.iter_mut().map(Vec::as_mut_slice).collect();
            if label == "wasm" {
                wasm.run_ports(&mut ctx, &mut state, &ins, &mut outs, n);
            } else {
                shader.run_ports(&mut ctx, &mut state, &mut vec![0; shader.scratch_words()], &ins, &mut outs, n);
            }
            for (k, (samples, want)) in out.iter().zip(&expected).enumerate() {
                assert!(samples.iter().zip(want).all(|(a,b)| a.to_bits() == b.to_bits()), "{label} n={n} channel {k}: {samples:?} vs {want:?}");
            }
        }
    }
}

#[test]
fn audio_port_kinds_follow_writes_through_helpers() {
    use makepad_script_audio_aot::{lower, parse};
    let code = format!("{PORT_EFFECT} let unused = output(2) let mixed = output(1)")
        .replace("fn block() {", "fn update() { mixed = 1 } fn block() { update();")
        .replace("send(read_key()", "update(); send(read_key()");
    let toks = parse::lex(&code).unwrap();
    let items = parse::Parser::new(&toks).items().unwrap();
    let lowered = lower::lower(&items, usize::MAX).unwrap();
    let ports: Vec<_> = lowered.ports.iter().map(|p| (p.name.as_str(), p.output, p.channels, p.first, p.control)).collect();
    assert_eq!(ports, [
        ("key", false, 2, 2, false), ("env", true, 1, 2, true),
        ("side", false, 1, 4, false), ("aux", true, 2, 3, false),
        ("unused", true, 2, 5, false), ("mixed", true, 1, 7, false),
    ]);
}

#[test]
fn audio_port_instance_matches_explicit_silence_across_calls() {
    let shader = build(PORT_EFFECT, Backend::Native);
    let report = check_with(PORT_EFFECT, &AudioBindings::default()).unwrap();
    assert_eq!((shader.input_channels(), shader.output_channels()), (5,5));
    assert_eq!((report.ports.len(), shader.port_count()), (4,4));
    for (k,p) in report.ports.iter().enumerate() {
        let v = shader.port_view(k);
        assert_eq!((v.name, v.output, v.channels, v.first, v.control), (p.name.as_str(), p.output, p.channels, p.first, p.control));
    }
    let mut instance = Instance::new(shader.clone(), RATE);
    let mut explicit = Instance::new(shader.clone(), RATE);
    let mut scratch = vec![0; shader.scratch_words()];
    let mut frame = 0;
    for n in [5, 64, 8] {
        let z = vec![0.0; n];
        let (l,r) = test_input(n);
        let (mut left,mut right) = (z.clone(),z.clone());
        assert_eq!(counting(|| instance.process(&l,&r,&mut left,&mut right)).1, 0, "ported Instance allocated");
        let mut out = vec![z.clone(); 5];
        let mut outs: Vec<_> = out.iter_mut().map(Vec::as_mut_slice).collect();
        explicit.ctx[CTX_FRAME as usize] = frame;
        shader.run_ports(&mut explicit.ctx, &mut explicit.state, &mut scratch, &[&l,&r,&z,&z,&z], &mut outs, n);
        assert_eq!((&left,&right), (&out[0],&out[1]));
        assert_eq!(instance.state, explicit.state, "held outputs across calls");
        frame += n as u32;
    }
    let test = "fn check_effect(frame, in, out) -> bool { out.y == in.y && key.x == 0 && side == 0 }";
    assert!(makepad_script_audio_aot::selftest(PORT_EFFECT, test, &AudioBindings::default()).unwrap().passed());
}

#[test]
fn audio_port_context_errors_name_the_declaration() {
    for (decl, body, reason) in [
        ("input(1)", "let signal = output(1) fn block() { signal = 1 }", "no audio input"),
        ("input(1)", "fn init() { read() } fn read() { bus } fn voice() { 0 }", "only be read"),
        ("input(2)", "fn block() { read() } fn read() { bus } fn voice() { 0 }", "only be read"),
        ("input(1)", "fn effect(l,r) { bus = 1; l }", "read-only"),
        ("output(1)", "fn init() { put() } fn put() { bus = 1 } fn voice() { 0 }", "cannot be assigned"),
        ("output(2)", "fn block() { put() } fn put() { bus.x = 1 } fn voice() { bus }", "instrument voices"),
        ("input(2)", "rate(2) fn voice() { 0 }", "base-rate"),
        ("input(1+1)", "fn voice() { 0 }", "literal"),
        ("output(3)", "fn voice() { 0 }", "literal"),
    ] {
        let error = compile_err(&format!("let bus = {decl} {body}"));
        assert!(error.message.contains("bus") && error.message.contains(reason), "{decl}, {body}: {}", error.message);
    }
    let message = compile_err("mono() let signal = output(1) fn block() { signal = 1 }").message;
    assert!(message.contains("mono()") && message.contains("no voices"), "{message}");
    let mut bindings = AudioBindings::default();
    bindings.oversample.factor = 4;
    let c = compile_audio_with(PORT_EFFECT, Backend::Interp, false, &bindings);
    assert!(c.error_count() > 0 && c.error(0).message.contains("key") && c.error(0).message.contains("base-rate"));
    let nodes = [FuseNode { name: "a", code: "fn effect(l,r) {l}", inputs: vec![Port::Input] },
        FuseNode { name: "b", code: PORT_EFFECT, inputs: vec![Port::Node(0)] }];
    let e = fuse(&nodes, &[Port::Node(1)], Backend::Interp).unwrap_err();
    assert_eq!(e.node, Some(1));
    assert!(e.error.message.contains("key") && e.error.message.contains("fused"));
}

#[test]
fn audio_port_inside_oversample_reads_the_base_frame() {
    let shader = build("let side = input(1) var os = Oversample2{}
        fn effect(l,r) { oversample(os,l,fn(u) { side },hold,hold) }", Backend::Interp);
    let side = [1.0, 3.0, 5.0, 7.0, 9.0];
    let (mut l, mut r) = ([0.0; 5], [0.0; 5]);
    let mut ctx = vec![0; shader.ctx_words()];
    shader.init_ctx(&mut ctx, RATE);
    shader.run_ports(&mut ctx, &mut shader.new_state(), &mut vec![0; shader.scratch_words()],
        &[&[0.0; 5], &[0.0; 5], &side], &mut [&mut l, &mut r], 5);
    assert_eq!((l,r), (side,side));
}

fn fft_input(n: usize) -> Vec<f32> {
    let mut seed = 0x62539542u32;
    (0..2 * n).map(|_| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as i32) as f32 / 2147483648.0
    }).collect()
}

// Shared inputs are initialized by the binding, so the only init-time arithmetic
// in this fixture is the operation under test. Return both planes and the twiddles.
fn fft_init(input: &[f32], body: &str, backend: Backend) -> Vec<u32> {
    let n = input.len() / 2;
    let samples = [
        SampleView { name: "re", frames: n as u32, channels: 1, data: &input[..n] },
        SampleView { name: "im", frames: n as u32, channels: 1, data: &input[n..] },
    ];
    let src = format!("let re = sample(1) let im = sample(1) fn init() {{ {body} }} fn effect(l,r) {{ re[0] }}");
    let shader = compile_audio_with(&src, backend, false, &AudioBindings { samples: &samples, ..Default::default() }).into_result().unwrap();
    let table = shader.shared_table();
    let words = 2*n + if n > 1 { n } else { 0 };
    table[table.len()-words..].to_vec()
}

fn fft_block(n: usize, name: &str) -> String {
    format!("var re = [0.0; {n}] var im = [0.0; {n}] fn block() {{ {name}(re, im) }} fn effect(l,r) {{ vec2(re[0], im[0]) }}")
}

#[test]
fn complex_bins_and_periodic_windows() {
    let src = "
        let win = [0.0; 1024]
        fn init() { fill_hann(win) }
        let bins = [vec2(0, 1), vec2(2, -3)]
        let factors = [vec2(0, 1), vec2(-4, 5)]
        let product = output(2)
        let conjugate = output(2)
        let polar = output(2)
        let tapers = output(2)
        var pos = int(0)
        fn effect(l, r) {
            let i = pos
            pos += 1
            let a = bins[i]
            product = cmul(a, factors[i])
            conjugate = cconj(a)
            let z = cpolar(abs(a.y), float(i % 7 - 3))
            polar = vec2(cabs(z), carg(z))
            tapers = vec2(hamming(i, len(win)), blackman(i, len(win)))
            vec2(win[i], hann(i, len(win)))
        }";
    const N: usize = 1024;
    let shader = build(src, Backend::Native);
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    assert_eq!(shader.backend(), Backend::Native, "complex and window helpers emitted native code");
    let mut channels = std::array::from_fn(|_| vec![0.0; N]);
    let mut outs: Vec<_> = channels.iter_mut().map(Vec::as_mut_slice).collect();
    shader.run_ports(&mut shader.new_ctx(RATE), &mut shader.new_state(), &mut shader.new_scratch(),
        &[&[0.0; N], &[0.0; N]], &mut outs, N);
    let [filled, hann, re, im, conj_re, conj_im, mag, phase, hamming, blackman] = channels;
    assert_eq!((re[0], im[0]), (-1.0, 0.0), "i times i");
    assert_eq!((re[1], im[1]), (7.0, 22.0), "(2 - 3i) times (-4 + 5i)");
    assert_eq!((conj_re[1], conj_im[1]), (2.0, 3.0), "complex conjugate");
    assert_eq!((hann[0], hann[N / 2]), (0.0, 1.0), "Hann origin and peak");
    let tol = 2.0 * f32::EPSILON;
    for i in 0..N {
        let m = if i % 2 == 0 { 1.0 } else { 3.0 };
        let p = (i % 7) as f32 - 3.0;
        assert!((mag[i] - m).abs() <= tol, "polar magnitude at {m}, {p} radians");
        assert!((phase[i] - p).abs() <= tol, "polar phase at {m}, {p} radians");
        assert_eq!(filled[i].to_bits(), hann[i].to_bits(), "Shared Hann table at {i}");
        assert_eq!(hann[i].to_bits(), hann[(N - i) % N].to_bits(), "Hann symmetry at {i}");
        assert!((hann[i] + hann[(i + N / 2) % N] - 1.0).abs() <= tol, "50% overlap at {i}");
        let angle = std::f64::consts::TAU * i as f64 / N as f64;
        for (name, got, want) in [
            ("Hann", hann[i], 0.5 - 0.5 * angle.cos()),
            ("Hamming", hamming[i], 0.54 - 0.46 * angle.cos()),
            ("Blackman", blackman[i], 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos()),
        ] {
            assert!((f64::from(got) - want).abs() <= 3.0 * f64::from(f32::EPSILON), "{name} at {i}");
        }
    }
    let slices: &[&[usize]] = &[&[31, 128]];
    assert_render_agreement("complex bins and periodic windows", &shader, N, slices, slices, slices);
}

#[test]
fn fft_numerics_and_diagnostics() {
    let mut biggest = Vec::new();
    let mut max_ulp = 0;
    let mut max_error = 0.0f64;
    for n in [65536usize, 4096, 64, 2, 1] {
        let random = fft_input(n);
        let data = fft_init(&random, "fft(re,im) ifft(re,im)", Backend::Native);
        let peak = random.iter().copied().map(f32::abs).fold(0.0, f32::max);
        let bound = n.ilog2() as f32 * f32::EPSILON * peak;
        for (&word, &want) in data.iter().zip(&random) {
            assert!((f32::from_bits(word) - want).abs() <= bound, "round trip N={n}, bound {bound}");
        }
        let tw = &data[2*n..];
        if n == 65536 { biggest.extend_from_slice(tw); }
        for k in 0..n/2 {
            let angle = std::f64::consts::TAU * (k as f64 / n as f64);
            let cos = if k*4 == n { 0.0 } else { angle.cos() };
            let neg_sin = if k == 0 { 0.0 } else { -angle.sin() };
            for (plane, expected) in [cos, neg_sin].into_iter().enumerate() {
                let word = tw[plane*n/2 + k];
                let error = (f64::from(f32::from_bits(word)) - expected).abs();
                max_error = max_error.max(error);
                max_ulp = max_ulp.max(word.abs_diff((expected as f32).to_bits()));
                assert_eq!(word, biggest[plane*32768 + k*(65536/n)], "twiddle bits independent of maximum N={n} k={k} plane={plane}");
            }
        }
    }
    println!("FFT twiddles through 65536: max abs {max_error:e}, max f32 ULP {max_ulp}");
    assert!(max_ulp <= 1, "twiddle error exceeds 1 ULP");
    // The runtime branch makes this a two-size program. In the smaller call,
    // take strided twiddles; both the conditional write and its value matter.
    let input = fft_input(65536);
    let small = [&input[..64], &input[65536..65536+64]].concat();
    let alone = fft_init(&small, "fft(re,im)", Backend::Native);
    let mixed = fft_init(&input, "
        let x = [0.0;64] let y = [0.0;64]
        for i in 0..64 { x[i]=re[i] y[i]=im[i] }
        let v = if re[0] > 2 { fft(re,im) 1.0 } else { 0.0 }
        if re[0] < 2 { fft(x,y) }
        for i in 0..64 { re[i]=x[i]+v im[i]=y[i] }
    ", Backend::Native);
    assert_eq!(&mixed[..64], &alone[..64], "smaller FFT real plane uses the large table stride");
    assert_eq!(&mixed[65536..65536+64], &alone[64..128], "smaller FFT imaginary plane uses the large table stride");
    let n = 64;
    let mut signal = vec![0.0; 2*n];
    signal[0] = 1.0;
    let impulse = fft_init(&signal, "fft(re,im)", Backend::Native);
    assert!(impulse[..n].iter().all(|&w| f32::from_bits(w) == 1.0) && impulse[n..2*n].iter().all(|&w| f32::from_bits(w) == 0.0), "unit impulse has an exactly flat spectrum");
    for (k, x) in signal[..n].iter_mut().enumerate() {
        *x = (std::f64::consts::TAU * (3*k) as f64 / n as f64).sin() as f32;
    }
    let sine = fft_init(&signal, "fft(re,im)", Backend::Native);
    for k in 0..n {
        let expected = if k == 3 { -32.0 } else if k == n-3 { 32.0 } else { 0.0 };
        let bound = n as f32 * f32::EPSILON;
        assert!(f32::from_bits(sine[k]).abs() <= bound && (f32::from_bits(sine[n+k]) - expected).abs() <= bound, "bin-centred sine at bin {k}, absolute bound {bound}");
    }
    for (decl, call, words) in [
        ("var re=[0.0;3] var im=[0.0;3]", "fft(re,im)", &["re", "3", "power of two"][..]),
        ("var re=[0.0;4] var im=[0.0;2]", "ifft(re,im)", &["re", "im", "same length"]),
        ("var re=[int(0);2] var im=[0.0;2]", "fft(re,im)", &["re", "f32 array"]),
    ] {
        let src = format!("{decl} fn voice() {{ {call} 0 }}");
        let error = compile_err(&src);
        assert_eq!(&src[error.start..error.end], call, "error at offending FFT call");
        assert!(words.iter().all(|w| error.message.contains(w)), "FFT diagnostic: {}", error.message);
    }
    let nested = |n| fft_block(n, "fft").replace("fft(re, im)", "for j in 0..4 { fft(re, im) }");
    let big = nested(65536);
    let limit = warning_fft_size(&big, "fft");
    assert!(limit < fft_frame_limit("fft"), "enclosing loop reduces the recommended FFT size");
    assert!(build(&nested(limit), Backend::Interp).warnings().is_empty(), "nested FFT fits at the reported size");
    assert_eq!(warning_fft_size(&nested(2*limit), "fft"), limit);
    let tied = fft_block(65536, "fft").replace("fft(re, im)", "if playing { fft(re, im) } else { fft(im, re) }");
    let compiled = build(&tied, Backend::Interp);
    let [warning] = compiled.warnings() else { panic!("expected one specific warning for the tied branches") };
    assert!(tied[warning.start..warning.end].starts_with("fft(") && warning.message.contains("no power-of-two size fits"),
        "equal-cost branches both exceed the threshold: {}", warning.message);
    let err = compile_err("var re=[0.0;2] let im=[0.0;2] fn init(){fft(re,im)} fn voice(){0}");
    assert!(err.message.contains("state array `re`") && err.message.contains("init()"), "state-array scope in init: {}", err.message);
    let src = "fn voice() { let re=[0.0,1.0,0.0,0.0] let im=[0.0;4] let v=if gate>0 { fft(re,im) 1.0 } else { 0.0 } vec2(re[1],im[1]*v) }";
    assert_eq!(perform(&build(src, Backend::Interp), true, &[1], 1), (vec![0.0], vec![-1.0]), "local arrays transform in a voice");
}

// Read the fitting size from the same borrowed view a host sees.
fn warning_fft_size(src: &str, name: &str) -> usize {
    let compiled = compile_audio_with(src, Backend::Interp, false, &AudioBindings::default());
    let view = compiled.view();
    assert!(view.shader.is_some() && view.errors.is_empty(), "a costly FFT still compiles");
    let [warning] = view.warnings else { panic!("expected one FFT warning") };
    assert_eq!(&src[warning.start..warning.end], format!("{name}(re, im)"));
    for word in [name, "re", "init()", "effect_block()", "hop"] {
        assert!(warning.message.contains(word), "actionable FFT warning: {}", warning.message);
    }
    warning.message.split("within the threshold for this call is ").nth(1).unwrap().split('.').next().unwrap().parse().unwrap()
}

fn fft_frame_limit(name: &str) -> usize {
    let n = warning_fft_size(&fft_block(65536, name), name);
    assert!(n.is_power_of_two());
    assert!(build(&fft_block(n, name), Backend::Interp).warnings().is_empty(), "reported size fits the threshold");
    assert_eq!(warning_fft_size(&fft_block(2*n, name), name), n, "the next size compiles with advice to use the fitting size");
    n
}

#[test]
fn fft_render_backends_agree() {
    for name in ["fft", "ifft"] {
        let largest = fft_frame_limit(name);
        println!("{name}: largest single block transform {largest}");
        for n in [1, 2, 64, 1024, largest, largest * 2] {
            let shader = build(&fft_block(n, name), Backend::Native);
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            assert_eq!(shader.backend(), Backend::Native, "FFT render emitted native code");
            let re = shader.state_vars().iter().find(|v| v.name == "re").unwrap().offset as usize;
            let im = shader.state_vars().iter().find(|v| v.name == "im").unwrap().offset as usize;
            let mut wasm = WasmRunner::new(&shader);
            let mut inputs = vec![fft_input(n)];
            if n == 2 && name == "ifft" { inputs.push(vec![f32::MIN_POSITIVE, 0.0, -f32::MIN_POSITIVE, 0.0]); }
            for input in inputs {
                let mut initial = shader.new_state();
                for (offset, plane) in [(re, &input[..n]), (im, &input[n..])] {
                    for (dst, x) in initial[offset..].iter_mut().zip(plane) { *dst = x.to_bits(); }
                }
                let mut reference = Vec::new();
                for backend in ["interp", "native", "wasm"] {
                    let mut state = initial.clone();
                    let mut ctx = shader.new_ctx(RATE);
                    let mut scratch = shader.new_scratch();
                    let (mut l, mut r) = ([0.0], [0.0]);
                    match backend {
                        "interp" => shader.run_interp(&mut ctx, &mut state, &mut scratch, &[0.0], &[0.0], &mut l, &mut r, 1),
                        "native" => shader.run(&mut ctx, &mut state, &mut scratch, &[0.0], &[0.0], &mut l, &mut r, 1),
                        _ => wasm.run(&mut ctx, &mut state, [&[0.0], &[0.0]], [&mut l, &mut r], 1),
                    }
                    let planes = [&state[re..re+n], &state[im..im+n]].concat();
                    if input[0] == f32::MIN_POSITIVE {
                        assert_eq!(planes, [0x00400000, 0x00400000, 0x80400000, 0x80400000], "subnormal ifft output on {backend}");
                    }
                    if backend == "interp" { reference = planes; }
                    else { assert!(planes == reference, "{backend} {name} N={n} differs at {:?}", planes.iter().zip(&reference).position(|(a,b)| a != b)); }
                }
            }
        }
    }
}

#[test]
fn fft_init_backends_agree() {
    let input = fft_input(65536);
    for name in ["fft", "ifft"] {
        let body = format!("{name}(re,im)");
        // AudioShader permits an interpreter fallback. Require that the native
        // compiler accepts the generated init program at these actual offsets.
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        {
            use makepad_script_compute::{lower, native, parse};
            let src = format!("let re=sample(1) let im=sample(1) fn init() {{ {body} }} fn effect(l,r) {{ re[0] }}");
            let tokens = parse::lex(&src).unwrap();
            let mut items = makepad_script_compute::parse_prelude(makepad_script_audio_aot::PRELUDE, src.len());
            items.extend(parse::Parser::new(&tokens).items().unwrap());
            let samples = |id: &str| Some((1, 65536, if id == "re" { &input[..65536] } else { &input[65536..] }));
            let lowered = lower::lower_with_samples(&items, src.len()+1, &samples).unwrap();
            assert!(native::compile_with(lowered.init.as_ref().unwrap(), 0).is_some(), "native code emitted for {name} init N=65536");
        }
        let reference = fft_init(&input, &body, Backend::Interp);
        let native = fft_init(&input, &body, Backend::Native);
        assert!(native == reference, "{name} init N=65536 differs at word {:?}", native.iter().zip(reference).position(|(a,b)| *a != b));
    }
}

const SPECTRAL_GATE: &str = include_str!("../../../../libs/score_player/shaders/spectral_gate.splash");
const CONVOLVER: &str = include_str!("../../../../libs/score_player/shaders/convolver.splash");

#[test]
fn convolver_matches_direct_from_frame_zero() {
    // Several isolated input samples exercise linear superposition, the
    // delay-line wrap and its last partial partition. Summing just the
    // nonzero products keeps the independent f64 reference linear in IR size.
    for (length, channels) in [(100, 1), (5000, 2), (50000, 2)] {
        let data = fft_input(length).into_iter().take(length * channels)
            .map(|v| v * 0.25).collect::<Vec<_>>();
        let table = SampleView { name: "ir", frames: length as u32, channels: channels as u32, data: &data };
        let shader = bound_shader(CONVOLVER, &[table]);
        assert_eq!(shader.latency(), 0);
        let energy: f64 = data.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() * (2 / channels) as f64;
        for (pulse, normalize) in [(true, true), (true, false), (false, false)] {
            let excitation: &[(usize, f32, f32)] = if pulse { &[(0, 1.0, 1.0)] }
                else { &[(0, -0.25, 0.5), (3, 0.5, -0.75), (67, 0.375, 0.0), (131, 0.0, 0.25)] };
            let frames = length + excitation.last().unwrap().0 + 1;
            let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
            for &(i, a, b) in excitation { l[i] = a; r[i] = b; }
            let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
            let mut inst = Instance::new(shader.clone(), RATE);
            if !normalize { assert!(inst.set_param("normalize", 0.0)); }
            assert!(inst.set_param("gain", 0.0));
            for at in (0..frames).step_by(97) {
                let end = (at + 97).min(frames);
                inst.process(&l[at..end], &r[at..end], &mut left[at..end], &mut right[at..end]);
            }
            for (ch, out) in [left, right].iter().enumerate() {
                for (i, &sample) in out.iter().enumerate() {
                    let expected: f64 = excitation.iter().filter_map(|&(at, a, b)| {
                        let j = i.checked_sub(at)?;
                        let tap = *data.get(j * channels + ch % channels)?;
                        Some(f64::from(tap) * f64::from(if ch == 0 { a } else { b }))
                    }).sum();
                    let expected = if normalize { expected / energy.sqrt() } else { expected };
                    assert!((f64::from(sample) - expected).abs() <= 2e-6,
                        "length {length}, pulse {pulse}, normalize {normalize}, channel {ch}, frame {i}: {sample} vs {expected}");
                }
            }
        }
    }
    for (tap, expected) in [(0.0_f32, 0.0), (f32::from_bits(1), 1.0)] {
        let ir = [tap, 0.0];
        let table = SampleView { name: "ir", frames: 1, channels: 2, data: &ir };
        let mut inst = Instance::new(bound_shader(CONVOLVER, &[table]), RATE);
        assert!(inst.set_param("gain", 0.0));
        let (mut l, mut r) = ([0.0], [0.0]);
        inst.process(&[1.0], &[1.0], &mut l, &mut r);
        assert!((l[0] - expected).abs() <= 2e-6 && r[0] == 0.0, "normalizing the one-frame tap {tap}: {l:?} {r:?}");
    }
}

#[test]
fn convolver_backend_and_slice_agreement() {
    use makepad_script_audio_aot::MAX_FRAMES;
    // The second tail partition starts at frame 4096. Play far enough to
    // hear its first bin sum, including work resumed across control blocks.
    let length = 4097;
    let ir = fft_input(length);
    let table = SampleView { name: "ir", channels: 2, frames: length as u32, data: &ir };
    let bindings = AudioBindings { samples: &[table], ..Default::default() };
    let native = compile_audio_with(CONVOLVER, Backend::Native, false, &bindings).into_result().unwrap();
    let interp = compile_audio_with(CONVOLVER, Backend::Interp, false, &bindings).into_result().unwrap();
    assert!(native.shared_table() == interp.shared_table(), "response spectra differ at init");
    let frames = length + 31;
    let whole = MAX_FRAMES as usize;
    let reference = perform(&interp, true, &[whole], frames);
    let mut wasm = WasmRunner::new(&native);
    for slice in [whole, 97, 1] {
        // The worklet keeps its entire delay line in linear memory across
        // calls. Transfer it only when starting a new performance.
        let web = perform_with(&native, &[slice], frames, &mut |ctx, state, ins, outs, n| {
            if ctx[CTX_FRAME as usize] == 0 {
                WasmRunner::put(wasm.mem.bytes_mut(&mut wasm.store), wasm.state, state.iter().copied());
            }
            wasm.run(ctx, &mut [], ins, outs, n)
        });
        for (backend, output) in [("wasm", web), ("native", perform(&native, false, &[slice], frames)),
            ("interpreter", perform(&interp, true, &[slice], frames))] {
            for (ch, (want, got)) in [(&reference.0, &output.0), (&reference.1, &output.1)].into_iter().enumerate() {
                assert_eq!(first_diff(want, got), None, "{backend} at slice {slice}, channel {ch}");
            }
        }
    }
}

#[test]
fn effect_block_delay_windows_and_slicing() {
    use makepad_script_audio_aot::MAX_FRAMES;
    for (block, overlap) in [(64, 1), (1024, 1), (16384, 1), (1024, 4), (16384, 4)] {
        let hop = block / overlap;
        let mut src = format!("let BLOCK = {block} let HOP = {hop}\n");
        if overlap == 1 {
            src.push_str("fn effect_block(l, r) {}");
        } else {
            src.push_str("let win = [0.0; BLOCK] fn init(){ fill_hann(win) }
                fn effect_block(l,r) {
                    for i in 0..BLOCK {
                        l[i] = (l[i] * win[i]) * (win[i] * (2.0/3.0))
                        r[i] = (r[i] * win[i]) * (win[i] * (2.0/3.0))
                    }
                }");
        }
        let shader = build(&src, Backend::Native);
        assert_eq!((shader.kind(), shader.latency()), (Kind::Effect, block as u32));
        let frames = 2 * block + 17;
        let (mut l, mut r) = test_input(frames);
        l[1] = -0.0;
        r[3] = -0.0;
        let mut reference: Option<(Vec<f32>, Vec<f32>)> = None;
        for slice in [MAX_FRAMES as usize, 97, 1] {
            let mut inst = Instance::new(shader.clone(), RATE);
            // Audio outputs mix additively. -0 is the initially empty accumulator
            // that preserves the sign of either zero contributed by the shader.
            let (mut left, mut right) = (vec![-0.0; frames], vec![-0.0; frames]);
            let (_, allocations) = counting(|| {
                for at in (0..frames).step_by(slice) {
                    let end = (at + slice).min(frames);
                    inst.process(&l[at..end], &r[at..end], &mut left[at..end], &mut right[at..end]);
                }
            });
            assert_eq!(allocations, 0, "block render allocated");
            for (out, input) in [(&left, &l), (&right, &r)] {
                for (i, &sample) in out.iter().enumerate() {
                    let expected = i.checked_sub(block).map_or(0.0, |j| input[j]);
                    if overlap == 1 {
                        assert_eq!(sample.to_bits(), expected.to_bits(), "exact delay at {block}/{hop}, frame {i}");
                    } else {
                        // Four squared Hann products total 3/2 exactly in real arithmetic.
                        // Allow window evaluation, two tapers, the 2/3 gain and four f32 additions.
                        assert!((sample - expected).abs() <= 16.0 * f32::EPSILON,
                            "windowed delay at {block}/{hop}, frame {i}: {sample} vs {expected}");
                    }
                }
            }
            if let Some((ref first_l, ref first_r)) = reference {
                assert!(first_diff(&left, first_l).is_none() && first_diff(&right, first_r).is_none(),
                    "host slicing changed {block}/{hop} at slice {slice}");
            } else {
                reference = Some((left, right));
            }
        }
        if block == 64 {
            let sizes: &[&[usize]] = &[&[97]];
            assert_render_agreement("exact block delay", &shader, 211, sizes, sizes, sizes);
        }
    }
}

#[test]
fn effect_block_gate_swap_and_cost() {
    let old = build(SPECTRAL_GATE, Backend::Native);
    let edited = build(&SPECTRAL_GATE.replace("param(-42,", "param(-9,"), Backend::Native);
    let at = 2048 + 73;
    let end = at + 2048;
    let (l, r) = test_input(end);
    let mut ctx = old.new_ctx(RATE);
    let mut state = old.new_state();
    let mut scratch = old.new_scratch();
    let (mut left, mut right) = (vec![0.0; at], vec![0.0; at]);
    old.run(&mut ctx, &mut state, &mut scratch, &l[..at], &r[..at], &mut left, &mut right, at);

    let mut carried = edited.new_state();
    assert!(edited.carry_state(&mut carried, &old, &state), "equal block and hop carry every state word");
    let after = |shader: &AudioShaderRef, mut ctx: Box<[u32]>, state: &mut [u32]| {
        ctx[CTX_FRAME as usize] = at as u32;
        let (mut left, mut right) = (vec![0.0; end-at], vec![0.0; end-at]);
        shader.run(&mut ctx, state, &mut shader.new_scratch(), &l[at..], &r[at..], &mut left, &mut right, end-at);
        (left, right)
    };
    let unchanged = after(&old, ctx.clone(), &mut state.clone());
    old.set_param(&mut ctx, old.param_index("threshold").unwrap(), -9.0);
    let changed = after(&old, ctx, &mut state);
    let swapped = after(&edited, edited.new_ctx(RATE), &mut carried);
    for (out, expected) in [(&swapped.0, &changed.0), (&swapped.1, &changed.1)] {
        assert!(first_diff(out, expected).is_none(), "mid-hop swap differs from the live param change");
        assert!(out.chunks(256).all(|hop| hop.iter().any(|x| x.abs() > 1e-5)), "a hop fell silent after the swap");
    }
    assert!(first_diff(&changed.0, &unchanged.0).is_some(), "threshold change must affect the signal");

    // Both channels share one transform. With the same bin on each side,
    // only the quiet side should be attenuated. Include DC and Nyquist.
    let loud: Vec<_> = (0..3072).map(|i| {
        let tone = [0.0, 1.0, 0.0, -1.0][i % 4];
        0.25 + 0.25 * tone + if i % 2 == 0 { 0.125 } else { -0.125 }
    }).collect();
    let soft: Vec<_> = loud.iter().map(|x| x * 0.001).collect();
    let (passed, gated) = effect_output(&old, &loud, &soft);
    let floor_gain = 10.0_f32.powf(-24.0 / 20.0);
    for i in 2048..3072 {
        assert!((passed[i] - loud[i-1024]).abs() < 2e-6, "open bin gain at {i}");
        assert!((gated[i] - soft[i-1024] * floor_gain).abs() < 2e-7, "floor gain at {i}");
    }

    let report = makepad_script_audio_aot::check(SPECTRAL_GATE).unwrap();
    assert_eq!(report.latency, 1024);
    assert!(report.peak_ops_per_block > report.ops_per_frame && report.warnings.is_empty(),
        "spectral_gate mean {}, peak {}, warnings {:?}", report.ops_per_frame, report.peak_ops_per_block, report.warnings);
}

#[test]
fn effect_block_declarations_and_layout() {
    let entry = "fn effect_block(l, r) {}";
    let dims = "let BLOCK=64 let HOP=16";
    for (source, message) in [
        (format!("{dims} {entry} fn voice() {{ 0 }}"), "rings per voice"),
        (format!("{dims} {entry} fn effect(l,r) {{ l }}"), "choose one entry"),
        (format!("let HOP=1 {entry}"), "positive power of two for BLOCK"),
        (format!("let BLOCK=4 {entry}"), "positive power of two for HOP"),
        (format!("let BLOCK=3 let HOP=1 {entry}"), "positive power of two for BLOCK"),
        (format!("let BLOCK=4 let HOP=3 {entry}"), "positive power of two for HOP"),
        (format!("let BLOCK=4 let HOP=8 {entry}"), "HOP <= BLOCK"),
        (format!("rate(1) {dims} {entry}"), "base-rate hop"),
        (format!("{dims} fn effect_block(l,r) {{ oversample() }}"), "base-rate hop"),
        (format!("let key=input(2) {dims} {entry}"), "ports on effect_block() are not supported yet"),
        (format!("let side=output(1) {dims} {entry}"), "ports on effect_block() are not supported yet"),
        (format!("latency(4294967295) {dims} {entry}"), "declared latency plus BLOCK exceeds"),
    ] {
        let e = compile_err(&source);
        assert!(e.message.contains(message), "expected {message:?}, got {} for {source}", e.message);
    }
    // Start the param ramp at sample 64. The hop ending at 95 reads its
    // midpoint, together with the second control-block update.
    let ramp = build("let BLOCK=32 let HOP=32 let gain=param(0,0,1) var ticks=0
        fn block(){ticks+=1} fn effect_block(l,r){l[0]=gain r[0]=ticks}", Backend::Native);
    let mut inst = Instance::new(ramp, RATE);
    let zeros = [0.0; 97];
    let (mut l, mut r) = ([0.0; 97], [0.0; 97]);
    inst.process(&zeros[..17], &zeros[..17], &mut l[..17], &mut r[..17]);
    inst.set_param("gain", 1.0);
    inst.process(&zeros[17..], &zeros[17..], &mut l[17..], &mut r[17..]);
    assert_eq!((l[96], r[96]), (0.5, 2.0), "param and block() state at the hop frame");

    let source = format!("{dims} latency(7) {entry}");
    let shader = build(&source, Backend::Native);
    assert_eq!(shader.latency(), 71);
    for dims in ["let BLOCK=64 let HOP=32", "let BLOCK=32 let HOP=16"] {
        let edited = build(&format!("{dims} {entry}"), Backend::Native);
        assert!(!edited.carry_state(&mut edited.new_state(), &shader, &shader.new_state()), "changed layout requests a fade");
    }
    let mut bindings = AudioBindings::default();
    bindings.oversample.factor = 2;
    let e = compile_audio_with(&source, Backend::Interp, false, &bindings).into_result().unwrap_err();
    assert!(e[0].message.contains("base-rate hop"));
    let err = fuse(&[FuseNode { name: "fx", code: &source, inputs: vec![Port::Input] }], &[Port::Node(0)], Backend::Interp).unwrap_err();
    assert!(err.error.message.contains("hop schedule and latency"));
}

#[test]
fn large_state_runs_at_the_same_offsets_on_every_backend() {
    use makepad_script_audio_aot::{lower::HEADER_WORDS, MAX_FRAMES};
    // Struct declarations size their defaults without allocating an instance.
    build("struct Full { data: [0.0; 4294967295] } fn effect(l,r) { l }", Backend::Interp);
    let beyond = u64::from(u32::MAX) + 1;
    let length = beyond - u64::from(HEADER_WORDS);
    let error = compile_err(&format!("var buf = [0.0; {length}] fn effect(l,r) {{ buf[0] }}"));
    assert!(error.message.contains(&format!("{beyond} words")) && error.message.contains("32-bit region length"), "{}", error.message);

    let shader = build("var buf = [0.0; 1572864] fn effect(l,r) {
        let last = buf[1572863]; buf[1572863] = last + 0.125; vec2(last, buf[1572863]) }", Backend::Native);
    assert_eq!(shader.state_words(), HEADER_WORDS as usize + 1572864);
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    assert!(shader.native_code_bytes() > 0);
    let mut wasm = WasmRunner::new(&shader);
    for slice in [MAX_FRAMES as usize, 97, 1] {
        // Execute the actual slice length. The state at the end of each call
        // feeds the next, without copying a large wasm image once per sample.
        let mut scratch = shader.new_scratch();
        for (backend, run) in ["interpreter", "native", "wasm"].into_iter().enumerate() {
            let mut ctx = shader.new_ctx(RATE);
            let mut state = shader.new_state();
            let zero = vec![0.0; slice];
            let (mut l, mut r) = (vec![0.0; slice], vec![0.0; slice]);
            for pass in 0..2 {
                l.fill(0.0);
                r.fill(0.0);
                ctx[CTX_FRAME as usize] = (pass * slice) as u32;
                match backend {
                    0 => shader.run_interp(&mut ctx, &mut state, &mut scratch, &zero, &zero, &mut l, &mut r, slice),
                    1 => shader.run(&mut ctx, &mut state, &mut scratch, &zero, &zero, &mut l, &mut r, slice),
                    _ => wasm.run(&mut ctx, &mut state, [&zero, &zero], [&mut l, &mut r], slice),
                }
                for i in 0..slice {
                    let expected = (pass * slice + i) as f32 * 0.125;
                    assert_eq!([l[i], r[i]].map(f32::to_bits), [expected, expected + 0.125].map(f32::to_bits), "{run}, slice {slice}, frame {i}");
                }
            }
        }
    }
}

#[test]
fn nested_oversample_regions() {
    let (signal, _) = test_input(384);
    let mut impulse = [0.0; 384];
    impulse[0] = 1.0;
    let nested = |outer, inner, latency, shape: &str| format!("latency({latency})
        var a = Oversample{outer}{{}} var b = Oversample{inner}{{}}
        fn stage(s,x) {{ oversample(s,x,fn(v) {{ {shape} }}) }}
        fn effect(l,r) {{ oversample(a,vec2(l,r),fn(u) {{ stage(b,u) }}) }}");
    let same = |label: &str, shader: &AudioShaderRef, reference: &(Vec<f32>, Vec<f32>), delay| {
        let got = effect_output(shader, &impulse, &signal);
        for (side, out, flat) in [("impulse", &got.0, &reference.0), ("signal", &got.1, &reference.1)] {
            assert_eq!(first_diff(out, flat), None, "{label}: first differing {side} frame (nested, flat)");
        }
        let peak = (0..got.0.len()).max_by(|&i, &j| got.0[i].abs().total_cmp(&got.0[j].abs())).unwrap();
        assert_eq!(peak, delay, "{label}: impulse peak");
        assert_eq!(shader.latency() as usize, delay, "{label}: reported latency");
    };
    for (outer, inner, delay) in [(4, 2, 40), (2, 2, 38), (2, 4, 40)] {
        let absolute = outer * inner;
        let flat = build(&format!("rate({absolute}) fn effect(l,r) {{ vec2(l,r) }}"), Backend::Native);
        let reference = effect_output(&flat, &impulse, &signal);
        let src = nested(outer, inner, delay, "v");
        same(&format!("{inner}x inside {outer}x"), &build(&src, Backend::Native), &reference, delay);
        if outer == 4 {
            let body = "latency(2) var b = Oversample2{} fn effect(l,r) { oversample(b,vec2(l,r),fn(v) { v }) }";
            same("inside rate(4)", &build(&format!("rate(4) {body}"), Backend::Native), &reference, delay);
            let mut bindings = AudioBindings::default();
            bindings.oversample.factor = 4;
            let node = compile_audio_with(&format!("rate(2) {body}"), Backend::Native, false, &bindings).into_result().unwrap();
            same("inside node override", &node, &reference, delay);
            // Hold/hold is an identity on each pair. The two outstanding
            // pads land at 8x even though the inner boundary has no FIR.
            let cheap = src.replace("latency(40)", "latency(38)").replace("fn(v) { v })", "fn(v) { v },hold)");
            let flat4 = build("rate(4) fn effect(l,r) { vec2(l,r) }", Backend::Native);
            same("cheap inner boundary", &build(&cheap, Backend::Native), &effect_output(&flat4, &impulse, &signal), 38);
            let triple = "latency(40) var a = Oversample2{} var b = Oversample2{} var c = Oversample2{}
                fn effect(l,r) { oversample(a,vec2(l,r),fn(x) { oversample(b,x,fn(y) { oversample(c,y,fn(z) { z }) }) }) }";
            same("three 2x regions", &build(triple, Backend::Native), &reference, delay);
        }
    }

    // The single down cascade contributes 19 frames. With linear up at the
    // inner boundary (+1/4 frame), the two outer up modes give 19.5 or 19.75.
    for (outer_up, delay) in [("hold", 19), ("linear", 20)] {
        let mixed = |n| nested(2, 2, n, "v")
            .replace("fn(v) { v })", "fn(v) { v },linear,filter)")
            .replace("fn(u) { stage(b,u) })", &format!("fn(u) {{ stage(b,u) }},{outer_up},filter)"));
        build(&mixed(delay), Backend::Native);
        let short = compile_err(&mixed(delay - 1));
        assert_eq!(short.message, format!("the oversample nest up to 4× needs {delay} base frames of latency"),
            "{outer_up} up: add both phase offsets before rounding the nest");
    }

    let clocks = build("var outer = Oversample4{} var inner = Oversample2{} var hits = 0 var steps = 0
        fn voice() { oversample(outer,0.0,fn(x) {
            oversample(inner,x,fn(y) {
                if trigger { hits += 1 }
                if sample_rate == 384000 && oversampling == 8 { steps += 1 }
                probe(deep, steps)
                y
            },hold)
            probe(restored, if sample_rate == 192000 && oversampling == 4 { steps } else { -1 })
            x
        },hold)
        vec2(hits, steps)
        }", Backend::Native);
    let mut ctx = clocks.new_ctx(RATE);
    let mut state = clocks.new_state();
    clocks.note_on(&mut state, 60.0, 1.0, 0, 7, 0);
    let (mut hits, mut steps) = ([0.0; 2], [0.0; 2]);
    clocks.run(&mut ctx, &mut state, &mut clocks.new_scratch(), &[0.0; 2], &[0.0; 2], &mut hits, &mut steps, 2);
    assert_eq!(hits, [1.0; 2], "trigger fires once across both counters");
    assert_eq!(steps, [8.0, 16.0], "inner sample rate and factor");
    for i in 0..2 {
        assert_eq!(clocks.probe_read(&ctx, i), Some((16.0, 8.0, 16.0)), "restored rate and last sub-sample at probe {i}");
    }

    let shaped = nested(4, 2, 40, "let z = v - v*v*v/3; vec2(z.x,z.y)").replace("stage(b,u)", "0.75 * stage(b,u)");
    let shader = build(&shaped, Backend::Native);
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    assert_eq!(shader.backend(), Backend::Native, "nested shaper native code generation");
    let full = makepad_script_audio_aot::MAX_FRAMES as usize;
    let frames = full + 1;
    let want = perform(&shader, true, &[full], frames);
    assert!(want.0.iter().any(|&x| x != 0.0), "nested shaper renders audio");
    let mut wasm = WasmRunner::new(&shader);
    for slice in [full, 97, 1] {
        let web = perform_with(&shader, &[slice], frames, &mut |ctx, state, ins, outs, n| wasm.run(ctx, state, ins, outs, n));
        for (backend, got) in [("interpreter", perform(&shader, true, &[slice], frames)),
            ("native", perform(&shader, false, &[slice], frames)), ("wasm", web)] {
            assert_eq!(first_diff(&want.0, &got.0), None, "nested shaper left on {backend}, slice {slice}");
            assert_eq!(first_diff(&want.1, &got.1), None, "nested shaper right on {backend}, slice {slice}");
        }
    }
    let counted = |s: &str| check_with(s, &AudioBindings::default()).unwrap().ops_per_frame;
    let ops = counted(&shaped);
    let extra = counted(&shaped.replace("vec2(z.x,z.y)", "vec2(z.x + z.y,z.y)")) - ops;
    assert!((extra - 8.0).abs() < 0.01, "one extra inner add must run eight times per base frame");

    for (src, at, message) in [
        (nested(4, 2, 39, "v"), "oversample(a,vec2(l,r),fn(u) { stage(b,u) })", "the oversample nest up to 8× needs 40 base frames of latency"),
        (nested(4, 4, 40, "v"), "oversample(s,x,fn(v) { v })", "nested oversample reaches 16×; the largest supported absolute rate is 8×"),
    ] {
        let e = compile_err(&src);
        assert_eq!(&src[e.start..e.end], at, "nested rate diagnostic span");
        assert_eq!(e.message, message, "nested rate diagnostic");
    }
}

#[test]
fn compile_control_preserves_tables_constants_and_render_code() {
    use makepad_script_audio_aot::ir;
    use std::sync::atomic::{AtomicU32, Ordering};
    // Each static bound exceeds the old init limit; the actual fold and FFT
    // finish quickly and must still publish exactly the same table and value.
    let source = r#"
        fn total() {
            var x = 0.0
            for i in 0..1000000000 {
                if i == 32 { break }
                for j in 0..32 {
                    if j == 0 { continue }
                    x += 0.25
                }
            }
            x
        }
        let folded = total()
        let re = [0.0; 16]
        let im = [0.0; 16]
        fn init() { for i in 0..1000000000 { re[0] = folded fft(re,im) break } }
        fn effect(l,r) { vec2(re[1], folded) }
    "#;
    for backend in [Backend::Interp, Backend::Native] {
        let control = AtomicU32::new(0);
        let bindings = AudioBindings { control: Some(&control), ..Default::default() };
        let ordinary = build(source, backend);
        let controlled = compile_audio_with(source, backend, false, &bindings).into_result().unwrap();
        let (left, right) = perform(&controlled, backend == Backend::Interp, &[1], 1);
        assert_eq!(left[0].to_bits(), 248.0f32.to_bits(), "FFT of a unit impulse scaled by the folded value");
        assert_eq!(right[0].to_bits(), 248.0f32.to_bits(), "constant from the nested loop");
        assert_eq!(controlled.shared_table(), ordinary.shared_table(), "all initialized table words");
        assert_eq!(ir::to_words(controlled.program()), ir::to_words(ordinary.program()), "the render program must contain no additional poll");
        control.store(1, Ordering::Relaxed);
        let cancelled = compile_audio_with(source, backend, false, &bindings);
        let view = cancelled.view();
        assert!(view.cancelled && view.shader.is_none() && view.errors.is_empty(), "cancellation publishes neither a table nor a diagnostic");
    }
}
