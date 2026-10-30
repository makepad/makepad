//! A shape lowered to the engine's program: a flat stream of f32 RGBA texels
//! that the bake shader and the CPU twin ([`crate::twin`]) both interpret, op
//! for op, so their results agree bit for bit.
//!
//! The stream is the node tree walked depth first:
//! - a leaf pushes its distance (and the material slot in force) on the value
//!   stack;
//! - a Combine pushes its first operand, then for each next one its program
//!   followed by a FOLD that combines the top two values (the slot rules of
//!   `csg_sdf::distance_slot`);
//! - point ops bracket their subtree: XFORM_PUSH moves the point into the
//!   child's frame and XFORM_POP restores it and scales the distance back;
//!   DEFORM_PUSH / DEFORM_POP warp it and divide by the warp's Lipschitz bound;
//! - Shell and Round act on the top value; MAT_PUSH / MAT_POP set the slot of
//!   the leaves inside (the innermost wins);
//! - Instance becomes a sharp union of its copies (each one an XFORM bracket
//!   around the same subprogram); Bone and Id leave the field as it is.
//!
//! All parameters are converted to f32 here, once: what the GPU and the twin
//! compute from is the same bits. A program starts with one header texel
//! (texel count, value-stack depth, point-stack depth, 0).

use makepad_csg_sdf::{SdfDeform, SdfNode, SdfOp, SdfPlace};

/// Op codes (the header texel's `x`).
pub mod op {
    pub const PRIM: f32 = 0.0;
    pub const XFORM_PUSH: f32 = 1.0;
    pub const XFORM_POP: f32 = 2.0;
    pub const DEFORM_PUSH: f32 = 3.0;
    pub const DEFORM_POP: f32 = 4.0;
    pub const FOLD: f32 = 5.0;
    pub const SHELL: f32 = 6.0;
    pub const ROUND: f32 = 7.0;
    pub const MAT_PUSH: f32 = 8.0;
    pub const MAT_POP: f32 = 9.0;
}

/// Primitive kinds (a PRIM's header `y`). A PRIM is 3 texels: header
/// (op, kind, a, b) then (c, d, e, f) and (g, h, i, j).
pub mod prim {
    /// a = radius
    pub const SPHERE: f32 = 0.0;
    /// a b c = half size less the rounding, d = rounding
    pub const BOX: f32 = 1.0;
    /// a = radius, b = half height
    pub const CYLINDER: f32 = 2.0;
    /// a = bottom radius, b = top radius, c = half height
    pub const CONE: f32 = 3.0;
    /// a = ring radius, b = tube radius
    pub const TORUS: f32 = 4.0;
    /// a b c = start, d e f = end - start, g = radius
    pub const CAPSULE: f32 = 5.0;
    /// a b c = radii
    pub const ELLIPSOID: f32 = 6.0;
    /// a = 2 pi / period, b = half wall
    pub const GYROID: f32 = 7.0;
}

/// Why a shape could not be lowered.
#[derive(Clone, Debug, PartialEq)]
pub enum LowerError {
    /// A compiled Splash kernel or a mesh field: baked on the CPU (not yet in
    /// the GPU program).
    Unsupported(&'static str),
}

/// A lowered program.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    /// The texels, header first.
    pub texels: Vec<[f32; 4]>,
    /// The deepest value stack and point stack it needs.
    pub value_depth: usize,
    pub point_depth: usize,
}

/// The value stack and point stack every interpreter (shader and twin)
/// provides. A deeper program is not run on the GPU; its bricks bake on the
/// CPU twin instead.
pub const MAX_VALUE_DEPTH: usize = 16;
pub const MAX_POINT_DEPTH: usize = 8;

impl Program {
    /// Whether the GPU interpreter can run it.
    pub fn fits_gpu(&self) -> bool {
        self.value_depth <= MAX_VALUE_DEPTH && self.point_depth <= MAX_POINT_DEPTH
    }
}

struct Lower {
    out: Vec<[f32; 4]>,
    vdepth: usize,
    vmax: usize,
    pdepth: usize,
    pmax: usize,
}

fn f(v: f64) -> f32 {
    v as f32
}

/// The world-to-local affine of a placement (its rows: 3 x (a b c t)) and its scale.
fn affine_into(r: [[f64; 3]; 3], at: [f64; 3], scale: f64) -> [[f32; 4]; 3] {
    // local = R^T (p - at) / scale
    std::array::from_fn(|i| {
        let a = [r[0][i] / scale, r[1][i] / scale, r[2][i] / scale];
        let t = -(a[0] * at[0] + a[1] * at[1] + a[2] * at[2]);
        [f(a[0]), f(a[1]), f(a[2]), f(t)]
    })
}

/// The rotation of `csg_sdf`'s Transform: about X, then Y, then Z (degrees).
fn rot_deg(deg: [f64; 3]) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let m = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| -> [[f64; 3]; 3] { std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())) };
    m(rz, m(ry, rx))
}

/// The rotation matrix of a unit quaternion (x y z w).
fn rot_quat(q: [f64; 4]) -> [[f64; 3]; 3] {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

impl Lower {
    fn push_value(&mut self) {
        self.vdepth += 1;
        self.vmax = self.vmax.max(self.vdepth);
    }

    fn prim(&mut self, kind: f32, p: [f32; 10]) {
        self.out.push([op::PRIM, kind, p[0], p[1]]);
        self.out.push([p[2], p[3], p[4], p[5]]);
        self.out.push([p[6], p[7], p[8], p[9]]);
        self.push_value();
    }

    fn xform(&mut self, rows: [[f32; 4]; 3], scale: f64, body: impl FnOnce(&mut Self) -> Result<(), LowerError>) -> Result<(), LowerError> {
        self.out.push([op::XFORM_PUSH, f(scale), 0.0, 0.0]);
        self.out.extend(rows);
        self.pdepth += 1;
        self.pmax = self.pmax.max(self.pdepth);
        body(self)?;
        self.pdepth -= 1;
        self.out.push([op::XFORM_POP, f(scale), 0.0, 0.0]);
        Ok(())
    }

    fn place(&mut self, pl: &SdfPlace, of: &SdfNode) -> Result<(), LowerError> {
        self.xform(affine_into(rot_quat(pl.rotate), pl.at, pl.scale), pl.scale, |s| s.node(of))
    }

    fn node(&mut self, n: &SdfNode) -> Result<(), LowerError> {
        let z = 0.0f32;
        match n {
            SdfNode::Sphere { radius } => self.prim(prim::SPHERE, [f(*radius), z, z, z, z, z, z, z, z, z]),
            SdfNode::Box { size, radius } => self.prim(prim::BOX, [f(size[0] * 0.5 - radius), f(size[1] * 0.5 - radius), f(size[2] * 0.5 - radius), f(*radius), z, z, z, z, z, z]),
            SdfNode::Cylinder { radius, top_radius, height } => {
                if radius == top_radius {
                    self.prim(prim::CYLINDER, [f(*radius), f(height * 0.5), z, z, z, z, z, z, z, z])
                } else {
                    self.prim(prim::CONE, [f(*radius), f(*top_radius), f(height * 0.5), z, z, z, z, z, z, z])
                }
            }
            SdfNode::Torus { radius, tube } => self.prim(prim::TORUS, [f(*radius), f(*tube), z, z, z, z, z, z, z, z]),
            SdfNode::Capsule { a, b, radius } => {
                self.prim(prim::CAPSULE, [f(a[0]), f(a[1]), f(a[2]), f(b[0] - a[0]), f(b[1] - a[1]), f(b[2] - a[2]), f(*radius), z, z, z])
            }
            SdfNode::Ellipsoid { radii } => self.prim(prim::ELLIPSOID, [f(radii[0]), f(radii[1]), f(radii[2]), z, z, z, z, z, z, z]),
            SdfNode::Gyroid { period, wall } => self.prim(prim::GYROID, [f(std::f64::consts::TAU / period), f(wall * 0.5), z, z, z, z, z, z, z, z]),
            SdfNode::Combine { op: o, blend, of } => {
                self.node(&of[0])?;
                let code = match o {
                    SdfOp::Union => 0.0,
                    SdfOp::Difference => 1.0,
                    SdfOp::Intersection => 2.0,
                };
                for c in &of[1..] {
                    self.node(c)?;
                    self.out.push([op::FOLD, code, f(*blend), 0.0]);
                    self.vdepth -= 1;
                }
            }
            SdfNode::Shell { thickness, of } => {
                self.node(of)?;
                self.out.push([op::SHELL, f(*thickness), 0.0, 0.0]);
            }
            SdfNode::Round { radius, of } => {
                self.node(of)?;
                self.out.push([op::ROUND, f(*radius), 0.0, 0.0]);
            }
            SdfNode::Transform { at, rotate, scale, of } => self.xform(affine_into(rot_deg(*rotate), *at, *scale), *scale, |s| s.node(of))?,
            SdfNode::Material { slot, of, .. } => {
                self.out.push([op::MAT_PUSH, *slot as f32, 0.0, 0.0]);
                self.node(of)?;
                self.out.push([op::MAT_POP, 0.0, 0.0, 0.0]);
            }
            SdfNode::Instance { places, of } => {
                for (i, pl) in places.iter().enumerate() {
                    self.place(pl, of)?;
                    if i > 0 {
                        self.out.push([op::FOLD, 0.0, 0.0, 0.0]);
                        self.vdepth -= 1;
                    }
                }
            }
            SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => self.node(of)?,
            SdfNode::Deform { deform, lipschitz, of } => {
                let (kind, rate, axis) = match *deform {
                    SdfDeform::Twist { rate } => (0.0, rate, 0),
                    SdfDeform::Bend { rate } => (1.0, rate, 0),
                    SdfDeform::Taper { rate } => (2.0, rate, 0),
                    SdfDeform::Mirror { axis } => (3.0, 0.0, axis.min(2)),
                };
                self.out.push([op::DEFORM_PUSH, kind, f(rate), axis as f32]);
                self.pdepth += 1;
                self.pmax = self.pmax.max(self.pdepth);
                self.node(of)?;
                self.pdepth -= 1;
                self.out.push([op::DEFORM_POP, f(*lipschitz), 0.0, 0.0]);
            }
            SdfNode::Kernel(_) => return Err(LowerError::Unsupported("a compiled Splash kernel")),
            SdfNode::Mesh(_) => return Err(LowerError::Unsupported("a mesh field")),
        }
        Ok(())
    }
}

/// Lowers a node (usually one already culled to a brick's region).
pub fn lower(n: &SdfNode) -> Result<Program, LowerError> {
    let mut l = Lower { out: vec![[0.0; 4]], vdepth: 0, vmax: 0, pdepth: 0, pmax: 0 };
    l.node(n)?;
    let len = l.out.len();
    l.out[0] = [len as f32, l.vmax as f32, l.pmax as f32, 0.0];
    Ok(Program { texels: l.out, value_depth: l.vmax, point_depth: l.pmax })
}
