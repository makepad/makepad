//! The typed SDF shape: a tree of analytic distance nodes and compiled Splash
//! distance kernels, with a canonical binary encoding (its key) and an evaluator
//! that needs no script VM for the analytic nodes.
//!
//! Analytic nodes match the Splash SDF library of the kernel prelude
//! (`sdf_sphere`, `smooth_union`, `sdf_shell` ...): same formulas, same
//! conventions (shapes centred on the origin, Y up, full sizes). A
//! [`SdfKernel`] is a Splash `fn(p)` compiled by the kernel compiler: `key` is
//! its program identity and `field` the compiled field.

use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use std::sync::Arc;

pub type P3 = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdfOp {
    Union,
    Difference,
    Intersection,
}

/// A compiled Splash distance kernel.
#[derive(Clone)]
pub struct SdfKernel {
    /// The program's identity (its whole IR, constants included).
    pub key: u64,
    pub field: Arc<dyn Sdf3 + Send + Sync>,
}

impl std::fmt::Debug for SdfKernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SdfKernel({:016x})", self.key)
    }
}

/// One node of a shape.
#[derive(Clone, Debug)]
pub enum SdfNode {
    Sphere { radius: f64 },
    /// `radius` rounds every edge inside `size`.
    Box { size: P3, radius: f64 },
    /// Along Y, `height` high: radius at the bottom, `top_radius` at the top.
    Cylinder { radius: f64, top_radius: f64, height: f64 },
    /// In the XZ plane.
    Torus { radius: f64, tube: f64 },
    Capsule { a: P3, b: P3, radius: f64 },
    Ellipsoid { radii: P3 },
    /// A gyroid sheet of period and wall.
    Gyroid { period: f64, wall: f64 },
    /// The first of `of` combined with each next one in order; `blend` 0 is sharp.
    Combine { op: SdfOp, blend: f64, of: Vec<SdfNode> },
    /// A skin `thickness` thick each side of the surface.
    Shell { thickness: f64, of: Box<SdfNode> },
    /// The surface grown outward by `radius`.
    Round { radius: f64, of: Box<SdfNode> },
    /// `of` scaled, then turned about X, Y, Z (degrees), then moved by `at`.
    Transform { at: P3, rotate: P3, scale: f64, of: Box<SdfNode> },
    Kernel(SdfKernel),
}

/// A shape and a box it fits in.
#[derive(Clone, Debug)]
pub struct SdfShape {
    pub root: SdfNode,
    pub min: P3,
    pub max: P3,
}

// ---- encoding

const MAGIC: &[u8; 4] = b"SDF1";

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f(&mut self, v: f64) {
        self.u64(if v == 0.0 { 0 } else { v.to_bits() });
    }
    fn p3(&mut self, p: P3) {
        p.iter().for_each(|x| self.f(*x));
    }
    fn node(&mut self, n: &SdfNode) {
        match n {
            SdfNode::Sphere { radius } => {
                self.u8(0);
                self.f(*radius);
            }
            SdfNode::Box { size, radius } => {
                self.u8(1);
                self.p3(*size);
                self.f(*radius);
            }
            SdfNode::Cylinder { radius, top_radius, height } => {
                self.u8(2);
                self.f(*radius);
                self.f(*top_radius);
                self.f(*height);
            }
            SdfNode::Torus { radius, tube } => {
                self.u8(3);
                self.f(*radius);
                self.f(*tube);
            }
            SdfNode::Capsule { a, b, radius } => {
                self.u8(4);
                self.p3(*a);
                self.p3(*b);
                self.f(*radius);
            }
            SdfNode::Ellipsoid { radii } => {
                self.u8(5);
                self.p3(*radii);
            }
            SdfNode::Gyroid { period, wall } => {
                self.u8(6);
                self.f(*period);
                self.f(*wall);
            }
            SdfNode::Combine { op, blend, of } => {
                self.u8(7);
                self.u8(*op as u8);
                self.f(*blend);
                self.u64(of.len() as u64);
                of.iter().for_each(|c| self.node(c));
            }
            SdfNode::Shell { thickness, of } => {
                self.u8(8);
                self.f(*thickness);
                self.node(of);
            }
            SdfNode::Round { radius, of } => {
                self.u8(9);
                self.f(*radius);
                self.node(of);
            }
            SdfNode::Transform { at, rotate, scale, of } => {
                self.u8(10);
                self.p3(*at);
                self.p3(*rotate);
                self.f(*scale);
                self.node(of);
            }
            SdfNode::Kernel(k) => {
                self.u8(11);
                self.u64(k.key);
            }
        }
    }
}

impl SdfShape {
    /// The canonical encoding (each kernel by its key).
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(MAGIC.to_vec());
        o.p3(self.min);
        o.p3(self.max);
        o.node(&self.root);
        o.0
    }

    /// The canonical identity: the encoding's hash.
    pub fn key(&self) -> [u8; 32] {
        makepad_core_util::sha256::sha256(&self.encode())
    }

    /// The shape as a field.
    pub fn field(&self) -> Arc<dyn Sdf3 + Send + Sync> {
        Arc::new(Eval(self.root.clone()))
    }
}

struct Eval(SdfNode);

impl Sdf3 for Eval {
    fn distance(&self, p: Vec3d) -> f64 {
        distance(&self.0, p)
    }
}

fn smin(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

fn rot(deg: P3) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let m = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| -> [[f64; 3]; 3] { std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())) };
    m(rz, m(ry, rx))
}

/// The distance of one node at `p`.
pub fn distance(n: &SdfNode, p: Vec3d) -> f64 {
    match n {
        SdfNode::Sphere { radius } => p.length() - radius,
        SdfNode::Box { size, radius } => {
            let q = [p.x.abs() - size[0] * 0.5 + radius, p.y.abs() - size[1] * 0.5 + radius, p.z.abs() - size[2] * 0.5 + radius];
            Vec3d::new(q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)).length() + q[0].max(q[1].max(q[2])).min(0.0) - radius
        }
        SdfNode::Cylinder { radius, top_radius, height } => {
            let hh = height * 0.5;
            let qx = (p.x * p.x + p.z * p.z).sqrt();
            if radius == top_radius {
                let (dx, dy) = (qx - radius, p.y.abs() - hh);
                return (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt() + dx.max(dy).min(0.0);
            }
            let (r1, r2, qy) = (*radius, *top_radius, p.y);
            let (k2x, k2y) = (r2 - r1, 2.0 * hh);
            let cax = qx - qx.min(if qy < 0.0 { r1 } else { r2 });
            let cay = qy.abs() - hh;
            let t = (((r2 - qx) * k2x + (hh - qy) * k2y) / (k2x * k2x + k2y * k2y)).clamp(0.0, 1.0);
            let (cbx, cby) = (qx - r2 + k2x * t, qy - hh + k2y * t);
            let s = if cbx < 0.0 && cay < 0.0 { -1.0 } else { 1.0 };
            s * (cax * cax + cay * cay).min(cbx * cbx + cby * cby).sqrt()
        }
        SdfNode::Torus { radius, tube } => {
            let q = (p.x * p.x + p.z * p.z).sqrt() - radius;
            (q * q + p.y * p.y).sqrt() - tube
        }
        SdfNode::Capsule { a, b, radius } => {
            let (a, b) = (Vec3d::new(a[0], a[1], a[2]), Vec3d::new(b[0], b[1], b[2]));
            let (pa, ba) = (p - a, b - a);
            let h = (pa.dot(ba) / ba.dot(ba)).clamp(0.0, 1.0);
            (pa - ba * h).length() - radius
        }
        SdfNode::Ellipsoid { radii: r } => {
            let k0 = Vec3d::new(p.x / r[0], p.y / r[1], p.z / r[2]).length();
            let k1 = Vec3d::new(p.x / (r[0] * r[0]), p.y / (r[1] * r[1]), p.z / (r[2] * r[2])).length();
            k0 * (k0 - 1.0) / k1.max(1e-30)
        }
        SdfNode::Gyroid { period, wall } => {
            let k = std::f64::consts::TAU / period;
            let g = (p.x * k).sin() * (p.y * k).cos() + (p.y * k).sin() * (p.z * k).cos() + (p.z * k).sin() * (p.x * k).cos();
            g.abs() / k - wall * 0.5
        }
        SdfNode::Combine { op, blend, of } => {
            let mut d = distance(&of[0], p);
            for c in &of[1..] {
                let e = distance(c, p);
                d = match op {
                    SdfOp::Union => smin(d, e, *blend),
                    SdfOp::Intersection => -smin(-d, -e, *blend),
                    SdfOp::Difference => -smin(-d, e, *blend),
                };
            }
            d
        }
        SdfNode::Shell { thickness, of } => distance(of, p).abs() - thickness,
        SdfNode::Round { radius, of } => distance(of, p) - radius,
        SdfNode::Transform { at, rotate, scale, of } => {
            let r = rot(*rotate);
            let d = p - Vec3d::new(at[0], at[1], at[2]);
            // The inverse rotation is the transpose.
            let q = Vec3d::new(
                r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z,
                r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z,
                r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z,
            );
            distance(of, q * (1.0 / scale)) * scale
        }
        SdfNode::Kernel(k) => k.field.distance(p),
    }
}
