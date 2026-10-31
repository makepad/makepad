// diff_fast corpus program: extend a HashSet / Vec from a slice iterator (copied / mapped), whose
// exact size_hint is inlined into the caller (platform hal fonts GlyphWait::note; render draw_models_inner
// parts.extend). Relax O3 reported "capacity overflow" in the inlined reserve in the Sandbox: arm64 emitted
// a fused pair (`add d, x, a, lsl #4`: the slice end) at the shift's position, so the end's def window
// (a register holding the spilled value from its definition on) was never written, and the length
// `(end - ptr) >> 4` read a stale register.
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
use std::collections::HashSet;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
struct Wait { pending: HashSet<(u32, u32, u32, u32)>, presented: HashSet<u64>, timed_out: bool, drawn_blank: usize, blank_frames: u64 }
impl Wait {
    #[inline(never)]
    fn note(&mut self, glyphs: &[(u32, u32, u32, u32)]) {
        self.drawn_blank += glyphs.len();
        self.pending.extend(glyphs.iter().copied());
    }
}
struct Part { idx: Vec<u32>, draws: Vec<(u32, [f32; 2], String)> }
#[inline(never)]
fn parts(ps: &[Part], extra: &[Part]) -> Vec<(usize, Vec<(u32, [f32; 2], String)>)> {
    let mut out = Vec::with_capacity(ps.len() + extra.len());
    out.extend(ps.iter().map(|p| (p.idx.len() / 3, p.draws.iter().map(|(g, s, n)| (*g, *s, n.clone())).collect())));
    out.extend(extra.iter().map(|p| (p.idx.len(), p.draws.iter().map(|(g, s, n)| (*g + 1, *s, n.clone())).collect())));
    out
}
#[inline(never)]
fn layered(l: &[((usize, usize, f32), usize)], buf: &mut Vec<(usize, usize, f32)>, pass: usize) -> usize {
    buf.clear();
    buf.extend(l.iter().filter(|(_, n)| *n > pass).map(|(e, _)| *e));
    buf.len()
}
// The O3 build of `note` (a large HashSet resize inlined) takes longer than the default 250 ms gap:
// rounds wait four gaps so the O3 code runs in the later rounds.
fn rounds(f: fn()) {
    let ms: u64 = 4 * std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

fn work() {
    let mut w = Wait { pending: HashSet::new(), presented: HashSet::new(), timed_out: false, drawn_blank: 0, blank_frames: 0 };
    let glyphs: Vec<(u32, u32, u32, u32)> = (0..37u32).map(|i| (i % 3, i, i & 1, 1000 + i % 5)).collect();
    for i in 0..3000usize {
        let n = i % 38;
        w.note(&glyphs[..n]);
        w.note(&glyphs[n / 2..n]);
        if i % 100 == 0 { w.pending.clear(); }
    }
    d!("note: drawn_blank={} pending={} presented={} {} {}", w.drawn_blank, w.pending.len(), w.presented.len(), w.timed_out, w.blank_frames);
    let mk = |k: usize| Part { idx: (0..(k as u32 * 3)).collect(), draws: (0..k as u32).map(|j| (j, [j as f32, 0.5], format!("d{j}"))).collect() };
    let ps: Vec<Part> = (0..4).map(mk).collect();
    let ex: Vec<Part> = (0..6).map(|k| mk(k + 1)).collect();
    let mut tot = 0usize;
    for i in 0..3000usize {
        let r = parts(&ps[..i % 5], &ex[..i % 7]);
        tot += r.len() + r.iter().map(|(t, d)| t + d.len()).sum::<usize>();
    }
    d!("parts: {}", tot);
    let l: Vec<((usize, usize, f32), usize)> = (0..50).map(|i| ((i, i % 3, i as f32 * 0.5), i % 7)).collect();
    let mut buf = Vec::new();
    let mut s = 0usize;
    for i in 0..3000usize { s += layered(&l[..i % 51], &mut buf, i % 8); }
    d!("layered: {} cap>=len {}", s, buf.capacity() >= buf.len());
}
