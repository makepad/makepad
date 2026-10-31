//! Compiler fuzz for the audio front end: truncated, spliced and mutated
//! shaders never panic the compiler, and every shader that compiles renders
//! the same bits natively and interpreted.

use makepad_script_audio_aot::{compile_with, Backend, Instance, Kind};

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

fn render(i: &mut Instance) -> Vec<u32> {
    let (mut l, mut r) = (vec![0.0f32; 300], vec![0.0f32; 300]);
    if i.shader.kind == Kind::Instrument {
        i.note_on(60.0, 0.8, 7);
        i.render(&mut l, &mut r);
    } else {
        let input: Vec<f32> = (0..300).map(|k| ((k * 37) % 101) as f32 / 50.0 - 1.0).collect();
        i.process(&input, &input, &mut l, &mut r);
    }
    l.iter().chain(&r).map(|x| x.to_bits()).collect()
}

#[test]
fn hostile_shaders_never_panic_and_render_bit_equal() {
    let dir = format!("{}/shaders", env!("CARGO_MANIFEST_DIR"));
    let mut corpus: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect();
    corpus.sort();
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("FUZZ_SEED", 0x1319_8A2E_0370_7344) | 1);
    let rounds = env("FUZZ_ROUNDS", 3000) as usize;
    let mut compiled = 0;
    for round in 0..rounds {
        let pick = r.below(corpus.len());
        let src = mutate(&mut r, &corpus[pick]);
        let native = match std::panic::catch_unwind(|| compile_with(&src, Backend::Native)) {
            Err(_) => panic!("round {}: the compiler panicked on:\n{}", round, src),
            Ok(Err(_)) => continue,
            Ok(Ok(s)) => s,
        };
        let interp = compile_with(&src, Backend::Interp).unwrap_or_else(|e| panic!("round {}: compiles natively only: {:?}", round, e));
        compiled += 1;
        let a = render(&mut Instance::new(native, 48000.0));
        let b = render(&mut Instance::new(interp, 48000.0));
        assert!(a == b, "round {}: native and interpreter differ on:\n{}", round, src);
    }
    eprintln!("{} mutants: {} compiled and rendered bit-equal", rounds, compiled);
    assert!(compiled > rounds / 20);
}
