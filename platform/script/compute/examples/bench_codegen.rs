//! Code generator quality: real kernels from Stage's plates against the
//! same maths hand-written in Rust (compiled by rustc --release), on one
//! thread, in ns per element.
//!
//! Each Rust twin computes exactly what the kernel computes: the same
//! polynomials for sin/cos/exp (the kernel's `math: fast` ones), the same
//! operation order, the same wrapped array indexing and clamped buffer
//! stores, so the comparison is honest about the safety cost. Every twin's
//! output is compared with the kernel's (bit-equal or the largest ulp
//! difference is printed).
//!
//! The plate kernels are the kernel text Motion's lower_document builds
//! for a `Kernel{}` block (the document's helpers and tables it reaches,
//! then its declarations and functions), read from `$KC1_KERNELS` or
//! `local/agent_state/edits/scratch/KC1/kernels`:
//!   sk_lines.splash  stack.motion.splash's sk_lines (341712 segments)
//!   k_myc.splash     room.motion.splash's k_myc (20007 hairlines, 8 sin per warp)
//!   k_room.splash    room.motion.splash's k_room (8162 elements, no Rust twin)
//! The fbm heightfield (noise) is inline.
//!
//! cargo run --release -p makepad-script-compute --example bench_codegen
//! (KC1_ASM=dir writes each kernel's scalar and NEON code as .s files.)

use makepad_script_compute::kernel::{compile_with, Access, FieldTy, Kernel, Layout, LayoutField};
use makepad_script_compute::Backend;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

fn segment() -> Layout {
    Layout {
        name: "Segment".into(),
        stride: 11,
        fields: vec![
            LayoutField { name: "a".into(), ty: FieldTy::Vec3, offset: 0 },
            LayoutField { name: "b".into(), ty: FieldTy::Vec3, offset: 3 },
            LayoutField { name: "color".into(), ty: FieldTy::Vec4, offset: 6 },
            LayoutField { name: "width".into(), ty: FieldTy::F32, offset: 10 },
        ],
    }
}

// ---------------------------------------------------------------------------
// The kernel maths in Rust (lower.rs Builder: sincos, exp, poly; min/max as
// selects; floor-to-int; Euclidean wrap)
// ---------------------------------------------------------------------------

#[inline(always)]
fn mn(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}
#[inline(always)]
fn mx(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}
#[inline(always)]
fn poly(x: f32, c: &[f32]) -> f32 {
    let mut acc = c[0];
    for k in &c[1..] {
        acc = acc * x + *k;
    }
    acc
}
#[inline(always)]
fn sincos(x: f32, cos: bool) -> f32 {
    const FOPI: f32 = 1.273_239_5;
    const DP1: f32 = 0.785_156_25;
    const DP2: f32 = 2.418_756_5e-4;
    const DP3: f32 = 3.774_895e-8;
    let ax = x.abs();
    let mut j = (ax * FOPI) as i32;
    j = j.wrapping_add(j & 1);
    let y = j as f32;
    let j = j & 7;
    let flip = j > 3;
    let j = if flip { j - 4 } else { j };
    let r = ax - y * DP1;
    let r = r - y * DP2;
    let r = r - y * DP3;
    let z = r * r;
    let sp = poly(z, &[-1.951_529_6e-4, 8.332_161e-3, -1.666_665_5e-1]) * z * r + r;
    let cp = poly(z, &[2.443_315_7e-5, -1.388_731_6e-3, 4.166_664_6e-2]) * z * z - z * 0.5 + 1.0;
    let mid = j == 1 || j == 2;
    let (v, neg) = if cos { (if mid { sp } else { cp }, flip != (j > 1)) } else { (if mid { cp } else { sp }, flip != (x < 0.0)) };
    if neg {
        -v
    } else {
        v
    }
}
#[inline(always)]
fn sin(x: f32) -> f32 {
    sincos(x, false)
}
#[inline(always)]
fn cos(x: f32) -> f32 {
    sincos(x, true)
}
#[inline(always)]
fn exp(x: f32) -> f32 {
    let x = mn(mx(x, -87.3), 88.0);
    let fx = (x * std::f32::consts::LOG2_E + 0.5).floor();
    let x = x - fx * 0.693_359_4;
    let x = x - fx * -2.121_944_4e-4;
    let z = x * x;
    let p = poly(x, &[1.987_569_1e-4, 1.398_199_9e-3, 8.333_452e-3, 4.166_579_6e-2, 1.666_666_5e-1, 5.000_000_1e-1]);
    let p = p * z + x + 1.0;
    let n = (fx as i32).wrapping_add(127).wrapping_shl(23);
    p * f32::from_bits(n as u32)
}
#[inline(always)]
fn fi(x: f32) -> i32 {
    x.floor() as i32
}
/// A table read: Euclidean wrap into the table, as the language defines it.
#[inline(always)]
fn tab<T: Copy>(t: &[T], i: i32) -> T {
    t[i.rem_euclid(t.len() as i32) as usize]
}
/// A buffer store: the word index clamped to the buffer, as every backend does.
#[inline(always)]
fn st(b: &mut [f32], at: usize, v: f32) {
    let n = b.len() - 1;
    b[at.min(n)] = v;
}

/// Numbers of `let NAME = [..]` in kernel text (vecN(..) flattened).
fn table(text: &str, name: &str) -> Vec<f64> {
    let head = format!("let {} = [", name);
    let s = text.find(&head).unwrap_or_else(|| panic!("no table {}", name)) + head.len();
    let e = s + text[s..].find(']').unwrap();
    text[s..e].replace("vec4(", "").replace("vec3(", "").replace("vec2(", "").replace(')', "").split(',').map(|x| x.trim().parse::<f64>().unwrap()).collect()
}

// ---------------------------------------------------------------------------
// sk_lines (stack)
// ---------------------------------------------------------------------------

struct SkTables {
    sty: Vec<[f32; 4]>,
    ta: Vec<[f32; 4]>,
    tb: Vec<[f32; 4]>,
    la: Vec<[f32; 3]>,
    lb: Vec<[f32; 3]>,
    loff: Vec<i32>,
    ln: Vec<i32>,
}

fn v4(t: Vec<f64>) -> Vec<[f32; 4]> {
    t.chunks(4).map(|c| [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32]).collect()
}
fn v3(t: Vec<f64>) -> Vec<[f32; 3]> {
    t.chunks(3).map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect()
}

fn sk_lines_rust(tb_: &SkTables, p: &HashMap<&str, f32>, out: &mut [f32], count: usize) {
    let g = |n: &str| p[n];
    let (pa_x, pa_y, pa_z, pa_w) = (g("pa_x"), g("pa_y"), g("pa_z"), g("pa_w"));
    let (pb_x, pb_y, pb_z, pb_w) = (g("pb_x"), g("pb_y"), g("pb_z"), g("pb_w"));
    let (pc_x, pc_y, pc_z, pc_w) = (g("pc_x"), g("pc_y"), g("pc_z"), g("pc_w"));
    let (pd_x, pd_y, pd_z, pd_w) = (g("pd_x"), g("pd_y"), g("pd_z"), g("pd_w"));
    let (pe_x, pe_y, pe_z) = (g("pe_x"), g("pe_y"), g("pe_z"));
    for i in 0..count {
        let per_copy = 28476.0f32;
        let fi_ = i as i32 as f32;
        let c = (fi_ / per_copy).floor();
        let r = fi_ - c * per_copy;
        let j = (r / 1582.0).floor();
        let s = fi(r - j * 1582.0);
        let k = pa_x + j;
        let mut alpha = 1.0f32;
        if c >= pa_z {
            alpha = 0.0
        }
        if k > pa_y + 14.0 {
            alpha = 0.0
        }
        let by = 0.0 - k * 7.6;
        let dy = pc_y - by;
        let dz = (pc_x * pc_x + dy * dy + pc_z * pc_z).sqrt();
        let fog_k = exp((0.0 - mx(0.0, dz - pc_w * 0.85)) / (pc_w * 1.1 * pd_x));
        if fog_k < 0.02 {
            alpha = 0.0
        }
        let mut a = [0.0f32; 3];
        let mut b = [0.0f32; 3];
        let mut col = [0.0f32; 4];
        let mut w = 1.0f32;
        let mut hot = false;
        if s < 1505 {
            let ta = tab(&tb_.ta, s);
            let tb = tab(&tb_.tb, s);
            a = [ta[0], ta[1], ta[2]];
            b = [tb[0], tb[1], tb[2]];
            col = tab(&tb_.sty, fi(ta[3]));
            w = tb[3];
            hot = s >= 1472;
        } else {
            let li = s.wrapping_sub(1505);
            let kk = fi(mn(k, 31.0));
            if li < tab(&tb_.ln, kk) {
                a = tab(&tb_.la, tab(&tb_.loff, kk).wrapping_add(li));
                b = tab(&tb_.lb, tab(&tb_.loff, kk).wrapping_add(li));
                col = [0.332452, 0.309469, 0.274677, 0.9];
                w = 1.1;
            } else {
                alpha = 0.0
            }
        }
        let is_focus = k == pa_y;
        let boost = if is_focus { pb_y } else { 1.0 };
        let mut rgb = [col[0] * boost, col[1] * boost, col[2] * boost];
        if hot && is_focus && pb_z > 0.0 {
            let ymid = (a[1] + b[1]) / 2.0;
            let mut glow = 0.0f32;
            if ymid < pb_w {
                glow = exp(0.0 - (pb_w - ymid) * 0.5) * pb_z
            }
            let t = [3.0f32, 0.2226407, 0.0181465];
            for q in 0..3 {
                rgb[q] = rgb[q] + (t[q] - rgb[q]) * glow;
            }
        }
        if k == 13.0 && pd_y > 0.5 {
            let (cy, sy, cr, sr) = (cos(pd_z), sin(pd_z), cos(pd_w), sin(pd_w));
            let ax1 = a[0] * cr - a[1] * sr;
            let ay1 = a[0] * sr + a[1] * cr;
            a = [ax1 * cy + a[2] * sy + pe_x, ay1 + pe_y, 0.0 - ax1 * sy + a[2] * cy + pe_z];
            let bx1 = b[0] * cr - b[1] * sr;
            let by1 = b[0] * sr + b[1] * cr;
            b = [bx1 * cy + b[2] * sy + pe_x, by1 + pe_y, 0.0 - bx1 * sy + b[2] * cy + pe_z];
        }
        let mut al = col[3] * fog_k * pb_x;
        let mut off = 0.0f32;
        if pa_z > 1.0 {
            off = pa_w * (c / (pa_z - 1.0));
            al = (al / pa_z) * 1.6
        }
        let o = 11 * i;
        let yo = by + off;
        st(out, o, a[0] + 0.0);
        st(out, o + 1, a[1] + yo);
        st(out, o + 2, a[2] + 0.0);
        st(out, o + 3, b[0] + 0.0);
        st(out, o + 4, b[1] + yo);
        st(out, o + 5, b[2] + 0.0);
        st(out, o + 6, rgb[0]);
        st(out, o + 7, rgb[1]);
        st(out, o + 8, rgb[2]);
        st(out, o + 9, al * alpha);
        st(out, o + 10, w);
    }
}

// ---------------------------------------------------------------------------
// k_myc (room): depth clip, projection, cull, 8-sine warp, four emit buffers
// ---------------------------------------------------------------------------

struct Emit<'a> {
    data: &'a mut [f32],
    cnt: &'a mut [f32],
    cap: usize,
}

#[inline(always)]
fn emit(e: &mut Emit, n: &mut usize, elem: usize, rec: &[f32; 11], overflow: &mut bool) {
    if *n < e.cap {
        let at = (elem * e.cap + *n) * 11;
        for (k, x) in rec.iter().enumerate() {
            st(e.data, at + k, *x);
        }
        *n += 1;
    } else {
        *overflow = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn k_myc_rust(myc: &[f32], p: &HashMap<&str, f32>, time: f32, bufs: [&mut Emit; 4], start: usize, count: usize) -> bool {
    let g = |n: &str| p[n];
    let c_p = [g("c_px"), g("c_py"), g("c_pz")];
    let c_r = [g("c_rx"), g("c_ry"), g("c_rz")];
    let c_u = [g("c_ux"), g("c_uy"), g("c_uz")];
    let c_f = [g("c_fx"), g("c_fy"), g("c_fz")];
    let (c_ff, c_wa, c_wt, b_th, b_sd, m_front) = (g("c_ff"), g("c_wa"), g("c_wt"), g("b_th"), g("b_sd"), g("m_front"));
    let [ib, if_, gb, gf] = bufs;
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let depth = |x: [f32; 3]| dot(sub(x, c_p), c_f);
    let proj = |x: [f32; 3]| {
        let rel = sub(x, c_p);
        let z = dot(rel, c_f);
        [960.0 + c_ff * dot(rel, c_r) / z, 540.0 - c_ff * dot(rel, c_u) / z]
    };
    let warp = |x: f32, y: f32| {
        if c_wa <= 0.0 {
            return [x, y];
        }
        let t = c_wt;
        let dx = sin(y * 0.0105 + t * 3.1 + sin(x * 0.006 - t * 1.7) * 1.6) + 0.5 * sin((x + y) * 0.019 - t * 4.3);
        let dy = sin(x * 0.0093 - t * 2.6 + sin(y * 0.008 + t * 1.3) * 1.4) + 0.5 * sin((x - y) * 0.017 + t * 3.7);
        [x + dx * c_wa, y + dy * c_wa]
    };
    let front = |x: [f32; 3]| {
        let n = [0.0 * b_sd, (0.0 - sin(b_th)) * b_sd, cos(b_th) * b_sd];
        dot(sub(x, [0.0, 3.0, (0.0f64 - 7.55) as f32]), n) > 0.0
    };
    let t_land = (28.873f64 - 0.02) as f32;
    let mut overflow = false;
    for i in start..count {
        let (mut nib, mut nif, mut ngb, mut ngf) = (0usize, 0usize, 0usize, 0usize);
        let tt = time + 26.146;
        let o = i as i32 as f32 * 7.0;
        let bd = tab(myc, fi(o + 6.0));
        if tt >= t_land && bd <= m_front {
            let tip = exp((0.0 - (m_front - bd)) / 0.3);
            let (c1, c2) = ([0.5300954f32, 0.5052049, 0.4575045], [0.652351f32, 0.95, 0.04292689]);
            let mt = 0.15 + 0.85 * tip;
            let col = [c1[0] + (c2[0] - c1[0]) * mt, c1[1] + (c2[1] - c1[1]) * mt, c1[2] + (c2[2] - c1[2]) * mt];
            let gk = 0.7 * tip;
            let gcol = [0.6866853 * gk, 1.0 * gk, 0.0451862 * gk];
            let s_a = [tab(myc, fi(o)), tab(myc, fi(o + 1.0)), tab(myc, fi(o + 2.0))];
            let s_b = [tab(myc, fi(o + 3.0)), tab(myc, fi(o + 4.0)), tab(myc, fi(o + 5.0))];
            for pass in 0..2 {
                if pass == 1 && !(tip > 0.25) {
                    break;
                }
                let (mut ca, mut cb) = (s_a, s_b);
                let (da, db) = (depth(ca), depth(cb));
                if da >= 0.1 || db >= 0.1 {
                    if da < 0.1 {
                        let u = (0.1 - da) / (db - da);
                        ca = [ca[0] + (cb[0] - ca[0]) * u, ca[1] + (cb[1] - ca[1]) * u, ca[2] + (cb[2] - ca[2]) * u];
                    } else if db < 0.1 {
                        let u = (0.1 - db) / (da - db);
                        cb = [cb[0] + (ca[0] - cb[0]) * u, cb[1] + (ca[1] - cb[1]) * u, cb[2] + (ca[2] - cb[2]) * u];
                    }
                    let pa = proj(ca);
                    let pb = proj(cb);
                    let cull = (pa[0] < -250.0 && pb[0] < -250.0) || (pa[0] > 2170.0 && pb[0] > 2170.0) || (pa[1] < -250.0 && pb[1] < -250.0) || (pa[1] > 1330.0 && pb[1] > 1330.0);
                    if !cull {
                        let wa = warp(pa[0], pa[1]);
                        let wb = warp(pb[0], pb[1]);
                        let (ww, c) = if pass == 0 { (0.9f64, col) } else { (1.8f64, gcol) };
                        let aa = (0.8 * (ww / 0.7).min(1.0)) as f32;
                        let rec = [wa[0], 0.0 - wa[1], 0.0, wb[0], 0.0 - wb[1], 0.0, c[0], c[1], c[2], aa, ww as f32];
                        match (pass, front(s_a)) {
                            (0, true) => emit(if_, &mut nif, i, &rec, &mut overflow),
                            (0, false) => emit(ib, &mut nib, i, &rec, &mut overflow),
                            (_, true) => emit(gf, &mut ngf, i, &rec, &mut overflow),
                            (_, false) => emit(gb, &mut ngb, i, &rec, &mut overflow),
                        }
                    }
                }
            }
        }
        // Counts in buffer order (ib, if_, gb, gf), as the kernel stores them.
        let n = ib.cnt.len() - 1;
        ib.cnt[i.min(n)] = f32::from_bits(nib as u32);
        if_.cnt[i.min(n)] = f32::from_bits(nif as u32);
        gb.cnt[i.min(n)] = f32::from_bits(ngb as u32);
        gf.cnt[i.min(n)] = f32::from_bits(ngf as u32);
    }
    overflow
}

// ---------------------------------------------------------------------------
// fbm heightfield (noise): the prelude's hash, gradient noise and fbm2
// ---------------------------------------------------------------------------

const HEIGHTS: &str = r#"
    let W = 1024
    let base = input(f32)
    let pos = output(vec3)
    let hgt = output(f32)
    let amp = param(6.0)
    fn vertex(i) {
        let x = float(i % W)
        let z = float(i / W)
        let h = base[i] + fbm2(vec2(x, z) * 0.01, 4, 2.0, 0.5) * amp
        pos[i] = vec3(x, h, z)
        hgt[i] = h
    }
"#;

#[inline(always)]
fn hash(x: i32) -> i32 {
    let s = x.wrapping_mul(747796405).wrapping_add(0xAC564B05u32 as i32);
    let w = (((s as u32) >> (((s as u32) >> 28).wrapping_add(4) & 31)) as i32 ^ s).wrapping_mul(277803737);
    ((w as u32) >> 22) as i32 ^ w
}
#[inline(always)]
fn hash2(a: i32, b: i32) -> i32 {
    hash(a ^ hash(b.wrapping_add(0x9E3779B9u32 as i32)))
}
const GRAD2: [f32; 16] = [1.0, 0.0, 0.70710678, 0.70710678, 0.0, 1.0, -0.70710678, 0.70710678, -1.0, 0.0, -0.70710678, -0.70710678, 0.0, -1.0, 0.70710678, -0.70710678];
#[inline(always)]
fn grad2(x: i32, y: i32, fx: f32, fy: f32) -> f32 {
    let h = (hash2(x, y) & 7).wrapping_mul(2);
    fx * GRAD2[(h & 15) as usize] + fy * GRAD2[(h.wrapping_add(1) & 15) as usize]
}
#[inline(always)]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
#[inline(always)]
fn gnoise2(px: f32, py: f32) -> f32 {
    let (ix, iy) = (px.floor(), py.floor());
    let (fx, fy) = (px - ix, py - iy);
    let (x, y) = (fi(ix), fi(iy));
    let (u, v) = (fade(fx), fade(fy));
    let g00 = grad2(x, y, fx, fy);
    let a = g00 + (grad2(x.wrapping_add(1), y, fx - 1.0, fy) - g00) * u;
    let g01 = grad2(x, y.wrapping_add(1), fx, fy - 1.0);
    let b = g01 + (grad2(x.wrapping_add(1), y.wrapping_add(1), fx - 1.0, fy - 1.0) - g01) * u;
    (a + (b - a) * v) * 1.4142
}
fn heights_rust(base: &[f32], amp: f32, pos: &mut [f32], hgt: &mut [f32], count: usize) {
    for i in 0..count {
        let ii = i as i32;
        let x = (ii % 1024) as f32;
        let z = (ii / 1024) as f32;
        let (mut qx, mut qy) = (x * 0.01, z * 0.01);
        let (mut s, mut a) = (0.0f32, 0.5f32);
        for _ in 0..4 {
            s = s + a * gnoise2(qx, qy);
            qx = qx * 2.0;
            qy = qy * 2.0;
            a = a * 0.5;
        }
        let h = base[i.min(base.len() - 1)] + s * amp;
        st(pos, 3 * i, x);
        st(pos, 3 * i + 1, h);
        st(pos, 3 * i + 2, z);
        st(hgt, i, h);
    }
}

// ---------------------------------------------------------------------------
// "rustc fast" twins: the same kernels with the freedom the kernel compiler
// takes -- fused multiply-adds anywhere, four elements at a time with
// explicit NEON (LLVM does not vectorize these loops: branches, gathers),
// TBL for the noise table, invariants hoisted by hand. Not bit-equal to
// the kernels (different fusion points); compared within tolerance.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod fast {
    use super::{fi, mn, mx, st, tab, SkTables};
    use std::arch::aarch64::*;
    use std::collections::HashMap;

    type F = float32x4_t;
    type I = int32x4_t;
    type M = uint32x4_t;

    #[inline(always)]
    unsafe fn s(x: f32) -> F {
        vdupq_n_f32(x)
    }
    #[inline(always)]
    unsafe fn si(x: i32) -> I {
        vdupq_n_s32(x)
    }
    #[inline(always)]
    unsafe fn poly(x: F, c: &[f32]) -> F {
        let mut acc = s(c[0]);
        for k in &c[1..] {
            acc = vfmaq_f32(s(*k), acc, x);
        }
        acc
    }
    #[inline(always)]
    unsafe fn sel(m: M, a: F, b: F) -> F {
        vbslq_f32(m, a, b)
    }
    #[inline(always)]
    unsafe fn any(m: M) -> bool {
        vmaxvq_u32(m) != 0
    }
    #[inline(always)]
    pub unsafe fn sincos(x: F, cos: bool) -> F {
        let ax = vabsq_f32(x);
        let mut j = vcvtq_s32_f32(vmulq_f32(ax, s(1.273_239_5)));
        j = vaddq_s32(j, vandq_s32(j, si(1)));
        let y = vcvtq_f32_s32(j);
        let j = vandq_s32(j, si(7));
        let flip = vcgtq_s32(j, si(3));
        let j = vbslq_s32(flip, vsubq_s32(j, si(4)), j);
        let r = vfmsq_f32(ax, y, s(0.785_156_25));
        let r = vfmsq_f32(r, y, s(2.418_756_5e-4));
        let r = vfmsq_f32(r, y, s(3.774_895e-8));
        let z = vmulq_f32(r, r);
        let sp = vfmaq_f32(r, vmulq_f32(poly(z, &[-1.951_529_6e-4, 8.332_161e-3, -1.666_665_5e-1]), z), r);
        let cp = vfmaq_f32(vfmsq_f32(s(1.0), z, s(0.5)), vmulq_f32(poly(z, &[2.443_315_7e-5, -1.388_731_6e-3, 4.166_664_6e-2]), z), z);
        let mid = vorrq_u32(vceqq_s32(j, si(1)), vceqq_s32(j, si(2)));
        let (v, neg) = if cos {
            (sel(mid, sp, cp), veorq_u32(flip, vcgtq_s32(j, si(1))))
        } else {
            (sel(mid, cp, sp), veorq_u32(flip, vcltq_f32(x, s(0.0))))
        };
        sel(neg, vnegq_f32(v), v)
    }
    #[inline(always)]
    pub unsafe fn sin(x: F) -> F {
        sincos(x, false)
    }
    #[inline(always)]
    pub unsafe fn exp(x: F) -> F {
        let x = vminq_f32(vmaxq_f32(x, s(-87.3)), s(88.0));
        let fx = vrndmq_f32(vfmaq_f32(s(0.5), x, s(std::f32::consts::LOG2_E)));
        let x = vfmsq_f32(x, fx, s(0.693_359_4));
        let x = vfmsq_f32(x, fx, s(-2.121_944_4e-4));
        let z = vmulq_f32(x, x);
        let p = poly(x, &[1.987_569_1e-4, 1.398_199_9e-3, 8.333_452e-3, 4.166_579_6e-2, 1.666_666_5e-1, 5.000_000_1e-1]);
        let p = vaddq_f32(vfmaq_f32(x, p, z), s(1.0));
        let n = vshlq_n_s32(vaddq_s32(vcvtq_s32_f32(fx), si(127)), 23);
        vmulq_f32(p, vreinterpretq_f32_s32(n))
    }
    unsafe fn lanes(v: F) -> [f32; 4] {
        let mut a = [0.0f32; 4];
        vst1q_f32(a.as_mut_ptr(), v);
        a
    }
    unsafe fn lanes_i(v: I) -> [i32; 4] {
        let mut a = [0i32; 4];
        vst1q_s32(a.as_mut_ptr(), v);
        a
    }
    unsafe fn lanes_m(v: M) -> [u32; 4] {
        let mut a = [0u32; 4];
        vst1q_u32(a.as_mut_ptr(), v);
        a
    }
    unsafe fn ld(a: &[f32; 4]) -> F {
        vld1q_f32(a.as_ptr())
    }

    // -- fbm -----------------------------------------------------------------
    #[inline(always)]
    unsafe fn hash(x: I) -> I {
        let sv = vmlaq_s32(si(0xAC564B05u32 as i32), x, si(747796405));
        let su = vreinterpretq_u32_s32(sv);
        let amt = vaddq_u32(vshrq_n_u32(su, 28), vdupq_n_u32(4));
        let sh = vreinterpretq_s32_u32(vshlq_u32(su, vnegq_s32(vreinterpretq_s32_u32(amt))));
        let w = vmulq_s32(veorq_s32(sh, sv), si(277803737));
        veorq_s32(vreinterpretq_s32_u32(vshrq_n_u32(vreinterpretq_u32_s32(w), 22)), w)
    }
    #[inline(always)]
    unsafe fn lookup(t: uint8x16x4_t, idx: I) -> F {
        let b = vaddq_s32(vmulq_s32(idx, si(0x0404_0404)), si(0x0302_0100));
        vreinterpretq_f32_u8(vqtbl4q_u8(t, vreinterpretq_u8_s32(b)))
    }
    #[inline(always)]
    unsafe fn grad2(t: uint8x16x4_t, x: I, y: I, fx: F, fy: F) -> F {
        let h = vshlq_n_s32(vandq_s32(hash(veorq_s32(x, hash(vaddq_s32(y, si(0x9E3779B9u32 as i32))))), si(7)), 1);
        vfmaq_f32(vmulq_f32(fx, lookup(t, h)), fy, lookup(t, vaddq_s32(h, si(1))))
    }
    #[inline(always)]
    unsafe fn fade(t: F) -> F {
        vmulq_f32(vmulq_f32(vmulq_f32(t, t), t), vfmaq_f32(s(10.0), t, vfmsq_f32(vmulq_f32(t, s(6.0)), s(15.0), s(1.0))))
    }
    #[inline(always)]
    unsafe fn gnoise2(t: uint8x16x4_t, px: F, py: F) -> F {
        let (ix, iy) = (vrndmq_f32(px), vrndmq_f32(py));
        let (fx, fy) = (vsubq_f32(px, ix), vsubq_f32(py, iy));
        let (x, y) = (vcvtq_s32_f32(ix), vcvtq_s32_f32(iy));
        let (u, v) = (fade(fx), fade(fy));
        let one = si(1);
        let (fx1, fy1) = (vsubq_f32(fx, s(1.0)), vsubq_f32(fy, s(1.0)));
        let g00 = grad2(t, x, y, fx, fy);
        let a = vfmaq_f32(g00, vsubq_f32(grad2(t, vaddq_s32(x, one), y, fx1, fy), g00), u);
        let g01 = grad2(t, x, vaddq_s32(y, one), fx, fy1);
        let b = vfmaq_f32(g01, vsubq_f32(grad2(t, vaddq_s32(x, one), vaddq_s32(y, one), fx1, fy1), g01), u);
        vmulq_f32(vfmaq_f32(a, vsubq_f32(b, a), v), s(1.4142))
    }
    pub fn heights(base: &[f32], amp: f32, pos: &mut [f32], hgt: &mut [f32], count: usize) {
        let n4 = count & !3;
        unsafe {
            let g = super::GRAD2;
            let t = vld4q_u8_x(g.as_ptr() as *const u8);
            let mut i = 0;
            while i < n4 {
                let iv = vaddq_s32(si(i as i32), vld1q_s32([0, 1, 2, 3].as_ptr()));
                let x = vcvtq_f32_s32(vandq_s32(iv, si(1023)));
                let z = vcvtq_f32_s32(vshrq_n_s32(iv, 10));
                let (mut qx, mut qy) = (vmulq_f32(x, s(0.01)), vmulq_f32(z, s(0.01)));
                let mut sum = s(0.0);
                let mut a = 0.5f32;
                for _ in 0..4 {
                    sum = vfmaq_f32(sum, s(a), gnoise2(t, qx, qy));
                    qx = vmulq_f32(qx, s(2.0));
                    qy = vmulq_f32(qy, s(2.0));
                    a *= 0.5;
                }
                // Clamped reads and writes: the last group is inside (n4 <= len).
                let b = if i + 3 < base.len() { vld1q_f32(base.as_ptr().add(i)) } else { ld(&[0.0; 4]) };
                let h = vfmaq_f32(b, sum, s(amp));
                if 3 * i + 11 < pos.len() && i + 3 < hgt.len() {
                    vst3q_f32(pos.as_mut_ptr().add(3 * i), float32x4x3_t(x, h, z));
                    vst1q_f32(hgt.as_mut_ptr().add(i), h);
                }
                i += 4;
            }
        }
        // The tail: scalar.
        if n4 < count {
            let mut p2 = vec![0.0f32; 3 * count];
            let mut h2 = vec![0.0f32; count];
            super::heights_rust(base, amp, &mut p2, &mut h2, count);
            pos[3 * n4..3 * count].copy_from_slice(&p2[3 * n4..]);
            hgt[n4..count].copy_from_slice(&h2[n4..]);
        }
    }
    unsafe fn vld4q_u8_x(p: *const u8) -> uint8x16x4_t {
        uint8x16x4_t(vld1q_u8(p), vld1q_u8(p.add(16)), vld1q_u8(p.add(32)), vld1q_u8(p.add(48)))
    }

    // -- sk_lines ------------------------------------------------------------
    pub fn sk_lines(tb_: &SkTables, p: &HashMap<&str, f32>, out: &mut [f32], count: usize) {
        let g = |n: &str| p[n];
        let (pa_x, pa_y, pa_z, pa_w) = (g("pa_x"), g("pa_y"), g("pa_z"), g("pa_w"));
        let (pb_x, pb_y, pb_z, pb_w) = (g("pb_x"), g("pb_y"), g("pb_z"), g("pb_w"));
        let (pc_x, pc_y, pc_z, pc_w) = (g("pc_x"), g("pc_y"), g("pc_z"), g("pc_w"));
        let (pd_x, pd_y, pd_z, pd_w) = (g("pd_x"), g("pd_y"), g("pd_z"), g("pd_w"));
        let (pe_x, pe_y, pe_z) = (g("pe_x"), g("pe_y"), g("pe_z"));
        // Hoisted by hand.
        let (cy, sy, cr, sr) = (super::cos(pd_z), super::sin(pd_z), super::cos(pd_w), super::sin(pd_w));
        let fog_den = pc_w * 1.1 * pd_x;
        let pcx2z2 = pc_x * pc_x;
        let n4 = count & !3;
        unsafe {
            let mut i = 0;
            while i < n4 {
                let fiv = vcvtq_f32_s32(vaddq_s32(si(i as i32), vld1q_s32([0, 1, 2, 3].as_ptr())));
                let per = s(28476.0);
                let c = vrndmq_f32(vdivq_f32(fiv, per));
                let r = vfmsq_f32(fiv, c, per);
                let j = vrndmq_f32(vdivq_f32(r, s(1582.0)));
                let sv = vcvtq_s32_f32(vrndmq_f32(vfmsq_f32(r, j, s(1582.0))));
                let k = vaddq_f32(s(pa_x), j);
                let mut alpha = s(1.0);
                alpha = sel(vcgeq_f32(c, s(pa_z)), s(0.0), alpha);
                alpha = sel(vcgtq_f32(k, s(pa_y + 14.0)), s(0.0), alpha);
                let by = vfmsq_f32(s(0.0), k, s(7.6));
                let dy = vsubq_f32(s(pc_y), by);
                let dz = vsqrtq_f32(vfmaq_f32(vfmaq_f32(s(pcx2z2), dy, dy), s(pc_z), s(pc_z)));
                let fog = exp(vdivq_f32(vsubq_f32(s(0.0), vmaxq_f32(s(0.0), vfmsq_f32(dz, s(pc_w), s(0.85)))), s(fog_den)));
                alpha = sel(vcltq_f32(fog, s(0.02)), s(0.0), alpha);
                // Per-lane table reads.
                let (sl, kl) = (lanes_i(sv), lanes(k));
                let (mut a, mut b, mut col) = ([[0.0f32; 4]; 3], [[0.0f32; 4]; 3], [[0.0f32; 4]; 4]);
                let (mut w, mut hot, mut am) = ([1.0f32; 4], [0u32; 4], [1.0f32; 4]);
                for l in 0..4 {
                    let sx = sl[l];
                    if sx < 1505 {
                        let ta = tab(&tb_.ta, sx);
                        let tb = tab(&tb_.tb, sx);
                        let cl = tab(&tb_.sty, fi(ta[3]));
                        for q in 0..3 {
                            a[q][l] = ta[q];
                            b[q][l] = tb[q];
                        }
                        for q in 0..4 {
                            col[q][l] = cl[q];
                        }
                        w[l] = tb[3];
                        hot[l] = if sx >= 1472 { !0 } else { 0 };
                    } else {
                        let li = sx.wrapping_sub(1505);
                        let kk = fi(mn(kl[l], 31.0));
                        if li < tab(&tb_.ln, kk) {
                            let o = tab(&tb_.loff, kk).wrapping_add(li);
                            let (la, lb) = (tab(&tb_.la, o), tab(&tb_.lb, o));
                            for q in 0..3 {
                                a[q][l] = la[q];
                                b[q][l] = lb[q];
                            }
                            col[0][l] = 0.332452;
                            col[1][l] = 0.309469;
                            col[2][l] = 0.274677;
                            col[3][l] = 0.9;
                            w[l] = 1.1;
                        } else {
                            am[l] = 0.0;
                        }
                    }
                }
                alpha = vmulq_f32(alpha, ld(&am));
                let (mut ax, mut ay, mut az) = (ld(&a[0]), ld(&a[1]), ld(&a[2]));
                let (mut bx, mut byy, mut bz) = (ld(&b[0]), ld(&b[1]), ld(&b[2]));
                let focus = vceqq_f32(k, s(pa_y));
                let boost = sel(focus, s(pb_y), s(1.0));
                let mut rgb = [vmulq_f32(ld(&col[0]), boost), vmulq_f32(ld(&col[1]), boost), vmulq_f32(ld(&col[2]), boost)];
                if pb_z > 0.0 {
                    let m = vandq_u32(vld1q_u32(hot.as_ptr()), focus);
                    if any(m) {
                        let ymid = vmulq_f32(vaddq_f32(ay, byy), s(0.5));
                        let glow = sel(vcltq_f32(ymid, s(pb_w)), vmulq_f32(exp(vmulq_f32(vsubq_f32(ymid, s(pb_w)), s(0.5))), s(pb_z)), s(0.0));
                        let glow = sel(m, glow, s(0.0));
                        let t = [3.0f32, 0.2226407, 0.0181465];
                        for q in 0..3 {
                            rgb[q] = vfmaq_f32(rgb[q], vsubq_f32(s(t[q]), rgb[q]), glow);
                        }
                    }
                }
                if pd_y > 0.5 {
                    let m = vceqq_f32(k, s(13.0));
                    if any(m) {
                        let ax1 = vfmsq_f32(vmulq_f32(ax, s(cr)), ay, s(sr));
                        let ay1 = vfmaq_f32(vmulq_f32(ax, s(sr)), ay, s(cr));
                        let nax = vaddq_f32(vfmaq_f32(vmulq_f32(ax1, s(cy)), az, s(sy)), s(pe_x));
                        let nay = vaddq_f32(ay1, s(pe_y));
                        let naz = vaddq_f32(vfmaq_f32(vmulq_f32(vnegq_f32(ax1), s(sy)), az, s(cy)), s(pe_z));
                        let bx1 = vfmsq_f32(vmulq_f32(bx, s(cr)), byy, s(sr));
                        let by1 = vfmaq_f32(vmulq_f32(bx, s(sr)), byy, s(cr));
                        let nbx = vaddq_f32(vfmaq_f32(vmulq_f32(bx1, s(cy)), bz, s(sy)), s(pe_x));
                        let nby = vaddq_f32(by1, s(pe_y));
                        let nbz = vaddq_f32(vfmaq_f32(vmulq_f32(vnegq_f32(bx1), s(sy)), bz, s(cy)), s(pe_z));
                        ax = sel(m, nax, ax);
                        ay = sel(m, nay, ay);
                        az = sel(m, naz, az);
                        bx = sel(m, nbx, bx);
                        byy = sel(m, nby, byy);
                        bz = sel(m, nbz, bz);
                    }
                }
                let mut al = vmulq_f32(vmulq_f32(ld(&col[3]), fog), s(pb_x));
                let mut off = s(0.0);
                if pa_z > 1.0 {
                    off = vmulq_f32(s(pa_w), vdivq_f32(c, s(pa_z - 1.0)));
                    al = vmulq_f32(vdivq_f32(al, s(pa_z)), s(1.6));
                }
                let yo = vaddq_f32(by, off);
                let words = [ax, vaddq_f32(ay, yo), az, bx, vaddq_f32(byy, yo), bz, rgb[0], rgb[1], rgb[2], vmulq_f32(al, alpha), ld(&w)];
                // One bounds check for the four records, then plain stores.
                if 11 * (i + 3) + 10 < out.len() {
                    let o = out.as_mut_ptr().add(11 * i);
                    for (q, v) in words.iter().enumerate() {
                        vst1q_lane_f32::<0>(o.add(q), *v);
                        vst1q_lane_f32::<1>(o.add(11 + q), *v);
                        vst1q_lane_f32::<2>(o.add(22 + q), *v);
                        vst1q_lane_f32::<3>(o.add(33 + q), *v);
                    }
                } else {
                    for (q, v) in words.iter().enumerate() {
                        let x = lanes(*v);
                        for l in 0..4 {
                            st(out, 11 * (i + l) + q, x[l]);
                        }
                    }
                }
                i += 4;
            }
        }
        if n4 < count {
            let mut o2 = vec![0.0f32; 11 * count];
            super::sk_lines_rust(tb_, p, &mut o2, count);
            out[11 * n4..11 * count].copy_from_slice(&o2[11 * n4..]);
        }
    }

    // -- k_myc ---------------------------------------------------------------
    pub fn k_myc(myc: &[f32], p: &HashMap<&str, f32>, time: f32, bufs: [&mut super::Emit; 4], count: usize) {
        let g = |n: &str| p[n];
        let c_p = [g("c_px"), g("c_py"), g("c_pz")];
        let c_r = [g("c_rx"), g("c_ry"), g("c_rz")];
        let c_u = [g("c_ux"), g("c_uy"), g("c_uz")];
        let c_f = [g("c_fx"), g("c_fy"), g("c_fz")];
        let (c_ff, c_wa, c_wt, b_th, b_sd, m_front) = (g("c_ff"), g("c_wa"), g("c_wt"), g("b_th"), g("b_sd"), g("m_front"));
        let [ib, if_, gb, gf] = bufs;
        // Hoisted by hand: the front plane, the warp's time terms.
        let nrm = [0.0 * b_sd, (0.0 - super::sin(b_th)) * b_sd, super::cos(b_th) * b_sd];
        let fo = [0.0f32, 3.0, (0.0f64 - 7.55) as f32];
        let t_land = (28.873f64 - 0.02) as f32;
        let tt = time + 26.146;
        let (t1, t2, t3, t4, t5, t6) = (c_wt * 3.1, c_wt * 1.7, c_wt * 4.3, c_wt * 2.6, c_wt * 1.3, c_wt * 3.7);
        let n4 = count & !3;
        unsafe {
            let dot = |x: [F; 3], c: [f32; 3]| vfmaq_f32(vfmaq_f32(vmulq_f32(x[0], s(c[0])), x[1], s(c[1])), x[2], s(c[2]));
            let rel = |x: [F; 3]| [vsubq_f32(x[0], s(c_p[0])), vsubq_f32(x[1], s(c_p[1])), vsubq_f32(x[2], s(c_p[2]))];
            let warp = |x: F, y: F| -> [F; 2] {
                if c_wa <= 0.0 {
                    return [x, y];
                }
                let dx = vfmaq_f32(sin(vfmaq_f32(vfmaq_f32(s(t1), y, s(0.0105)), sin(vfmsq_f32(vmulq_f32(x, s(0.006)), s(t2), s(1.0))), s(1.6))), s(0.5), sin(vfmsq_f32(vmulq_f32(vaddq_f32(x, y), s(0.019)), s(t3), s(1.0))));
                let dy = vfmaq_f32(sin(vfmaq_f32(vfmsq_f32(vmulq_f32(x, s(0.0093)), s(t4), s(1.0)), sin(vfmaq_f32(s(t5), y, s(0.008))), s(1.4))), s(0.5), sin(vfmaq_f32(s(t6), vsubq_f32(x, y), s(0.017))));
                [vfmaq_f32(x, dx, s(c_wa)), vfmaq_f32(y, dy, s(c_wa))]
            };
            let mut i = 0;
            let mut overflow = false;
            while i < n4 {
                let mut cnt = [[0usize; 4]; 4];
                let mut bd = [0.0f32; 4];
                let mut sa = [[0.0f32; 4]; 3];
                let mut sb = [[0.0f32; 4]; 3];
                for l in 0..4 {
                    let o = (i + l) as i32 as f32 * 7.0;
                    bd[l] = tab(myc, fi(o + 6.0));
                    for q in 0..3 {
                        sa[q][l] = tab(myc, fi(o + q as f32));
                        sb[q][l] = tab(myc, fi(o + 3.0 + q as f32));
                    }
                }
                let bdv = ld(&bd);
                let active = vandq_u32(if tt >= t_land { vdupq_n_u32(!0) } else { vdupq_n_u32(0) }, vcleq_f32(bdv, s(m_front)));
                if any(active) {
                    let tip = exp(vdivq_f32(vsubq_f32(s(0.0), vsubq_f32(s(m_front), bdv)), s(0.3)));
                    let mt = vfmaq_f32(s(0.15), tip, s(0.85));
                    let (c1, c2) = ([0.5300954f32, 0.5052049, 0.4575045], [0.652351f32, 0.95, 0.04292689]);
                    let col = [vfmaq_f32(s(c1[0]), s(c2[0] - c1[0]), mt), vfmaq_f32(s(c1[1]), s(c2[1] - c1[1]), mt), vfmaq_f32(s(c1[2]), s(c2[2] - c1[2]), mt)];
                    let gk = vmulq_f32(tip, s(0.7));
                    let gcol = [vmulq_f32(gk, s(0.6866853)), gk, vmulq_f32(gk, s(0.0451862))];
                    let sav = [ld(&sa[0]), ld(&sa[1]), ld(&sa[2])];
                    let sbv = [ld(&sb[0]), ld(&sb[1]), ld(&sb[2])];
                    let front = lanes_m(vcgtq_f32(dot([vsubq_f32(sav[0], s(fo[0])), vsubq_f32(sav[1], s(fo[1])), vsubq_f32(sav[2], s(fo[2]))], nrm), s(0.0)));
                    let glow_m = vandq_u32(active, vcgtq_f32(tip, s(0.25)));
                    for pass in 0..2 {
                        let pm = if pass == 0 { active } else { glow_m };
                        if !any(pm) {
                            continue;
                        }
                        let (mut ca, mut cb) = (sav, sbv);
                        let da = dot(rel(ca), c_f);
                        let db = dot(rel(cb), c_f);
                        let vis = vorrq_u32(vcgeq_f32(da, s(0.1)), vcgeq_f32(db, s(0.1)));
                        let ma = vcltq_f32(da, s(0.1));
                        let mb = vbicq_u32(vcltq_f32(db, s(0.1)), ma);
                        let ua = vdivq_f32(vsubq_f32(s(0.1), da), vsubq_f32(db, da));
                        let ub = vdivq_f32(vsubq_f32(s(0.1), db), vsubq_f32(da, db));
                        for q in 0..3 {
                            let na = vfmaq_f32(ca[q], vsubq_f32(cb[q], ca[q]), ua);
                            let nb = vfmaq_f32(cb[q], vsubq_f32(ca[q], cb[q]), ub);
                            ca[q] = sel(ma, na, ca[q]);
                            cb[q] = sel(mb, nb, cb[q]);
                        }
                        let proj = |x: [F; 3]| {
                            let r = rel(x);
                            let z = dot(r, c_f);
                            [vaddq_f32(s(960.0), vdivq_f32(vmulq_f32(s(c_ff), dot(r, c_r)), z)), vsubq_f32(s(540.0), vdivq_f32(vmulq_f32(s(c_ff), dot(r, c_u)), z))]
                        };
                        let pa = proj(ca);
                        let pb = proj(cb);
                        let both = |m1: M, m2: M| vandq_u32(m1, m2);
                        let cull = vorrq_u32(
                            vorrq_u32(both(vcltq_f32(pa[0], s(-250.0)), vcltq_f32(pb[0], s(-250.0))), both(vcgtq_f32(pa[0], s(2170.0)), vcgtq_f32(pb[0], s(2170.0)))),
                            vorrq_u32(both(vcltq_f32(pa[1], s(-250.0)), vcltq_f32(pb[1], s(-250.0))), both(vcgtq_f32(pa[1], s(1330.0)), vcgtq_f32(pb[1], s(1330.0)))),
                        );
                        let live = vbicq_u32(vandq_u32(pm, vis), cull);
                        if !any(live) {
                            continue;
                        }
                        let wa = warp(pa[0], pa[1]);
                        let wb = warp(pb[0], pb[1]);
                        let (ww, c) = if pass == 0 { (0.9f64, col) } else { (1.8f64, gcol) };
                        let aa = (0.8 * (ww / 0.7).min(1.0)) as f32;
                        let (wax, way, wbx, wby) = (lanes(wa[0]), lanes(wa[1]), lanes(wb[0]), lanes(wb[1]));
                        let (cr_, cg, cbl) = (lanes(c[0]), lanes(c[1]), lanes(c[2]));
                        let lv = lanes_m(live);
                        for l in 0..4 {
                            if lv[l] == 0 {
                                continue;
                            }
                            let rec = [wax[l], 0.0 - way[l], 0.0, wbx[l], 0.0 - wby[l], 0.0, cr_[l], cg[l], cbl[l], aa, ww as f32];
                            let (e, which): (&mut super::Emit, usize) = match (pass, front[l] != 0) {
                                (0, true) => (&mut *if_, 1),
                                (0, false) => (&mut *ib, 0),
                                (_, true) => (&mut *gf, 3),
                                (_, false) => (&mut *gb, 2),
                            };
                            super::emit(e, &mut cnt[which][l], i + l, &rec, &mut overflow);
                        }
                    }
                }
                for l in 0..4 {
                    let n = ib.cnt.len() - 1;
                    let at = (i + l).min(n);
                    ib.cnt[at] = f32::from_bits(cnt[0][l] as u32);
                    if_.cnt[at] = f32::from_bits(cnt[1][l] as u32);
                    gb.cnt[at] = f32::from_bits(cnt[2][l] as u32);
                    gf.cnt[at] = f32::from_bits(cnt[3][l] as u32);
                }
                i += 4;
            }
            let _ = (mx(0.0, 0.0), overflow);
        }
        if n4 < count {
            // The tail through the scalar twin (it writes only its own elements).
            super::k_myc_rust(myc, p, time, [ib, if_, gb, gf], n4, count);
        }
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn best(reps: usize, mut f: impl FnMut()) -> f64 {
    (0..reps)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min)
}

/// Every buffer a kernel writes, sized for `count` elements.
fn outputs(k: &Kernel, count: usize) -> Vec<(String, Vec<f32>)> {
    k.buffers()
        .iter()
        .skip(1)
        .filter_map(|b| {
            let words = match b.access {
                Access::Read => return None,
                Access::Write => b.stride as usize * count,
                Access::Emit { width, capacity } => (width * capacity) as usize * count,
                Access::EmitCount => count,
            };
            Some((b.name.clone(), vec![0.0f32; words.max(1)]))
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum How {
    Interp,
    Scalar,
    Neon,
}

fn run_kernel(k: &Kernel, how: How, count: usize, params: &[(&str, f32)], time: f32, inputs: &[(&str, &[f32])], outs: &mut [(String, Vec<f32>)]) {
    let mut call = k.call();
    for (n, v) in params {
        call.set_param(n, *v);
    }
    call.set_time(time);
    for (n, d) in inputs {
        call.input(n, d).unwrap();
    }
    for (n, d) in outs.iter_mut() {
        call.output(n, d).unwrap();
    }
    call.set_simd(how == How::Neon);
    if how == How::Interp {
        call.run_interp(count).unwrap();
    } else {
        call.run(count).unwrap();
    }
}

/// Words that differ, and the largest difference relative to the
/// buffer's largest magnitude (fused multiply-adds round once where the
/// twin rounds twice).
fn ulps(a: &[f32], b: &[f32]) -> (usize, f64) {
    let scale = b.iter().filter(|x| x.is_finite()).fold(0.0f64, |m, x| m.max(x.abs() as f64)).max(1e-30);
    let mut diff = 0;
    let mut worst = 0.0f64;
    for (x, y) in a.iter().zip(b) {
        if x.to_bits() != y.to_bits() {
            diff += 1;
            if !(x.is_nan() && y.is_nan()) {
                worst = worst.max((*x as f64 - *y as f64).abs() / scale);
            }
        }
    }
    (diff, worst)
}

fn asm(dir: &str, name: &str, words: &[u32]) {
    if words.is_empty() {
        return;
    }
    let mut s = String::from(".text\n.globl _k\n_k:\n");
    for w in words {
        s += &format!("  .inst 0x{:08x}\n", w);
    }
    std::fs::write(format!("{}/{}.s", dir, name), s).unwrap();
}

struct Row {
    name: &'static str,
    count: usize,
    ns: [Option<f64>; 5],
    check: String,
}

type Twin<'a> = Option<&'a mut dyn FnMut() -> Vec<(String, Vec<f32>)>>;

#[allow(clippy::too_many_arguments)]
fn bench<'a>(name: &'static str, k: &Arc<Kernel>, count: usize, params: &[(&str, f32)], time: f32, inputs: &[(&str, &[f32])], rust: Twin<'a>, fast: Twin<'a>, reps: usize) -> Row {
    if let Ok(dir) = std::env::var("KC1_ASM") {
        let (s, v) = k.code_words();
        asm(&dir, &format!("{}_scalar", name), s);
        asm(&dir, &format!("{}_neon", name), v);
        std::fs::write(format!("{}/{}.air", dir, name), format!("{:#?}", k.program())).unwrap();
    }
    let mut ns = [None; 5];
    let mut results: Vec<Vec<(String, Vec<f32>)>> = Vec::new();
    for (slot, how) in [(0, How::Interp), (1, How::Scalar), (2, How::Neon)] {
        if how == How::Neon && !k.simd() {
            continue;
        }
        let n = if how == How::Interp { count.min(20_000) } else { count };
        let mut outs = outputs(k, count);
        let r = if how == How::Interp { 1 } else { reps };
        let t = best(r, || run_kernel(k, how, n, params, time, inputs, &mut outs));
        ns[slot] = Some(t * 1e9 / n as f64);
        if how != How::Interp {
            results.push(outs);
        }
    }
    let mut check = String::new();
    if results.len() == 2 {
        let same = results[0].iter().zip(&results[1]).all(|(a, b)| a.1.iter().zip(&b.1).all(|(x, y)| x.to_bits() == y.to_bits()));
        check += if same { "neon=scalar " } else { "NEON DIFFERS " };
    }
    for (slot, twin) in [(3usize, rust), (4, fast)] {
        let Some(f) = twin else { continue };
        let mut got = Vec::new();
        let t = best(reps, || got = f());
        ns[slot] = Some(t * 1e9 / count as f64);
        if slot == 4 {
            check += "| fast: ";
        }
        for (n, want) in &got {
            let ours = &results[0].iter().find(|(m, _)| m == n).unwrap().1;
            let (d, w) = ulps(ours, want);
            if d == 0 {
                check += &format!("{}: bit-equal ", n);
            } else {
                check += &format!("{}: {}/{} words differ (max {:.1e} of range) ", n, d, want.len(), w);
            }
        }
    }
    Row { name, count, ns, check }
}

fn main() {
    let dir = std::env::var("KC1_KERNELS").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../local/agent_state/edits/scratch/KC1/kernels").to_string());
    let reps: usize = std::env::var("KC1_REPS").ok().and_then(|x| x.parse().ok()).unwrap_or(7);
    let only = std::env::var("KC1_ONLY").ok();
    let want = |n: &str| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == n));
    let layouts = [segment()];
    let compile = |text: &str| {
        let t0 = Instant::now();
        let k = compile_with(text, &layouts, Backend::Native).unwrap_or_else(|e| panic!("{:?}", &e[..e.len().min(3)]));
        (k, t0.elapsed().as_secs_f64() * 1e3)
    };
    let mut rows = Vec::new();

    // -- sk_lines ------------------------------------------------------------
    if want("sk_lines") {
        let text = std::fs::read_to_string(format!("{}/sk_lines.splash", dir)).expect("sk_lines.splash");
        let (k, ms) = compile(&text);
        println!("sk_lines: compiled in {:.0} ms; simd {}; {} vals", ms, k.simd(), k.program().vals.len());
        let tables = SkTables {
            sty: v4(table(&text, "SK_STY")),
            ta: v4(table(&text, "SK_TA")),
            tb: v4(table(&text, "SK_TB")),
            la: v3(table(&text, "SK_LA")),
            lb: v3(table(&text, "SK_LB")),
            loff: table(&text, "SK_LOFF").iter().map(|x| *x as i32).collect(),
            ln: table(&text, "SK_LN").iter().map(|x| *x as i32).collect(),
        };
        // A frame mid-shot: blocks 0..14 from block 3, focus on 8, three
        // smear copies, the disobeying block rolling.
        let params: Vec<(&str, f32)> = vec![
            ("pa_x", 3.0), ("pa_y", 8.0), ("pa_z", 3.0), ("pa_w", 0.4),
            ("pb_x", 1.0), ("pb_y", 1.6), ("pb_z", 0.8), ("pb_w", -40.0),
            ("pc_x", 0.5), ("pc_y", -60.0), ("pc_z", 45.0), ("pc_w", 50.0),
            ("pd_x", 1.2), ("pd_y", 1.0), ("pd_z", 0.3), ("pd_w", 0.2),
            ("pe_x", 0.5), ("pe_y", 0.2), ("pe_z", -0.3), ("pe_w", 0.0),
        ];
        let count = 3 * 28476;
        let pm: HashMap<&str, f32> = params.iter().copied().collect();
        let mut rust = || {
            let mut out = vec![0.0f32; count * 11];
            sk_lines_rust(&tables, &pm, &mut out, count);
            vec![("out".to_string(), out)]
        };
        let mut fastf = || {
            let mut out = vec![0.0f32; count * 11];
            fast::sk_lines(&tables, &pm, &mut out, count);
            vec![("out".to_string(), out)]
        };
        rows.push(bench("sk_lines", &k, count, &params, 0.0, &[], Some(&mut rust), Some(&mut fastf), reps));
    }

    // -- k_myc ---------------------------------------------------------------
    if want("k_myc") {
        let text = std::fs::read_to_string(format!("{}/k_myc.splash", dir)).expect("k_myc.splash");
        let (k, ms) = compile(&text);
        println!("k_myc: compiled in {:.0} ms; simd {}; {} vals", ms, k.simd(), k.program().vals.len());
        let myc: Vec<f32> = table(&text, "R_MYC").iter().map(|x| *x as f32).collect();
        // A camera 12 units back looking down -z at the room, warp on,
        // the growth front past every strand.
        let params: Vec<(&str, f32)> = vec![
            ("c_px", 0.0), ("c_py", 1.5), ("c_pz", 6.0),
            ("c_rx", 1.0), ("c_ry", 0.0), ("c_rz", 0.0),
            ("c_ux", 0.0), ("c_uy", 1.0), ("c_uz", 0.0),
            ("c_fx", 0.0), ("c_fy", 0.0), ("c_fz", -1.0),
            ("c_ff", 1100.0), ("c_wa", std::env::var("KC1_WA").ok().and_then(|x| x.parse().ok()).unwrap_or(3.0)), ("c_wt", 31.5), ("c_lk", 0.0),
            ("b_th", 0.3), ("b_sd", 1.0), ("c_am", 1.0), ("m_front", 40.0),
        ];
        let count = 20007;
        let time = 5.0;
        let pm: HashMap<&str, f32> = params.iter().copied().collect();
        let rust = |use_fast: bool| {
            let mut bufs: Vec<(Vec<f32>, Vec<f32>)> = (0..4).map(|_| (vec![0.0f32; count * 11], vec![0.0f32; count])).collect();
            {
                let [a, b, c, d] = &mut bufs[..] else { unreachable!() };
                let mut e = [
                    Emit { data: &mut a.0, cnt: &mut a.1, cap: 1 },
                    Emit { data: &mut b.0, cnt: &mut b.1, cap: 1 },
                    Emit { data: &mut c.0, cnt: &mut c.1, cap: 1 },
                    Emit { data: &mut d.0, cnt: &mut d.1, cap: 1 },
                ];
                let [e0, e1, e2, e3] = &mut e;
                if use_fast {
                    fast::k_myc(&myc, &pm, time, [e0, e1, e2, e3], count);
                } else {
                    k_myc_rust(&myc, &pm, time, [e0, e1, e2, e3], 0, count);
                }
            }
            let names = ["ib", "if_", "gb", "gf"];
            let mut out = Vec::new();
            for (n, (d, c)) in names.iter().zip(bufs) {
                out.push((n.to_string(), d));
                out.push((format!("{}_count", n), c));
            }
            out
        };
        let mut rust_s = || rust(false);
        let mut rust_f = || rust(true);
        rows.push(bench("k_myc", &k, count, &params, time, &[], Some(&mut rust_s), Some(&mut rust_f), reps));
    }

    // -- k_room (ours only) ------------------------------------------------------
    if want("k_room") {
        if let Ok(text) = std::fs::read_to_string(format!("{}/k_room.splash", dir)) {
            let (k, ms) = compile(&text);
            println!("k_room: compiled in {:.0} ms; simd {}; {} vals", ms, k.simd(), k.program().vals.len());
            let mut params: Vec<(String, f32)> = Vec::new();
            for q in 0..8 {
                for (f, v) in [("px", 0.0), ("py", 1.5), ("pz", 6.0), ("rx", 1.0), ("ry", 0.0), ("rz", 0.0), ("ux", 0.0), ("uy", 1.0), ("uz", 0.0), ("ff", 1100.0), ("wa", 3.0), ("wt", 31.5), ("lk", 0.0), ("am", 1.0)] {
                    params.push((format!("q{}_{}", q, f), v));
                }
            }
            params.push(("b_th".into(), 0.3));
            params.push(("b_sd".into(), 1.0));
            let p: Vec<(&str, f32)> = params.iter().map(|(n, v)| (n.as_str(), *v)).collect();
            rows.push(bench("k_room", &k, 8162, &p, 5.0, &[], None, None, reps));
        }
    }

    // -- fbm heights ---------------------------------------------------------------
    if want("fbm") {
        let (k, ms) = compile(HEIGHTS);
        println!("fbm: compiled in {:.0} ms; simd {}", ms, k.simd());
        let n = 1024 * 256;
        let base: Vec<f32> = (0..n).map(|i| ((i % 97) as f32) * 0.01).collect();
        let mut rust = || {
            let mut pos = vec![0.0f32; n * 3];
            let mut hgt = vec![0.0f32; n];
            heights_rust(&base, 6.0, &mut pos, &mut hgt, n);
            vec![("pos".to_string(), pos), ("hgt".to_string(), hgt)]
        };
        let mut fastf = || {
            let mut pos = vec![0.0f32; n * 3];
            let mut hgt = vec![0.0f32; n];
            fast::heights(&base, 6.0, &mut pos, &mut hgt, n);
            vec![("pos".to_string(), pos), ("hgt".to_string(), hgt)]
        };
        rows.push(bench("fbm", &k, n, &[], 0.0, &[("base", &base)], Some(&mut rust), Some(&mut fastf), reps));
    }

    println!();
    println!("{:<9} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8}   check", "kernel", "count", "interp", "scalar", "neon x4", "rustc", "rs fast", "vs rs", "vs fast");
    for r in &rows {
        let f = |x: Option<f64>| x.map_or("-".to_string(), |x| format!("{:.2}", x));
        let ours = r.ns[2].or(r.ns[1]);
        let gap = |t: Option<f64>| match (ours, t) {
            (Some(a), Some(b)) => format!("{:.2}x", a / b),
            _ => "-".into(),
        };
        println!("{:<9} {:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8}   {}", r.name, r.count, f(r.ns[0]), f(r.ns[1]), f(r.ns[2]), f(r.ns[3]), f(r.ns[4]), gap(r.ns[3]), gap(r.ns[4]), r.check);
    }
    println!("(ns per element, one thread, best of {})", reps);
}
