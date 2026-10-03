//! A kernel run laid out in one wasm linear memory image, with the
//! interpreter's result: the differential checks of the wasm backend
//! (tests/wasm.rs runs scalar modules on stitch; examples/wasm_diff.rs
//! hands cases to V8 through node for the SIMD128 form) run generated code
//! on a copy of `image` and compare it with `expected`.
//!
//! Image layout (byte addresses): 64 zero bytes, ctx, state (1 word),
//! shared tables, the buffer table ((byte address, words) per buffer),
//! every host buffer between GUARD words, then the frame (16-aligned,
//! exactly the ABI's `frame_words * 4 + 16` words) and GUARD words after
//! it. Everything but the frame is compared (guards included: a write
//! outside a buffer or past the frame shows); the frame is scratch.
#![allow(dead_code)]

use makepad_script_compute::ir::{self, Program};
use makepad_script_compute::kernel::{compile_with, Access, Layout, K_BASE, K_COUNT, K_PARAMS, K_TIME};
use makepad_script_compute::{wasm, Backend};

pub const GUARD_WORDS: usize = 3;
pub const GUARD: u32 = 0xDEAD_BEEF;

pub struct Case {
    pub name: String,
    pub program: Program,
    /// Four-wide allowed (element-local, outputs hold every record).
    pub parallel_safe: bool,
    pub n: u32,
    /// Byte addresses: ctx, state, shared, table, frame.
    pub ctx: u32,
    pub state: u32,
    pub shared: u32,
    pub table: u32,
    pub frame: u32,
    pub image: Vec<u32>,
    pub expected: Vec<u32>,
}

impl Case {
    /// The frame's words (scratch, not compared): `start..end`.
    pub fn frame_words(&self) -> (usize, usize) {
        let start = self.frame as usize / 4;
        (start, start + wasm::frame_words(&self.program, true))
    }

    /// Whether `got` equals the interpreter's image outside the frame;
    /// else the first differing word.
    pub fn first_difference(&self, got: &[u32]) -> Option<usize> {
        let (a, b) = self.frame_words();
        (0..self.image.len()).filter(|k| *k < a || *k >= b).find(|k| got[*k] != self.expected[*k])
    }

    /// The module for this case: run0 scalar, run1 four wide (when the
    /// program is element-local and the backend compiles it four wide).
    pub fn module(&self, relaxed_fma: bool) -> Option<(Vec<u8>, bool)> {
        let simd = self.parallel_safe && wasm::simd_supported(&self.program);
        let mut entries = vec![wasm::Entry { program: &self.program, simd: false }];
        if simd {
            entries.push(wasm::Entry { program: &self.program, simd: true });
        }
        wasm::module(&entries, wasm::Target { relaxed_fma, ..Default::default() }).map(|m| (m, simd))
    }

    /// The ctx word K_BASE's byte address.
    pub fn k_base(&self) -> u32 {
        self.ctx + 4 * K_BASE
    }
}

/// Lays out a run of `p` over `n` elements and runs the interpreter on it.
/// `bufs`: each host buffer's words and whether it is writable.
pub fn layout(name: String, p: Program, parallel_safe: bool, ctx: &[u32], shared: &[u32], bufs: &[(Vec<u32>, bool)], n: u32) -> Case {
    let mut image = vec![0u32; 16];
    let put = |image: &mut Vec<u32>, words: &[u32]| -> u32 {
        let at = image.len() as u32 * 4;
        image.extend_from_slice(words);
        at
    };
    let ctx_at = put(&mut image, ctx);
    let state_at = put(&mut image, &[0]);
    let shared_at = put(&mut image, if shared.is_empty() { &[0] } else { shared });
    let table_at = put(&mut image, &vec![0u32; 2 * bufs.len().max(2)]);
    let mut buf_at = Vec::new();
    for (words, _) in bufs {
        put(&mut image, &[GUARD; GUARD_WORDS]);
        buf_at.push(put(&mut image, words));
        put(&mut image, &[GUARD; GUARD_WORDS]);
    }
    for (k, (words, _)) in bufs.iter().enumerate() {
        image[table_at as usize / 4 + 2 * k] = buf_at[k];
        image[table_at as usize / 4 + 2 * k + 1] = words.len() as u32;
    }
    while image.len() % 4 != 0 {
        image.push(0);
    }
    let frame_at = image.len() as u32 * 4;
    image.extend(std::iter::repeat_n(0, wasm::frame_words(&p, true)));
    image.extend_from_slice(&[GUARD; 16]);
    // The interpreter, on a copy.
    let mut out = image.clone();
    let mut ctx = ctx.to_vec();
    {
        let base = out.as_mut_ptr();
        let raw: Vec<ir::RawBuf> = bufs
            .iter()
            .enumerate()
            // SAFETY: each buffer lies inside `out`, which outlives the run
            // and is not otherwise touched during it.
            .map(|(k, (words, w))| ir::RawBuf { ptr: unsafe { base.add(buf_at[k] as usize / 4) }, len: words.len(), writable: *w })
            .collect();
        let mut state = [0u32; 1];
        let mut scratch = vec![0u32; p.scratch_words()];
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut mem = ir::Mem { ctx: &mut ctx, state: &mut state, shared: ir::Shared::Read(shared), bufs: &raw };
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(&p, &mut scratch, &mut mem, &mut io, n);
    }
    out[ctx_at as usize / 4..ctx_at as usize / 4 + ctx.len()].copy_from_slice(&ctx);
    Case { name, program: p, parallel_safe, n, ctx: ctx_at, state: state_at, shared: shared_at, table: table_at, frame: frame_at, image, expected: out }
}

/// A kernel from Splash source over `n` elements: params at their
/// defaults except `params`, inputs filled with a pattern, outputs zero.
pub fn source_case(name: &str, src: &str, layouts: &[Layout], params: &[(&str, f32)], time: f32, n: u32) -> Case {
    source_case_with(name, src, layouts, params, time, n, &|_, _| {})
}

/// [`source_case`] with input buffer `k`'s words then given by `fill`.
pub fn source_case_with(name: &str, src: &str, layouts: &[Layout], params: &[(&str, f32)], time: f32, n: u32, fill: &dyn Fn(usize, &mut Vec<u32>)) -> Case {
    source_case_spare(name, src, layouts, params, time, n, 0, fill)
}

/// [`source_case_with`] with every buffer sized for `spare` elements more
/// than run (as a call over part of a larger run sees them: the words past
/// the n elements must keep their contents).
#[allow(clippy::too_many_arguments)]
pub fn source_case_spare(name: &str, src: &str, layouts: &[Layout], params: &[(&str, f32)], time: f32, n: u32, spare: u32, fill: &dyn Fn(usize, &mut Vec<u32>)) -> Case {
    let k = compile_with(src, layouts, Backend::Interp).unwrap_or_else(|e| panic!("{}: {:?}", name, e));
    let mut ctx = vec![0u32; k.ctx_words()];
    for (i, p) in k.params().iter().enumerate() {
        ctx[K_PARAMS as usize + i] = p.default.to_bits();
    }
    for (pname, v) in params {
        if let Some(i) = k.param_index(pname) {
            let p = &k.params()[i];
            ctx[K_PARAMS as usize + i] = v.clamp(p.min, p.max).to_bits();
        }
    }
    ctx[K_TIME as usize] = time.to_bits();
    ctx[K_COUNT as usize] = n;
    let count = (n + spare) as usize;
    let bufs: Vec<(Vec<u32>, bool)> = k
        .buffers()
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let words = if i == 0 {
                1
            } else {
                match b.access {
                    Access::Read => count * b.stride.max(1) as usize,
                    Access::Write => b.stride.max(1) as usize * count,
                    Access::Emit { width, capacity } => (width * capacity) as usize * count,
                    Access::EmitCount => count,
                }
            };
            let mut v = vec![0u32; words.max(1)];
            if i > 0 && b.access == Access::Read {
                for (j, w) in v.iter_mut().enumerate() {
                    *w = (((j % 97) as f32) * 0.01 - 0.3).to_bits();
                }
                fill(i, &mut v);
            }
            (v, i > 0 && b.access != Access::Read)
        })
        .chain(std::iter::once((vec![0u32; makepad_script_compute::kernel::CHUNK], true)))
        .collect();
    let name = if spare > 0 { format!("{} n={} spare {}", name, n, spare) } else { format!("{} n={}", name, n) };
    layout(name, k.program().clone(), k.parallel_safe, &ctx, k.shared_table(), &bufs, n)
}

/// Kernels from source the backends are checked on (the NEON tests'
/// corpus, emits, helpers with early returns, tables).
pub const CORPUS: &[(&str, &str)] = &[
    ("terrain", "let W = 97\nlet pos = output(vec3)\nlet amp = param(8.0)\nfn vertex(i) { let x = float(i % W)\n let z = float(i / W)\n pos[i] = vec3(x, fbm2(vec2(x, z) * 0.02, 5, 2.0, 0.5) * amp + ridged2(vec2(z, x) * 0.01, 3), z) }"),
    ("branches", "let o = output(vec4)\nlet amp = param(1.0)\nfn element(i) {\n let a = [0.0; 5]\n let s = 0.0\n let k = 0\n while k < i % 13 { a[k % 5] = a[k % 5] + float(k) * amp\n if hash01(i, k) > 0.8 { break }\n k = k + 1 }\n if i % 3 == 0 { s = sin(float(i)) } elif i % 3 == 1 { s = sqrt(float(i)) } else { s = -1.0 }\n o[i] = vec4(a[0] + a[4], s, float(k), hash01(i, 7)) }"),
    ("rows", "let T4 = [vec4(1.0, 2.0, 3.0, 4.0), vec4(5.0, 6.0, 7.0, 8.0), vec4(9.0, 10.0, 11.0, 12.0), vec4(13.0, 14.0, 15.0, 16.0), vec4(17.0, 18.0, 19.0, 20.0), vec4(-1.0, -2.0, -3.0, -4.0), vec4(0.5, 0.25, 0.125, 2.0)]\nlet T3 = [vec3(1.0, 2.0, 3.0), vec3(4.0, 5.0, 6.0), vec3(7.0, 8.0, 9.0), vec3(10.0, 11.0, 12.0), vec3(13.0, 14.0, 15.0)]\nlet T2 = [vec2(1.5, 2.5), vec2(3.5, 4.5), vec2(5.5, 6.5)]\nlet o = output(vec4)\nfn element(i) {\n let a = T4[i]\n let b = T3[i % 5]\n let c = T2[(i * 7) % 3]\n var d = vec4(0.0, 0.0, 0.0, 0.0)\n if i % 3 == 1 { d = T4[i / 2 + 3] }\n o[i] = vec4(a.x + b.y, a.w * c.x + d.y, b.z - c.y + d.x, a.y + a.z + b.x + d.w) }"),
    ("scattered", "let P = [vec4(1.0, 2.0, 3.0, 4.0), vec4(5.0, 6.0, 7.0, 8.0), vec4(-1.5, 2.5, -3.5, 4.5), vec4(0.25, 0.5, 0.75, 1.0), vec4(9.0, 8.0, 7.0, 6.0), vec4(-9.0, -8.0, -7.0, -6.0), vec4(0.0, 0.0, 1.0, 0.0), vec4(3.0, 1.0, 4.0, 1.5), vec4(2.0, 7.0, 1.0, 8.0)]\nlet o = output(vec4)\nfn element(i) {\n let a = P[hash(i) % 9]\n let b = P[(i + 5) % 9]\n o[i] = vec4(a.x * b.w, a.y + b.z, a.z - b.y, a.w * 2.0 + b.x) }"),
    ("tables", "let A8 = [0.5, -1.0, 2.0, 3.5, -4.25, 5.0, 6.125, -7.0]\nlet B16 = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, -16.0]\nlet C12 = [1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5, 9.5, 10.5, 11.5, 12.5]\nlet o = output(vec4)\nfn element(i) {\n let h = hash(i)\n let a = A8[h & 7] + A8[(h >> 3) & 7]\n let b = B16[(h >> 6) & 15] * B16[i]\n var c = 0.0\n if (h & 1) == 1 { c = A8[i] - B16[h >> 28] }\n o[i] = vec4(a, b, c, C12[h % 12]) }"),
    ("returns", "let o = output(vec2)\nfn f(i) { if i % 5 == 2 { return float(i / 3) }\n let x = i * 7919\n return float((x >> 3) ^ (x << 5) % 11) }\nfn element(i) { o[i] = vec2(f(i), f(i + 1)) }"),
    ("emit", "let seg = emit_buffer(3, 4)\nfn primitive(i) { for k in 0..(i % 5) { if hash01(i, k) > 0.3 { emit(seg, float(i), float(k), hash01(i, k)) } } }"),
    ("fbm", "let W = 1024\nlet base = input(f32)\nlet pos = output(vec3)\nlet hgt = output(f32)\nlet amp = param(6.0)\nfn vertex(i) {\n let x = float(i % W)\n let z = float(i / W)\n let h = base[i] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp\n pos[i] = vec3(x, h, z)\n hgt[i] = h\n}"),
    ("fast", "let pts = input(vec3)\nlet o = output(vec4)\nlet k = param(0.7)\nfn element(i) {\n let p = pts[i]\n let q = p * k + vec3(0.25, -0.5, 1.0)\n let d = dot(q, p) * 0.5 - length(q)\n o[i] = vec4(sin(d) * q.x + cos(q.y), fract(q.z * 3.7), smoothstep(0.0, 1.0, d), atan2(q.y, q.x)) }"),
    ("portable", "let math = portable\nlet o = output(vec2)\nfn element(i) { let x = float(i) * 0.37 - 20.0\n o[i] = vec2(sin(x) * exp(x * 0.01), pow(abs(x) + 0.5, 1.3)) }"),
    ("trig", "let out = output(f32)\nfn element(i) { out[i] = sin(float(i) * 0.001 - 500.0) + exp(float(i) * 0.00001 - 5.0) }"),
];

/// Fused multiply-adds in every form (`a * b + c`, `c - a * b`,
/// `a * b - c`; one per kernel: a product with other uses is not fused),
/// fed by [`fma_inputs`].
pub const FMA_KERNELS: [&str; 3] = [
    "let abc = input(vec3)\nlet o = output(f32)\nfn element(i) { let v = abc[i]\n o[i] = v.x * v.y + v.z }",
    "let abc = input(vec3)\nlet o = output(f32)\nfn element(i) { let v = abc[i]\n o[i] = v.z - v.x * v.y }",
    "let abc = input(vec3)\nlet o = output(f32)\nfn element(i) { let v = abc[i]\n o[i] = v.x * v.y - v.z }",
];

/// (a, b, c) triples where rounding a * b + c twice (to f64, then to f32)
/// differs from rounding it once: products half an ulp of c off by a
/// hair, results in the subnormal range, products' own rounding errors;
/// and plain random words. Deterministic in `seed`.
pub fn fma_inputs(seed: u64, words: &mut [u32]) {
    let mut r = seed | 1;
    let mut next = move || {
        r ^= r << 13;
        r ^= r >> 7;
        r ^= r << 17;
        r
    };
    let f = |e: i32, m: f32| m * 2f32.powi(e);
    for t in words.chunks_mut(3) {
        if t.len() < 3 {
            break;
        }
        let x = next();
        let sign = |b: u64| if x >> b & 1 == 1 { -1.0f32 } else { 1.0 };
        let (a, b, c) = match x % 5 {
            0 | 1 => {
                // c's half ulp times (1 - 2^-46) or (1 + 2^-22 + 2^-46):
                // the f64 sum lands on, or next to, an f32 midpoint.
                let e = (x >> 8) as i32 % 60 - 30;
                let c = f(e, 1.0 + ((x >> 16) as u32 & 0x7F_FFFF) as f32 * 2f32.powi(-23));
                let half = e - 24;
                let ea = (x >> 40) as i32 % 20 - 10;
                let lo = if x >> 50 & 1 == 1 { 1.0 - 2f32.powi(-23) } else { 1.0 + 2f32.powi(-23) };
                (f(ea, 1.0 + 2f32.powi(-23)) * sign(5), f(half - ea, lo) * sign(6), c * sign(7))
            }
            2 => {
                // c = -round(a * b): the result is the product's rounding
                // error, often subnormal.
                let a = f32::from_bits(((x >> 8) as u32 & 0x007F_FFFF) | (((x >> 32) as u32 % 40 + 1) << 23));
                let b = f32::from_bits(((x >> 20) as u32 & 0x007F_FFFF) | (((x >> 40) as u32 % 40 + 1) << 23));
                (a * sign(5), b, -(a * b) * sign(5))
            }
            3 => {
                // Tiny results around the smallest normal.
                let a = f(-63 - (x >> 8) as i32 % 3, 1.0 + ((x >> 12) as u32 & 0xFFFF) as f32 * 2f32.powi(-23));
                let b = f(-63, 1.0 + ((x >> 30) as u32 & 0xFFFF) as f32 * 2f32.powi(-23));
                (a, b * sign(5), f(-126, ((x >> 50) as u32 & 0xFF) as f32 * 2f32.powi(-8)) * sign(6))
            }
            _ => (f32::from_bits(next() as u32), f32::from_bits(next() as u32), f32::from_bits(next() as u32)),
        };
        t[0] = a.to_bits();
        t[1] = b.to_bits();
        t[2] = c.to_bits();
    }
}
