//! Every example and shipped library shader plays the same output words on native code,
//! the reference interpreter and generated wasm in CEF on Linux. Each shader runs with
//! 128-frame calls and with repeating 128, 7, 1-frame calls. Set MAKEPAD_WEB_AUDIO_LONG=1
//! for the longer phrase and delay tails (60,000 frames instead of 8,192).

use makepad_script_audio_aot::{boundary::compile_audio_with, AudioBindings, AudioShaderRef, Backend, Kind, Module, SampleView, CTX_FRAME, MAX_FRAMES};
use std::{path::Path, process::ExitCode};

const RATE: f32 = 48000.0;
const KEY_PERIOD: usize = 2048;
const SLICES: &[&[usize]] = &[&[128], &[128, 7, 1]];

struct Phrase {
    frames: usize,
    // Note on, parameter change, release, second note, second release.
    events: [usize; 5],
}

fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for (dir, prefix) in [("shaders", ""), ("../../../libs/score_player/shaders", "lib/")] {
        let mut paths: Vec<_> = std::fs::read_dir(root.join(dir)).unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "splash")).collect();
        paths.sort();
        for path in paths {
            let name = path.file_stem().unwrap().to_str().unwrap();
            // tone is the shared module imported by tone_drive.
            if prefix.is_empty() && name == "tone" { continue; }
            out.push((format!("{prefix}{name}"), std::fs::read_to_string(&path).unwrap()));
        }
    }
    // The library currently exposes side inputs only. Give extra outputs
    // distinct signals while the main pair passes through untouched.
    out.push(("ports/aux_outputs".into(), "
        let key = input(2)
        let aux = output(2)
        let side = input(1)
        let send = output(1)
        fn effect(l, r) {
            aux = vec2(l, r) + key
            send = side
            vec2(l, r)
        }".into()));
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

/// The stereo key has staggered, decaying bursts and a quiet gap each period.
fn side_input(channel: usize, frames: usize) -> Vec<f32> {
    let delay = (channel * 256) % KEY_PERIOD;
    (0..frames).map(|frame| {
        let age = (frame + KEY_PERIOD - delay) % KEY_PERIOD;
        let env = (1.0 - age as f32 / 384.0).max(0.0);
        if channel % 2 == 0 { env * env } else { -env * env }
    }).collect()
}

fn initial_ctx(shader: &AudioShaderRef) -> Vec<u32> {
    let mut ctx = vec![0; shader.ctx_words()];
    shader.init_ctx(&mut ctx, RATE);
    ctx
}

struct Playback<'a> {
    shader: &'a AudioShaderRef,
    ctx: Vec<u32>,
    state: Vec<u32>,
    scratch: Vec<u32>,
    outputs: Vec<Vec<f32>>,
}

impl<'a> Playback<'a> {
    fn new(shader: &'a AudioShaderRef, frames: usize) -> Self {
        Self {
            shader,
            ctx: initial_ctx(shader),
            state: shader.state_init().to_vec(),
            scratch: vec![0; shader.scratch_words()],
            outputs: vec![vec![0.0; frames]; shader.output_channels()],
        }
    }

    fn event(&mut self, index: usize, frame: usize) {
        match index {
            0 => self.shader.note_on(&mut self.state, 57.0, 0.9, frame as u32, 7, 0),
            1 => {
                if let Some(p) = self.shader.params().first() {
                    self.shader.set_param(&mut self.ctx, 0, p.min + (p.max - p.min) * 0.3);
                }
            }
            2 | 4 => self.shader.note_off(&mut self.state, 0.5),
            3 => self.shader.note_on(&mut self.state, 64.5, 0.6, frame as u32, 11, 0),
            _ => unreachable!(),
        }
    }

    fn render(&mut self, inputs: &[&[f32]], frame: usize, n: usize) {
        self.ctx[CTX_FRAME as usize] = frame as u32;
        let mut outs: Vec<_> = self.outputs.iter_mut().map(|ch| &mut ch[frame..frame + n]).collect();
        self.shader.run_ports(&mut self.ctx, &mut self.state, &mut self.scratch, inputs, &mut outs, n);
    }
}

/// Record only host writes. Between events, the sole change is the absolute frame.
/// In particular, the page must carry its own DSP state from one call to the next.
fn record(out: &mut Out, name: &str, shader: &AudioShaderRef, native: &AudioShaderRef, inputs: &[Vec<f32>], phrase: &Phrase, slices: &[usize]) {
    let mut reference = Playback::new(shader, phrase.frames);
    let mut native = Playback::new(native, phrase.frames);
    let mut calls = Out(Vec::new());
    let (mut frame, mut ncalls) = (0, 0);
    while frame < phrase.frames {
        let next = phrase.events.iter().copied().find(|e| *e > frame).unwrap_or(phrase.frames);
        let n = slices[ncalls % slices.len()].min(next - frame);
        let mut patch = vec![(0, CTX_FRAME, frame as u32)];
        if let Some(event) = phrase.events.iter().position(|&at| at == frame) {
            let before = [reference.ctx.clone(), reference.state.clone()];
            reference.event(event, frame);
            native.event(event, frame);
            for (region, (now, was)) in [&reference.ctx, &reference.state].into_iter().zip(before).enumerate() {
                for (word, (&new, old)) in now.iter().zip(was).enumerate() {
                    if new != old {
                        patch.push((region as u32, word as u32, new));
                    }
                }
            }
        }
        calls.u32(frame as u32);
        calls.u32(n as u32);
        calls.u32(patch.len() as u32);
        for (region, word, value) in patch {
            calls.u32(region);
            calls.u32(word);
            calls.u32(value);
        }
        let ins: Vec<_> = inputs.iter().map(|ch| &ch[frame..frame + n]).collect();
        reference.render(&ins, frame, n);
        native.render(&ins, frame, n);
        ncalls += 1;
        frame += n;
    }

    for frame in 0..phrase.frames {
        for (ch, (actual, expected)) in native.outputs.iter().zip(&reference.outputs).enumerate() {
            let (actual, expected) = (actual[frame].to_bits(), expected[frame].to_bits());
            assert!(actual == expected, "{name}, slices {slices:?}: native differs at frame {frame}, output channel {ch}: got {actual:08x}, expected {expected:08x}");
        }
    }
    if name == "lib/keyed_comp" {
        let gain = |f: usize| reference.outputs[0][f] / inputs[0][f];
        for start in [0, 3 * KEY_PERIOD] {
            let reduced = gain(start + 640);
            assert!(reduced < gain(start + 1) && reduced < gain(start + KEY_PERIOD - 1),
                "{name}, slices {slices:?}: the burst at frame {start} must engage and release the compressor");
        }
    }

    out.words(slices.iter().map(|&n| n as u32));
    out.u32(ncalls as u32);
    out.0.extend_from_slice(&calls.0);
    for ch in &reference.outputs { out.words(ch.iter().map(|x| x.to_bits())); }
}

fn batch(phrase: Phrase) -> Vec<u8> {
    let srcs = sources();
    let modules = [Module { path: "tone", source: include_str!("../shaders/tone.splash") }];
    let (left, right) = test_input(phrase.frames);
    let chirp: Vec<_> = left.iter().zip(&right).flat_map(|(&l, &r)| [l, r]).collect();
    let samples = [SampleView { name: "src", channels: 2, frames: phrase.frames as u32, data: &chirp }];
    let bindings = AudioBindings { modules: &modules, samples: &samples, ..Default::default() };
    let mut out = Out(Vec::new());
    out.u32(0x3341_5641); // AVA3
    out.u32(srcs.len() as u32);
    out.u32(MAX_FRAMES);
    for (name, src) in srcs {
        let compile = |backend| compile_audio_with(&src, backend, false, &bindings).into_result()
            .unwrap_or_else(|e| panic!("{name}: {}", e[0].message));
        let shader = compile(Backend::Interp);
        let native = compile(Backend::Native);
        assert_eq!(native.backend(), Backend::Native, "{name}: native comparison requires native code");
        assert_eq!(native.output_channels(), shader.output_channels(), "{name}: native output count");
        let module = makepad_script_audio_aot::wasm::module_checked(&[&shader], Default::default())
            .unwrap_or_else(|e| panic!("{name}: {}", e.message));
        out.bytes(name.as_bytes());
        out.bytes(&module);
        out.u32(makepad_script_audio_aot::wasm::frame_words(&shader) as u32);
        out.u32(shader.input_channels() as u32);
        out.u32(shader.output_channels() as u32);
        out.words(shader.shared_table().iter().copied());
        out.words(initial_ctx(&shader).into_iter());
        out.words(shader.state_init().iter().copied());
        let mut inputs = match shader.kind {
            Kind::Effect => vec![left.clone(), right.clone()],
            Kind::Instrument => vec![vec![0.0; phrase.frames]; 2],
            Kind::Control => Vec::new(),
        };
        inputs.extend((2..shader.input_channels()).map(|ch| side_input(ch - 2, phrase.frames)));
        for ch in &inputs { out.words(ch.iter().map(|x| x.to_bits())); }
        out.u32(SLICES.len() as u32);
        for slices in SLICES {
            record(&mut out, &name, &shader, &native, &inputs, &phrase, slices);
        }
    }
    out.0
}

fn main() -> ExitCode {
    if let Some(code) = makepad_cef::run_helper_if_subprocess() {
        std::process::exit(code);
    }
    let dirs = &["web"];
    if !cfg!(target_os = "linux") || std::env::args().any(|arg| arg == "--list") {
        return makepad_web_test::main(Path::new(env!("CARGO_MANIFEST_DIR")), dirs);
    }
    let phrase = if std::env::var("MAKEPAD_WEB_AUDIO_LONG").as_deref() == Ok("1") {
        Phrase { frames: 60_000, events: [0, 12345, 24000, 30000, 52000] }
    } else {
        Phrase { frames: 8192, events: [0, 2345, 4000, 5003, 7001] }
    };
    let batch = batch(phrase);
    let root = std::env::temp_dir().join(format!("makepad-web-audio-{}", std::process::id()));
    std::fs::create_dir(&root).expect("create private audio test root");
    let web = root.join("web");
    let result = (|| -> std::io::Result<ExitCode> {
        std::fs::create_dir(&web)?;
        std::fs::write(web.join("audio.bin"), batch)?;
        std::fs::write(web.join("web_audio.test.mjs"), include_str!("web/web_audio.test.mjs"))?;
        Ok(makepad_web_test::main(&root, dirs))
    })();
    std::fs::remove_dir_all(root).expect("remove audio test root");
    result.expect("stage the audio test page and batch")
}
