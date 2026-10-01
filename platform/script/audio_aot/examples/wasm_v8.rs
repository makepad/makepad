//! The generated wasm of every example and shipped library audio shader in
//! V8 (the engine of Chrome and node): each renders the golden test's
//! scripted performance (two notes, a param change, releases; effects on
//! a chirp with clicks) in node, call by call as a host makes them, and
//! must give the interpreter's bits (an FNV hash of both channels, the
//! golden test's). Prints V8's cost per frame next to the interpreter's.
//!
//! ```text
//! cargo run --release -p makepad-script-audio-aot --example wasm_v8 -- [--dir <scratch>] [--slices 128,7,1]
//! ```
//!
//! Needs `node` on the PATH. The batch node reads (see wasm_v8.mjs), per
//! shader: name, module, memory layout, shared tables, initial ctx and
//! state, both inputs, then per call its first frame, n and the ctx/state
//! words the host changed since the previous call (note on/off, params,
//! the frame counter), then the expected hash.

use makepad_script_audio_aot::{compile_with, AudioShader, Backend, Kind, CTX_FRAME, MAX_FRAMES};
use std::sync::Arc;

const RATE: f32 = 48000.0;
const TOTAL: usize = 30000;

fn sources() -> Vec<(String, String)> {
    let root = env!("CARGO_MANIFEST_DIR");
    let mut out: Vec<(String, String)> = ["saw_svf", "fm4", "pluck", "wavetable_pad", "fm2_svf", "allpass_reverb", "waveshaper4x"]
        .iter()
        .map(|n| (n.to_string(), std::fs::read_to_string(format!("{}/shaders/{}.splash", root, n)).unwrap()))
        .collect();
    if let Ok(dir) = std::fs::read_dir(format!("{}/../../../apps/commercial/engine/score_player/shaders", root)) {
        let mut names: Vec<_> = dir.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        names.sort();
        for p in names {
            out.push((format!("lib/{}", p.file_stem().unwrap().to_string_lossy()), std::fs::read_to_string(&p).unwrap()));
        }
    }
    out
}

/// The golden test's input for effects.
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

struct Out(Vec<u8>);

impl Out {
    fn u32(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn words(&mut self, w: impl ExactSizeIterator<Item = u32>) {
        self.u32(w.len() as u32);
        for x in w {
            self.u32(x);
        }
    }
}

/// Renders the performance on the interpreter, writing the batch entry;
/// returns (hash, interpreter ns per frame).
fn record(out: &mut Out, name: &str, shader: &Arc<AudioShader>, slices: &[usize]) -> (u64, f64) {
    let module = makepad_script_audio_aot::wasm::module(&[shader], Default::default()).expect("compiles to wasm");
    out.bytes(name.as_bytes());
    out.bytes(&module);
    out.u32(makepad_script_audio_aot::wasm::frame_words(shader) as u32);
    out.words(shader.shared_table().iter().copied());
    let mut ctx = shader.new_ctx(RATE);
    let mut state = shader.new_state();
    out.words(ctx.iter().copied());
    out.words(state.iter().copied());
    let (in_l, in_r) = if shader.kind == Kind::Effect { test_input(TOTAL) } else { (vec![0.0; TOTAL], vec![0.0; TOTAL]) };
    out.words(in_l.iter().map(|x| x.to_bits()));
    out.words(in_r.iter().map(|x| x.to_bits()));
    let mut scratch = shader.new_scratch();
    let (mut out_l, mut out_r) = (vec![0.0f32; TOTAL], vec![0.0f32; TOTAL]);
    let events = [0usize, 12345, 24000, 30000, 52000];
    let (mut prev_ctx, mut prev_state) = (ctx.clone(), state.clone());
    let mut calls = Out(Vec::new());
    let mut ncalls = 0;
    let (mut frame, mut k) = (0usize, 0usize);
    let mut secs = 0.0;
    while frame < TOTAL {
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
        let next_event = events.iter().copied().filter(|e| *e > frame).min().unwrap_or(TOTAL);
        let n = slices[k % slices.len()].min(next_event - frame).min(TOTAL - frame).min(MAX_FRAMES as usize);
        k += 1;
        ctx[CTX_FRAME as usize] = frame as u32;
        // The words the host changed since the program last left them.
        let mut patch = Vec::new();
        for (region, (now, was)) in [(&ctx, &prev_ctx), (&state, &prev_state)].into_iter().enumerate() {
            for (w, (a, b)) in now.iter().zip(was.iter()).enumerate() {
                if a != b {
                    patch.push((region as u32, w as u32, *a));
                }
            }
        }
        calls.u32(frame as u32);
        calls.u32(n as u32);
        calls.u32(patch.len() as u32);
        for (r, w, x) in patch {
            calls.u32(r);
            calls.u32(w);
            calls.u32(x);
        }
        ncalls += 1;
        let t0 = std::time::Instant::now();
        shader.run_interp(&mut ctx, &mut state, &mut scratch, [&in_l[frame..frame + n], &in_r[frame..frame + n]], [&mut out_l[frame..frame + n], &mut out_r[frame..frame + n]], n);
        secs += t0.elapsed().as_secs_f64();
        prev_ctx.copy_from_slice(&ctx);
        prev_state.copy_from_slice(&state);
        frame += n;
    }
    out.u32(ncalls);
    out.0.extend_from_slice(&calls.0);
    let hash = fnv(out_l.iter().chain(out_r.iter()).map(|x| x.to_bits()));
    out.u32(hash as u32);
    out.u32((hash >> 32) as u32);
    (hash, secs * 1e9 / TOTAL as f64)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let dir = std::path::PathBuf::from(arg("--dir").unwrap_or_else(|| std::env::temp_dir().join("audio_wasm_v8").to_string_lossy().into_owned()));
    let slices: Vec<usize> = arg("--slices").map(|s| s.split(',').filter_map(|x| x.parse().ok()).filter(|x| *x > 0).collect()).unwrap_or_else(|| vec![128]);
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = Out(Vec::new());
    out.u32(0x3141_5641); // "AVA1"
    let srcs = sources();
    out.u32(srcs.len() as u32);
    let mut interp_ns = Vec::new();
    for (name, src) in &srcs {
        let shader = compile_with(src, Backend::Interp).unwrap_or_else(|e| panic!("{}: {}", name, e[0].message));
        let (_, ns) = record(&mut out, name, &shader, &slices);
        interp_ns.push(ns);
    }
    let path = dir.join("audio_batch.bin");
    std::fs::write(&path, &out.0).unwrap();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/wasm_v8.mjs");
    let run = std::process::Command::new("node").arg(script).arg(&path).output().expect("run node (is it on the PATH?)");
    let text = String::from_utf8_lossy(&run.stdout);
    let _ = std::fs::remove_file(&path);
    if !run.status.success() {
        panic!("node failed: {}\n{}", text, String::from_utf8_lossy(&run.stderr));
    }
    let mut fails = 0;
    println!("{:22} {:>8} {:>12} {:>12}", "shader", "", "v8 ns/f", "interp ns/f");
    for (line, ns) in text.lines().filter(|l| l.starts_with("OK ") || l.starts_with("FAIL ")).zip(&interp_ns) {
        let mut it = line.split_whitespace();
        let (verdict, name, v8) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap_or("-"));
        fails += (verdict == "FAIL") as usize;
        println!("{:22} {:>8} {:>12} {:>12.1}", name, verdict, v8, ns);
    }
    for l in text.lines().filter(|l| !l.starts_with("OK ") && !l.starts_with("FAIL ")) {
        println!("{}", l);
    }
    if fails > 0 {
        eprintln!("{} of {} shaders differ in V8", fails, srcs.len());
        std::process::exit(1);
    }
    println!("all {} shaders bit-identical in V8", srcs.len());
}
