//! The typed SDF shape: a tree of analytic distance nodes and compiled Splash
//! distance kernels, with a canonical binary encoding (its key) and an evaluator
//! that needs no script VM for the analytic nodes.
//!
//! Analytic nodes match the Splash SDF library of the kernel prelude
//! (`sdf_sphere`, `smooth_union`, `sdf_shell` ...): same formulas, same
//! conventions (shapes centred on the origin, Y up, full sizes). A
//! [`SdfKernel`] is a Splash `fn(p)` compiled by the kernel compiler: `key` is
//! its program identity and `field` the compiled field.

use crate::sdf::{Hints, Sdf3};
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
    /// The Splash program behind `field`, when it is one: what a GPU
    /// route compiles (its structure) and feeds (its parameters).
    pub splash: Option<Arc<crate::SdfSplashExpr>>,
    /// A box holding the solid in the kernel's own frame (the document's
    /// declared bounds); `None`: unbounded.
    pub bounds: Option<crate::bounds::Bounds>,
    /// The grid whose read `field` is and nothing more ([`SdfKernel::grid_read`]): its solid is
    /// known from its samples, without reading the field.
    pub solid: Option<Arc<crate::SdfGrid>>,
}

impl SdfKernel {
    /// A Splash field as a kernel node, its solid inside `bounds`.
    pub fn splash(field: crate::SdfSplashExpr, bounds: Option<crate::bounds::Bounds>) -> SdfKernel {
        let field = Arc::new(field);
        SdfKernel { key: field.key(), field: field.clone(), splash: Some(field), bounds, solid: None }
    }

    /// A Splash field that is its grid's read and nothing more (`sdf_grid3(p)`), read inside
    /// `within`: its bounds are the box of the grid's solid there.
    pub fn grid_read(field: crate::SdfSplashExpr, within: crate::bounds::Bounds) -> SdfKernel {
        let grid = field.grid().expect("a field that reads a grid").clone();
        let centre: crate::shape::P3 = std::array::from_fn(|k| 0.5 * (within.0[k] + within.1[k]));
        let bounds = grid.solid_box().and_then(|b| crate::bounds::meet(&b, &within)).unwrap_or((centre, centre));
        SdfKernel { solid: Some(grid), ..SdfKernel::splash(field, Some(bounds)) }
    }

    /// The kernel as points in `region` (its frame) read it: a gridded field with the piece of
    /// its grid they read, any other the same.
    pub fn cropped(&self, region: &crate::bounds::Bounds) -> SdfKernel {
        let Some(s) = &self.splash else { return self.clone() };
        let Some(g) = s.grid() else { return self.clone() };
        let piece = g.crop(region);
        if piece.hash() == g.hash() {
            return self.clone();
        }
        SdfKernel { solid: self.solid.clone(), ..SdfKernel::splash(s.with_grid(piece), self.bounds) }
    }
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
    /// A triangle mesh's signed distance (its own frame; place it with a Transform).
    Mesh(Arc<crate::mesh_field::MeshField>),
    /// Separate links (chains, chainmail): closed polylines swept by one radius, each in its own slot.
    Links(Arc<crate::links::Links>),
    /// Smooth tubes through points, each point with its own radius ([`crate::tubes`]).
    Tubes(Arc<crate::tubes::Tubes>),
    /// A sketch extruded or revolved ([`crate::sketches`]).
    Sketch(Arc<crate::sketches::SketchSolid>),
    /// `of` in material `slot` of [`SdfShape::materials`] (0 is the default);
    /// the innermost Material wins. Slots through the other nodes: see
    /// [`distance_slot`].
    Material { slot: u8, within: Option<Box<SdfNode>>, of: Box<SdfNode> },
    /// `of` placed at each of `places`: the nearest copy. `of` keeps its own
    /// key ([`SdfNode::key`]), so moving copies re-bakes nothing.
    Instance { places: Vec<SdfPlace>, of: Box<SdfNode> },
    /// `of` (object space, rest pose) bound to bone `id`, whose rest frame is
    /// `rest`; the pose is a renderer's runtime data.
    Bone { id: u32, rest: SdfPlace, of: Box<SdfNode> },
    /// `of` warped, its distance divided by `lipschitz` (made by
    /// [`SdfNode::deform`] from the warp and `of`'s bounds).
    Deform { deform: SdfDeform, lipschitz: f64, of: Box<SdfNode> },
    /// The picking id of one source part: the low 32 bits of the sha256 of
    /// its source id (`bolts[2]`, `left.body`), see [`part_id`].
    Id { id: u32, of: Box<SdfNode> },
    /// `of` sculpted by the shape strokes of a strokes log in `of`'s frame
    /// ([`crate::strokes`]): each applied one in order, its tube smooth-unioned
    /// (raise) or smooth-subtracted (lower, crease). A point reads only the
    /// strokes indexed to its brick; culled to a region it is the plain
    /// Combines of the capsules there ([`crate::bounds::cull`]), so the engines
    /// draw it with the nodes they have.
    Sculpt { strokes: Arc<crate::strokes::Strokes>, of: Box<SdfNode> },
    /// `of` displaced by a pattern ([`crate::texture`]): a knurl, a hex grip, noise.
    Texture { tex: Arc<crate::texture::SdfTexture>, of: Box<SdfNode> },
}

/// A rigid placement with a uniform scale: scaled, then turned by the unit
/// quaternion `rotate` (x y z w), then moved by `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdfPlace {
    pub at: P3,
    pub rotate: [f64; 4],
    pub scale: f64,
}

impl SdfPlace {
    pub const IDENTITY: SdfPlace = SdfPlace { at: [0.0; 3], rotate: [0.0, 0.0, 0.0, 1.0], scale: 1.0 };

    /// A point of the placed node into the outer frame.
    pub fn out_of(&self, q: P3) -> P3 {
        let r = quat_rotate(self.rotate, q.map(|x| x * self.scale));
        std::array::from_fn(|i| r[i] + self.at[i])
    }

    /// A point of the outer frame into the placed node.
    pub fn into(&self, p: P3) -> P3 {
        let [x, y, z, w] = self.rotate;
        quat_rotate([-x, -y, -z, w], std::array::from_fn(|i| p[i] - self.at[i])).map(|v| v / self.scale)
    }
}

/// `v` turned by the unit quaternion `q` (x y z w).
pub fn quat_rotate(q: [f64; 4], v: P3) -> P3 {
    let u = [q[0], q[1], q[2]];
    let cross = |a: P3, b: P3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = cross(u, v).map(|c| c * 2.0);
    let ut = cross(u, t);
    std::array::from_fn(|i| v[i] + q[3] * t[i] + ut[i])
}

/// The unit quaternion of turns about X, then Y, then Z (degrees): the
/// rotation of [`SdfNode::Transform`].
pub fn quat_from_degrees(deg: P3) -> [f64; 4] {
    let h = deg.map(|d| d.to_radians() * 0.5);
    let (sx, cx) = h[0].sin_cos();
    let (sy, cy) = h[1].sin_cos();
    let (sz, cz) = h[2].sin_cos();
    // q = qz * qy * qx
    [sx * cy * cz - cx * sy * sz, cx * sy * cz + sx * cy * sz, cx * cy * sz - sx * sy * cz, cx * cy * cz + sx * sy * sz]
}

/// A warp of [`SdfNode::Deform`]. Each works about the node's own axes
/// through its origin; a Transform round it picks another axis and centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SdfDeform {
    /// Turned about Y by `rate` radians per unit of height.
    Twist { rate: f64 },
    /// Bent in the XY plane about Z: turned by `rate` radians per unit of X.
    Bend { rate: f64 },
    /// X and Z scaled by `1 + rate * y` (kept above zero over the node).
    Taper { rate: f64 },
    /// Mirrored across the plane through the origin normal to `axis` (0 x,
    /// 1 y, 2 z): the half on the positive side, twice.
    Mirror { axis: u8 },
}

/// The smallest taper scale used: a taper that reaches zero is clamped here.
pub const TAPER_MIN: f64 = 1e-3;

impl SdfDeform {
    /// A bound on how much the warp stretches distances over a node with
    /// bounds `of` (its own frame); `None` bounds count as a unit box.
    pub fn lipschitz(&self, of: Option<&crate::bounds::Bounds>) -> f64 {
        let b = of.copied().unwrap_or(([-1.0; 3], [1.0; 3]));
        let ext = |i: usize| b.0[i].abs().max(b.1[i].abs());
        // The largest stretch of a unit shear by `s`.
        let shear = |s: f64| (s.abs() + (s * s + 4.0).sqrt()) * 0.5;
        match *self {
            SdfDeform::Twist { rate } => shear(rate * ext(0).hypot(ext(2))),
            SdfDeform::Bend { rate } => shear(rate * ext(0).hypot(ext(1))),
            SdfDeform::Taper { rate } => {
                let (s0, s1) = (1.0 + rate * b.0[1], 1.0 + rate * b.1[1]);
                let smin = s0.min(s1).max(TAPER_MIN);
                (1.0 / smin).max(1.0) + rate.abs() * ext(0).hypot(ext(2)) / smin
            }
            SdfDeform::Mirror { .. } => 1.0,
        }
    }

    /// A point of the outer frame into the warped node.
    pub fn warp(&self, p: Vec3d) -> Vec3d {
        match *self {
            SdfDeform::Twist { rate } => {
                let (s, c) = (rate * p.y).sin_cos();
                Vec3d::new(c * p.x + s * p.z, p.y, -s * p.x + c * p.z)
            }
            SdfDeform::Bend { rate } => {
                let (s, c) = (rate * p.x).sin_cos();
                Vec3d::new(c * p.x + s * p.y, -s * p.x + c * p.y, p.z)
            }
            SdfDeform::Taper { rate } => {
                let k = 1.0 / (1.0 + rate * p.y).max(TAPER_MIN);
                Vec3d::new(p.x * k, p.y, p.z * k)
            }
            SdfDeform::Mirror { axis } => match axis {
                0 => Vec3d::new(p.x.abs(), p.y, p.z),
                1 => Vec3d::new(p.x, p.y.abs(), p.z),
                _ => Vec3d::new(p.x, p.y, p.z.abs()),
            },
        }
    }
}

impl SdfNode {
    /// Whether a run so far of a program under this node, for any reader, had a `while` or `loop`
    /// reach the language's stop ([`crate::SdfSplashExpr::stopped`]): its value there is what the
    /// loop had by then.
    pub fn loop_stopped(&self) -> bool {
        use SdfNode::*;
        let kernel = |k: &SdfKernel| k.splash.as_ref().is_some_and(|s| s.stopped());
        match self {
            Sphere { .. } | Box { .. } | Cylinder { .. } | Torus { .. } | Capsule { .. } | Ellipsoid { .. } | Gyroid { .. } | Mesh(_) | Links(_) | Tubes(_) | Sketch(_) => false,
            Kernel(k) => kernel(k),
            Combine { of, .. } => of.iter().any(SdfNode::loop_stopped),
            Material { within, of, .. } => of.loop_stopped() || within.as_deref().is_some_and(SdfNode::loop_stopped),
            Texture { tex, of } => kernel(&tex.kernel) || of.loop_stopped(),
            Shell { of, .. } | Round { of, .. } | Transform { of, .. } | Instance { of, .. } | Bone { of, .. } | Deform { of, .. } | Id { of, .. } | Sculpt { of, .. } => of.loop_stopped(),
        }
    }

    /// The node as a field: [`distance`] and [`distance_many`] at its points, bit for bit, and
    /// read only as far as bounds need ([`Sdf3::distance_bounded`]).
    pub fn field(&self) -> Arc<dyn Sdf3 + Send + Sync> {
        Arc::new(Compiled::new(self))
    }

    /// `of` warped by `deform`, with its Lipschitz bound.
    pub fn deform(deform: SdfDeform, of: SdfNode) -> SdfNode {
        let lipschitz = deform.lipschitz(crate::bounds::node_bounds(&of).as_ref());
        SdfNode::Deform { deform, lipschitz, of: Box::new(of) }
    }

    /// The canonical encoding of this subtree.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(Vec::new());
        o.node(self);
        o.0
    }

    /// This subtree's own identity (a renderer bakes an instanced node once
    /// by it).
    pub fn key(&self) -> [u8; 32] {
        makepad_core_util::sha256::sha256(&self.encode())
    }
}

/// The picking id of a source part id: the low 32 bits of its sha256.
pub fn part_id(source_id: &str) -> u32 {
    let h = makepad_core_util::sha256::sha256(source_id.as_bytes());
    u32::from_le_bytes([h[0], h[1], h[2], h[3]])
}

/// A material of [`SdfShape::materials`]; outside the shape's key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdfMaterial {
    pub albedo: [f32; 3],
    pub fade: f32,
    pub metal: f32,
    pub rough: f32,
    pub emissive: f32,
    pub emissive_color: [f32; 3],
    pub unlit: bool,
    pub colorize: bool,
    pub splat: SdfSplat,
}

impl Default for SdfMaterial {
    fn default() -> Self {
        SdfMaterial { albedo: [0.8; 3], fade: 0.0, metal: 0.0, rough: 0.5, emissive: 0.0, emissive_color: [1.0; 3], unlit: false, colorize: false, splat: SdfSplat::default() }
    }
}

/// A material's splat look.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SdfSplat {
    pub size: f32,
    pub opacity: f32,
    pub fluff: f32,
    pub jitter_size: f32,
    pub jitter_pos: f32,
    pub jitter_rot: f32,
    pub jitter_bump: f32,
    pub jitter_metal: f32,
    pub jitter_bright: f32,
    pub jitter_color: f32,
}

/// A shape, a box it fits in and its material table (slot `i` is
/// `materials[i]`; a missing slot is the default material).
#[derive(Clone, Debug)]
pub struct SdfShape {
    pub root: SdfNode,
    pub min: P3,
    pub max: P3,
    /// Outside the key: a colour change re-meshes and re-bakes nothing.
    pub materials: Vec<SdfMaterial>,
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
    fn place(&mut self, p: &SdfPlace) {
        self.p3(p.at);
        p.rotate.iter().for_each(|x| self.f(*x));
        self.f(p.scale);
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
                // The declared box decides culling: part of the identity.
                if let Some((lo, hi)) = k.bounds {
                    self.p3(lo);
                    self.p3(hi);
                }
            }
            SdfNode::Mesh(m) => {
                self.u8(12);
                self.u64(m.key());
            }
            SdfNode::Material { slot, within, of } => {
                self.u8(13);
                self.u8(*slot);
                match within {
                    Some(w) => {
                        self.u8(1);
                        self.node(w);
                    }
                    None => self.u8(0),
                }
                self.node(of);
            }
            SdfNode::Instance { places, of } => {
                self.u8(14);
                self.u64(places.len() as u64);
                places.iter().for_each(|p| self.place(p));
                self.node(of);
            }
            SdfNode::Bone { id, rest, of } => {
                self.u8(15);
                self.u64(*id as u64);
                self.place(rest);
                self.node(of);
            }
            SdfNode::Deform { deform, lipschitz, of } => {
                self.u8(16);
                match *deform {
                    SdfDeform::Twist { rate } => {
                        self.u8(0);
                        self.f(rate);
                    }
                    SdfDeform::Bend { rate } => {
                        self.u8(1);
                        self.f(rate);
                    }
                    SdfDeform::Taper { rate } => {
                        self.u8(2);
                        self.f(rate);
                    }
                    SdfDeform::Mirror { axis } => {
                        self.u8(3);
                        self.u8(axis);
                    }
                }
                self.f(*lipschitz);
                self.node(of);
            }
            SdfNode::Id { id, of } => {
                self.u8(17);
                self.u64(*id as u64);
                self.node(of);
            }
            SdfNode::Links(l) => {
                self.u8(18);
                l.key().iter().for_each(|b| self.u8(*b));
            }
            SdfNode::Tubes(t) => {
                self.u8(21);
                t.key().iter().for_each(|b| self.u8(*b));
            }
            SdfNode::Sketch(s) => {
                self.u8(22);
                s.key().iter().for_each(|b| self.u8(*b));
            }
            SdfNode::Sculpt { strokes, of } => {
                self.u8(19);
                self.u64(strokes.shape_key());
                self.node(of);
            }
            SdfNode::Texture { tex, of } => {
                self.u8(20);
                self.u64(tex.kernel.key);
                match tex.project {
                    crate::texture::TexProject::Axis(a) => self.u8(a),
                    crate::texture::TexProject::Triplanar => self.u8(3),
                }
                for v in [tex.kscale, tex.depth, tex.lipschitz, tex.step] {
                    self.f(v);
                }
                for m in [tex.top, tex.bottom] {
                    let (lo, hi) = m.unwrap_or((2.0, 2.0));
                    self.f(lo);
                    self.f(hi);
                }
                self.node(of);
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
        self.root.field()
    }

    /// This shape sculpted by a strokes log's shape strokes (in its own frame): its solid under
    /// one [`SdfNode::Sculpt`] of `strokes` (a sculpt at its root takes the new log in place of
    /// its own), its box grown by the raised tubes. A shape sculpted by a log, then by that log
    /// with strokes added, equals the unsculpted shape sculpted by the longer log, key included.
    pub fn sculpted(self, strokes: Arc<crate::strokes::Strokes>) -> SdfShape {
        let of = match self.root {
            SdfNode::Sculpt { of, .. } => of,
            root => Box::new(root),
        };
        let (mut min, mut max) = (self.min, self.max);
        if let Some((lo, hi)) = strokes.raised_bounds() {
            for k in 0..3 {
                min[k] = min[k].min(lo[k]);
                max[k] = max[k].max(hi[k]);
            }
        }
        SdfShape { root: SdfNode::Sculpt { strokes, of }, min, max, materials: self.materials }
    }
}

/// The field evaluator owns its matrices. The public tree remains editable and its
/// canonical encoding unchanged; constructing another field compiles its current tree.
/// Batched kernels keep their vectorized path. The leaves read under bounds are their own
/// ([`crate::bounded`]): a mesh, numbered for its hints ([`Hints::part`]), and tubes with no
/// fillet; a combination knows which of its operands read under bounds.
pub(crate) enum Compiled {
    Leaf(SdfNode),
    Mesh(Arc<crate::mesh_field::MeshField>, usize),
    Tubes(Arc<crate::tubes::Tubes>),
    Combine(SdfOp, f64, Vec<Compiled>, Vec<bool>),
    Shell(f64, Box<Compiled>),
    Round(f64, Box<Compiled>),
    Transform { at: P3, rotation: [[f64; 3]; 3], scale: f64, of: Box<Compiled> },
    Instance(Vec<SdfPlace>, Box<Compiled>),
    Deform(SdfDeform, f64, Box<Compiled>),
    Sculpt(Arc<crate::strokes::Strokes>, Box<Compiled>),
    Texture(Arc<crate::texture::SdfTexture>, Box<Compiled>),
}

impl Compiled {
    pub(crate) fn new(node: &SdfNode) -> Self {
        Self::of(node, &mut 0)
    }

    /// `node`, its meshes numbered from `meshes` on.
    fn of(node: &SdfNode, meshes: &mut usize) -> Self {
        match node {
            SdfNode::Combine { op, blend, of } => {
                let of: Vec<Compiled> = of.iter().map(|c| Self::of(c, meshes)).collect();
                let bounded = of.iter().map(Compiled::reads_bounded).collect();
                Self::Combine(*op, *blend, of, bounded)
            }
            SdfNode::Shell { thickness, of } => Self::Shell(*thickness, Box::new(Self::of(of, meshes))),
            SdfNode::Round { radius, of } => Self::Round(*radius, Box::new(Self::of(of, meshes))),
            SdfNode::Transform { at, rotate, scale, of } => Self::Transform { at: *at, rotation: rot(*rotate), scale: *scale, of: Box::new(Self::of(of, meshes)) },
            SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => Self::of(of, meshes),
            SdfNode::Instance { places, of } => Self::Instance(places.clone(), Box::new(Self::of(of, meshes))),
            SdfNode::Deform { deform, lipschitz, of } => Self::Deform(*deform, *lipschitz, Box::new(Self::of(of, meshes))),
            SdfNode::Sculpt { strokes, of } => Self::Sculpt(strokes.clone(), Box::new(Self::of(of, meshes))),
            SdfNode::Texture { tex, of } => Self::Texture(tex.clone(), Box::new(Self::of(of, meshes))),
            SdfNode::Mesh(m) => {
                *meshes += 1;
                Self::Mesh(m.clone(), *meshes - 1)
            }
            SdfNode::Tubes(t) if t.blend == 0.0 => Self::Tubes(t.clone()),
            SdfNode::Sphere { .. } | SdfNode::Box { .. } | SdfNode::Cylinder { .. }
            | SdfNode::Torus { .. } | SdfNode::Capsule { .. } | SdfNode::Ellipsoid { .. }
            | SdfNode::Gyroid { .. } | SdfNode::Kernel(_) | SdfNode::Links(_) | SdfNode::Tubes(_) | SdfNode::Sketch(_) => Self::Leaf(node.clone()),
        }
    }

    /// A leaf read under bounds is in it, through no sculpt or texture (they read theirs in full).
    fn reads_bounded(&self) -> bool {
        match self {
            Self::Mesh(..) | Self::Tubes(_) => true,
            Self::Combine(_, _, _, bounded) => bounded.contains(&true),
            Self::Shell(_, of) | Self::Round(_, of) | Self::Transform { of, .. } | Self::Instance(_, of) | Self::Deform(_, _, of) => of.reads_bounded(),
            Self::Leaf(_) | Self::Sculpt(..) | Self::Texture(..) => false,
        }
    }
}

/// A point of a [`Compiled::Transform`]'s parent into its frame: the subtraction, transpose
/// multiply, reciprocal and scale in precisely the order of [`transform_into`].
pub(crate) fn framed(at: &P3, r: &[[f64; 3]; 3], scale: f64, p: Vec3d) -> Vec3d {
    let d = p - Vec3d::new(at[0], at[1], at[2]);
    Vec3d::new(r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z, r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z, r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z) * (1.0 / scale)
}

impl Sdf3 for Compiled {
    /// Read only as far as the bounds need ([`crate::bounded`]).
    fn distance_bounded(&self, p: Vec3d, lower: f64, upper: f64, hints: &mut Hints) -> f64 {
        self.bounded(p, lower, upper, hints)
    }

    fn distance_many_bounded(&self, ps: &[Vec3d], lower: f64, upper: f64, out: &mut [f64], hints: &mut Hints) {
        self.bounded_many(ps, lower, upper, out, hints)
    }

    fn distance(&self, p: Vec3d) -> f64 {
        match self {
            Self::Leaf(node) => distance(node, p),
            Self::Mesh(m, _) => m.signed64(p),
            Self::Tubes(t) => t.distance(p),
            Self::Combine(op, blend, of, _) => {
                let mut d = of[0].distance(p);
                for child in &of[1..] {
                    let e = child.distance(p);
                    d = match op {
                        SdfOp::Union => smin(d, e, *blend),
                        SdfOp::Intersection => -smin(-d, -e, *blend),
                        SdfOp::Difference => -smin(-d, e, *blend),
                    };
                }
                d
            }
            Self::Shell(thickness, of) => of.distance(p).abs() - thickness,
            Self::Round(radius, of) => of.distance(p) - radius,
            Self::Transform { at, rotation, scale, of } => of.distance(framed(at, rotation, *scale, p)) * scale,
            Self::Instance(places, of) => places.iter().map(|pl| of.distance(v3(pl.into([p.x, p.y, p.z]))) * pl.scale).fold(f64::INFINITY, f64::min),
            Self::Deform(deform, lipschitz, of) => of.distance(deform.warp(p)) / lipschitz,
            Self::Sculpt(strokes, of) => strokes.sculpt([p.x, p.y, p.z], of.distance(p), smin),
            Self::Texture(tex, of) => tex.eval(p, |q| of.distance(q)),
        }
    }

    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        match self {
            Self::Leaf(n) => distance_many(n, ps, out),
            Self::Mesh(m, _) => {
                for (p, o) in ps.iter().zip(out.iter_mut()) {
                    *o = m.signed64(*p);
                }
            }
            Self::Tubes(t) => {
                for (p, o) in ps.iter().zip(out.iter_mut()) {
                    *o = t.distance(*p);
                }
            }
            Self::Combine(op, blend, of, _) => {
                of[0].distance_many(ps, out);
                let mut e = vec![0.0; ps.len()];
                for c in &of[1..] {
                    c.distance_many(ps, &mut e);
                    for (d, x) in out.iter_mut().zip(&e) {
                        *d = match op {
                            SdfOp::Union => smin(*d, *x, *blend),
                            SdfOp::Intersection => -smin(-*d, -*x, *blend),
                            SdfOp::Difference => -smin(-*d, *x, *blend),
                        };
                    }
                }
            }
            Self::Shell(thickness, of) => {
                of.distance_many(ps, out);
                out.iter_mut().for_each(|d| *d = d.abs() - thickness);
            }
            Self::Round(radius, of) => {
                of.distance_many(ps, out);
                out.iter_mut().for_each(|d| *d -= radius);
            }
            Self::Transform { at, rotation, scale, of } => {
                let q: Vec<Vec3d> = ps.iter().map(|p| framed(at, rotation, *scale, *p)).collect();
                of.distance_many(&q, out);
                out.iter_mut().for_each(|d| *d *= scale);
            }
            Self::Instance(places, of) => {
                out.iter_mut().for_each(|d| *d = f64::INFINITY);
                let mut e = vec![0.0; ps.len()];
                for pl in places {
                    let q: Vec<Vec3d> = ps.iter().map(|p| v3(pl.into([p.x, p.y, p.z]))).collect();
                    of.distance_many(&q, &mut e);
                    for (d, x) in out.iter_mut().zip(&e) {
                        *d = d.min(x * pl.scale);
                    }
                }
            }
            Self::Deform(deform, lipschitz, of) => {
                let q: Vec<Vec3d> = ps.iter().map(|p| deform.warp(*p)).collect();
                of.distance_many(&q, out);
                out.iter_mut().for_each(|d| *d /= lipschitz);
            }
            Self::Sculpt(strokes, of) => {
                of.distance_many(ps, out);
                for (p, d) in ps.iter().zip(out.iter_mut()) {
                    *d = strokes.sculpt([p.x, p.y, p.z], *d, smin);
                }
            }
            Self::Texture(tex, of) => tex.eval_many(ps, out, |q, o| of.distance_many(q, o)),
        }
    }
}

/// [`distance`] at many points: a kernel answers its points in one call.
pub fn distance_many(n: &SdfNode, ps: &[Vec3d], out: &mut [f64]) {
    match n {
        SdfNode::Kernel(k) => k.field.distance_many(ps, out),
        SdfNode::Combine { op, blend, of } => {
            distance_many(&of[0], ps, out);
            let mut e = vec![0.0; ps.len()];
            for c in &of[1..] {
                distance_many(c, ps, &mut e);
                for (d, x) in out.iter_mut().zip(&e) {
                    *d = match op {
                        SdfOp::Union => smin(*d, *x, *blend),
                        SdfOp::Intersection => -smin(-*d, -*x, *blend),
                        SdfOp::Difference => -smin(-*d, *x, *blend),
                    };
                }
            }
        }
        SdfNode::Shell { thickness, of } => {
            distance_many(of, ps, out);
            out.iter_mut().for_each(|d| *d = d.abs() - thickness);
        }
        SdfNode::Round { radius, of } => {
            distance_many(of, ps, out);
            out.iter_mut().for_each(|d| *d -= radius);
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let r = rot(*rotate);
            let q: Vec<Vec3d> = ps
                .iter()
                .map(|p| {
                    let d = *p - Vec3d::new(at[0], at[1], at[2]);
                    Vec3d::new(r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z, r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z, r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z) * (1.0 / scale)
                })
                .collect();
            distance_many(of, &q, out);
            out.iter_mut().for_each(|d| *d *= scale);
        }
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => distance_many(of, ps, out),
        SdfNode::Instance { places, of } => {
            out.iter_mut().for_each(|d| *d = f64::INFINITY);
            let mut e = vec![0.0; ps.len()];
            for pl in places {
                let q: Vec<Vec3d> = ps.iter().map(|p| v3(pl.into([p.x, p.y, p.z]))).collect();
                distance_many(of, &q, &mut e);
                for (d, x) in out.iter_mut().zip(&e) {
                    *d = d.min(x * pl.scale);
                }
            }
        }
        SdfNode::Deform { deform, lipschitz, of } => {
            let q: Vec<Vec3d> = ps.iter().map(|p| deform.warp(*p)).collect();
            distance_many(of, &q, out);
            out.iter_mut().for_each(|d| *d /= lipschitz);
        }
        SdfNode::Sculpt { strokes, of } => {
            distance_many(of, ps, out);
            for (p, d) in ps.iter().zip(out.iter_mut()) {
                *d = strokes.sculpt([p.x, p.y, p.z], *d, smin);
            }
        }
        SdfNode::Texture { tex, of } => tex.eval_many(ps, out, |q, o| distance_many(of, q, o)),
        other => {
            for (p, o) in ps.iter().zip(out.iter_mut()) {
                *o = distance(other, *p);
            }
        }
    }
}

pub(crate) fn v3(p: P3) -> Vec3d {
    Vec3d::new(p[0], p[1], p[2])
}

/// The distance of one node at `p` and the material slot there:
/// - Material: the innermost wins (0 outside every Material); one with a
///   `within` region gives its slot only inside the region (distance <= 0)
///   and its distance is always its child's;
/// - union and smooth union: the nearer operand's (a tie keeps the earlier);
/// - difference: the base's (a cut surface takes the base material);
/// - intersection: the farther operand's (a tie keeps the earlier);
/// - Instance: the nearest copy's; every other node: its child's.
pub fn distance_slot(n: &SdfNode, p: Vec3d) -> (f64, u8) {
    slot_in(n, p, 0)
}

/// The distance of one node at `p` and the innermost [`SdfNode::Id`] whose
/// subtree makes the surface there (`None` outside every Id): what a pick
/// at a surface point names. As [`distance_slot`], except that a difference
/// names the cutting operand where its surface wins (clicking a hole picks
/// the hole).
pub fn distance_id(n: &SdfNode, p: Vec3d) -> (f64, Option<u32>) {
    id_in(n, p, None)
}

fn id_in(n: &SdfNode, p: Vec3d, inherit: Option<u32>) -> (f64, Option<u32>) {
    match n {
        SdfNode::Id { id, of } => id_in(of, p, Some(*id)),
        SdfNode::Combine { op, blend, of } => {
            let (mut d, mut id) = id_in(&of[0], p, inherit);
            for c in &of[1..] {
                let (e, i) = id_in(c, p, inherit);
                match op {
                    SdfOp::Union => {
                        if e < d {
                            id = i;
                        }
                        d = smin(d, e, *blend);
                    }
                    SdfOp::Intersection => {
                        if e > d {
                            id = i;
                        }
                        d = -smin(-d, -e, *blend);
                    }
                    SdfOp::Difference => {
                        if -e > d {
                            id = i;
                        }
                        d = -smin(-d, e, *blend);
                    }
                }
            }
            (d, id)
        }
        SdfNode::Shell { thickness, of } => {
            let (d, i) = id_in(of, p, inherit);
            (d.abs() - thickness, i)
        }
        SdfNode::Round { radius, of } => {
            let (d, i) = id_in(of, p, inherit);
            (d - radius, i)
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let (d, i) = id_in(of, transform_into(*at, *rotate, *scale, p), inherit);
            (d * scale, i)
        }
        SdfNode::Instance { places, of } => places.iter().fold((f64::INFINITY, inherit), |acc, pl| {
            let (d, i) = id_in(of, v3(pl.into([p.x, p.y, p.z])), inherit);
            if d * pl.scale < acc.0 {
                (d * pl.scale, i)
            } else {
                acc
            }
        }),
        SdfNode::Deform { deform, lipschitz, of } => {
            let (d, i) = id_in(of, deform.warp(p), inherit);
            (d / lipschitz, i)
        }
        SdfNode::Sculpt { strokes, of } => {
            let (d, i) = id_in(of, p, inherit);
            (strokes.sculpt([p.x, p.y, p.z], d, smin), i)
        }
        SdfNode::Texture { tex, of } => (tex.eval(p, |q| distance(of, q)), id_in(of, p, inherit).1),
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } => id_in(of, p, inherit),
        leaf => (distance(leaf, p), inherit),
    }
}

fn slot_in(n: &SdfNode, p: Vec3d, inherit: u8) -> (f64, u8) {
    match n {
        SdfNode::Material { slot, within, of } => {
            let here = match within {
                Some(w) if distance(w, p) > 0.0 => inherit,
                _ => *slot,
            };
            slot_in(of, p, here)
        }
        SdfNode::Combine { op, blend, of } => {
            let (mut d, mut slot) = slot_in(&of[0], p, inherit);
            for c in &of[1..] {
                let (e, s) = slot_in(c, p, inherit);
                match op {
                    SdfOp::Union => {
                        if e < d {
                            slot = s;
                        }
                        d = smin(d, e, *blend);
                    }
                    SdfOp::Intersection => {
                        if e > d {
                            slot = s;
                        }
                        d = -smin(-d, -e, *blend);
                    }
                    SdfOp::Difference => d = -smin(-d, e, *blend),
                }
            }
            (d, slot)
        }
        SdfNode::Shell { thickness, of } => {
            let (d, s) = slot_in(of, p, inherit);
            (d.abs() - thickness, s)
        }
        SdfNode::Round { radius, of } => {
            let (d, s) = slot_in(of, p, inherit);
            (d - radius, s)
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let (d, s) = slot_in(of, transform_into(*at, *rotate, *scale, p), inherit);
            (d * scale, s)
        }
        SdfNode::Instance { places, of } => places.iter().fold((f64::INFINITY, inherit), |acc, pl| {
            let (d, s) = slot_in(of, v3(pl.into([p.x, p.y, p.z])), inherit);
            if d * pl.scale < acc.0 {
                (d * pl.scale, s)
            } else {
                acc
            }
        }),
        SdfNode::Deform { deform, lipschitz, of } => {
            let (d, s) = slot_in(of, deform.warp(p), inherit);
            (d / lipschitz, s)
        }
        SdfNode::Sculpt { strokes, of } => {
            let (d, s) = slot_in(of, p, inherit);
            (strokes.sculpt([p.x, p.y, p.z], d, smin), s)
        }
        SdfNode::Texture { tex, of } => (tex.eval(p, |q| distance(of, q)), slot_in(of, p, inherit).1),
        SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => slot_in(of, p, inherit),
        SdfNode::Links(l) => {
            let (d, s) = l.distance_slot(p);
            (d, if s == 0 { inherit } else { s })
        }
        SdfNode::Tubes(t) => {
            let (d, s) = t.distance_slot(p);
            (d, if s == 0 { inherit } else { s })
        }
        leaf => (distance(leaf, p), inherit),
    }
}

/// A point of the outer frame into a [`SdfNode::Transform`] (rotation in degrees).
pub fn transform_into(at: P3, rotate: P3, scale: f64, p: Vec3d) -> Vec3d {
    let r = rot(rotate);
    let d = p - Vec3d::new(at[0], at[1], at[2]);
    // The inverse rotation is the transpose.
    Vec3d::new(r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z, r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z, r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z) * (1.0 / scale)
}

/// A point of a [`SdfNode::Transform`]'s frame into its parent's: [`transform_into`] undone.
pub fn transform_out_of(at: P3, rotate: P3, scale: f64, q: P3) -> P3 {
    let r = rot(rotate);
    std::array::from_fn(|i| (r[i][0] * q[0] + r[i][1] * q[1] + r[i][2] * q[2]) * scale + at[i])
}

pub(crate) fn smin(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    // Outside the blend the nearer operand exactly (`b + (a - b)` rounds):
    // a far operand, or one culled away, leaves the value bit for bit.
    if h >= 1.0 {
        return a;
    }
    if h <= 0.0 {
        return b;
    }
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
            crate::strokes::capsule([p.x, p.y, p.z], *a, *b) - radius
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
        SdfNode::Transform { at, rotate, scale, of } => distance(of, transform_into(*at, *rotate, *scale, p)) * scale,
        SdfNode::Kernel(k) => k.field.distance(p),
        SdfNode::Mesh(m) => m.signed64(p),
        SdfNode::Links(l) => l.distance(p),
        SdfNode::Tubes(t) => t.distance(p),
        SdfNode::Sketch(s) => s.distance(p),
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => distance(of, p),
        SdfNode::Instance { places, of } => places.iter().map(|pl| distance(of, v3(pl.into([p.x, p.y, p.z]))) * pl.scale).fold(f64::INFINITY, f64::min),
        SdfNode::Deform { deform, lipschitz, of } => distance(of, deform.warp(p)) / lipschitz,
        SdfNode::Sculpt { strokes, of } => strokes.sculpt([p.x, p.y, p.z], distance(of, p), smin),
        SdfNode::Texture { tex, of } => tex.eval(p, |q| distance(of, q)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::node_bounds;

    fn ball(r: f64, at: P3) -> SdfNode {
        SdfNode::Transform { at, rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: r }) }
    }

    /// A capsule with no length is the sphere at its point, anywhere, in `distance` and `distance_many`.
    #[test]
    fn a_capsule_with_no_length_is_a_sphere() {
        let at = [1.0, 2.0, 3.0];
        let (capsule, sphere) = (SdfNode::Capsule { a: at, b: at, radius: 1.5 }, ball(1.5, at));
        let ps = lattice(&([-4.0; 3], [6.0; 3]), 5);
        let mut many = vec![0.0; ps.len()];
        distance_many(&capsule, &ps, &mut many);
        for (p, m) in ps.iter().zip(&many) {
            let want = distance(&sphere, *p);
            assert_eq!((distance(&capsule, *p), *m), (want, want), "at {p:?}");
        }
    }

    /// Points on a lattice through `b`, grown a little.
    fn lattice(b: &crate::bounds::Bounds, n: usize) -> Vec<Vec3d> {
        let mut out = Vec::new();
        let g = |i: usize, k: usize| b.0[k] - 1.0 + (b.1[k] - b.0[k] + 2.0) * i as f64 / (n - 1) as f64;
        for x in 0..n {
            for y in 0..n {
                for z in 0..n {
                    out.push(Vec3d::new(g(x, 0), g(y, 1), g(z, 2)));
                }
            }
        }
        out
    }

    fn nodes() -> Vec<SdfNode> {
        let bar = SdfNode::Box { size: [4.0, 20.0, 8.0], radius: 0.5 };
        vec![
            SdfNode::Instance { places: vec![SdfPlace { at: [10.0, 0.0, 0.0], rotate: quat_from_degrees([0.0, 45.0, 0.0]), scale: 2.0 }, SdfPlace::IDENTITY], of: Box::new(bar.clone()) },
            SdfNode::deform(SdfDeform::Twist { rate: 0.1 }, bar.clone()),
            SdfNode::deform(SdfDeform::Bend { rate: 0.05 }, SdfNode::Box { size: [30.0, 4.0, 4.0], radius: 0.0 }),
            SdfNode::deform(SdfDeform::Taper { rate: -0.03 }, bar.clone()),
            SdfNode::deform(SdfDeform::Mirror { axis: 0 }, ball(3.0, [4.0, 1.0, 0.0])),
            SdfNode::Material { slot: 2, within: None, of: Box::new(SdfNode::Bone { id: 7, rest: SdfPlace::IDENTITY, of: Box::new(SdfNode::Id { id: part_id("arm"), of: Box::new(bar) }) }) },
        ]
    }

    // An exhaustive match makes adding a node a compile error until its fixture is considered.
    fn variant(n: &SdfNode) -> usize {
        match n {
            SdfNode::Sphere { .. } => 0, SdfNode::Box { .. } => 1,
            SdfNode::Cylinder { .. } => 2, SdfNode::Torus { .. } => 3,
            SdfNode::Capsule { .. } => 4, SdfNode::Ellipsoid { .. } => 5,
            SdfNode::Gyroid { .. } => 6, SdfNode::Combine { .. } => 7,
            SdfNode::Shell { .. } => 8, SdfNode::Round { .. } => 9,
            SdfNode::Transform { .. } => 10, SdfNode::Kernel(_) => 11,
            SdfNode::Mesh(_) => 12, SdfNode::Links(_) => 13,
            SdfNode::Material { .. } => 14, SdfNode::Instance { .. } => 15,
            SdfNode::Bone { .. } => 16, SdfNode::Deform { .. } => 17,
            SdfNode::Id { .. } => 18, SdfNode::Sculpt { .. } => 19,
            SdfNode::Texture { .. } => 20, SdfNode::Tubes(_) => 21,
            SdfNode::Sketch(_) => 22,
        }
    }

    fn compiled_fixtures() -> Vec<SdfNode> {
        use crate::{links::{LinkLoop, Links}, mesh_field::{MeshData, MeshField}, strokes::{StrokeOp, Strokes}, texture::{SdfTexture, TexProject}};
        let sphere = SdfNode::Sphere { radius: 2.0 };
        let child = || Box::new(ball(2.0, [0.2, -0.3, 0.4]));
        // A real leaf kernel, with independently exercised scalar and batch dispatch.
        let kernel = SdfKernel { key: 1, field: Arc::new(Compiled::Leaf(sphere.clone())), splash: None, bounds: None, solid: None };
        let mut strokes = Strokes::new();
        strokes.push(StrokeOp::Raise, 0.8, 0.6, &[[1.5, -1.0, 0.0], [1.5, 1.0, 0.0]]);
        strokes.push(StrokeOp::Lower, 0.5, 0.7, &[[0.0, 1.8, -1.0], [0.0, 1.8, 1.0]]);
        let mut out = vec![
            sphere, SdfNode::Box { size: [3.0, 4.0, 2.0], radius: 0.2 },
            SdfNode::Cylinder { radius: 1.0, top_radius: 0.7, height: 3.0 },
            SdfNode::Cylinder { radius: 1.0, top_radius: 1.0, height: 3.0 },
            SdfNode::Torus { radius: 2.0, tube: 0.6 },
            SdfNode::Capsule { a: [-1.0, 0.0, 0.0], b: [1.0, 2.0, 0.0], radius: 0.7 },
            SdfNode::Ellipsoid { radii: [1.5, 2.0, 0.7] },
            SdfNode::Gyroid { period: 3.0, wall: 0.3 },
            SdfNode::Combine { op: SdfOp::Union, blend: 0.4, of: vec![*child(), ball(1.5, [1.0, 0.0, 0.0])] },
            SdfNode::Shell { thickness: 0.3, of: child() },
            SdfNode::Round { radius: 0.3, of: child() },
            *child(), SdfNode::Kernel(kernel.clone()),
            SdfNode::Mesh(Arc::new(MeshField::new(MeshData {
                positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
                triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            }))),
            SdfNode::Links(Arc::new(Links::new(vec![LinkLoop { pts: vec![[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]], slot: 1 }], 0.3, None))),
            SdfNode::Material { slot: 2, within: Some(Box::new(SdfNode::Sphere { radius: 1.0 })), of: child() },
            SdfNode::Instance { places: vec![SdfPlace::IDENTITY, SdfPlace { at: [1.0, 0.0, 0.0], rotate: quat_from_degrees([10.0, 30.0, -5.0]), scale: 0.7 }], of: child() },
            SdfNode::Bone { id: 2, rest: SdfPlace { at: [2.0; 3], ..SdfPlace::IDENTITY }, of: child() },
            SdfNode::deform(SdfDeform::Twist { rate: 0.2 }, *child()),
            SdfNode::Id { id: 3, of: child() },
            SdfNode::Sculpt { strokes: Arc::new(strokes), of: child() },
            SdfNode::Tubes(Arc::new(crate::tubes::Tubes::new(vec![crate::tubes::TubeCurve { pts: vec![[-1.5, 0.0, 0.0, 0.5], [0.0, 1.0, 0.5, 0.7], [1.5, 0.5, -0.5, 0.3]], slot: 1 }], 0.0))),
            SdfNode::Sketch(Arc::new(crate::sketches::SketchSolid::new(&[vec![vec![[0.5, -1.0], [2.0, -1.0], [1.5, 1.5]]]], crate::sketches::Form::Revolve { angle: 270.0 }).unwrap())),
        ];
        for project in [TexProject::Axis(0), TexProject::Triplanar] {
            out.push(SdfNode::Texture { tex: Arc::new(SdfTexture::new(kernel.clone(), project, 0.8, 0.2, 0.01, Some((-0.5, 0.5)), Some((-0.3, 0.7)), 1.0, 0.2)), of: child() });
        }
        let mut covered = [false; 23];
        for n in &out { covered[variant(n)] = true; }
        assert!(covered.into_iter().all(|v| v), "every node needs an independent fixture");
        out
    }

    fn assert_compiled_bits(root: SdfNode, points: &[Vec3d]) {
        let field = Compiled::new(&root);
        let old = Compiled::Leaf(root.clone());
        let mut batch = vec![0.0; points.len()];
        let mut reference = vec![0.0; points.len()];
        field.distance_many(points, &mut batch);
        distance_many(&root, points, &mut reference);
        for ((&p, a), b) in points.iter().zip(batch).zip(reference) {
            assert_eq!(a.to_bits(), b.to_bits(), "variant {} batch at {p:?}", variant(&root));
            assert_eq!(field.distance(p).to_bits(), distance(&root, p).to_bits(), "variant {} scalar at {p:?}", variant(&root));
            let (a, b) = (field.normal(p), old.normal(p));
            assert_eq!([a.x.to_bits(), a.y.to_bits(), a.z.to_bits()], [b.x.to_bits(), b.y.to_bits(), b.z.to_bits()], "variant {} normal at {p:?}", variant(&root));
        }
    }

    #[test]
    fn compiled_transforms_preserve_every_distance_bit() {
        // Test each node alone so a combine cannot hide a changed operand's distance.
        let small = lattice(&([-2.5; 3], [2.5; 3]), 5);
        for root in compiled_fixtures() {
            assert_compiled_bits(root.clone(), &small);
            for scale in [0.7, -1.5] {
                assert_compiled_bits(SdfNode::Transform { at: [0.2, -0.4, 0.1], rotate: [31.0, -57.0, 109.0], scale, of: Box::new(root.clone()) }, &small);
            }
        }
        let wrap = |of, rotate, scale| SdfNode::Transform { at: [1.25, -3.7, 0.125], rotate, scale, of: Box::new(of) };
        let points = lattice(&([-17.0; 3], [19.0; 3]), 9);
        for op in [SdfOp::Union, SdfOp::Difference, SdfOp::Intersection] {
            for blend in [0.0, 1.3] {
                for scale in [0.125, 1.0, 2.7, -1.5] {
                    let mut children = nodes();
                    children.push(SdfNode::Cylinder { radius: 3.0, top_radius: 1.25, height: 7.0 });
                    children.push(SdfNode::Shell { thickness: 0.3, of: Box::new(SdfNode::Round { radius: 0.15, of: Box::new(SdfNode::Torus { radius: 4.0, tube: 0.8 }) }) });
                    let root = wrap(SdfNode::Combine {
                        op, blend,
                        of: children.into_iter().enumerate().map(|(i, n)| wrap(wrap(n, [30.0, -50.0, 110.0], 0.7), [i as f64 * 17.3, -90.0, 0.01], scale)).collect(),
                    }, [-21.25, 68.75, 180.0], 1.125);
                    let shape = SdfShape { root, min: [-20.0; 3], max: [20.0; 3], materials: Vec::new() };
                    let field = shape.field();
                    let old = Compiled::Leaf(shape.root.clone());
                    let mut batch = vec![0.0; points.len()];
                    let mut reference = vec![0.0; points.len()];
                    field.distance_many(&points, &mut batch);
                    distance_many(&shape.root, &points, &mut reference);
                    assert_eq!(batch.iter().map(|d| d.to_bits()).collect::<Vec<_>>(), reference.iter().map(|d| d.to_bits()).collect::<Vec<_>>(), "batch distance bits");
                    for &p in &points {
                        assert_eq!(field.distance(p).to_bits(), distance(&shape.root, p).to_bits(), "{op:?}, blend {blend}, scale {scale}, point {p:?}");
                        let a = field.normal(p);
                        let b = old.normal(p);
                        assert_eq!([a.x.to_bits(), a.y.to_bits(), a.z.to_bits()], [b.x.to_bits(), b.y.to_bits(), b.z.to_bits()], "normal at {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn quaternion_turns_like_a_transform() {
        let deg = [30.0, -50.0, 110.0];
        let bar = SdfNode::Box { size: [4.0, 20.0, 8.0], radius: 0.0 };
        let t = SdfNode::Transform { at: [1.0, 2.0, 3.0], rotate: deg, scale: 1.5, of: Box::new(bar.clone()) };
        let i = SdfNode::Instance { places: vec![SdfPlace { at: [1.0, 2.0, 3.0], rotate: quat_from_degrees(deg), scale: 1.5 }], of: Box::new(bar) };
        for p in lattice(&node_bounds(&t).unwrap(), 7) {
            assert!((distance(&t, p) - distance(&i, p)).abs() < 1e-9);
        }
    }

    #[test]
    fn new_nodes_are_bounded_and_lipschitz() {
        for n in nodes() {
            let b = node_bounds(&n).unwrap();
            let ps = lattice(&b, 15);
            let mut many = vec![0.0; ps.len()];
            distance_many(&n, &ps, &mut many);
            for (k, p) in ps.iter().enumerate() {
                let d = distance(&n, *p);
                assert!((d - many[k]).abs() < 1e-9);
                if d <= 0.0 {
                    assert!((0..3).all(|i| [p.x, p.y, p.z][i] >= b.0[i] - 1e-9 && [p.x, p.y, p.z][i] <= b.1[i] + 1e-9), "{n:?} solid at {p:?} outside {b:?}");
                }
                // Never more than the true distance's change between neighbours.
                if k + 1 < ps.len() {
                    let q = ps[k + 1];
                    assert!((d - distance(&n, q)).abs() <= (*p - q).length() * (1.0 + 1e-6), "{n:?} at {p:?}");
                }
            }
        }
    }

    #[test]
    fn mirror_folds_the_positive_half() {
        let n = SdfNode::deform(SdfDeform::Mirror { axis: 0 }, ball(3.0, [4.0, 0.0, 0.0]));
        assert!(distance(&n, Vec3d::new(-4.0, 0.0, 0.0)) < -2.9);
        assert!(distance(&n, Vec3d::new(4.0, 0.0, 0.0)) < -2.9);
    }

    #[test]
    fn slots_follow_the_rules() {
        let a = SdfNode::Material { slot: 1, within: None, of: Box::new(ball(5.0, [0.0; 3])) };
        let b = SdfNode::Material { slot: 2, within: None, of: Box::new(ball(5.0, [8.0, 0.0, 0.0])) };
        let at = |x: f64| Vec3d::new(x, 0.0, 0.0);
        let u = SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![a.clone(), b.clone()] };
        assert_eq!(distance_slot(&u, at(-1.0)).1, 1);
        assert_eq!(distance_slot(&u, at(9.0)).1, 2);
        assert_eq!(distance_slot(&u, at(4.0)).1, 1, "a tie keeps the earlier");
        let d = SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![a.clone(), b.clone()] };
        assert_eq!(distance_slot(&d, at(3.5)).1, 1, "a cut takes the base's");
        let i = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.0, of: vec![a.clone(), b.clone()] };
        assert_eq!(distance_slot(&i, at(1.0)).1, 2);
        // The innermost wins; outside every Material, 0.
        let outer = SdfNode::Material { slot: 3, within: None, of: Box::new(SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![a, ball(2.0, [-20.0, 0.0, 0.0])] }) };
        assert_eq!(distance_slot(&outer, at(0.0)).1, 1);
        assert_eq!(distance_slot(&outer, at(-20.0)).1, 3);
        assert_eq!(distance_slot(&ball(1.0, [0.0; 3]), at(0.0)).1, 0);
        // The distance is the plain one.
        assert_eq!(distance_slot(&u, at(2.0)).0, distance(&u, at(2.0)));
    }

    #[test]
    fn ids_name_the_surface_there() {
        let body = SdfNode::Id { id: 1, of: Box::new(ball(5.0, [0.0; 3])) };
        let hole = SdfNode::Id { id: 2, of: Box::new(ball(2.0, [5.0, 0.0, 0.0])) };
        let n = SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![body, hole] };
        assert_eq!(distance_id(&n, Vec3d::new(-5.0, 0.0, 0.0)).1, Some(1));
        assert_eq!(distance_id(&n, Vec3d::new(3.0, 0.0, 0.0)).1, Some(2), "the hole's own surface");
        assert_eq!(distance_id(&ball(1.0, [0.0; 3]), Vec3d::new(1.0, 0.0, 0.0)).1, None);
    }

    #[test]
    fn a_material_within_a_region_paints_only_there() {
        let body = ball(5.0, [0.0; 3]);
        let region = ball(2.0, [5.0, 0.0, 0.0]);
        let n = SdfNode::Material { slot: 4, within: Some(Box::new(region)), of: Box::new(body.clone()) };
        for (x, slot) in [(5.0, 4), (-5.0, 0)] {
            let (d, s) = distance_slot(&n, Vec3d::new(x, 0.0, 0.0));
            assert_eq!(s, slot, "at x {x}");
            assert!((d - distance(&body, Vec3d::new(x, 0.0, 0.0))).abs() < 1e-12, "the distance is the body's");
        }
    }

    #[test]
    fn instance_keeps_its_source_key() {
        let bar = SdfNode::Box { size: [1.0; 3], radius: 0.0 };
        let one = SdfNode::Instance { places: vec![SdfPlace::IDENTITY], of: Box::new(bar.clone()) };
        let moved = SdfNode::Instance { places: vec![SdfPlace { at: [5.0, 0.0, 0.0], ..SdfPlace::IDENTITY }], of: Box::new(bar.clone()) };
        assert_ne!(one.key(), moved.key());
        let (SdfNode::Instance { of: a, .. }, SdfNode::Instance { of: b, .. }) = (&one, &moved) else { unreachable!() };
        assert_eq!(a.key(), b.key());
        assert_eq!(a.key(), bar.key());
        // Only the moved copy is dirty.
        let c = crate::bounds::changed(&one, &moved).unwrap();
        assert!(c.0[0] >= -0.5 - 1e-9 && c.1[0] <= 5.5 + 1e-9);
    }
}
