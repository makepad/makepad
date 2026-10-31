//! The CPU twin of the bake shader: the same program interpreted with the same
//! f32 operations in the same order. With the bake pipeline's `precise_math`
//! (no contraction, correctly rounded division and sqrt) the two agree on
//! every bit, so the twin answers distance queries (physics, picking checks,
//! allocation) exactly as the GPU bricks hold them.
//!
//! The fixed forms both sides write out: `len3(x, y, z) = sqrt((x*x + y*y) +
//! z*z)`, `dot` left to right, `clamp(x, lo, hi) = min(max(x, lo), hi)`, and
//! sin/cos as the polynomials of [`psin_pcos`] (built from + and * only).

use crate::program::{op, prim};

/// sin and cos of `x`: a quadrant reduction by pi/2 in two parts, then
/// polynomials on [-pi/4, pi/4] (error about 1e-7). Only `floor`, `+` and
/// `*`, so every backend computes the same bits.
pub fn psin_pcos(x: f32) -> (f32, f32) {
    // shader literals are parsed to f64 and then rounded to f32: the same here
    let k = |v: f64| v as f32;
    let q = (x * k(0.6366198) + 0.5).floor();
    let r = (x - q * k(1.5707964)) - q * (0.0 - k(0.00000004371139));
    let r2 = r * r;
    let s = r + r * r2 * ((0.0 - k(0.16666667)) + r2 * (k(0.008333331) + r2 * ((0.0 - k(0.00019840874)) + r2 * k(0.0000027525562))));
    let c = 1.0 + r2 * ((0.0 - 0.5) + r2 * (k(0.041666638) + r2 * ((0.0 - k(0.0013888378)) + r2 * k(0.000024760495))));
    let n = q - (q * 0.25).floor() * 4.0;
    if n < 0.5 {
        (s, c)
    } else if n < 1.5 {
        (c, -s)
    } else if n < 2.5 {
        (-s, -c)
    } else {
        (-c, s)
    }
}

fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}

fn len2(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

fn len3(x: f32, y: f32, z: f32) -> f32 {
    ((x * x + y * y) + z * z).sqrt()
}

/// csg_sdf's smooth minimum (`k <= 0`: the hard minimum).
fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = clamp(0.5 + (0.5 * (b - a)) / k, 0.0, 1.0);
    (b + (a - b) * h) - (k * h) * (1.0 - h)
}

/// One primitive at local point `p`; `t` holds its 3 texels.
fn prim(t: &[[f32; 4]], p: [f32; 3]) -> f32 {
    let [_, kind, a, b] = t[0];
    let [c, d, e, ff] = t[1];
    let [g, ..] = t[2];
    let [px, py, pz] = p;
    if kind == prim::SPHERE {
        len3(px, py, pz) - a
    } else if kind == prim::BOX {
        let qx = px.abs() - a;
        let qy = py.abs() - b;
        let qz = pz.abs() - c;
        len3(qx.max(0.0), qy.max(0.0), qz.max(0.0)) + qx.max(qy.max(qz)).min(0.0) - d
    } else if kind == prim::CYLINDER {
        let qx = len2(px, pz);
        let dx = qx - a;
        let dy = py.abs() - b;
        let mx = dx.max(0.0);
        let my = dy.max(0.0);
        (mx * mx + my * my).sqrt() + dx.max(dy).min(0.0)
    } else if kind == prim::CONE {
        let (r1, r2, hh) = (a, b, c);
        let qx = len2(px, pz);
        let qy = py;
        let k2x = r2 - r1;
        let k2y = hh * 2.0;
        let cax = qx - qx.min(if qy < 0.0 { r1 } else { r2 });
        let cay = qy.abs() - hh;
        let t = clamp(((r2 - qx) * k2x + (hh - qy) * k2y) / (k2x * k2x + k2y * k2y), 0.0, 1.0);
        let cbx = (qx - r2) + k2x * t;
        let cby = (qy - hh) + k2y * t;
        let s = if cbx < 0.0 && cay < 0.0 { -1.0 } else { 1.0 };
        s * (cax * cax + cay * cay).min(cbx * cbx + cby * cby).sqrt()
    } else if kind == prim::TORUS {
        let q = len2(px, pz) - a;
        len2(q, py) - b
    } else if kind == prim::CAPSULE {
        let (pax, pay, paz) = (px - a, py - b, pz - c);
        let (bx, by, bz) = (d, e, ff);
        let h = clamp(((pax * bx + pay * by) + paz * bz) / ((bx * bx + by * by) + bz * bz), 0.0, 1.0);
        len3(pax - bx * h, pay - by * h, paz - bz * h) - g
    } else if kind == prim::ELLIPSOID {
        let k0 = len3(px / a, py / b, pz / c);
        let k1 = len3(px / (a * a), py / (b * b), pz / (c * c));
        (k0 * (k0 - 1.0)) / k1.max(0.000000000001f64 as f32)
    } else {
        // GYROID
        let (sx, cx) = psin_pcos(px * a);
        let (sy, cy) = psin_pcos(py * a);
        let (sz, cz) = psin_pcos(pz * a);
        let gv = (sx * cy + sy * cz) + sz * cx;
        gv.abs() / a - b
    }
}

/// The distance and material slot of `program` at `p` (the object frame).
pub fn eval(program: &[[f32; 4]], p: [f32; 3]) -> (f32, u8) {
    eval_with(program, p, &[])
}

/// [`eval`] of a program that calls kernels (`kernels`: its volume's table).
pub fn eval_with(program: &[[f32; 4]], p: [f32; 3], kernels: &[crate::kernel::KernelSlot]) -> (f32, u8) {
    let mut st = St::new(p);
    let len = program[0][0] as usize;
    let mut pc = 1;
    while pc < len {
        let h = program[pc];
        if h[0] == op::KERNEL {
            let (params, n) = kernel_params(program, pc);
            let d = kernels.get(h[1] as usize).map_or(f32::MAX, |k| k.distance(&params[..n], st.q));
            st.push(d);
            pc += 1 + h[2] as usize;
        } else {
            pc = st.step(program, pc);
        }
    }
    (st.vs[0], st.ss[0])
}

/// [`eval_with`] at many points: the program is straight-line (no op
/// depends on a value), so the points step through it together and each
/// kernel op is one batched kernel call. The same bits as one at a time.
pub fn eval_many(program: &[[f32; 4]], ps: &[[f32; 3]], kernels: &[crate::kernel::KernelSlot]) -> Vec<(f32, u8)> {
    let mut sts: Vec<St> = ps.iter().map(|p| St::new(*p)).collect();
    let len = program[0][0] as usize;
    let mut pc = 1;
    let mut qs = Vec::with_capacity(ps.len());
    while pc < len {
        let h = program[pc];
        if h[0] == op::KERNEL {
            let (params, n) = kernel_params(program, pc);
            qs.clear();
            qs.extend(sts.iter().map(|s| s.q));
            match kernels.get(h[1] as usize) {
                Some(k) => {
                    let d = k.distance_many(&params[..n], &qs);
                    for (s, d) in sts.iter_mut().zip(d) {
                        s.push(d);
                    }
                }
                None => sts.iter_mut().for_each(|s| s.push(f32::MAX)),
            }
            pc += 1 + h[2] as usize;
        } else {
            let mut next = pc;
            for s in &mut sts {
                next = s.step(program, pc);
            }
            pc = next;
        }
    }
    sts.iter().map(|s| (s.vs[0], s.ss[0])).collect()
}

/// A KERNEL op's parameters (at most 16) and their count.
fn kernel_params(program: &[[f32; 4]], pc: usize) -> ([f32; 16], usize) {
    let n = (program[pc][3] as usize).min(16);
    let mut params = [0.0f32; 16];
    for k in 0..n {
        params[k] = program[pc + 1 + k / 4][k % 4];
    }
    (params, n)
}

/// One point's interpreter state.
struct St {
    vs: [f32; 64],
    ss: [u8; 64],
    ps: [[f32; 3]; 32],
    ms: [u8; 32],
    vn: usize,
    pn: usize,
    mn: usize,
    q: [f32; 3],
}

impl St {
    fn new(p: [f32; 3]) -> St {
        St { vs: [0.0; 64], ss: [0; 64], ps: [[0.0; 3]; 32], ms: [0; 32], vn: 0, pn: 0, mn: 0, q: p }
    }

    /// Pushes a leaf's distance with the slot in force.
    fn push(&mut self, d: f32) {
        self.vs[self.vn] = d;
        self.ss[self.vn] = if self.mn > 0 { self.ms[self.mn - 1] } else { 0 };
        self.vn += 1;
    }

    /// Runs the (non-kernel) op at `pc`; the next op's index.
    fn step(&mut self, program: &[[f32; 4]], pc: usize) -> usize {
        let St { vs, ss, ps, ms, vn, pn, mn, q } = self;
        let h = program[pc];
        let code = h[0];
        if code == op::PRIM {
            vs[*vn] = prim(&program[pc..pc + 3], *q);
            ss[*vn] = if *mn > 0 { ms[*mn - 1] } else { 0 };
            *vn += 1;
            pc + 3
        } else if code == op::XFORM_PUSH {
            ps[*pn] = *q;
            *pn += 1;
            let r = &program[pc + 1..pc + 4];
            let v = *q;
            *q = std::array::from_fn(|i| ((r[i][0] * v[0] + r[i][1] * v[1]) + r[i][2] * v[2]) + r[i][3]);
            pc + 4
        } else if code == op::XFORM_POP {
            *pn -= 1;
            *q = ps[*pn];
            vs[*vn - 1] *= h[1];
            pc + 1
        } else if code == op::DEFORM_PUSH {
            ps[*pn] = *q;
            *pn += 1;
            let (kind, rate, axis) = (h[1], h[2], h[3]);
            let [x, y, z] = *q;
            *q = if kind == 0.0 {
                let (s, c) = psin_pcos(rate * y);
                [c * x + s * z, y, c * z - s * x]
            } else if kind == 1.0 {
                let (s, c) = psin_pcos(rate * x);
                [c * x + s * y, c * y - s * x, z]
            } else if kind == 2.0 {
                let k = 1.0 / (1.0 + rate * y).max(0.001f64 as f32);
                [x * k, y, z * k]
            } else if axis == 0.0 {
                [x.abs(), y, z]
            } else if axis == 1.0 {
                [x, y.abs(), z]
            } else {
                [x, y, z.abs()]
            };
            pc + 1
        } else if code == op::DEFORM_POP {
            *pn -= 1;
            *q = ps[*pn];
            vs[*vn - 1] /= h[1];
            pc + 1
        } else if code == op::FOLD {
            let (e, se) = (vs[*vn - 1], ss[*vn - 1]);
            let (d, sd) = (vs[*vn - 2], ss[*vn - 2]);
            *vn -= 1;
            let k = h[2];
            let (v, s) = if h[1] == 0.0 {
                (smin(d, e, k), if e < d { se } else { sd })
            } else if h[1] == 1.0 {
                (-smin(-d, e, k), sd)
            } else {
                (-smin(-d, -e, k), if e > d { se } else { sd })
            };
            vs[*vn - 1] = v;
            ss[*vn - 1] = s;
            pc + 1
        } else if code == op::SHELL {
            vs[*vn - 1] = vs[*vn - 1].abs() - h[1];
            pc + 1
        } else if code == op::ROUND {
            vs[*vn - 1] -= h[1];
            pc + 1
        } else if code == op::MAT_PUSH {
            ms[*mn] = h[1] as u8;
            *mn += 1;
            pc + 1
        } else if code == op::MAT_POP {
            *mn -= 1;
            pc + 1
        } else if code == op::SCALE {
            vs[*vn - 1] *= h[1];
            pc + 1
        } else {
            // MAT_WITHIN: the region's distance is on top
            *vn -= 1;
            ms[*mn] = if vs[*vn] > 0.0 { if *mn > 0 { ms[*mn - 1] } else { 0 } } else { h[1] as u8 };
            *mn += 1;
            pc + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::program::lower;
    use makepad_csg_math::Vec3d;
    use makepad_csg_sdf::{distance_slot, SdfNode};

    #[test]
    fn a_material_within_a_region_follows_csg_sdf() {
        let body = SdfNode::Material { slot: 2, within: None, of: Box::new(SdfNode::Sphere { radius: 5.0 }) };
        let region = SdfNode::Transform { at: [5.0, 0.0, 0.0], rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: 2.0 }) };
        let n = SdfNode::Material { slot: 4, within: Some(Box::new(region)), of: Box::new(body) };
        let p = lower(&n).unwrap();
        for x in [-5.0f32, -1.0, 2.0, 4.5, 5.0, 6.5] {
            let (d, s) = super::eval(&p.texels, [x, 0.0, 0.0]);
            let (d64, s64) = distance_slot(&n, Vec3d::new(x as f64, 0.0, 0.0));
            assert_eq!(s, s64, "slot at x {x}");
            assert!((d as f64 - d64).abs() < 1e-5, "distance at x {x}");
        }
    }
}
