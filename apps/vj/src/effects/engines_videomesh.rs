//! Videomesh — live video mapped onto parametric 3D geometry.
//!
//! The CPU builds ONE indexed mesh at load from a shape catalogue (box,
//! sphere, torus, disc, cylinder, capsule, octahedron, star_prism, facets,
//! grid, corridor), stamped out `instances` times with per-instance ids and
//! hashes riding the vertex stream. Everything that moves per frame is
//! shader math: the DOCUMENT owns the choreography through three vertex
//! hooks (`fx_place` — per-instance position + spin angle, `fx_axis` — the
//! spin axis, `fx_scale` — per-instance scale) and the pixel look through
//! `fx_color` (sampling input0 through the baked surface uv) plus
//! `fx_backdrop` (the optional full-frame quad behind the geometry).
//!
//! `tex1` is bound as well (deck B), so a videomesh document with
//! `decks: 2` is a TWO-DECK TRANSITION: the host feeds deck A into tex0,
//! deck B into tex1 and sweeps `p3` with the crossfader, exactly the duo
//! contract (`self.deck_a(uv)` / `self.deck_b(uv)` helpers in scope).
//!
//! # Vertex channels (CubeVertex layout — documented in CONTRACT.md)
//!   geom_pos = vertex position in the shape's LOCAL frame (origin-centred)
//!   a_id     = instance id 0..instances-1
//!   normal   = surface normal, LOCAL frame
//!   a_aux    = face id (box 0..5, cylinder 0 side/1 top/2 bottom, corridor
//!              0 left/1 right/2 floor/3 ceiling, facets = facet index);
//!              -1 marks the BACKDROP quad (clip-space, screen uv)
//!   uv       = surface uv — the window into input0 (after `uv_split`)
//!   a_r0     = per-instance hash 0..1
//!   a_r1     = per-face hash 0..1
//!
//! # Document keys (`engine: "videomesh"`)
//! `shape` ("box" | "sphere" | "torus" | "disc" | "cylinder" | "capsule" |
//! "octahedron" | "star_prism" | "facets" | "grid" | "corridor"),
//! `instances` (1, ≤[`MAX_INSTANCES`]), `size` (1.6 — the shape's world
//! scale), `spread` (3.0 — default ring radius; the corridor's segment
//! pitch), `detail` (24 — tessellation), `points` (5 — star points),
//! `aspect` (1.0 — height stretch: box/grid slabs, cylinder/capsule length,
//! star thickness, corridor height), `spin` (1.0 — default tumble rate),
//! `relief` (0 — input0-luma displacement along the normal, vertex stage),
//! `uv_split` ("none" | "bands_x" | "bands_y" | "cells" — cut the video
//! into per-instance windows; bands/cells count from the image TOP-LEFT),
//! `cam` ("orbit" | "inside" | "corridor" — default orbit, corridor shape
//! defaults to the corridor rig), `fly` (1.0 camera speed), `alt` (orbit
//! eye height, auto when unset), `backdrop` (0 — 1 appends the full-frame
//! quad behind the scene, drawn through `fx_backdrop`), `decks` (1 — 2
//! marks the doc as a two-deck transition: the host binds both decks and
//! p3 is the crossfader).
//! Bindings: `p0` adds spin drive, `p1` scales relief/pump, `p2` adds edge
//! glow, `p3` = the crossfader on two-deck docs.
//! Hooks: vertex `fx_place(id, hash, t) -> vec4` (xyz position + w spin
//! angle), `fx_axis(id, hash) -> vec3`, `fx_scale(id, hash, t) -> float`;
//! pixel `fx_color(t = light drive, attr = (hash, face, edge, id),
//! content, cmix)`, `fx_backdrop(uv, t = crossfader) -> vec4`.

use super::engines::{CamPose, EngineUniforms};
use super::mesh::{FxMesh, FxRng};
use makepad_widgets::*;

/// Instance stamp cap — every instance re-emits the whole shape.
pub const MAX_INSTANCES: usize = 64;

/// Total vertex budget across all instances; `detail` (then `instances`)
/// is reduced until a build fits.
const VERT_BUDGET: usize = 150_000;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VideomeshShape {
    Box = 0,
    Sphere = 1,
    Torus = 2,
    Disc = 3,
    Cylinder = 4,
    Capsule = 5,
    Octahedron = 6,
    StarPrism = 7,
    Facets = 8,
    Grid = 9,
    Corridor = 10,
}

impl VideomeshShape {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "box" | "cube" => Self::Box,
            "sphere" | "ball" => Self::Sphere,
            "torus" | "ring" => Self::Torus,
            "disc" | "disk" => Self::Disc,
            "cylinder" | "drum" => Self::Cylinder,
            "capsule" | "pill" => Self::Capsule,
            "octahedron" | "octa" => Self::Octahedron,
            "star_prism" | "star" => Self::StarPrism,
            "facets" | "gem" => Self::Facets,
            "grid" | "plane" | "slat" => Self::Grid,
            "corridor" | "hall" => Self::Corridor,
            _ => return None,
        })
    }
}

/// How the video is cut into per-instance uv windows. Bands and cells
/// count from the image's TOP-LEFT (uv v = 0 is the top of the picture).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum UvSplit {
    None,
    /// Instance i carries the vertical strip u ∈ [i/n, (i+1)/n].
    BandsX,
    /// Instance i carries the horizontal band v ∈ [i/n, (i+1)/n].
    BandsY,
    /// Row-major cells on a ceil(sqrt(n)) grid.
    Cells,
}

impl UvSplit {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" | "full" => Self::None,
            "bands_x" | "columns" => Self::BandsX,
            "bands_y" | "bands" | "rows" => Self::BandsY,
            "cells" | "mosaic" => Self::Cells,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VideomeshCam {
    Orbit,
    Inside,
    Corridor,
}

impl VideomeshCam {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "orbit" => Self::Orbit,
            "inside" | "interior" => Self::Inside,
            "corridor" | "fly" => Self::Corridor,
            _ => return None,
        })
    }
}

pub struct VideomeshConfig {
    pub shape: VideomeshShape,
    pub instances: usize,
    /// World scale of one shape.
    pub size: f32,
    /// Default ring radius (multi-instance) / corridor segment pitch.
    pub spread: f32,
    /// Tessellation (sphere/torus/capsule segments, grid subdiv).
    pub detail: usize,
    /// Star points.
    pub points: usize,
    /// Height stretch (box/grid slabs, cylinder/capsule length, star and
    /// corridor thickness).
    pub aspect: f32,
    /// Default tumble rate multiplier (shader `flow.x`).
    pub spin: f32,
    /// input0-luma displacement along the normal (vertex stage).
    pub relief: f32,
    pub split: UvSplit,
    /// None = auto (corridor shape flies the corridor rig, else orbit).
    pub cam: Option<VideomeshCam>,
    /// Camera speed multiplier.
    pub fly: f32,
    /// Orbit eye height (NaN = auto).
    pub alt: f32,
    /// > 0.5 appends the full-frame backdrop quad (`fx_backdrop`).
    pub backdrop: f32,
    /// 2 = two-deck transition doc: host feeds both decks, p3 = crossfader.
    pub decks: usize,
    pub seed: u64,
}

impl Default for VideomeshConfig {
    fn default() -> Self {
        Self {
            shape: VideomeshShape::Box,
            instances: 1,
            size: 1.6,
            spread: 3.0,
            detail: 24,
            points: 5,
            aspect: 1.0,
            spin: 1.0,
            relief: 0.0,
            split: UvSplit::None,
            cam: None,
            fly: 1.0,
            alt: f32::NAN,
            backdrop: 0.0,
            decks: 1,
            seed: 7,
        }
    }
}

/// A per-instance window into the video: `map` sends shape-local uv 0..1
/// into the instance's slice of the image.
#[derive(Clone, Copy)]
struct UvWin {
    o: Vec2f,
    s: Vec2f,
}

impl UvWin {
    fn full() -> Self {
        Self { o: vec2f(0.0, 0.0), s: vec2f(1.0, 1.0) }
    }
    #[inline]
    fn map(&self, u: f32, v: f32) -> Vec2f {
        vec2f(self.o.x + u * self.s.x, self.o.y + v * self.s.y)
    }
}

pub struct VideomeshEngine {
    pub cfg: VideomeshConfig,
    pub(crate) built: bool,
    /// Effective tessellation after the vertex-budget clamp (set at new).
    pub detail: usize,
    /// Effective instance count after the budget clamp.
    pub instances: usize,
}

impl VideomeshEngine {
    pub fn new(cfg: VideomeshConfig) -> Self {
        let mut instances = cfg.instances.clamp(1, MAX_INSTANCES);
        let mut detail = cfg.detail.clamp(4, 64);
        let points = cfg.points.clamp(3, 12);
        // Budget clamp: shrink detail first, then instances, so a doc that
        // asks for too much degrades instead of stalling.
        while instances * Self::est_verts(cfg.shape, detail, points) > VERT_BUDGET {
            if detail > 6 {
                detail = (detail * 3 / 4).max(6);
            } else if instances > 1 {
                instances -= 1;
            } else {
                break;
            }
        }
        Self { cfg, built: false, detail, instances }
    }

    fn san(v: f32, d: f32) -> f32 {
        if v.is_finite() {
            v
        } else {
            d
        }
    }

    /// Rough per-instance vertex count, for the budget clamp.
    fn est_verts(shape: VideomeshShape, detail: usize, points: usize) -> usize {
        match shape {
            VideomeshShape::Box => 24,
            VideomeshShape::Sphere => (detail + 1) * (detail / 2 + 1),
            VideomeshShape::Torus => (detail + 1) * (detail * 2 / 3 + 1),
            VideomeshShape::Disc => (detail + 2) * 3,
            VideomeshShape::Cylinder => (detail + 1) * 2 + (detail + 2) * 2 * 3,
            VideomeshShape::Capsule => (detail + 1) * (detail + 1),
            VideomeshShape::Octahedron => 24,
            VideomeshShape::StarPrism => points * 20,
            VideomeshShape::Facets => 8 * 9 * 3,
            VideomeshShape::Grid => {
                let g = (detail / 4).clamp(1, 24);
                (g + 1) * (g + 1)
            }
            VideomeshShape::Corridor => 16,
        }
    }

    fn size(&self) -> f32 {
        Self::san(self.cfg.size, 1.6).clamp(0.05, 60.0)
    }

    fn spread(&self) -> f32 {
        Self::san(self.cfg.spread, 3.0).clamp(0.0, 80.0)
    }

    fn aspect(&self) -> f32 {
        Self::san(self.cfg.aspect, 1.0).clamp(0.02, 4.0)
    }

    /// Corridor segment pitch — camera and build MUST agree on it.
    fn pitch(&self) -> f32 {
        self.spread().max(self.size() * 0.6).max(0.5)
    }

    fn cam_mode(&self) -> VideomeshCam {
        self.cfg.cam.unwrap_or(match self.cfg.shape {
            VideomeshShape::Corridor => VideomeshCam::Corridor,
            _ => VideomeshCam::Orbit,
        })
    }

    /// The instance's window into the video (see [`UvSplit`]).
    fn window(&self, inst: usize) -> UvWin {
        let n = self.instances.max(1);
        match self.cfg.split {
            UvSplit::None => UvWin::full(),
            UvSplit::BandsX => UvWin {
                o: vec2f(inst as f32 / n as f32, 0.0),
                s: vec2f(1.0 / n as f32, 1.0),
            },
            UvSplit::BandsY => UvWin {
                o: vec2f(0.0, inst as f32 / n as f32),
                s: vec2f(1.0, 1.0 / n as f32),
            },
            UvSplit::Cells => {
                let gw = (n as f32).sqrt().ceil().max(1.0) as usize;
                let gh = n.div_ceil(gw);
                let gx = inst % gw;
                let gy = inst / gw;
                UvWin {
                    o: vec2f(gx as f32 / gw as f32, gy as f32 / gh as f32),
                    s: vec2f(1.0 / gw as f32, 1.0 / gh as f32),
                }
            }
        }
    }

    /// One quad, all channels supplied; uv already windowed.
    #[allow(clippy::too_many_arguments)]
    fn quad(
        mesh: &mut FxMesh,
        id: f32,
        face: f32,
        hash: f32,
        fhash: f32,
        p: [Vec3f; 4],
        n: [Vec3f; 4],
        uv: [Vec2f; 4],
    ) {
        let mut ids = [0u32; 4];
        for k in 0..4 {
            ids[k] = mesh.push_vert(p[k], id, n[k], face, uv[k], hash, fhash);
        }
        mesh.push_quad(ids[0], ids[1], ids[2], ids[3]);
    }

    /// One triangle (flat shading), uv already windowed.
    #[allow(clippy::too_many_arguments)]
    fn tri(
        mesh: &mut FxMesh,
        id: f32,
        face: f32,
        hash: f32,
        fhash: f32,
        p: [Vec3f; 3],
        n: Vec3f,
        uv: [Vec2f; 3],
    ) {
        let mut ids = [0u32; 3];
        for k in 0..3 {
            ids[k] = mesh.push_vert(p[k], id, n, face, uv[k], hash, fhash);
        }
        mesh.push_tri(ids[0], ids[1], ids[2]);
    }

    fn emit_box(mesh: &mut FxMesh, id: f32, hash: f32, rng: &mut FxRng, s: f32, aspect: f32, w: UvWin) {
        let hx = s * 0.5;
        let hy = s * 0.5 * aspect;
        let hz = s * 0.5;
        // (face, normal, corners a..d with uv (0,0)(1,0)(1,1)(0,1) — v = 0
        // at the TOP of the face seen from outside).
        let faces: [(f32, [f32; 3], [[f32; 3]; 4]); 6] = [
            (0.0, [0.0, 0.0, 1.0],
             [[-hx, hy, hz], [hx, hy, hz], [hx, -hy, hz], [-hx, -hy, hz]]),
            (1.0, [0.0, 0.0, -1.0],
             [[hx, hy, -hz], [-hx, hy, -hz], [-hx, -hy, -hz], [hx, -hy, -hz]]),
            (2.0, [1.0, 0.0, 0.0],
             [[hx, hy, hz], [hx, hy, -hz], [hx, -hy, -hz], [hx, -hy, hz]]),
            (3.0, [-1.0, 0.0, 0.0],
             [[-hx, hy, -hz], [-hx, hy, hz], [-hx, -hy, hz], [-hx, -hy, -hz]]),
            (4.0, [0.0, 1.0, 0.0],
             [[-hx, hy, -hz], [hx, hy, -hz], [hx, hy, hz], [-hx, hy, hz]]),
            (5.0, [0.0, -1.0, 0.0],
             [[-hx, -hy, hz], [hx, -hy, hz], [hx, -hy, -hz], [-hx, -hy, -hz]]),
        ];
        let uvs = [w.map(0.0, 0.0), w.map(1.0, 0.0), w.map(1.0, 1.0), w.map(0.0, 1.0)];
        for (face, n, c) in &faces {
            let fh = rng.next_f32();
            let nn = vec3f(n[0], n[1], n[2]);
            Self::quad(
                mesh,
                id,
                *face,
                hash,
                fh,
                [
                    vec3f(c[0][0], c[0][1], c[0][2]),
                    vec3f(c[1][0], c[1][1], c[1][2]),
                    vec3f(c[2][0], c[2][1], c[2][2]),
                    vec3f(c[3][0], c[3][1], c[3][2]),
                ],
                [nn; 4],
                uvs,
            );
        }
    }

    fn emit_sphere(mesh: &mut FxMesh, id: f32, hash: f32, fhash: f32, detail: usize, r: f32, w: UvWin) {
        let seg = detail.clamp(8, 64);
        let rings = (detail / 2).clamp(6, 32);
        let mut rows: Vec<Vec<u32>> = Vec::with_capacity(rings + 1);
        for j in 0..=rings {
            let v = j as f32 / rings as f32;
            let theta = v * std::f32::consts::PI;
            let (st, ct) = theta.sin_cos();
            let mut row = Vec::with_capacity(seg + 1);
            for i in 0..=seg {
                let u = i as f32 / seg as f32;
                let phi = u * std::f32::consts::TAU;
                let (sp, cp) = phi.sin_cos();
                let n = vec3f(sp * st, ct, cp * st);
                row.push(mesh.push_vert(n * r, id, n, 0.0, w.map(u, v), hash, fhash));
            }
            rows.push(row);
        }
        for j in 0..rings {
            for i in 0..seg {
                mesh.push_quad(rows[j][i], rows[j][i + 1], rows[j + 1][i + 1], rows[j + 1][i]);
            }
        }
    }

    fn emit_torus(mesh: &mut FxMesh, id: f32, hash: f32, fhash: f32, detail: usize, s: f32, w: UvWin) {
        let seg = detail.clamp(8, 64);
        let tub = (detail * 2 / 3).clamp(6, 32);
        let rr = s * 0.55;
        let tube = s * 0.24;
        let mut rows: Vec<Vec<u32>> = Vec::with_capacity(seg + 1);
        for i in 0..=seg {
            let u = i as f32 / seg as f32;
            let phi = u * std::f32::consts::TAU;
            let (sp, cp) = phi.sin_cos();
            let center = vec3f(sp * rr, 0.0, cp * rr);
            let mut row = Vec::with_capacity(tub + 1);
            for j in 0..=tub {
                let v = j as f32 / tub as f32;
                let psi = v * std::f32::consts::TAU;
                let (ss, cs) = psi.sin_cos();
                let n = vec3f(sp * cs, ss, cp * cs);
                row.push(mesh.push_vert(center + n * tube, id, n, 0.0, w.map(u, v), hash, fhash));
            }
            rows.push(row);
        }
        for i in 0..seg {
            for j in 0..tub {
                mesh.push_quad(rows[i][j], rows[i + 1][j], rows[i + 1][j + 1], rows[i][j + 1]);
            }
        }
    }

    fn emit_disc(mesh: &mut FxMesh, id: f32, hash: f32, fhash: f32, detail: usize, s: f32, w: UvWin) {
        let seg = detail.clamp(8, 64);
        let r = s * 0.7;
        let n = vec3f(0.0, 0.0, 1.0);
        let cid = mesh.push_vert(vec3f(0.0, 0.0, 0.0), id, n, 0.0, w.map(0.5, 0.5), hash, fhash);
        let mut ring = Vec::with_capacity(seg + 1);
        for i in 0..=seg {
            let a = i as f32 / seg as f32 * std::f32::consts::TAU;
            let (sa, ca) = a.sin_cos();
            let p = vec3f(sa * r, ca * r, 0.0);
            // Planar map, v = 0 at the top (+y).
            ring.push(mesh.push_vert(
                p,
                id,
                n,
                0.0,
                w.map(0.5 + sa * 0.5, 0.5 - ca * 0.5),
                hash,
                fhash,
            ));
        }
        for i in 0..seg {
            mesh.push_tri(cid, ring[i], ring[i + 1]);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_cylinder(
        mesh: &mut FxMesh,
        id: f32,
        hash: f32,
        rng: &mut FxRng,
        detail: usize,
        s: f32,
        aspect: f32,
        w: UvWin,
    ) {
        let seg = detail.clamp(8, 64);
        let r = s * 0.42;
        let hh = s * 0.55 * aspect;
        let side_h = rng.next_f32();
        let mut top = Vec::with_capacity(seg + 1);
        let mut bot = Vec::with_capacity(seg + 1);
        for i in 0..=seg {
            let u = i as f32 / seg as f32;
            let a = u * std::f32::consts::TAU;
            let (sa, ca) = a.sin_cos();
            let n = vec3f(sa, 0.0, ca);
            top.push(mesh.push_vert(vec3f(sa * r, hh, ca * r), id, n, 0.0, w.map(u, 0.0), hash, side_h));
            bot.push(mesh.push_vert(vec3f(sa * r, 0.0 - hh, ca * r), id, n, 0.0, w.map(u, 1.0), hash, side_h));
        }
        for i in 0..seg {
            mesh.push_quad(top[i], top[i + 1], bot[i + 1], bot[i]);
        }
        // Caps: planar uv fans.
        for (face, y, ny) in [(1.0f32, hh, 1.0f32), (2.0, 0.0 - hh, -1.0)] {
            let fh = rng.next_f32();
            let n = vec3f(0.0, ny, 0.0);
            let cid = mesh.push_vert(vec3f(0.0, y, 0.0), id, n, face, w.map(0.5, 0.5), hash, fh);
            let mut ring = Vec::with_capacity(seg + 1);
            for i in 0..=seg {
                let a = i as f32 / seg as f32 * std::f32::consts::TAU;
                let (sa, ca) = a.sin_cos();
                ring.push(mesh.push_vert(
                    vec3f(sa * r, y, ca * r),
                    id,
                    n,
                    face,
                    w.map(0.5 + sa * 0.5, 0.5 - ca * 0.5),
                    hash,
                    fh,
                ));
            }
            for i in 0..seg {
                mesh.push_tri(cid, ring[i], ring[i + 1]);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_capsule(
        mesh: &mut FxMesh,
        id: f32,
        hash: f32,
        fhash: f32,
        detail: usize,
        s: f32,
        aspect: f32,
        w: UvWin,
    ) {
        let seg = detail.clamp(8, 64);
        let rings = detail.clamp(8, 48);
        let r = s * 0.4;
        let ch = s * 0.5 * aspect;
        let mut rows: Vec<Vec<u32>> = Vec::with_capacity(rings + 1);
        for j in 0..=rings {
            let v = j as f32 / rings as f32;
            // 0..0.35 top hemisphere, 0.35..0.65 cylinder, 0.65..1 bottom.
            let (y, rr, ny_bias) = if v < 0.35 {
                let th = v / 0.35 * std::f32::consts::FRAC_PI_2;
                (ch + r * th.cos(), r * th.sin(), th.cos())
            } else if v > 0.65 {
                let th = (v - 0.65) / 0.35 * std::f32::consts::FRAC_PI_2;
                (0.0 - ch - r * th.sin(), r * th.cos(), 0.0 - th.sin())
            } else {
                (ch - (v - 0.35) / 0.30 * 2.0 * ch, r, 0.0)
            };
            let mut row = Vec::with_capacity(seg + 1);
            for i in 0..=seg {
                let u = i as f32 / seg as f32;
                let a = u * std::f32::consts::TAU;
                let (sa, ca) = a.sin_cos();
                let lat = (1.0 - ny_bias * ny_bias).max(0.0).sqrt();
                let n = vec3f(sa * lat, ny_bias, ca * lat);
                row.push(mesh.push_vert(vec3f(sa * rr, y, ca * rr), id, n, 0.0, w.map(u, v), hash, fhash));
            }
            rows.push(row);
        }
        for j in 0..rings {
            for i in 0..seg {
                mesh.push_quad(rows[j][i], rows[j][i + 1], rows[j + 1][i + 1], rows[j + 1][i]);
            }
        }
    }

    fn emit_octahedron(mesh: &mut FxMesh, id: f32, hash: f32, rng: &mut FxRng, s: f32, w: UvWin) {
        let px = vec3f(s, 0.0, 0.0);
        let nx = vec3f(0.0 - s, 0.0, 0.0);
        let py = vec3f(0.0, s, 0.0);
        let ny = vec3f(0.0, 0.0 - s, 0.0);
        let pz = vec3f(0.0, 0.0, s);
        let nz = vec3f(0.0, 0.0, 0.0 - s);
        // (apex, base a, base b) — apex carries uv (0.5, 0.08) so every
        // face reads the picture upright-ish.
        let faces = [
            (py, pz, px),
            (py, px, nz),
            (py, nz, nx),
            (py, nx, pz),
            (ny, px, pz),
            (ny, nz, px),
            (ny, nx, nz),
            (ny, pz, nx),
        ];
        for (k, (a, b, c)) in faces.iter().enumerate() {
            let fh = rng.next_f32();
            let e1 = *b - *a;
            let e2 = *c - *a;
            let mut n = vec3f(
                e1.y * e2.z - e1.z * e2.y,
                e1.z * e2.x - e1.x * e2.z,
                e1.x * e2.y - e1.y * e2.x,
            );
            let nl = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1e-5);
            n = n / nl;
            // Flat outward normal (flip if it points inward).
            let ctr = (*a + *b + *c) * (1.0 / 3.0);
            if n.x * ctr.x + n.y * ctr.y + n.z * ctr.z < 0.0 {
                n = vec3f(0.0 - n.x, 0.0 - n.y, 0.0 - n.z);
            }
            Self::tri(
                mesh,
                id,
                k as f32,
                hash,
                fh,
                [*a, *b, *c],
                n,
                [w.map(0.5, 0.08), w.map(0.08, 0.92), w.map(0.92, 0.92)],
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_star_prism(
        mesh: &mut FxMesh,
        id: f32,
        hash: f32,
        rng: &mut FxRng,
        points: usize,
        s: f32,
        aspect: f32,
        w: UvWin,
    ) {
        let outer = s * 0.7;
        let inner = s * 0.30;
        let ht = s * 0.16 * aspect;
        let n2 = points * 2;
        // Outline in the xy plane, first point straight up.
        let mut outline = Vec::with_capacity(n2);
        for k in 0..n2 {
            let r = if k % 2 == 0 { outer } else { inner };
            let a = k as f32 / n2 as f32 * std::f32::consts::TAU;
            outline.push(vec2f(a.sin() * r, a.cos() * r));
        }
        // Front (+z, face 0) and back (-z, face 1): planar-uv fans.
        for (face, z, nz) in [(0.0f32, ht, 1.0f32), (1.0, 0.0 - ht, -1.0)] {
            let fh = rng.next_f32();
            let n = vec3f(0.0, 0.0, nz);
            let cid = mesh.push_vert(vec3f(0.0, 0.0, z), id, n, face, w.map(0.5, 0.5), hash, fh);
            let mut ring = Vec::with_capacity(n2 + 1);
            for k in 0..=n2 {
                let o = outline[k % n2];
                ring.push(mesh.push_vert(
                    vec3f(o.x, o.y, z),
                    id,
                    n,
                    face,
                    w.map(0.5 + o.x / (outer * 2.0), 0.5 - o.y / (outer * 2.0)),
                    hash,
                    fh,
                ));
            }
            for k in 0..n2 {
                mesh.push_tri(cid, ring[k], ring[k + 1]);
            }
        }
        // Sides (face 2): one quad per outline edge, uv along the edge.
        for k in 0..n2 {
            let a = outline[k];
            let b = outline[(k + 1) % n2];
            let e = vec2f(b.x - a.x, b.y - a.y);
            let el = (e.x * e.x + e.y * e.y).sqrt().max(1e-5);
            // Outward edge normal in xy.
            let mut n = vec3f(e.y / el, 0.0 - e.x / el, 0.0);
            let mid = vec2f((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
            if n.x * mid.x + n.y * mid.y < 0.0 {
                n = vec3f(0.0 - n.x, 0.0 - n.y, 0.0);
            }
            let fh = rng.next_f32();
            Self::quad(
                mesh,
                id,
                2.0,
                hash,
                fh,
                [
                    vec3f(a.x, a.y, ht),
                    vec3f(b.x, b.y, ht),
                    vec3f(b.x, b.y, 0.0 - ht),
                    vec3f(a.x, a.y, 0.0 - ht),
                ],
                [n; 4],
                [w.map(0.0, 0.0), w.map(1.0, 0.0), w.map(1.0, 1.0), w.map(0.0, 1.0)],
            );
        }
    }

    /// A chunky gem: octahedron faces subdivided, every facet flat with a
    /// jittered radius; uv = a front planar projection so the picture
    /// reads across the whole stone.
    fn emit_facets(mesh: &mut FxMesh, id: f32, hash: f32, rng: &mut FxRng, s: f32, w: UvWin) {
        let f = 3usize; // 8 * f^2 = 72 facets
        let base = s * 0.62;
        let px = vec3f(1.0, 0.0, 0.0);
        let nx = vec3f(-1.0, 0.0, 0.0);
        let py = vec3f(0.0, 1.0, 0.0);
        let ny = vec3f(0.0, -1.0, 0.0);
        let pz = vec3f(0.0, 0.0, 1.0);
        let nz = vec3f(0.0, 0.0, -1.0);
        let faces = [
            (py, pz, px),
            (py, px, nz),
            (py, nz, nx),
            (py, nx, pz),
            (ny, px, pz),
            (ny, nz, px),
            (ny, nx, nz),
            (ny, pz, nx),
        ];
        let norm = |v: Vec3f| {
            let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-5);
            v / l
        };
        let mut facet = 0.0f32;
        for (a, b, c) in faces {
            // Barycentric subdivision of the octant triangle: lattice point
            // L(r, i) = a + (b-a)·(r-i)/f + (c-a)·i/f, 0 ≤ i ≤ r ≤ f.
            let at = |r: usize, i: usize| {
                norm(a + (b - a) * ((r - i) as f32 / f as f32) + (c - a) * (i as f32 / f as f32))
            };
            for row in 0..f {
                for col in 0..(2 * row + 1) {
                    let up = col % 2 == 0;
                    let i = col / 2;
                    let (p0, p1, p2) = if up {
                        (at(row, i), at(row + 1, i), at(row + 1, i + 1))
                    } else {
                        (at(row, i), at(row, i + 1), at(row + 1, i + 1))
                    };
                    let fh = rng.next_f32();
                    let r = base * (0.84 + 0.30 * fh);
                    let (q0, q1, q2) = (p0 * r, p1 * r, p2 * r);
                    let e1 = q1 - q0;
                    let e2 = q2 - q0;
                    let mut n = vec3f(
                        e1.y * e2.z - e1.z * e2.y,
                        e1.z * e2.x - e1.x * e2.z,
                        e1.x * e2.y - e1.y * e2.x,
                    );
                    let nl = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1e-5);
                    n = n / nl;
                    let ctr = (q0 + q1 + q2) * (1.0 / 3.0);
                    if n.x * ctr.x + n.y * ctr.y + n.z * ctr.z < 0.0 {
                        n = vec3f(0.0 - n.x, 0.0 - n.y, 0.0 - n.z);
                    }
                    let uvp = |p: Vec3f| {
                        w.map(
                            (0.5 + p.x / (base * 2.6)).clamp(0.0, 1.0),
                            (0.5 - p.y / (base * 2.6)).clamp(0.0, 1.0),
                        )
                    };
                    Self::tri(
                        mesh,
                        id,
                        facet,
                        hash,
                        fh,
                        [q0, q1, q2],
                        n,
                        [uvp(q0), uvp(q1), uvp(q2)],
                    );
                    facet += 1.0;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_grid(
        mesh: &mut FxMesh,
        id: f32,
        hash: f32,
        fhash: f32,
        detail: usize,
        s: f32,
        aspect: f32,
        w: UvWin,
    ) {
        let g = (detail / 4).clamp(1, 24);
        let hw = s * 0.9;
        let hh = s * 0.9 * aspect;
        let n = vec3f(0.0, 0.0, 1.0);
        let mut rows: Vec<u32> = Vec::with_capacity((g + 1) * (g + 1));
        for j in 0..=g {
            let v = j as f32 / g as f32;
            for i in 0..=g {
                let u = i as f32 / g as f32;
                // v = 0 at the TOP (+y): the picture stands upright.
                rows.push(mesh.push_vert(
                    vec3f((u - 0.5) * 2.0 * hw, (0.5 - v) * 2.0 * hh, 0.0),
                    id,
                    n,
                    0.0,
                    w.map(u, v),
                    hash,
                    fhash,
                ));
            }
        }
        for j in 0..g {
            for i in 0..g {
                let a = rows[j * (g + 1) + i];
                let b = rows[j * (g + 1) + i + 1];
                let c = rows[(j + 1) * (g + 1) + i + 1];
                let d = rows[(j + 1) * (g + 1) + i];
                mesh.push_quad(a, b, c, d);
            }
        }
    }

    /// One corridor SEGMENT per instance: 4 inward-facing walls spanning
    /// z ∈ [pitch − id·pitch, −id·pitch] — segment 0 starts behind the
    /// camera loop, identical walls every pitch so the flight wraps
    /// seamlessly (the camera loops one pitch, fog hides the far end).
    #[allow(clippy::too_many_arguments)]
    fn emit_corridor(
        mesh: &mut FxMesh,
        inst: usize,
        id: f32,
        hash: f32,
        rng: &mut FxRng,
        s: f32,
        aspect: f32,
        pitch: f32,
        w: UvWin,
    ) {
        let hw = s * 0.8;
        let hh = s * 0.8 * aspect;
        let z1 = pitch - inst as f32 * pitch;
        let z0 = z1 - pitch;
        let uvs = [w.map(0.0, 0.0), w.map(1.0, 0.0), w.map(1.0, 1.0), w.map(0.0, 1.0)];
        // Left wall (face 0, normal +x): u runs down the flight (-z).
        let fh0 = rng.next_f32();
        Self::quad(
            mesh, id, 0.0, hash, fh0,
            [
                vec3f(0.0 - hw, hh, z1),
                vec3f(0.0 - hw, hh, z0),
                vec3f(0.0 - hw, 0.0 - hh, z0),
                vec3f(0.0 - hw, 0.0 - hh, z1),
            ],
            [vec3f(1.0, 0.0, 0.0); 4],
            uvs,
        );
        // Right wall (face 1, normal -x): mirrored so the picture reads.
        let fh1 = rng.next_f32();
        Self::quad(
            mesh, id, 1.0, hash, fh1,
            [
                vec3f(hw, hh, z0),
                vec3f(hw, hh, z1),
                vec3f(hw, 0.0 - hh, z1),
                vec3f(hw, 0.0 - hh, z0),
            ],
            [vec3f(-1.0, 0.0, 0.0); 4],
            uvs,
        );
        // Floor (face 2, normal +y).
        let fh2 = rng.next_f32();
        Self::quad(
            mesh, id, 2.0, hash, fh2,
            [
                vec3f(0.0 - hw, 0.0 - hh, z1),
                vec3f(hw, 0.0 - hh, z1),
                vec3f(hw, 0.0 - hh, z0),
                vec3f(0.0 - hw, 0.0 - hh, z0),
            ],
            [vec3f(0.0, 1.0, 0.0); 4],
            uvs,
        );
        // Ceiling (face 3, normal -y).
        let fh3 = rng.next_f32();
        Self::quad(
            mesh, id, 3.0, hash, fh3,
            [
                vec3f(0.0 - hw, hh, z0),
                vec3f(hw, hh, z0),
                vec3f(hw, hh, z1),
                vec3f(0.0 - hw, hh, z1),
            ],
            [vec3f(0.0, -1.0, 0.0); 4],
            uvs,
        );
    }

    pub(crate) fn build(&mut self, mesh: &mut FxMesh) {
        let s = self.size();
        let aspect = self.aspect();
        let pitch = self.pitch();
        let points = self.cfg.points.clamp(3, 12);
        let mut rng = FxRng::new(self.cfg.seed);
        for inst in 0..self.instances {
            let id = inst as f32;
            let hash = rng.next_f32();
            let fhash = rng.next_f32();
            let w = self.window(inst);
            match self.cfg.shape {
                VideomeshShape::Box => Self::emit_box(mesh, id, hash, &mut rng, s, aspect, w),
                VideomeshShape::Sphere => {
                    Self::emit_sphere(mesh, id, hash, fhash, self.detail, s * 0.6, w)
                }
                VideomeshShape::Torus => Self::emit_torus(mesh, id, hash, fhash, self.detail, s, w),
                VideomeshShape::Disc => Self::emit_disc(mesh, id, hash, fhash, self.detail, s, w),
                VideomeshShape::Cylinder => {
                    Self::emit_cylinder(mesh, id, hash, &mut rng, self.detail, s, aspect, w)
                }
                VideomeshShape::Capsule => {
                    Self::emit_capsule(mesh, id, hash, fhash, self.detail, s, aspect, w)
                }
                VideomeshShape::Octahedron => Self::emit_octahedron(mesh, id, hash, &mut rng, s, w),
                VideomeshShape::StarPrism => {
                    Self::emit_star_prism(mesh, id, hash, &mut rng, points, s, aspect, w)
                }
                VideomeshShape::Facets => Self::emit_facets(mesh, id, hash, &mut rng, s, w),
                VideomeshShape::Grid => {
                    Self::emit_grid(mesh, id, hash, fhash, self.detail, s, aspect, w)
                }
                VideomeshShape::Corridor => {
                    Self::emit_corridor(mesh, inst, id, hash, &mut rng, s, aspect, pitch, w)
                }
            }
        }
        // The optional full-frame backdrop (a_aux = -1): a clip-space quad
        // at far depth, drawn through the doc's `fx_backdrop` hook — how a
        // two-deck videomesh transition keeps both fader ends exact.
        if self.cfg.backdrop > 0.5 {
            let id = self.instances as f32;
            let n = vec3f(0.0, 0.0, 1.0);
            let a = mesh.push_vert(vec3f(-1.0, -1.0, 0.0), id, n, -1.0, vec2f(0.0, 1.0), 0.0, 0.0);
            let b = mesh.push_vert(vec3f(1.0, -1.0, 0.0), id, n, -1.0, vec2f(1.0, 1.0), 0.0, 0.0);
            let c = mesh.push_vert(vec3f(1.0, 1.0, 0.0), id, n, -1.0, vec2f(1.0, 0.0), 0.0, 0.0);
            let d = mesh.push_vert(vec3f(-1.0, 1.0, 0.0), id, n, -1.0, vec2f(0.0, 0.0), 0.0, 0.0);
            mesh.push_quad(a, b, c, d);
        }
    }

    /// Engine-authored camera (the tiles/city law: the doc's cam_* keys are
    /// ignored; `cam`/`fly`/`alt` steer it). Every pose is a bounded
    /// function of time — orbit angles feed sin/cos, the corridor flight
    /// loops one segment pitch under `fract`.
    pub fn camera(&self, time: f32) -> CamPose {
        let fly = Self::san(self.cfg.fly, 1.0).clamp(0.05, 8.0);
        let s = self.size();
        let spread = self.spread();
        match self.cam_mode() {
            VideomeshCam::Corridor => {
                let pitch = self.pitch();
                let speed = fly * pitch * 0.4;
                let zloop = (time * speed / pitch).fract() * pitch;
                let sway = (time * 0.5).sin() * s * 0.06;
                let sway_y = (time * 0.37).sin() * s * 0.05;
                CamPose {
                    eye: vec3f(sway, sway_y, 0.0 - zloop),
                    target: vec3f(sway * 0.3, sway_y * 0.3, 0.0 - zloop - pitch * 2.0),
                    fov: 68.0,
                }
            }
            VideomeshCam::Inside => {
                let a = time * 0.16 * fly;
                let eye = vec3f(
                    (a * 0.63).sin() * s * 0.08,
                    (a * 0.41).sin() * s * 0.06,
                    0.0,
                );
                CamPose {
                    eye,
                    target: eye + vec3f(a.sin(), (a * 0.57).sin() * 0.35, a.cos()),
                    fov: 74.0,
                }
            }
            VideomeshCam::Orbit => {
                // A stretched shape (capsule/cylinder/slab, aspect > 1) is
                // longer than `size` — grow the reach with it or the orbit
                // sits inside the geometry.
                let reach = s * 0.75 * self.aspect().max(1.0)
                    + if self.instances > 1 { spread } else { 0.0 };
                let dist = (reach * 2.3).max(1.5);
                let alt = if self.cfg.alt.is_finite() { self.cfg.alt } else { reach * 0.45 };
                let a = time * 0.21 * fly;
                let bob = (time * 0.13).sin() * reach * 0.08;
                CamPose {
                    eye: vec3f(a.sin() * dist, alt + bob, a.cos() * dist),
                    target: vec3f(0.0, 0.0, 0.0),
                    fov: 50.0,
                }
            }
        }
    }

    pub fn uniforms(&self) -> EngineUniforms {
        let san = Self::san;
        EngineUniforms {
            shape: vec4(
                self.cfg.shape as i32 as f32,
                self.instances as f32,
                self.size(),
                self.spread(),
            ),
            flow: vec4(
                san(self.cfg.spin, 1.0).clamp(0.0, 12.0),
                san(self.cfg.relief, 0.0).clamp(0.0, 4.0),
                // Split kind, so the pixel stage can reconstruct the
                // shape-LOCAL uv (window edges) from the windowed uv.
                match self.cfg.split {
                    UvSplit::None => 0.0,
                    UvSplit::BandsX => 1.0,
                    UvSplit::BandsY => 2.0,
                    UvSplit::Cells => 3.0,
                },
                self.aspect(),
            ),
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom

    // -----------------------------------------------------------------------
    // Videomesh: opaque depth-tested video-textured shapes. The VERTEX
    // stage runs the doc's placement hooks (position + spin + scale per
    // instance, one Rodrigues rotation) plus the optional luma relief; the
    // PIXEL stage samples input0 through the baked surface uv and hands the
    // lit texel to the doc's fx_color. tex1 (deck B) is bound as well: a
    // `decks: 2` doc is a two-deck transition (deck_a/deck_b helpers, p3 =
    // the crossfader) with the full-frame backdrop quad keeping both fader
    // ends exact.
    // -----------------------------------------------------------------------
    mod.draw.DrawVjFxVideomesh = set_type_default() do #(DrawVjFxVideomesh::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        tex0: texture_2d(float)
        tex1: texture_2d(float)
        has_content: uniform(0.0)
        backface_culling: false
        alpha_blend: false
        depth_write: true

        v_uv: varying(vec2f)
        v_normal: varying(vec3f)
        v_world: varying(vec3f)
        // (instance hash, face id — -1 = backdrop, instance id, face hash)
        v_attr: varying(vec4f)

        // Vertex-stage helper (fx_axis default only — pixel code hashes
        // inline, a helper fn binds to ONE stage).
        hash1: fn(x: float) -> float {
            return fract(sin(x * 12.9898) * 43758.5453)
        }

        xcross: fn(a: vec3, b: vec3) -> vec3 {
            return vec3(
                a.y * b.z - a.z * b.y,
                a.z * b.x - a.x * b.z,
                a.x * b.y - a.y * b.x
            )
        }

        // Rodrigues rotation of v around unit axis a by (cos c, sin s).
        rodr: fn(v: vec3, a: vec3, c: float, s: float) -> vec3 {
            let axv = self.xcross(a, v)
            let ad = dot(a, v)
            return v * c + axv * s + a * (ad * (1.0 - c))
        }

        // ---- THE PLACEMENT HOOKS (vertex stage, doc-replaceable) ----------
        // fx_place(id, hash, t) -> vec4: xyz = instance position, w = spin
        // angle around fx_axis. t = document time (self.time_beat.x); every
        // signal (self.sig, self.user, …) is in scope. Default: a single
        // instance tumbles in place, a multi-instance doc becomes a slow
        // carousel on the spread radius; corridor segments stay put so the
        // camera loop wraps seamlessly.
        fx_place: fn(id: float, hash: float, t: float) -> vec4 {
            let rate = self.flow.x * (1.0 + clamp(self.user.x, 0.0, 4.0))
            if self.shape.x > 9.5 {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
            let n = max(self.shape.y, 1.0)
            if n < 1.5 {
                return vec4(0.0, 0.0, 0.0, t * rate * 0.4)
            }
            let a = (id / n + t * rate * 0.02) * 6.2831853
            let r = self.shape.w
            return vec4(sin(a) * r, 0.0, cos(a) * r, t * rate * (0.25 + hash * 0.4))
        }

        // fx_axis(id, hash) -> vec3 (normalized by the caller). Default: a
        // per-instance hashed axis biased toward yaw.
        fx_axis: fn(id: float, hash: float) -> vec3 {
            return vec3(
                self.hash1(id * 3.7 + 1.0) - 0.5,
                self.hash1(id * 5.1 + 2.0) + 0.3,
                self.hash1(id * 7.3 + 3.0) - 0.5
            )
        }

        // fx_scale(id, hash, t) -> float. Default: a gentle beat pump.
        fx_scale: fn(id: float, hash: float, t: float) -> float {
            return 1.0 + self.time_beat.w * 0.06
        }

        vertex: fn() {
            let face = self.geom.geom_pad
            if face < -0.5 {
                // Backdrop quad: clip-space passthrough at far depth.
                self.v_uv = self.geom.geom_uv
                self.v_normal = vec3(0.0, 0.0, 1.0)
                self.v_world = vec3(0.0, 0.0, 0.0)
                self.v_attr = vec4(0.0, face, self.geom.geom_id, 0.0)
                self.vertex_pos = vec4(
                    self.geom.geom_pos.x,
                    self.geom.geom_pos.y,
                    0.99995,
                    1.0
                )
            } else {
                let inst = self.geom.geom_id
                let hash = self.geom.geom_tail_pad_0
                let fhash = self.geom.geom_tail_pad_1
                let t = self.time_beat.x
                let place = self.fx_place(inst, hash, t)
                let ax = self.fx_axis(inst, hash)
                let axis = ax / max(length(ax), 0.001)
                let scl = max(self.fx_scale(inst, hash, t), 0.0)
                // Luma relief: displace along the normal by the video's
                // brightness at this vertex (flow.y = relief, p1 gains it).
                let vtex = self.tex0.sample_nearest(self.geom.geom_uv, 0.0)
                let lum = dot(vtex.xyz, vec3(0.299, 0.587, 0.114))
                let bump = lum * self.flow.y * self.shape.z * 0.45
                    * clamp(1.0 + self.user.y, 0.0, 3.0)
                let local = (self.geom.geom_pos + self.geom.geom_normal * bump) * scl
                let c = cos(place.w)
                let s = sin(place.w)
                let p = self.rodr(local, axis, c, s) + place.xyz
                let n = self.rodr(self.geom.geom_normal, axis, c, s)
                let world = self.draw_list.view_transform * vec4(p.x, p.y, p.z, 1.0)
                self.v_world = world.xyz
                self.v_normal = n
                self.v_uv = self.geom.geom_uv
                self.v_attr = vec4(hash, face, inst, fhash)
                let view_pos = self.draw_pass.camera_view * world
                self.vertex_pos = self.draw_pass.camera_projection * view_pos
            }
            return self.vertex_pos
        }

        // Deck samplers (pixel stage). tex0 = input0 / deck A, tex1 = deck B.
        deck_a: fn(uv: vec2) -> vec4 {
            let u = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            return self.tex0.sample_as_bgra(u)
        }

        deck_b: fn(uv: vec2) -> vec4 {
            let u = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            return self.tex1.sample_as_bgra(u)
        }

        // ---- THE BACKDROP HOOK (pixel stage, doc-replaceable) -------------
        // Only reached when the doc set `backdrop: 1`. uv = screen uv,
        // t = the crossfader (p3, clamped 0..1). Default: the background
        // colour washed with the video by the content strength — a
        // transition doc overrides this with its own deck mix.
        fx_backdrop: fn(uv: vec2, t: float) -> vec4 {
            let c = self.deck_a(uv)
            let m = clamp(self.fog.z, 0.0, 1.0) * 0.6
            return vec4(self.col_bg.xyz.mix(c.xyz * 0.7, m), 1.0)
        }

        // ---- THE LOOK (pixel stage, doc-replaceable — CONTRACT.md) --------
        //   t       = light drive: headlamp diffuse × beat lift, ~0.4..1.6
        //   attr    = (instance hash, face id, edge 0..1 — 0 at the uv
        //             border, 1 inside —, instance id)
        //   content = input0 at THIS FRAGMENT's surface uv — the geometry
        //             IS the picture in this family, no cmix ramp needed
        //   cmix    = pre-gated content strength, for looks that want it
        fx_color: fn(t: float, attr: vec4, content: vec4, cmix: float) -> vec4 {
            let lit = content.xyz * (t * (0.45 + 0.55 * attr.z))
            let rim = self.col_c.xyz * (1.0 - attr.z)
                * (0.25 + self.time_beat.w * 0.5 + clamp(self.user.z, 0.0, 4.0))
            return vec4((lit + rim) * self.fog.y, 1.0)
        }

        pixel: fn() {
            let mut out = vec4(0.0, 0.0, 0.0, 1.0)
            if self.v_attr.y < -0.5 {
                out = self.fx_backdrop(self.v_uv, clamp(self.user.w, 0.0, 1.0))
            } else {
                let content = self.tex0.sample_as_bgra(self.v_uv)
                // Shape-LOCAL uv: undo the per-instance uv_split window so
                // the edge term hugs each instance's own border.
                let n = max(self.shape.y, 1.0)
                let sp = self.flow.z
                let mut luv = self.v_uv
                if sp > 2.5 {
                    let gw = 0.0 - floor(0.0 - sqrt(n))
                    let gh = 0.0 - floor(0.0 - n / gw)
                    luv = vec2(fract(self.v_uv.x * gw), fract(self.v_uv.y * gh))
                } else { if sp > 1.5 {
                    luv = vec2(self.v_uv.x, fract(self.v_uv.y * n))
                } else { if sp > 0.5 {
                    luv = vec2(fract(self.v_uv.x * n), self.v_uv.y)
                }}}
                let e = min(
                    min(luv.x, 1.0 - luv.x),
                    min(luv.y, 1.0 - luv.y)
                )
                let edge = smoothstep(0.0, 0.05, e)
                let cam = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
                let cpos = cam.xyz / max(cam.w, 0.0001)
                let vd = cpos - self.v_world
                let vdl = max(length(vd), 0.0001)
                let nrm = self.v_normal / max(length(self.v_normal), 0.0001)
                let ndl = abs(dot(nrm, vd / vdl))
                let lite = (0.40 + 0.60 * ndl) * (1.0 + self.time_beat.w * 0.35)
                let col = self.fx_color(
                    lite,
                    vec4(self.v_attr.x, self.v_attr.y, edge, self.v_attr.z),
                    content,
                    self.fog.z
                )
                let fogf = 1.0 - exp(0.0 - vdl * self.fog.x)
                out = vec4(col.xyz.mix(self.col_bg.xyz, fogf), 1.0)
            }
            return out
        }

        fragment: fn() {
            self.fb0 = self.pixel()
        }
    }
}

/// Standard fx draw layout (see shaders.rs — the view writes these fields).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVjFxVideomesh {
    #[deref]
    pub draw_vars: DrawVars,
    /// (time, beat position, beat phase 0..1, eased pulse).
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub time_beat: Vec4f,
    /// (bar phase 0..1, bpm, audio energy 0..1, dt).
    #[live(vec4(0.0, 120.0, 0.0, 0.0))]
    pub sig: Vec4f,
    /// p0 = spin drive add, p1 = relief/pump gain, p2 = edge glow,
    /// p3 = THE CROSSFADER on two-deck docs.
    #[live(vec4(0.0, 0.0, 0.0, 0.0))]
    pub user: Vec4f,
    /// (sway, sway_freq, growth 0..1, twist).
    #[live(vec4(0.0, 1.0, 1.0, 0.0))]
    pub anim: Vec4f,
    /// (shape kind, instances, size, spread).
    #[live(vec4(0.0, 1.0, 1.6, 3.0))]
    pub shape: Vec4f,
    /// (spin, relief, uv_split kind, aspect).
    #[live(vec4(1.0, 0.0, 0.0, 1.0))]
    pub flow: Vec4f,
    #[live(vec4(0.28, 0.94, 1.0, 1.0))]
    pub col_a: Vec4f,
    #[live(vec4(1.0, 0.25, 0.63, 1.0))]
    pub col_b: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub col_c: Vec4f,
    #[live(vec4(0.01, 0.012, 0.03, 1.0))]
    pub col_bg: Vec4f,
    /// (fog density, emissive gain, tex mix, unused).
    #[live(vec4(0.03, 1.0, 0.0, 0.0))]
    pub fog: Vec4f,
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::mesh::VERT_FLOATS;

    fn build(cfg: VideomeshConfig) -> (VideomeshEngine, FxMesh) {
        let mut e = VideomeshEngine::new(cfg);
        let mut mesh = FxMesh::default();
        e.build(&mut mesh);
        (e, mesh)
    }

    #[test]
    fn every_shape_builds_finite_geometry() {
        for shape in [
            VideomeshShape::Box,
            VideomeshShape::Sphere,
            VideomeshShape::Torus,
            VideomeshShape::Disc,
            VideomeshShape::Cylinder,
            VideomeshShape::Capsule,
            VideomeshShape::Octahedron,
            VideomeshShape::StarPrism,
            VideomeshShape::Facets,
            VideomeshShape::Grid,
            VideomeshShape::Corridor,
        ] {
            let (_, mesh) = build(VideomeshConfig { shape, instances: 3, ..Default::default() });
            assert!(mesh.triangle_count() > 0, "{shape:?} built nothing");
            for v in mesh.verts.chunks(VERT_FLOATS) {
                for f in v {
                    assert!(f.is_finite(), "{shape:?} leaked non-finite data");
                }
                // uv stays inside the image.
                assert!((-1e-4..=1.0001).contains(&v[8]), "{shape:?} u out of range: {}", v[8]);
                assert!((-1e-4..=1.0001).contains(&v[9]), "{shape:?} v out of range: {}", v[9]);
                // face id -1 is reserved for the backdrop.
                assert!(v[7] >= 0.0, "{shape:?} face id below zero without backdrop");
            }
        }
    }

    #[test]
    fn instances_and_windows_ride_the_stream() {
        let (e, mesh) = build(VideomeshConfig {
            shape: VideomeshShape::Box,
            instances: 9,
            split: UvSplit::Cells,
            ..Default::default()
        });
        assert_eq!(e.instances, 9);
        let mut seen = [false; 9];
        for v in mesh.verts.chunks(VERT_FLOATS) {
            let id = v[3] as usize;
            assert!(id < 9);
            seen[id] = true;
            // Cell windows: instance id's cell contains every uv.
            let gw = 3.0;
            let gx = (id % 3) as f32;
            let gy = (id / 3) as f32;
            assert!(v[8] >= gx / gw - 1e-4 && v[8] <= (gx + 1.0) / gw + 1e-4);
            assert!(v[9] >= gy / gw - 1e-4 && v[9] <= (gy + 1.0) / gw + 1e-4);
        }
        assert!(seen.iter().all(|s| *s), "an instance id is missing from the stream");
    }

    #[test]
    fn backdrop_quad_is_flagged_and_last() {
        let (_, mesh) = build(VideomeshConfig {
            shape: VideomeshShape::Sphere,
            backdrop: 1.0,
            decks: 2,
            ..Default::default()
        });
        let floats = VERT_FLOATS;
        let n = mesh.vertex_count();
        let mut flagged = 0;
        for (i, v) in mesh.verts.chunks(floats).enumerate() {
            if v[7] < -0.5 {
                flagged += 1;
                assert!(i >= n - 4, "backdrop verts must be the last four");
                assert!(v[0].abs() == 1.0 && v[1].abs() == 1.0, "backdrop is clip-space");
            }
        }
        assert_eq!(flagged, 4, "backdrop quad missing");
    }

    #[test]
    fn corridor_segments_tile_one_pitch_apart() {
        let (e, mesh) = build(VideomeshConfig {
            shape: VideomeshShape::Corridor,
            instances: 6,
            spread: 3.0,
            ..Default::default()
        });
        let pitch = e.pitch();
        for v in mesh.verts.chunks(VERT_FLOATS) {
            let id = v[3];
            let z = v[2];
            let hi = pitch - id * pitch;
            assert!(
                z <= hi + 1e-3 && z >= hi - pitch - 1e-3,
                "segment {id} vertex z {z} outside its pitch slot"
            );
        }
        // Camera loop wraps within one pitch and stays finite.
        for i in 0..200 {
            let cam = e.camera(i as f32 * 0.31);
            assert!(cam.eye.z.is_finite() && cam.eye.z <= 0.001 && cam.eye.z >= -pitch - 1e-3);
        }
    }

    #[test]
    fn degenerate_params_stay_safe() {
        let (e, mesh) = build(VideomeshConfig {
            shape: VideomeshShape::Capsule,
            instances: 500,
            size: f32::NAN,
            spread: f32::INFINITY,
            detail: 9999,
            aspect: -8.0,
            fly: f32::NAN,
            relief: f32::NAN,
            ..Default::default()
        });
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.vertex_count() <= VERT_BUDGET, "budget clamp failed");
        for v in mesh.verts.chunks(VERT_FLOATS) {
            for f in v {
                assert!(f.is_finite(), "degenerate cfg leaked non-finite data");
            }
        }
        let u = e.uniforms();
        for f in [u.shape.x, u.shape.y, u.shape.z, u.shape.w, u.flow.x, u.flow.y, u.flow.z, u.flow.w] {
            assert!(f.is_finite(), "uniform not sanitized");
        }
        let cam = e.camera(7.3);
        assert!(cam.eye.x.is_finite() && cam.eye.y.is_finite() && cam.eye.z.is_finite());
    }

    #[test]
    fn orbit_camera_is_bounded_forever() {
        let e = VideomeshEngine::new(VideomeshConfig::default());
        for i in 0..500 {
            // Deck reality: time can be an hour in.
            let cam = e.camera(3600.0 + i as f32 * 7.7);
            let r = (cam.eye.x * cam.eye.x + cam.eye.z * cam.eye.z).sqrt();
            assert!(r < 60.0, "orbit walked away: {r}");
            assert!(cam.eye.y.is_finite());
        }
    }
}
