//! Compiler fuzz for the audio front end: truncated, spliced and mutated
//! shaders (and the ones the fuzz once found panicking) never panic the
//! compiler. Within the render budget, native, interpreter and wasm must
//! agree on every diagnostic or render the same bits on every output.

use makepad_script_audio_aot::{compile_with, wasm, AudioShader, Backend, Kind, ShaderError, MAX_FRAMES};
use makepad_stitch as stitch;

const SPLICES: &[&str] = &[
    "for", "while", "loop", "break", "continue", "return", "if", "else", "match", "fn", "let", "var", "{", "}", "(", ")", "[", "]", ",", ".",
    "=", "0..", "0..1000000000", "1e38", "-2147483648", "2147483647", "0.0/0.0", "/0", "%0", "<<", ">>>", "[0.0; 100000000]",
    "fn f(x) { f(x) }", "init", "note", "gate", "freq", "sample_rate", "\n", " ",
];
const NUMBERS: &[&str] = &["0", "1", "-1", "3", "4096", "100000", "2147483647", "-2147483648", "1e38", "-1e38", "0.5", "1e-45", "65536"];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn boundary(s: &str, mut at: usize) -> usize {
    at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn mutate(r: &mut Rng, src: &str) -> String {
    let mut s = src.to_string();
    let edits = if r.below(10) < 7 { 1 } else { 2 + r.below(2) };
    for _ in 0..edits {
        let at = boundary(&s, r.below(s.len() + 1));
        match r.below(8) {
            0 => s.truncate(at),
            1 => {
                let end = boundary(&s, at + 1 + r.below(12));
                s.replace_range(at..end.max(at), "");
            }
            2 | 3 => s.insert_str(at, SPLICES[r.below(SPLICES.len())]),
            4..=6 => {
                let digits: Vec<usize> = s.char_indices().filter(|(_, c)| c.is_ascii_digit()).map(|(k, _)| k).collect();
                if digits.is_empty() {
                    continue;
                }
                let start = digits[r.below(digits.len())];
                let end = s[start..].find(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(s.len(), |e| start + e);
                s.replace_range(start..end, NUMBERS[r.below(NUMBERS.len())]);
            }
            _ => s.insert(at, (b' ' + r.below(95) as u8) as char),
        }
    }
    s
}

const FRAMES: usize = 300;

fn write_words(bytes: &mut [u8], at: usize, words: impl Iterator<Item = u32>) {
    for (i, word) in words.enumerate() {
        bytes[at + 4 * i..at + 4 * (i + 1)].copy_from_slice(&word.to_le_bytes());
    }
}

// One call to the generated run0, with the same initial buffers as the other
// backends. Each channel has MAX_FRAMES storage as required by the wasm import.
fn render_wasm(shader: &AudioShader, module: &[u8], ctx: &[u32], state: &[u32], inputs: &[&[f32]]) -> Vec<u32> {
    let mut end = 0;
    let mut region = |words: usize| {
        let start = end;
        end += words * 4;
        start
    };
    let ctx_at = region(ctx.len());
    let state_at = region(state.len());
    let shared = region(shader.shared_table().len());
    let channels = inputs.len() + shader.output_channels();
    let table = region(channels);
    let channel_base = region(channels * MAX_FRAMES as usize);
    let channel = |i: usize| channel_base + i * MAX_FRAMES as usize * 4;
    let frame = region(wasm::frame_words(shader));

    let engine = stitch::Engine::new();
    let module = stitch::Module::new(&engine, module).expect("decode wasm");
    let mut store = stitch::Store::new(engine);
    let mem = stitch::Mem::new(&mut store, stitch::MemType { limits: stitch::Limits { min: end.div_ceil(65536) as u32, max: None } });
    let mut linker = stitch::Linker::new();
    linker.define("env", "memory", mem);
    let instance = linker.instantiate(&mut store, &module).expect("instantiate wasm");
    let bytes = mem.bytes_mut(&mut store);
    write_words(bytes, ctx_at, ctx.iter().copied());
    write_words(bytes, state_at, state.iter().copied());
    write_words(bytes, shared, shader.shared_table().iter().copied());
    write_words(bytes, table, (0..channels).map(|i| channel(i) as u32));
    for (i, input) in inputs.iter().enumerate() {
        write_words(bytes, channel(i), input.iter().map(|x| x.to_bits()));
    }
    let args = [ctx_at, state_at, shared, table, FRAMES, frame].map(|v| stitch::Val::I32(v as i32));
    instance.exported_func("run0").unwrap().call(&mut store, &args, &mut []).expect("run wasm");
    let bytes = mem.bytes(&store);
    let mut bits = Vec::with_capacity(shader.output_channels() * FRAMES);
    for i in inputs.len()..channels {
        let at = channel(i);
        bits.extend(bytes[at..at + 4 * FRAMES].chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())));
    }
    bits
}

fn render(shader: &AudioShader, module: Option<&[u8]>) -> Vec<u32> {
    let mut ctx = vec![0; shader.ctx_words()];
    shader.init_ctx(&mut ctx, 48000.0);
    let mut state = shader.state_init().to_vec();
    if shader.kind == Kind::Instrument {
        shader.note_on(&mut state, 60.0, 0.8, 0, 7, 0);
    }
    let signal: [f32; FRAMES] = std::array::from_fn(|k| ((k * 37) % 101) as f32 / 50.0 - 1.0);
    let silence = [0.0; FRAMES];
    let inputs: Vec<&[f32]> = (0..shader.input_channels()).map(|ch| {
        if shader.kind == Kind::Effect && ch < 2 { &signal[..] } else { &silence[..] }
    }).collect();
    if let Some(module) = module {
        return render_wasm(shader, module, &ctx, &state, &inputs);
    }
    let mut scratch = vec![0; shader.scratch_words()];
    let mut samples = vec![0.0; shader.output_channels() * FRAMES];
    let mut outputs: Vec<_> = samples.chunks_exact_mut(FRAMES).collect();
    shader.run_ports(&mut ctx, &mut state, &mut scratch, &inputs, &mut outputs, FRAMES);
    samples.into_iter().map(f32::to_bits).collect()
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome { Skipped, Refused, Compiled }

fn compiles_and_agrees(label: &str, src: &str) -> Outcome {
    // Keep the differential render fast even when mutation creates a huge loop.
    const RENDER_BUDGET: u64 = 4_000_000;
    let cost = std::panic::catch_unwind(|| makepad_script_audio_aot::static_render_cost(src, &Default::default()))
        .unwrap_or_else(|_| panic!("{label}: static cost panicked on:\n{src}"));
    if cost.is_ok_and(|cost| cost > RENDER_BUDGET) {
        return Outcome::Skipped;
    }
    // Wasm takes the interpreter front end, then emits a module. Drive its
    // source entry independently, including when the other two refuse.
    let [native, interp, wasm] = [(Backend::Native, false), (Backend::Interp, false), (Backend::Interp, true)].map(|(backend, emit_wasm)| {
        std::panic::catch_unwind(|| -> Result<_, Vec<ShaderError>> {
            let shader = compile_with(src, backend)?;
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            assert_eq!(shader.backend(), backend, "the requested backend must run");
            let module = if emit_wasm { Some(wasm::module_checked(&[&shader], Default::default()).map_err(|e| vec![e])?) } else { None };
            Ok(render(&shader, module.as_deref()))
        })
    });
    match (&native, &interp, &wasm) {
        (Ok(Err(a)), Ok(Err(b)), Ok(Err(c))) if !a.is_empty() && a == b && b == c => Outcome::Refused,
        (Ok(Ok(a)), Ok(Ok(b)), Ok(Ok(c))) => {
            if let Some(i) = (0..a.len().max(b.len()).max(c.len())).find(|&i| a.get(i) != b.get(i) || b.get(i) != c.get(i)) {
                panic!("{label}: channel {}, frame {}: native {:08x?}, interpreter {:08x?}, wasm {:08x?}\n{src}",
                    i / FRAMES, i % FRAMES, a.get(i), b.get(i), c.get(i));
            }
            Outcome::Compiled
        }
        _ => {
            let results = [native, interp, wasm].map(|r| match r {
                Ok(Ok(_)) => "compiled".to_owned(),
                Ok(Err(errors)) => format!("refused {errors:?}"),
                Err(p) => format!("panicked: {}", p.downcast_ref::<String>().map(String::as_str)
                    .or_else(|| p.downcast_ref::<&str>().copied()).unwrap_or("unknown panic")),
            });
            panic!("{label}: native {}, interpreter {}, wasm {}\n{src}", results[0], results[1], results[2]);
        }
    }
}

#[test]
fn hostile_shaders_never_panic_and_render_bit_equal() {
    // Shaders the fuzz once found panicking, checked directly so each fix
    // holds whatever the corpus and seed. A constant loop whose bound lies
    // far outside i64 overflowed its iteration count: an empty range runs
    // no times, and a range too long to run is refused.
    let found = |bounds: &str| format!("fn voice() {{\n    let s = 0.0\n    for h in {bounds} {{ s = s + 1.0 }}\n    gate * 0.1 + s\n}}");
    assert_eq!(compiles_and_agrees("found", &found("1..1-1e38")), Outcome::Compiled, "the empty range `1..1-1e38` compiles");
    assert_eq!(compiles_and_agrees("found", &found("-1e38..1")), Outcome::Refused, "the range `-1e38..1` is refused as too long");

    let dir = format!("{}/shaders", env!("CARGO_MANIFEST_DIR"));
    let mut corpus: Vec<String> = std::fs::read_dir(&dir).unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect();
    corpus.sort();
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("FUZZ_SEED", 0x1319_8A2E_0370_7344) | 1);
    let rounds = env("FUZZ_ROUNDS", 3000) as usize;
    let (mut compiled, mut refused, mut skipped) = (0, 0, 0);
    for round in 0..rounds {
        let pick = r.below(corpus.len());
        let src = mutate(&mut r, &corpus[pick]);
        match compiles_and_agrees(&format!("round {round}"), &src) {
            Outcome::Compiled => compiled += 1,
            Outcome::Refused => refused += 1,
            Outcome::Skipped => skipped += 1,
        }
    }
    eprintln!("{rounds} mutants: {compiled} compiled bit-equal, {refused} refused identically, {skipped} skipped over the render budget");
    assert!(compiled > rounds / 20);
}
