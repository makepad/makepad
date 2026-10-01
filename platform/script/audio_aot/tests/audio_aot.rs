//! The audio shader suite: every example shader compiles, renders the same
//! bits on every backend and under any host slicing, never allocates on
//! the audio path, and the math and error paths behave.

use makepad_script_audio_aot::{compile_with, AudioShader, Backend, Instance, Kind, ShaderError, CTX_FRAME};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

fn build(src: &str, backend: Backend) -> Arc<AudioShader> {
    match compile_with(src, backend) {
        Ok(s) => s,
        Err(errs) => {
            let e = &errs[0];
            let (line, col) = e.line_col(src);
            panic!("compile error at {}:{}: {}\n{}", line, col, e.message, src.lines().nth(line - 1).unwrap_or(""));
        }
    }
}

const INSTRUMENTS: &[&str] = &["saw_svf", "fm4", "pluck", "wavetable_pad", "fm2_svf"];
const EFFECTS: &[&str] = &["allpass_reverb", "waveshaper4x"];

/// A deterministic test input for effects: a decaying chirp plus clicks.
fn test_input(n: usize) -> (Vec<f32>, Vec<f32>) {
    let mut l = vec![0.0; n];
    let mut r = vec![0.0; n];
    for i in 0..n {
        let t = i as f32 / RATE;
        let env = (-(t % 0.5) * 6.0).exp();
        l[i] = (t * 220.0 * (1.0 + t) * std::f32::consts::TAU).sin() * env * 0.7;
        r[i] = if i % 9000 == 0 { 0.9 } else { l[i] * 0.5 };
    }
    (l, r)
}

/// Renders a scripted performance: two notes, a param change, releases.
/// `slices` gives the host's call sizes (cycled).
fn perform(shader: &Arc<AudioShader>, interp: bool, slices: &[usize], total: usize) -> (Vec<f32>, Vec<f32>) {
    let mut scratch = shader.new_scratch();
    perform_with(shader, slices, total, &mut |ctx, state, ins, outs, n| {
        if interp {
            shader.run_interp(ctx, state, &mut scratch, ins, outs, n);
        } else {
            shader.run(ctx, state, &mut scratch, ins, outs, n);
        }
    })
}

/// One host call: (ctx, state, ins, outs, n).
type Run<'a> = dyn FnMut(&mut [u32], &mut [u32], [&[f32]; 2], [&mut [f32]; 2], usize) + 'a;

/// [`perform`] through any runner of the shader's calls.
fn perform_with(shader: &Arc<AudioShader>, slices: &[usize], total: usize, run: &mut Run) -> (Vec<f32>, Vec<f32>) {
    let mut ctx = shader.new_ctx(RATE);
    let mut state = shader.new_state();
    let mut out_l = vec![0.0f32; total];
    let mut out_r = vec![0.0f32; total];
    let (in_l, in_r) = test_input(total);
    let zeros = vec![0.0f32; total];
    // Event frames: note on at 0 and at 30000; param at 12345; off at 24000.
    let events = [0usize, 12345, 24000, 30000, 52000];
    let mut frame = 0usize;
    let mut k = 0;
    while frame < total {
        for (e, at) in events.iter().enumerate() {
            if *at == frame {
                match e {
                    0 => shader.note_on(&mut state, 57.0, 0.9, frame as u32, 7),
                    1 => {
                        if !shader.params().is_empty() {
                            let p = &shader.params()[0];
                            shader.set_param(&mut ctx, 0, p.min + (p.max - p.min) * 0.3);
                        }
                    }
                    2 => shader.note_off(&mut state),
                    3 => shader.note_on(&mut state, 64.5, 0.6, frame as u32, 11),
                    _ => shader.note_off(&mut state),
                }
            }
        }
        let next_event = events.iter().copied().filter(|e| *e > frame).min().unwrap_or(total);
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
        let s = build(&src, Backend::Native);
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
    }
}

#[test]
fn native_is_bit_identical_to_the_interpreter() {
    for name in INSTRUMENTS.iter().chain(EFFECTS) {
        let s = build(&shader_src(name), Backend::Native);
        let (nl, nr) = perform(&s, false, &[128], 60000);
        let (il, ir) = perform(&s, true, &[128], 60000);
        if let Some(d) = first_diff(&nl, &il).or(first_diff(&nr, &ir)) {
            panic!("{}: native and interpreter differ at frame {}: {} vs {}", name, d.0, d.1, d.2);
        }
    }
}

#[test]
fn host_slicing_never_changes_the_output() {
    for name in INSTRUMENTS.iter().chain(EFFECTS) {
        let s = build(&shader_src(name), Backend::Native);
        let whole = perform(&s, false, &[128], 40000);
        let ragged = perform(&s, false, &[1, 7, 64, 128, 3, 100, 31, 128, 2], 40000);
        let big = perform(&s, false, &[4096], 40000);
        assert_eq!(bits(&whole.0), bits(&ragged.0), "{}: slicing changed the left channel", name);
        assert_eq!(bits(&whole.1), bits(&ragged.1), "{}: slicing changed the right channel", name);
        assert_eq!(bits(&whole.0), bits(&big.0), "{}: 4096-frame calls changed the output", name);
    }
}

#[test]
fn renders_are_deterministic_across_compiles() {
    for name in INSTRUMENTS {
        let a = build(&shader_src(name), Backend::Native);
        let b = build(&shader_src(name), Backend::Native);
        assert_eq!(bits(&perform(&a, false, &[128], 20000).0), bits(&perform(&b, false, &[128], 20000).0), "{}", name);
    }
}

#[test]
fn the_audio_path_never_allocates() {
    for name in INSTRUMENTS.iter().chain(EFFECTS) {
        for backend in [Backend::Native, Backend::Interp] {
            let s = build(&shader_src(name), backend);
            let mut inst = Instance::new(s.clone(), RATE);
            let mut l = vec![0.0f32; 128];
            let mut r = vec![0.0f32; 128];
            let (in_l, in_r) = test_input(128);
            let (_, allocs) = counting(|| {
                inst.note_on(60.0, 1.0, 1);
                for block in 0..200 {
                    if block == 100 {
                        inst.note_off();
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
fn math_probe(expr: &str) -> Arc<AudioShader> {
    build(&format!("fn effect(l, r) {{ let x = l\n vec2({}, 0.0) }}", expr), Backend::Native)
}

fn probe(s: &Arc<AudioShader>, xs: &[f32], interp: bool) -> Vec<f32> {
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
            s.run_interp(&mut ctx, &mut state, &mut scratch, [chunk, &zeros[..n]], [&mut out[at..at + n], &mut other[at..at + n]], n);
        } else {
            s.run(&mut ctx, &mut state, &mut scratch, [chunk, &zeros[..n]], [&mut out[at..at + n], &mut other[at..at + n]], n);
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
    s.run(&mut ctx, &mut st_a, &mut scratch, [&l, &r], [&mut a0, &mut a1], n);
    s.run_interp(&mut ctx, &mut st_b, &mut scratch, [&l, &r], [&mut b0, &mut b1], n);
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
    s.run(&mut ctx, &mut sa, &mut scratch, [&l, &r], [&mut a0, &mut a1], 3000);
    s.run_interp(&mut ctx, &mut sb, &mut scratch, [&l, &r], [&mut b0, &mut b1], 3000);
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
    s.run(&mut ctx, &mut sa, &mut scratch, [&l, &r], [&mut a0, &mut a1], 4096);
    s.run_interp(&mut ctx, &mut sb, &mut scratch, [&l, &r], [&mut b0, &mut b1], 4096);
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
fn errors_point_at_the_code() {
    let cases: &[(&str, &str, &str)] = &[
        ("fn voice() { foo + 1.0 }", "foo", "unknown name"),
        ("fn voice() { let x = 1.0\n x = \"s\" }", "\"s", "no strings"),
        ("fn voice() { f(1.0) }\nfn f(x) { f(x) }", "f(x)", "itself"),
        ("var b = [0.0; n]\nfn voice() { 0.0 }", "n]", "unknown name"),
        ("fn voice() { break }", "break", "outside a loop"),
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
fn fused_instrument_chain_equals_the_nodes_run_one_by_one() {
    let synth = shader_src("saw_svf");
    let drive = shader_src("waveshaper4x");
    let fused = fuse(
        &[FuseNode { name: "lead", code: &synth, inputs: vec![] }, FuseNode { name: "drive", code: &drive, inputs: vec![Port::Node(0)] }],
        &[Port::Node(1)],
        Backend::Native,
    )
    .unwrap_or_else(|e| panic!("{:?}", e));
    assert!(fused.param_index("lead_cutoff").is_some() && fused.param_index("drive_drive").is_some());
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
    let drive = shader_src("waveshaper4x");
    let par = fuse(
        &[
            FuseNode { name: "verb", code: &verb, inputs: vec![Port::Input] },
            FuseNode { name: "drive", code: &drive, inputs: vec![Port::Input] },
        ],
        &[Port::Node(0), Port::Node(1)],
        Backend::Native,
    )
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
    let bad = "fn effect(l, r) { vec2(l, nope) }";
    let e = fuse(&[FuseNode { name: "a", code: good, inputs: vec![Port::Input] }, FuseNode { name: "b", code: bad, inputs: vec![Port::Node(0)] }], &[Port::Node(1)], Backend::Interp).unwrap_err();
    assert_eq!(e.node, Some(1));
    assert_eq!(&bad[e.error.start..e.error.end], "nope");
    let e = fuse(&[FuseNode { name: "a", code: good, inputs: vec![Port::Node(0)] }], &[Port::Node(0)], Backend::Interp).unwrap_err();
    assert!(e.error.message.contains("not before it"));
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
        var os = Os4{}
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
            for k in 0..4 {
                os4_push(os, soft_clip(os4_up(os, s, k)))
            }
            let e = adsr(env, 0.01, 0.1, 0.5, 0.1) * decay_env(dec, 0.5)
            width(pan(os4_out(os), 0.3), 1.5) * e * 0.05 + vec2(atan(x) * 0.001, atan2(x, 0.5) * 0.001)
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
    let good = makepad_script_audio_aot::check(&shader_src("saw_svf")).unwrap();
    assert!(good.warnings.is_empty(), "{:?}", good.warnings);
    assert!(good.tail_secs.is_some() && good.peak > 0.05);
    let endless = makepad_script_audio_aot::check("var p = 0.0\nfn voice() { p = fract(p + freq / sample_rate)\n sin(TAU * p) * 0.3 }").unwrap();
    assert!(endless.warnings.iter().any(|w| w.contains("never end")), "{:?}", endless.warnings);
    let broken = makepad_script_audio_aot::check("fn effect(l, r) { vec2(log(0.0 * l) * 0.0, l * 3.0) }").unwrap();
    assert!(broken.nonfinite > 0 && broken.warnings.len() >= 2, "{:?}", broken.warnings);
}

// -- golden renders: the compute-core refactor must not change a bit -------------

fn fnv(bits: impl Iterator<Item = u32>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bits {
        for byte in b.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// Every example and shipped library shader, rendered through the scripted
/// performance on both backends, hashed.
fn golden_sources() -> Vec<(String, String)> {
    let lib_dir = format!("{}/../../../apps/commercial/engine/score_player/shaders", env!("CARGO_MANIFEST_DIR"));
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

fn golden_hashes() -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for (name, src) in golden_sources() {
        for interp in [false, true] {
            let s = build(&src, Backend::Native);
            let (l, r) = perform(&s, interp, &[128], 30000);
            out.push((format!("{}{}", name, if interp { "/interp" } else { "" }), fnv(bits(&l).into_iter().chain(bits(&r)))));
        }
    }
    out
}

const GOLDEN: &[(&str, u64)] = &[
    ("saw_svf", 0xc8dfc528a8eeed81),
    ("saw_svf/interp", 0xc8dfc528a8eeed81),
    ("fm4", 0x27017e5aa48b62a5),
    ("fm4/interp", 0x27017e5aa48b62a5),
    ("pluck", 0xdc303800f51fe491),
    ("pluck/interp", 0xdc303800f51fe491),
    ("wavetable_pad", 0x978a07348239c3bf),
    ("wavetable_pad/interp", 0x978a07348239c3bf),
    ("fm2_svf", 0x0d7fe18dcc2fd471),
    ("fm2_svf/interp", 0x0d7fe18dcc2fd471),
    ("allpass_reverb", 0xebe5fd62eeab7d0a),
    ("allpass_reverb/interp", 0xebe5fd62eeab7d0a),
    ("waveshaper4x", 0xbaf761b5859d7176),
    ("waveshaper4x/interp", 0xbaf761b5859d7176),
    ("lib/acid", 0xc785649af8f0a525),
    ("lib/acid/interp", 0xc785649af8f0a525),
    ("lib/analog_poly", 0xa14f9b3d160ebab5),
    ("lib/analog_poly/interp", 0xa14f9b3d160ebab5),
    ("lib/drive", 0x279d54cd64bcb051),
    ("lib/drive/interp", 0x279d54cd64bcb051),
    ("lib/echo", 0xabd15e701858205a),
    ("lib/echo/interp", 0xabd15e701858205a),
    ("lib/epiano", 0x580aad531bfdd513),
    ("lib/epiano/interp", 0x580aad531bfdd513),
    ("lib/flanger", 0xa207408b506acf19),
    ("lib/flanger/interp", 0xa207408b506acf19),
    ("lib/modal_bell", 0x628b207e9249e181),
    ("lib/modal_bell/interp", 0x628b207e9249e181),
    ("lib/phaser", 0x3d92343036a714f3),
    ("lib/phaser/interp", 0x3d92343036a714f3),
    ("lib/pluck", 0x44ccdb790d68505d),
    ("lib/pluck/interp", 0x44ccdb790d68505d),
    ("lib/supersaw", 0xc259b05b8867b5f2),
    ("lib/supersaw/interp", 0xc259b05b8867b5f2),
    ("lib/techno_kick", 0xae479453bd1470d5),
    ("lib/techno_kick/interp", 0xae479453bd1470d5),
];

#[test]
fn renders_match_the_golden_hashes() {
    let got = golden_hashes();
    if GOLDEN.is_empty() || std::env::var("AUDIO_AOT_PRINT_GOLDEN").is_ok() {
        for (n, h) in &got {
            println!("    (\"{}\", 0x{:016x}),", n, h);
        }
        return;
    }
    for (n, h) in &got {
        let want = GOLDEN.iter().find(|(g, _)| g == n).map(|(_, h)| *h);
        assert_eq!(want, Some(*h), "{} changed", n);
    }
}

/// A constant `for` in an audio program renders the same bits as the same
/// loop written out by hand: the lowering unrolls it, so expressions of its
/// counter fold exactly as the hand-written literals do. (An unroll rule
/// that kept wavetable_pad's 16-harmonic loop a loop computed `mix(saw, sq,
/// t * 2.0)` in f32 step by step instead of folding it, and its golden
/// render changed.)
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
    chans: [usize; 4],
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
        let io = alloc(4);
        let chans = [alloc(max), alloc(max), alloc(max), alloc(max)];
        let frame = alloc(makepad_script_audio_aot::wasm::frame_words(shader));
        let pages = at.div_ceil(65536) as u32;
        let mem = stitch::Mem::new(&mut store, stitch::MemType { limits: stitch::Limits { min: pages, max: None } });
        let mut linker = stitch::Linker::new();
        linker.define("env", "memory", mem);
        let inst = linker.instantiate(&mut store, &m).unwrap_or_else(|e| panic!("instantiate: {:?}", e));
        let func = inst.exported_func("run0").expect("run0");
        let bytes = mem.bytes_mut(&mut store);
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

    fn run(&mut self, ctx: &mut [u32], state: &mut [u32], ins: [&[f32]; 2], outs: [&mut [f32]; 2], n: usize) {
        use makepad_stitch::Val;
        let bytes = self.mem.bytes_mut(&mut self.store);
        Self::put(bytes, self.ctx, ctx.iter().copied());
        Self::put(bytes, self.state, state.iter().copied());
        Self::put(bytes, self.chans[0], ins[0][..n].iter().map(|x| x.to_bits()));
        Self::put(bytes, self.chans[1], ins[1][..n].iter().map(|x| x.to_bits()));
        Self::put(bytes, self.chans[2], outs[0][..n].iter().map(|x| x.to_bits()));
        Self::put(bytes, self.chans[3], outs[1][..n].iter().map(|x| x.to_bits()));
        let args = [self.ctx, self.state, self.shared, self.io, n, self.frame].map(|x| Val::I32(x as i32));
        self.func.call(&mut self.store, &args, &mut []).unwrap_or_else(|e| panic!("trap: {:?}", e));
        let bytes = self.mem.bytes(&self.store);
        Self::get(bytes, self.ctx, ctx);
        Self::get(bytes, self.state, state);
        for (c, out) in outs.into_iter().enumerate() {
            let mut w = vec![0u32; n];
            Self::get(bytes, self.chans[2 + c], &mut w);
            for (o, x) in out.iter_mut().zip(w) {
                *o = f32::from_bits(x);
            }
        }
    }
}

fn perform_wasm(shader: &Arc<AudioShader>, slices: &[usize], total: usize) -> (Vec<f32>, Vec<f32>) {
    let mut w = WasmRunner::new(shader);
    perform_with(shader, slices, total, &mut |ctx, state, ins, outs, n| w.run(ctx, state, ins, outs, n))
}

/// The generated wasm of every example and shipped library shader renders
/// the scripted performance to the golden hashes (the native backend's and
/// the interpreter's).
#[test]
fn wasm_renders_match_the_golden_hashes() {
    for (name, src) in golden_sources() {
        let s = build(&src, Backend::Interp);
        let (l, r) = perform_wasm(&s, &[128], 30000);
        let got = fnv(bits(&l).into_iter().chain(bits(&r)));
        let want = GOLDEN.iter().find(|(g, _)| *g == name).map(|(_, h)| *h);
        if want != Some(got) {
            let (il, _) = perform(&s, true, &[128], 30000);
            panic!("{}: wasm render 0x{:016x}, golden {:?}; first difference from the interpreter {:?}", name, got, want.map(|h| format!("0x{:016x}", h)), first_diff(&l, &il));
        }
    }
}

/// Host slicing (odd sizes, single frames) through the generated wasm gives
/// the interpreter's bits.
#[test]
fn wasm_host_slicing_matches_the_interpreter() {
    for name in INSTRUMENTS.iter().chain(EFFECTS) {
        let s = build(&shader_src(name), Backend::Interp);
        let want = perform(&s, true, &[128], 6000);
        let got = perform_wasm(&s, &[1, 7, 128, 33, 64, 3], 6000);
        assert!(bits(&want.0) == bits(&got.0) && bits(&want.1) == bits(&got.1), "{}: first difference {:?}", name, first_diff(&want.0, &got.0));
    }
}
