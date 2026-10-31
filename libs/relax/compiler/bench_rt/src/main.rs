// Fixed runtime benchmark: identical source under rustc --release and `makepad run`.
// Each kernel runs once untimed (Relax compiles lazily on that call), then best of 5.
// Output per kernel: `| name ms checksum` (checksums must match between the two).
#[path = "../../../../../libs/math/src/deterministic.rs"]
#[allow(dead_code)]
mod det;

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::hint::black_box;

fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

// ---- libs/math deterministic.rs ----
fn det_trig() -> u64 {
    let mut h = 0u64;
    for i in 0..200_000 {
        let x = (i as f32) * 0.000731 - 70.0;
        let (s, c) = det::sincos(black_box(x));
        let a = det::atan2(s, c + 0.5);
        h = h.rotate_left(5) ^ (s.to_bits() as u64) ^ ((c.to_bits() as u64) << 7) ^ (a.to_bits() as u64);
    }
    h
}

fn det_explog() -> u64 {
    let mut h = 0u64;
    for i in 1..200_000 {
        let x = (i as f32) * 0.00037;
        let e = det::exp(black_box(x * 0.25 - 10.0));
        let l = det::ln(x);
        let p = det::pow(x, 1.37);
        h = h.rotate_left(5) ^ (e.to_bits() as u64) ^ ((l.to_bits() as u64) << 9) ^ (p.to_bits() as u64);
    }
    h
}

// ---- tests/bench1.rs kernels ----
#[derive(Clone, Copy)]
struct V2 {
    x: f32,
    y: f32,
}
impl V2 {
    fn new(x: f32, y: f32) -> V2 {
        V2 { x, y }
    }
    fn add(self, o: V2) -> V2 {
        V2 { x: self.x + o.x, y: self.y + o.y }
    }
    fn scale(self, s: f32) -> V2 {
        V2 { x: self.x * s, y: self.y * s }
    }
    fn dot(self, o: V2) -> f32 {
        self.x * o.x + self.y * o.y
    }
}

fn hash_bytes_k(seed: u64, bytes: &[u8]) -> u64 {
    let mut x = seed;
    let mut i = 0;
    while i < bytes.len() {
        x = x.wrapping_add(bytes[i] as u64);
        x ^= x >> 32;
        x = x.wrapping_mul(0xd6e8_feb8_6659_fd93);
        x ^= x >> 32;
        i += 1;
    }
    x & 0x0000_3fff_ffff_ffff
}

fn hash_bytes() -> u64 {
    let mut buf = [0u8; 4096];
    let mut x = 7u64;
    for b in buf.iter_mut() {
        *b = next(&mut x) as u8;
    }
    let mut h = 0u64;
    for r in 0..400 {
        h ^= hash_bytes_k(r, black_box(&buf));
    }
    h
}

fn vec_step_k(p: &mut [V2; 64], v: &mut [V2; 64], dt: f32) -> f32 {
    let g = V2::new(0.0, -9.81);
    let mut e = 0.0f32;
    for i in 0..64 {
        v[i] = v[i].add(g.scale(dt));
        p[i] = p[i].add(v[i].scale(dt));
        if p[i].y < 0.0 {
            p[i].y = -p[i].y;
            v[i].y = -v[i].y * 0.9;
        }
        e += v[i].dot(v[i]);
    }
    e
}

fn vec_step() -> u64 {
    let mut p = [V2::new(0.0, 0.0); 64];
    let mut v = [V2::new(0.0, 0.0); 64];
    for i in 0..64 {
        p[i] = V2::new(i as f32, 10.0 + i as f32 * 0.5);
        v[i] = V2::new(1.0, 0.0);
    }
    let mut e = 0.0f32;
    for _ in 0..40_000 {
        e += vec_step_k(black_box(&mut p), &mut v, 0.001);
    }
    e.to_bits() as u64
}

fn sort_ints_k(a: &mut [i32; 256]) {
    let mut i = 1;
    while i < 256 {
        let k = a[i];
        let mut j = i;
        while j > 0 && a[j - 1] > k {
            a[j] = a[j - 1];
            j -= 1;
        }
        a[j] = k;
        i += 1;
    }
}

fn sort_ints() -> u64 {
    let mut x = 3u64;
    let mut h = 0u64;
    for _ in 0..2_000 {
        let mut a = [0i32; 256];
        for e in a.iter_mut() {
            *e = next(&mut x) as i32;
        }
        sort_ints_k(black_box(&mut a));
        h = h.rotate_left(3) ^ (a[0] as u64) ^ (a[128] as u64) ^ (a[255] as u64);
    }
    h
}

// ---- std-heavy kernels (shapes from the std lane's bench) ----
fn vec_push() -> u64 {
    let mut v = Vec::new();
    for i in 0..4_000_000u32 {
        v.push(black_box(i));
    }
    let mut sum = 0;
    for x in v {
        sum += x as u64;
    }
    sum
}

fn vec_extend() -> u64 {
    let a = [17u32; 1024];
    let mut v = Vec::new();
    for _ in 0..4_000 {
        v.extend_from_slice(black_box(&a));
    }
    let mut sum = 0;
    for x in v {
        sum += x as u64;
    }
    sum
}

#[derive(Clone, Copy)]
struct Record {
    value: f32,
    key: u32,
}

fn sort_by_key() -> u64 {
    let mut v = Vec::new();
    let mut x = 42;
    for i in 0..400_000 {
        v.push(Record { value: i as f32, key: next(&mut x) as u32 });
    }
    v.sort_by_key(|r| r.key);
    let mut h = 0u64;
    for r in v {
        h = h.rotate_left(7) ^ r.key as u64 ^ r.value.to_bits() as u64;
    }
    h
}

fn parse_ints() -> u64 {
    let a = ["0", "123456789", "4294967295", "987654"];
    let b = ["-9223372036854775808", "123456789", "9223372036854775807", "-987654"];
    let mut sum = 0u64;
    for i in 0..400_000 {
        let u: u32 = black_box(a[i % 4]).parse().unwrap();
        let s: i64 = black_box(b[i % 4]).parse().unwrap();
        sum = sum.wrapping_add(u as u64).wrapping_add(s as u64);
    }
    sum
}

fn rc_refcell() -> u64 {
    let r = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut sum = 0;
    for i in 0..2_000_000u32 {
        let c = black_box(r.clone());
        {
            let mut v = c.borrow_mut();
            v.push(i);
            sum += v.pop().unwrap() as u64;
        }
        drop(c);
    }
    sum
}

fn iter_chain() -> u64 {
    let mut a = Vec::new();
    for i in 0..1_000_000 {
        a.push((i % 1024) as f32);
    }
    let b: Vec<f32> = a.iter().filter(|x| **x > 100.0).map(|x| x * 0.5).zip(a.iter()).map(|(x, y)| x + y).collect();
    let sum: f32 = black_box(&b).iter().copied().sum();
    let cnt = a.iter().enumerate().filter(|(i, x)| (*i & 3) == 0 && **x < 500.0).count();
    (sum.to_bits() as u64) ^ ((cnt as u64) << 32)
}

// ---- makepad-shaped hot paths ----
// FxHasher (libs/fxhash word step); bytes go through the same step per byte.
#[derive(Default)]
struct Fx(u64);
impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0.rotate_left(5) ^ (*b as u64)).wrapping_mul(0x517cc1b727220a95);
        }
    }
    fn write_u16(&mut self, x: u16) {
        self.0 = (self.0.rotate_left(5) ^ (x as u64)).wrapping_mul(0x517cc1b727220a95);
    }
    fn write_u32(&mut self, x: u32) {
        self.0 = (self.0.rotate_left(5) ^ (x as u64)).wrapping_mul(0x517cc1b727220a95);
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517cc1b727220a95);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    glyph: u16,
    size_q: u32,
}

#[derive(Clone, Copy)]
struct AtlasRect {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

/// glyph atlas lookup: hit-mostly HashMap gets with an insert on miss (draw_text's cache shape)
fn glyph_cache() -> u64 {
    let mut cache: HashMap<GlyphKey, AtlasRect, BuildHasherDefault<Fx>> = HashMap::default();
    let mut x = 9u64;
    let mut cur = (0u16, 0u16);
    let mut h = 0u64;
    for _ in 0..1_000_000 {
        let r = next(&mut x);
        let key = GlyphKey { font: 0x1234_5678 + (r & 3), glyph: ((r >> 8) % 400) as u16, size_q: 12 + ((r >> 20) & 7) as u32 };
        let rect = match cache.get(&key) {
            Some(r) => *r,
            None => {
                let w = 4 + (key.glyph & 15);
                let rr = AtlasRect { x: cur.0, y: cur.1, w, h: 16 };
                cur.0 += w;
                if cur.0 > 2000 {
                    cur.0 = 0;
                    cur.1 += 16;
                }
                cache.insert(key, rr);
                rr
            }
        };
        h = h.wrapping_add((rect.x as u64) ^ ((rect.y as u64) << 16) ^ ((rect.w as u64) << 32) ^ ((rect.h as u64) << 48));
    }
    h ^ cache.len() as u64
}

#[derive(Clone, Copy, PartialEq)]
enum Size {
    Fixed(f64),
    Fill,
    Fit,
}

#[derive(Clone, Copy, PartialEq)]
enum Flow {
    Right,
    Down,
}

#[derive(Clone, Copy, Default)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

struct Node {
    width: Size,
    height: Size,
    flow: Flow,
    pad: f64,
    spacing: f64,
    first_child: usize,
    child_count: usize,
    measured: (f64, f64),
    rect: Rect,
}

impl Node {
    fn inner(&self, r: Rect) -> Rect {
        Rect { x: r.x + self.pad, y: r.y + self.pad, w: (r.w - 2.0 * self.pad).max(0.0), h: (r.h - 2.0 * self.pad).max(0.0) }
    }
}

fn build_tree() -> Vec<Node> {
    // root -> 30 rows -> 12 cells -> 4 leaves (turtle-style nested views)
    let mut nodes = Vec::new();
    let mut x = 5u64;
    nodes.push(Node { width: Size::Fill, height: Size::Fit, flow: Flow::Down, pad: 4.0, spacing: 2.0, first_child: 1, child_count: 30, measured: (0.0, 0.0), rect: Rect::default() });
    let rows_start = 1;
    for _ in 0..30 {
        nodes.push(Node { width: Size::Fill, height: Size::Fit, flow: Flow::Right, pad: 2.0, spacing: 3.0, first_child: 0, child_count: 12, measured: (0.0, 0.0), rect: Rect::default() });
    }
    for r in 0..30 {
        nodes[rows_start + r].first_child = nodes.len();
        for _ in 0..12 {
            let w = if next(&mut x) % 5 == 0 { Size::Fill } else { Size::Fit };
            nodes.push(Node { width: w, height: Size::Fit, flow: Flow::Down, pad: 1.0, spacing: 1.0, first_child: 0, child_count: 4, measured: (0.0, 0.0), rect: Rect::default() });
        }
    }
    let cells_end = nodes.len();
    for c in 31..cells_end {
        nodes[c].first_child = nodes.len();
        for _ in 0..4 {
            let w = 8.0 + (next(&mut x) % 40) as f64;
            let h = 10.0 + (next(&mut x) % 8) as f64;
            nodes.push(Node { width: Size::Fixed(w), height: Size::Fixed(h), flow: Flow::Right, pad: 0.0, spacing: 0.0, first_child: 0, child_count: 0, measured: (0.0, 0.0), rect: Rect::default() });
        }
    }
    nodes
}

fn measure(nodes: &mut Vec<Node>, i: usize) -> (f64, f64) {
    let (first, count, flow, pad, spacing) = {
        let n = &nodes[i];
        (n.first_child, n.child_count, n.flow, n.pad, n.spacing)
    };
    let mut main = 0.0f64;
    let mut cross = 0.0f64;
    for k in 0..count {
        let (cw, ch) = measure(nodes, first + k);
        let (m, c) = if flow == Flow::Right { (cw, ch) } else { (ch, cw) };
        main += m;
        if k > 0 {
            main += spacing;
        }
        cross = cross.max(c);
    }
    let (fw, fh) = if flow == Flow::Right { (main, cross) } else { (cross, main) };
    let n = &mut nodes[i];
    let w = match n.width {
        Size::Fixed(v) => v,
        Size::Fill | Size::Fit => fw + 2.0 * pad,
    };
    let h = match n.height {
        Size::Fixed(v) => v,
        Size::Fill | Size::Fit => fh + 2.0 * pad,
    };
    n.measured = (w, h);
    (w, h)
}

fn place(nodes: &mut Vec<Node>, i: usize, r: Rect) {
    nodes[i].rect = r;
    let inner = nodes[i].inner(r);
    let (first, count, flow, spacing) = {
        let n = &nodes[i];
        (n.first_child, n.child_count, n.flow, n.spacing)
    };
    // distribute leftover space among Fill children along the flow
    let mut used = 0.0;
    let mut fills = 0;
    for k in 0..count {
        let c = &nodes[first + k];
        let fill = if flow == Flow::Right { c.width == Size::Fill } else { c.height == Size::Fill };
        if fill {
            fills += 1;
        } else {
            used += if flow == Flow::Right { c.measured.0 } else { c.measured.1 };
        }
        if k > 0 {
            used += spacing;
        }
    }
    let avail = if flow == Flow::Right { inner.w } else { inner.h };
    let per_fill = if fills > 0 { ((avail - used) / fills as f64).max(0.0) } else { 0.0 };
    let mut pos = 0.0;
    for k in 0..count {
        let c = first + k;
        let (mw, mh, wf, hf) = {
            let n = &nodes[c];
            (n.measured.0, n.measured.1, n.width == Size::Fill, n.height == Size::Fill)
        };
        let rect = if flow == Flow::Right {
            let w = if wf { per_fill } else { mw };
            Rect { x: inner.x + pos, y: inner.y, w, h: if hf { inner.h } else { mh } }
        } else {
            let h = if hf { per_fill } else { mh };
            Rect { x: inner.x, y: inner.y + pos, w: if wf { inner.w } else { mw }, h }
        };
        pos += if flow == Flow::Right { rect.w } else { rect.h } + spacing;
        place(nodes, c, rect);
    }
}

fn layout() -> u64 {
    let mut nodes = build_tree();
    let mut h = 0u64;
    for pass in 0..200 {
        let width = 1200.0 + pass as f64;
        measure(&mut nodes, 0);
        place(&mut nodes, 0, Rect { x: 0.0, y: 0.0, w: width, h: 800.0 });
        let r = nodes[black_box(nodes.len() - 1)].rect;
        h = h.rotate_left(3) ^ r.x.to_bits() ^ r.y.to_bits().rotate_left(17) ^ r.w.to_bits();
    }
    h
}

const TEXT: &str = "Makepad shapes text with rustybuzz: the quick brown fox jumps over the lazy dog, \
ffi fl office affluent waffle -- kerning AV To Ty Wa, numbers 0123456789, and punctuation (!?;:). \
Typography on the GPU needs fast shaping because every frame may lay out a paragraph like this one. ";

/// The test font: BENCH_FONT, else the tree this crate was built in (a binary built on another
/// machine runs with BENCH_FONT set by run.py)
fn font_path() -> String {
    std::env::var("BENCH_FONT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../widgets/resources/IBMPlexSans-Text.ttf").to_string())
}

fn shape() -> u64 {
    let data = std::fs::read(font_path()).unwrap();
    let face = rustybuzz::Face::from_slice(&data, 0).unwrap();
    let feats: Vec<rustybuzz::Feature> = Vec::new();
    let mut h = 0u64;
    for _ in 0..60 {
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(black_box(TEXT));
        let out = rustybuzz::shape(&face, &feats, buf);
        for (i, p) in out.glyph_infos().iter().zip(out.glyph_positions()) {
            h = h.rotate_left(5) ^ (i.glyph_id as u64) ^ ((p.x_advance as u32 as u64) << 20) ^ ((p.x_offset as u32 as u64) << 40);
        }
    }
    h
}

fn measure_k(name: &str, f: fn() -> u64) {
    let expected = black_box(f());
    // BENCH_WARM_MS: steady state -- keep calling until this much time has passed (tier-up
    // under Relax settles), then time
    if let Some(ms) = std::env::var("BENCH_WARM_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        let start = std::time::Instant::now();
        while start.elapsed().as_millis() < ms as u128 {
            assert_eq!(black_box(f()), expected);
        }
    }
    // BENCH_PERF_CTL=<ctl fifo>,<ack fifo>: count only the timed calls (`perf stat --control
    // fifo:<ctl>,<ack> -D -1`): instructions per steady-state call, independent of the warm-up
    let ctl = std::env::var("BENCH_PERF_CTL").ok();
    let perf_ctl = |cmd: &str| {
        if let Some(c) = &ctl {
            let mut it = c.split(',');
            let (cf, af) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
            use std::io::{Read, Write};
            if let Ok(mut w) = std::fs::OpenOptions::new().write(true).open(cf) {
                let _ = w.write_all(cmd.as_bytes());
            }
            if let Ok(mut r) = std::fs::File::open(af) {
                let mut b = [0u8; 4];
                let _ = r.read_exact(&mut b);
            }
        }
    };
    perf_ctl("enable\n");
    let mut best = f64::MAX;
    // BENCH_REPS: more timed calls (profiling runs, where the kernel must dominate)
    let reps: usize = std::env::var("BENCH_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    for _ in 0..reps {
        let start = std::time::Instant::now();
        let result = black_box(f());
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(result, expected);
        if ms < best {
            best = ms;
        }
    }
    perf_ctl("disable\n");
    println!("| {} {:.3} {}", name, best, expected);
}

/// Fixed built-in script kernel, expressed as typed Rust. Keep the legacy workload selector
/// so existing result tables remain readable. The state transitions and f64 evaluation order
/// match the original document; there is no runtime-authored input here.
fn splash_vm() -> u64 {
    struct Particle { x: f64, y: f64, vx: f64, vy: f64 }
    fn step(o: &mut Particle, dt: f64) -> f64 {
        o.x = o.x + o.vx * dt;
        o.y = o.y + o.vy * dt;
        if o.y > 10.0 { o.vy = 0.0 - o.vy; }
        o.x * 0.5 + o.y
    }
    let mut p = Particle { x: 0.0, y: 1.0, vx: 1.5, vy: 0.25 };
    let mut names = HashMap::new();
    let mut acc = 0.0;
    let mut xs = Vec::new();
    for i in 0..20_000 {
        acc = acc + step(&mut p, black_box(0.01));
        if i % 64 == 0 {
            xs.push(i);
            names.insert(i % 7, acc);
        }
    }
    black_box(names);
    (acc + xs.len() as f64 * 1000.0).to_bits()
}

// ---- statics: domain-local statics cost one base load per static address (domain.rs)
static mut ST_COUNTER: u64 = 0;
static mut ST_TABLE: [u32; 256] = [0; 256];

/// A tight loop over statics in one function: the statics' addresses are computed once.
fn statics_loop() -> u64 {
    unsafe {
        ST_COUNTER = 0;
        for i in 0..256 {
            ST_TABLE[i] = i as u32;
        }
        for r in 0..20_000u32 {
            for i in 0..256usize {
                let v = ST_TABLE[i].wrapping_mul(31).wrapping_add(ST_COUNTER as u32 ^ r);
                ST_TABLE[i] = v;
                ST_COUNTER = ST_COUNTER.wrapping_add(v as u64);
            }
        }
        ST_COUNTER
    }
}

#[inline(never)]
fn st_tick(x: u32) -> u32 {
    unsafe {
        ST_COUNTER = ST_COUNTER.wrapping_add(x as u64);
        let k = (x & 255) as usize;
        ST_TABLE[k] ^= x;
        ST_TABLE[k]
    }
}

/// One static access per call: the worst case, every call computes the statics' addresses.
fn statics_calls() -> u64 {
    unsafe {
        ST_COUNTER = 0;
        for i in 0..256 {
            ST_TABLE[i] = 0;
        }
    }
    let mut acc = 0u64;
    for i in 0..3_000_000u32 {
        acc = acc.wrapping_add(st_tick(black_box(i)) as u64);
    }
    acc ^ unsafe { ST_COUNTER }
}

// ---- codeperf kernels: std containers, closures, dyn dispatch, bounds checks, SIMD, struct moves, tokenizer ----

/// HashMap<u64,u64> (std SipHash) insert / lookup / remove mix, then a String-keyed map
fn hashmap() -> u64 {
    let mut m: HashMap<u64, u64> = HashMap::new();
    let mut x = 12345u64;
    let mut h = 0u64;
    for i in 0..30_000u64 {
        let k = next(&mut x) & 0x3fff;
        match i % 4 {
            0 | 1 => {
                m.insert(k, i);
            }
            2 => {
                h = h.rotate_left(3) ^ m.get(&k).copied().unwrap_or(7);
            }
            _ => {
                h = h.rotate_left(5) ^ m.remove(&k).unwrap_or(9);
            }
        }
    }
    let mut keys: Vec<u64> = m.keys().copied().collect();
    keys.sort_unstable();
    for k in keys.iter().step_by(13) {
        h = h.rotate_left(7) ^ k ^ m[k];
    }
    let mut s: HashMap<String, u32> = HashMap::new();
    for i in 0..6_000u32 {
        let k = format!("key{}", next(&mut x) % 900);
        *s.entry(k).or_insert(0) += i;
    }
    let mut ks: Vec<(&String, &u32)> = s.iter().collect();
    ks.sort();
    for (k, v) in ks {
        h = h.rotate_left(5) ^ (k.len() as u64) ^ ((*v as u64) << 8) ^ k.as_bytes()[k.len() - 1] as u64;
    }
    h ^ m.len() as u64
}

/// iterator map/filter/fold with captured state, plus a Vec<Box<dyn Fn>>
fn closures() -> u64 {
    let data: Vec<u32> = (0..400_000u32).map(|i| i.wrapping_mul(2654435761) >> 7).collect();
    let bias = black_box(17u32);
    let mut seen = 0u64;
    let acc = data
        .iter()
        .map(|v| v.wrapping_add(bias))
        .filter(|v| v & 3 != 0)
        .fold(0u64, |a, v| {
            seen += 1;
            a.rotate_left(3) ^ v as u64
        });
    let k1 = black_box(3u64);
    let k2 = black_box(11u64);
    let mut fns: Vec<Box<dyn Fn(u64) -> u64>> = Vec::new();
    fns.push(Box::new(move |x| x.wrapping_mul(k1)));
    fns.push(Box::new(move |x| x ^ (x >> k2)));
    fns.push(Box::new(|x| x.rotate_left(9)));
    fns.push(Box::new(move |x| x.wrapping_add(k1 * k2)));
    let mut h = acc ^ seen;
    for i in 0..480_000u64 {
        h = fns[(i % 4) as usize](h ^ i);
    }
    let mut counter = 0u64;
    let mut bump = |d: u64| {
        counter += d;
        counter
    };
    for i in 0..160_000u64 {
        h ^= bump(i & 7);
    }
    h
}

trait Shape {
    fn area(&self) -> f64;
    fn grow(&mut self, k: f64);
}
struct Circle(f64);
struct Rectangle(f64, f64);
struct Tri(f64, f64);
impl Shape for Circle {
    fn area(&self) -> f64 {
        3.14159 * self.0 * self.0
    }
    fn grow(&mut self, k: f64) {
        self.0 += k;
    }
}
impl Shape for Rectangle {
    fn area(&self) -> f64 {
        self.0 * self.1
    }
    fn grow(&mut self, k: f64) {
        self.0 += k;
        self.1 += k * 0.5;
    }
}
impl Shape for Tri {
    fn area(&self) -> f64 {
        0.5 * self.0 * self.1
    }
    fn grow(&mut self, k: f64) {
        self.1 += k;
    }
}

/// Vec<Box<dyn Shape>> virtual calls in a loop
fn dyn_trait() -> u64 {
    let mut shapes: Vec<Box<dyn Shape>> = Vec::new();
    for i in 0..300 {
        let f = i as f64 * 0.01;
        match i % 3 {
            0 => shapes.push(Box::new(Circle(1.0 + f))),
            1 => shapes.push(Box::new(Rectangle(1.0 + f, 2.0))),
            _ => shapes.push(Box::new(Tri(2.0, 1.0 + f))),
        }
    }
    let mut total = 0.0f64;
    for round in 0..2000 {
        for s in shapes.iter_mut() {
            total += s.area();
            s.grow(black_box(0.001) * (round & 3) as f64);
        }
    }
    total.to_bits()
}

/// a[i] += b[i-1]*c[i+1]: bounds checks elidable (iterator zip over windows) and not (index arithmetic
/// through a runtime-valued stride the compiler cannot prove)
#[inline(never)]
fn idx_elidable(a: &mut [f32], b: &[f32], c: &[f32]) {
    let n = a.len().min(b.len()).min(c.len());
    let (a, b, c) = (&mut a[..n], &b[..n], &c[..n]);
    for i in 0..n {
        a[i] += b[i] * c[i];
    }
}
#[inline(never)]
fn idx_checked(a: &mut [f32], b: &[f32], c: &[f32], stride: usize) {
    let mut i = 1usize;
    while i + 1 < a.len() {
        a[i] += b[i - 1] * c[i + 1];
        i += stride;
    }
}
fn bounds_idx() -> u64 {
    let n = 4096usize;
    let mut a: Vec<f32> = (0..n).map(|i| i as f32 * 0.25).collect();
    let b: Vec<f32> = (0..n).map(|i| (i % 17) as f32 * 0.5).collect();
    let c: Vec<f32> = (0..n).map(|i| (i % 13) as f32 * 0.125).collect();
    let stride = black_box(1usize);
    for _ in 0..600 {
        idx_elidable(&mut a, &b, &c);
        idx_checked(&mut a, &b, &c, stride);
        for v in a.iter_mut() {
            *v *= 0.999;
        }
    }
    a.iter().fold(0u64, |h, v| h.rotate_left(5) ^ v.to_bits() as u64)
}

/// 8-lane f32 dot / mul-add: explicit core::arch NEON / SSE intrinsics, and the plain-array version
#[cfg(target_arch = "aarch64")]
fn dot_simd(a: &[f32], b: &[f32]) -> f32 {
    use core::arch::aarch64::*;
    unsafe {
        let mut acc0 = vdupq_n_f32(0.0);
        let mut acc1 = vdupq_n_f32(0.0);
        let mut i = 0;
        while i + 8 <= a.len() {
            acc0 = vfmaq_f32(acc0, vld1q_f32(a.as_ptr().add(i)), vld1q_f32(b.as_ptr().add(i)));
            acc1 = vfmaq_f32(acc1, vld1q_f32(a.as_ptr().add(i + 4)), vld1q_f32(b.as_ptr().add(i + 4)));
            i += 8;
        }
        vaddvq_f32(vaddq_f32(acc0, acc1))
    }
}
#[cfg(target_arch = "x86_64")]
fn dot_simd(a: &[f32], b: &[f32]) -> f32 {
    use core::arch::x86_64::*;
    unsafe {
        let mut acc0 = _mm_setzero_ps();
        let mut acc1 = _mm_setzero_ps();
        let mut i = 0;
        while i + 8 <= a.len() {
            acc0 = _mm_add_ps(acc0, _mm_mul_ps(_mm_loadu_ps(a.as_ptr().add(i)), _mm_loadu_ps(b.as_ptr().add(i))));
            acc1 = _mm_add_ps(acc1, _mm_mul_ps(_mm_loadu_ps(a.as_ptr().add(i + 4)), _mm_loadu_ps(b.as_ptr().add(i + 4))));
            i += 8;
        }
        let s = _mm_add_ps(acc0, acc1);
        let mut out = [0.0f32; 4];
        _mm_storeu_ps(out.as_mut_ptr(), s);
        (out[0] + out[1]) + (out[2] + out[3])
    }
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn dot_simd(a: &[f32], b: &[f32]) -> f32 {
    dot_plain(a, b)
}
/// the same 8 independent partial sums in plain arrays (auto-vectorisable); not bit-identical to the
/// intrinsic version on aarch64 (fused multiply-add), each version has its own checksum
fn dot_plain(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 8];
    for (x, y) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
        for l in 0..8 {
            acc[l] += x[l] * y[l];
        }
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]))
}
fn muladd_plain(y: &mut [f32], x: &[f32], k: f32) {
    for (o, v) in y.iter_mut().zip(x.iter()) {
        *o += *v * k;
    }
}
fn f32_simd() -> u64 {
    let n = 8192usize;
    let a: Vec<f32> = (0..n).map(|i| ((i * 31) % 101) as f32 * 0.01 - 0.5).collect();
    let b: Vec<f32> = (0..n).map(|i| ((i * 17) % 89) as f32 * 0.02 - 0.9).collect();
    let mut y = vec![0.0f32; n];
    let mut h = 0u64;
    for r in 0..800 {
        let d1 = dot_simd(black_box(&a), black_box(&b));
        let d2 = dot_plain(black_box(&a), black_box(&b));
        muladd_plain(&mut y, &a, 0.001 * (r & 7) as f32);
        h = h.rotate_left(7) ^ d1.to_bits() as u64 ^ ((d2.to_bits() as u64) << 20);
    }
    for v in y.iter().step_by(31) {
        h = h.rotate_left(3) ^ v.to_bits() as u64;
    }
    h
}

/// Vec of 64..160-byte structs copied/moved through Option and pushed (the leap MirrorEntry pattern)
#[derive(Clone)]
struct Entry {
    id: u64,
    kind: u32,
    flags: u32,
    pos: [f64; 4],
    extent: [f32; 8],
    tag: [u8; 24],
}
#[derive(Clone)]
struct Big {
    head: Entry,
    tail: [u64; 5],
}
fn make_entry(i: u64) -> Entry {
    Entry { id: i, kind: (i % 7) as u32, flags: (i >> 3) as u32, pos: [i as f64; 4], extent: [i as f32 * 0.5; 8], tag: [(i & 255) as u8; 24] }
}
#[inline(never)]
fn maybe_entry(i: u64) -> Option<Big> {
    if i % 5 == 4 {
        None
    } else {
        Some(Big { head: make_entry(i), tail: [i; 5] })
    }
}
fn struct_copy() -> u64 {
    let mut out: Vec<Big> = Vec::new();
    let mut small: Vec<Entry> = Vec::new();
    for round in 0..400u64 {
        out.clear();
        small.clear();
        for i in 0..400u64 {
            if let Some(b) = maybe_entry(round * 1000 + i) {
                small.push(b.head.clone());
                out.push(b);
            }
        }
        // move through Option and back, rewrite a field
        let mut slot: Option<Big> = None;
        for b in out.iter_mut() {
            let prev = slot.replace(b.clone());
            if let Some(mut p) = prev {
                p.head.flags ^= 1;
                *b = p;
            }
        }
    }
    let mut h = 0u64;
    for b in out.iter() {
        h = h.rotate_left(5) ^ b.head.id ^ b.head.flags as u64 ^ b.tail[2] ^ b.head.tag[3] as u64;
    }
    for e in small.iter() {
        h = h.rotate_left(3) ^ e.id ^ e.kind as u64 ^ e.extent[1].to_bits() as u64 ^ e.pos[2].to_bits();
    }
    h
}

/// a byte-level state machine with a big match (the tokenizer pattern)
fn match_tokens() -> u64 {
    #[derive(Clone, Copy, PartialEq)]
    enum St {
        Start,
        Ident,
        Num,
        Str,
        StrEsc,
        Slash,
        Line,
        Block,
        BlockStar,
        Punct,
    }
    let src: Vec<u8> = {
        let words = [&b"fn main() {\n"[..], b"    let x_1 = 42 + y * 3.5; // note\n", b"    /* block * comment */ s = \"str\\\"ing\";\n", b"}\n", b"if a == b { c += 1 }\n"];
        let mut v = Vec::new();
        for i in 0..48000 {
            v.extend_from_slice(words[i % words.len()]);
        }
        v
    };
    let mut st = St::Start;
    let (mut h, mut ntok) = (0u64, 0u64);
    let mut start = 0usize;
    for (i, &c) in src.iter().enumerate() {
        let (next_st, emit) = match st {
            St::Start => match c {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => (St::Ident, false),
                b'0'..=b'9' => (St::Num, false),
                b'"' => (St::Str, false),
                b'/' => (St::Slash, false),
                b' ' | b'\n' | b'\t' | b'\r' => (St::Start, false),
                _ => (St::Punct, false),
            },
            St::Ident => match c {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9' => (St::Ident, false),
                _ => (St::Start, true),
            },
            St::Num => match c {
                b'0'..=b'9' | b'.' => (St::Num, false),
                _ => (St::Start, true),
            },
            St::Str => match c {
                b'\\' => (St::StrEsc, false),
                b'"' => (St::Start, true),
                _ => (St::Str, false),
            },
            St::StrEsc => (St::Str, false),
            St::Slash => match c {
                b'/' => (St::Line, false),
                b'*' => (St::Block, false),
                _ => (St::Start, true),
            },
            St::Line => match c {
                b'\n' => (St::Start, true),
                _ => (St::Line, false),
            },
            St::Block => match c {
                b'*' => (St::BlockStar, false),
                _ => (St::Block, false),
            },
            St::BlockStar => match c {
                b'/' => (St::Start, true),
                b'*' => (St::BlockStar, false),
                _ => (St::Block, false),
            },
            St::Punct => (St::Start, true),
        };
        if emit {
            ntok += 1;
            h = h.rotate_left(5) ^ ((st as u64) << 32) ^ (i - start) as u64 ^ src[start] as u64;
            // the byte that ended the token starts the next one
            st = St::Start;
            start = i;
            let again = match c {
                b'a'..=b'z' | b'A'..=b'Z' | b'_' => St::Ident,
                b'0'..=b'9' => St::Num,
                b'"' => St::Str,
                b'/' => St::Slash,
                b' ' | b'\n' | b'\t' | b'\r' => St::Start,
                _ => St::Punct,
            };
            if next_st == St::Start || next_st == St::Punct {
                st = again;
            }
        } else {
            if st == St::Start && next_st != St::Start {
                start = i;
            }
            st = next_st;
        }
    }
    h ^ ntok
}

fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    if only.iter().any(|o| o == "shape_dump") {
        shape_dump();
        return;
    }
    let all: [(&str, fn() -> u64); 30] = [
        ("det_trig", det_trig),
        ("det_explog", det_explog),
        ("hash_bytes", hash_bytes),
        ("vec_step", vec_step),
        ("sort_ints", sort_ints),
        ("vec_push", vec_push),
        ("vec_extend", vec_extend),
        ("sort_by_key", sort_by_key),
        ("parse_ints", parse_ints),
        ("rc_refcell", rc_refcell),
        ("iter_chain", iter_chain),
        ("glyph_cache", glyph_cache),
        ("layout", layout),
        ("shape", shape),
        ("hash_check", hash_check),
        ("sum_slice", sum_slice),
        ("llm_logits", llm_logits),
        ("deflate", deflate),
        ("inflate", inflate),
        ("adler32", adler32),
        ("splash_vm", splash_vm),
        ("statics_loop", statics_loop),
        ("statics_calls", statics_calls),
        ("hashmap", hashmap),
        ("closures", closures),
        ("dyn_trait", dyn_trait),
        ("bounds_idx", bounds_idx),
        ("f32_simd", f32_simd),
        ("struct_copy", struct_copy),
        ("match_tokens", match_tokens),
    ];
    for (n, f) in all.iter() {
        if only.is_empty() || only.iter().any(|o| o == n) {
            measure_k(n, *f);
        }
    }
}

/// derive(Hash) + == on a small key struct (rustc inlines the whole hasher)
fn hash_check() -> u64 {
    let mut h = 0u64;
    let mut x = 1u64;
    for _ in 0..1_000_000 {
        let k = GlyphKey { font: next(&mut x), glyph: x as u16, size_q: (x >> 16) as u32 };
        let mut s = Fx::default();
        k.hash(&mut s);
        h = h.wrapping_add(s.finish());
    }
    h
}

/// slice iteration with bounds-check-free indexing (simple loop facts)
fn sum_slice() -> u64 {
    let v: Vec<u32> = (0..100_000u32).collect();
    let mut t = 0u64;
    for r in 0..100 {
        let s = black_box(&v[..]);
        let mut acc = 0u64;
        for i in 0..s.len() {
            acc = acc.wrapping_add((s[i] as u64) * (r + 1));
        }
        t ^= acc;
    }
    t
}

/// prints one shaping result glyph by glyph (differential debugging against rustc)
fn shape_dump() {
    let data = std::fs::read(font_path()).unwrap();
    let face = rustybuzz::Face::from_slice(&data, 0).unwrap();
    let feats: Vec<rustybuzz::Feature> = Vec::new();
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(TEXT);
    let out = rustybuzz::shape(&face, &feats, buf);
    for (i, p) in out.glyph_infos().iter().zip(out.glyph_positions()) {
        println!("{} {} {} {} {} {}", i.glyph_id, i.cluster, p.x_advance, p.y_advance, p.x_offset, p.y_offset);
    }
}

/// image-like bytes: smooth gradients with noise and flat runs (a grab's PNG rows)
fn image_bytes() -> Vec<u8> {
    let mut v = Vec::with_capacity(256 * 1024);
    let mut x = 77u64;
    for y in 0..256u32 {
        for i in 0..1024u32 {
            let flat = (i / 128) % 3 == 0;
            let b = if flat { 200 } else { ((i ^ y) as u8).wrapping_add((next(&mut x) & 3) as u8) };
            v.push(b);
        }
    }
    v
}

/// zlib compression (miniz port; makepad's PNG encoder for grabs)
fn deflate() -> u64 {
    let data = image_bytes();
    let mut h = 0u64;
    for level in [1u32, 6] {
        let out = makepad_fast_inflate::zlib_compress(black_box(&data), level);
        h = h.rotate_left(9) ^ out.len() as u64;
        for c in out.chunks(8) {
            let mut w = 0u64;
            for b in c {
                w = (w << 8) | *b as u64;
            }
            h = h.rotate_left(5) ^ w;
        }
    }
    h
}

/// zlib's checksum alone: NEON / SSE2 core::arch intrinsics
fn adler32() -> u64 {
    let data = image_bytes();
    let mut h = 1u32;
    for _ in 0..16 {
        h = makepad_fast_inflate::adler32(h, black_box(&data));
    }
    h as u64
}

/// zlib decompression of the same data
fn inflate() -> u64 {
    let data = image_bytes();
    let z = makepad_fast_inflate::zlib_compress(&data, 6);
    let mut h = 0u64;
    for _ in 0..4 {
        let out = makepad_fast_inflate::inflate::decompress_to_vec_zlib(black_box(&z)).unwrap();
        h = h.rotate_left(3) ^ out.len() as u64 ^ out[out.len() / 3] as u64;
    }
    h
}

/// the CUDA LLM's per-token host work: logits bytes -> Vec<f32> through a Result-collecting map
/// over chunks_exact (makepad_ai_llm runtime.rs f32_bytes_to_vec), then the greedy argmax
#[derive(Debug)]
#[allow(dead_code)]
pub struct LlmErr(String);
#[inline(never)]
fn llm_f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>, LlmErr> {
    if bytes.len() % 4 != 0 {
        return Err(LlmErr(format!("bad len {}", bytes.len())));
    }
    bytes.chunks_exact(4).map(|chunk| Ok(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))).collect()
}
#[inline(never)]
fn llm_argmax(v: &[f32]) -> usize {
    let mut best = 0;
    let mut bv = f32::NEG_INFINITY;
    for (i, x) in v.iter().enumerate() {
        if *x > bv {
            bv = *x;
            best = i;
        }
    }
    best
}
fn llm_logits() -> u64 {
    let n = 151_936usize;
    let mut bytes = Vec::with_capacity(n * 4);
    let mut s = 1u32;
    for _ in 0..n {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        bytes.extend_from_slice(&((s >> 9) as f32 / 1000.0).to_le_bytes());
    }
    let mut acc = 0u64;
    for _ in 0..8 {
        let v = black_box(llm_f32_bytes_to_vec(black_box(&bytes))).unwrap();
        acc = acc.wrapping_add(llm_argmax(&v) as u64);
    }
    acc
}
