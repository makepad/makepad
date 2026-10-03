//! Compiler fuzz over hostile source: truncated, spliced and mutated
//! kernels never panic the compiler, and every kernel that does compile is
//! admitted (or refused) without panicking and then runs natively and
//! interpreted with bit-equal results, never writing outside its buffers
//! (guard words on both sides of every buffer).

use makepad_script_compute::admission::{DeviceLimits, JobBudget, Ledger, Origin};
use makepad_script_compute::kernel::{compile_with, Access, FieldTy, Kernel, Layout, LayoutField};
use makepad_script_compute::Backend;

const CORPUS: &[&str] = &[
    "let W = 64\nlet pos = output(vec3)\nlet amp = param(8.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n pos[i] = vec3(x, fbm2(vec2(x, z) * 0.02, 3, 2.0, 0.5) * amp, z) }",
    "let v = input(f32)\nfn reduce_sum(i) { v[i] * 2.0 }",
    "let seg = emit_buffer(2, 4)\nfn primitive(i) { for k in 0..(i % 3) { emit(seg, float(i), float(k)) } }",
    "let inst = output(Inst)\nfn instance(i) { inst[i].pos = vec3(float(i), 1.0, 2.0)\n inst[i].phase = 0.5\n inst[i].id = i * 2 }",
    "let src = input(f32)\nlet dst = output(f32)\nfn element(i) { dst[i * 7919 - 100000] = src[-i * 1000003] + src[2147483647] }",
    "let o = output(vec4, 8, 2)\nfn element(i) { o[i] = vec4(float(i), hash01(i, 1), 0.5, -float(i)) }",
    "let s = emit_buffer(Inst, 2)\nfn primitive(i) { let r = Inst{}\n r.pos = vec3(float(i), 1.0, 2.0)\n r.phase = 0.5\n emit(s, r) }",
    "let T = [1.0, 2.0, 3.0, 4.0]\nlet o = output(f32)\nfn element(i) { let s = 0.0\n let k = 0\n while k < i { s = s + T[k % 4]\n k = k + 1\n if s > 10.0 { break } }\n o[i] = s }",
    "let o = output(mat4)\nlet t = param(0.0, -10, 10)\nfn instance(i) { let q = quat_axis_angle(vec3(0.0, 1.0, 0.0), t + float(i))\n o[i] = mat4_trs(vec3(float(i)), q, vec3(1.0)) }",
    "let h = input(f32)\nlet o = output(vec3)\nfn helper(a, b) { if a > b { a - b } elif a < b { b - a } else { 0.0 } }\nfn element(i) { let x = match i % 3 { 0 => 1.0, 1 => 2.0, _ => 3.0 }\n o[i] = vec3(helper(x, h[i]), sample_grid(h, 8, 8, x, 0.5), time) }",
    "let o = output(i32)\nfn element(i) { let a = i / 0\n let b = i % 0\n let c = (i << 40) >>> 33\n o[i] = a + b + c + int(1e30) + int(0.0 / 0.0) }",
];

const SPLICES: &[&str] = &[
    "for", "while", "loop", "break", "continue", "return", "if", "else", "match", "fn", "let", "{", "}", "(", ")", "[", "]", ",", ".", "=",
    "0..", "0..1000000000", "1e38", "-2147483648", "2147483647", "0.0/0.0", "/0", "%0", "<<", ">>>", "i", "i * 4294967", "emit(", "output(",
    "input(", "emit_buffer(", "param(", "vec4(", "mat4", "Inst", "Inst{}", ".xyzw", ".q", "float(", "int(", "fbm2(", "hash(", "count", "time",
    "seed", "[1.0, 2.0]", "[0.0; 100000000]", "fn f(x) { f(x) }", "\"", "'", "#", "@", "\n", " ", "_",
];

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

fn layouts() -> Vec<Layout> {
    vec![Layout {
        name: "Inst".into(),
        stride: 8,
        fields: vec![
            LayoutField { name: "pos".into(), ty: FieldTy::Vec3, offset: 0 },
            LayoutField { name: "phase".into(), ty: FieldTy::F32, offset: 4 },
            LayoutField { name: "id".into(), ty: FieldTy::I32, offset: 5 },
        ],
    }]
}

fn char_boundary(s: &str, mut at: usize) -> usize {
    at = at.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

const NUMBERS: &[&str] = &["0", "1", "-1", "3", "4096", "100000", "2147483647", "-2147483648", "1e38", "-1e38", "0.5", "1e-45", "65536", "1073741823"];

fn mutate(r: &mut Rng, src: &str) -> String {
    let mut s = src.to_string();
    let edits = if r.below(10) < 7 { 1 } else { 2 + r.below(2) };
    for _ in 0..edits {
        let at = char_boundary(&s, r.below(s.len() + 1));
        match r.below(8) {
            // Truncate.
            0 => s.truncate(at),
            // Delete a run.
            1 => {
                let end = char_boundary(&s, at + 1 + r.below(12));
                s.replace_range(at..end.max(at), "");
            }
            // Splice a token.
            2 | 3 => s.insert_str(at, SPLICES[r.below(SPLICES.len())]),
            // Swap a number literal for a hostile one.
            4..=6 => {
                let digits: Vec<usize> = s.char_indices().filter(|(_, c)| c.is_ascii_digit()).map(|(k, _)| k).collect();
                if digits.is_empty() {
                    continue;
                }
                let start = digits[r.below(digits.len())];
                let end = s[start..].find(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(s.len(), |e| start + e);
                s.replace_range(start..end, NUMBERS[r.below(NUMBERS.len())]);
            }
            // A random printable byte.
            _ => s.insert(at, (b' ' + r.below(95) as u8) as char),
        }
    }
    s
}

const GUARD: u32 = 0xDEAD_BEEF;
const ELEMENTS: usize = 3;

/// Runs `k` on `ELEMENTS` elements with small guarded buffers; returns every
/// buffer's words, or None when the call refuses to run.
fn run(k: &Kernel, interp: bool) -> Option<Vec<Vec<u32>>> {
    let mut bufs: Vec<Vec<u32>> = k
        .buffers()
        .iter()
        .map(|b| {
            let per = match b.access {
                Access::Emit { width, capacity } => (width * capacity) as usize,
                _ => b.stride as usize,
            };
            let words = (per * ELEMENTS).clamp(1, 1 << 16) + 5;
            let mut v = vec![GUARD; words + 2];
            for (w, x) in v[1..=words].iter_mut().enumerate() {
                *x = (w as f32 * 0.25).to_bits();
            }
            v
        })
        .collect();
    {
        let mut call = k.call();
        let (_, rest) = bufs.split_at_mut(1);
        for (d, v) in k.buffers().iter().skip(1).zip(rest.iter_mut()) {
            let n = v.len();
            let inner = &mut v[1..n - 1];
            let r = match d.access {
                Access::Read => call.input_u32(&d.name, &*inner),
                _ => call.output_u32(&d.name, inner),
            };
            r.unwrap_or_else(|e| panic!("binding `{}`: {}", d.name, e));
        }
        let r = if interp { call.run_interp(ELEMENTS) } else { call.run(ELEMENTS) };
        r.ok()?;
    }
    for (d, v) in k.buffers().iter().zip(&bufs).skip(1) {
        assert!(v[0] == GUARD && v[v.len() - 1] == GUARD, "buffer `{}` was written outside its bounds", d.name);
    }
    Some(bufs.into_iter().skip(1).collect())
}

#[test]
fn hostile_source_never_panics_and_what_compiles_runs_bit_equal() {
    let layouts = layouts();
    for (n, src) in CORPUS.iter().enumerate() {
        compile_with(src, &layouts, Backend::Native).unwrap_or_else(|e| panic!("corpus {} does not compile: {:?}", n, e));
    }
    // Untrusted jobs whose worst case passes 2e8 ops are refused: the fuzz
    // runs only what finishes quickly.
    let ledger = Ledger::new(DeviceLimits { untrusted_worst_case: 200_000_000, ..DeviceLimits::default() });
    let budget = JobBudget { work: 200_000_000 };
    // FUZZ_SEED / FUZZ_ROUNDS explore further than the default run.
    let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut r = Rng(env("FUZZ_SEED", 0x243F_6A88_85A3_08D3) | 1);
    let (mut compiled, mut ran, mut refused) = (0, 0, 0);
    let rounds = env("FUZZ_ROUNDS", 20000) as usize;
    for round in 0..rounds {
        let base = CORPUS[r.below(CORPUS.len())];
        let src = mutate(&mut r, base);
        let result = std::panic::catch_unwind(|| compile_with(&src, &layouts, Backend::Native));
        let k = match result {
            Err(_) => panic!("round {}: the compiler panicked on:\n{}", round, src),
            Ok(Err(_)) => continue,
            Ok(Ok(k)) => k,
        };
        compiled += 1;
        // Untrusted admission first: a job it refuses never runs.
        if ledger.admit(&k, ELEMENTS, Origin::Ai, &budget).is_err() {
            refused += 1;
            continue;
        }
        let native = run(&k, false);
        let interp = run(&k, true);
        assert_eq!(native, interp, "round {}: native and interpreter differ on:\n{}", round, src);
        ran += native.is_some() as usize;
    }
    eprintln!("{} mutants: {} compiled, {} refused by admission, {} ran bit-equal", rounds, compiled, refused, ran);
    assert!(compiled > rounds / 10, "the mutator should leave a fair share compiling");
}

#[test]
fn every_truncation_of_every_corpus_kernel_is_handled() {
    let layouts = layouts();
    for src in CORPUS {
        let mut at = 0;
        while at <= src.len() {
            let cut = &src[..char_boundary(src, at)];
            if std::panic::catch_unwind(|| compile_with(cut, &layouts, Backend::Interp)).is_err() {
                panic!("the compiler panicked on the truncation:\n{}", cut);
            }
            at += 1;
        }
    }
}
