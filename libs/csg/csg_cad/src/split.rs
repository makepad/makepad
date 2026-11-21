//! A solid split in two along a plane, with alignment pins across the cut: where the pins stand
//! (in the cut's section, clear of its outline and of each other, as deep as both halves take)
//! and the round rods each piece gains or loses. The engines build the pieces from the plan,
//! with the mesh booleans or the exact kernel, so every piece of one split agrees on it.

use crate::measure::{section_plane, PlaneFrame};
use crate::region::{Region, P2};
use makepad_csg_math::{dvec3, Vec3d};
use makepad_csg_mesh::bvh::Bvh;
use makepad_csg_mesh::mesh::TriMesh;

/// A piece of a split: the side behind the plane's normal, the side it faces, or the dowel pins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Piece {
    Below,
    Above,
    Pins,
}

/// The pins across a split's cut (lengths in the mesh's units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pins {
    /// How many (0: none).
    pub count: u32,
    /// Their radius; none: from the section (the room inside its outline).
    pub radius: Option<f64>,
    /// How far each reaches into a half; none: twice its radius.
    pub depth: Option<f64>,
    /// Pins of their own in holes in both halves; else pegs on the half below that go into
    /// holes in the one above.
    pub dowels: bool,
    /// A hole's radius over its pin's.
    pub clearance: f64,
    /// A hole's floor past its pin's end.
    pub clearance_end: f64,
    /// The material kept round a hole and past its floor.
    pub wall: f64,
}

/// A round rod from `a` to `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rod {
    pub a: Vec3d,
    pub b: Vec3d,
    pub radius: f64,
}

impl Rod {
    pub fn middle(&self) -> Vec3d {
        (self.a + self.b) * 0.5
    }

    pub fn length(&self) -> f64 {
        (self.b - self.a).length()
    }
}

/// Pins asked for that the cut has no room for: the radius tried, and the largest that has room
/// and depth somewhere on the cut (none when not even a pin as thick as a hole's wall has).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoRoom {
    pub radius: f64,
    pub fits: Option<f64>,
}

/// Where a split cuts and where its pins stand.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub plane: PlaneFrame,
    /// A cube of side `slab` centred at `slab_center`, square to [`Plan::turn`], one face on the
    /// plane: the half-space the normal faces, as far as the part goes.
    pub slab_center: Vec3d,
    pub slab: f64,
    /// The pins' feet on the cut.
    pub spots: Vec<Vec3d>,
    pub radius: f64,
    /// How far each pin reaches into a half.
    pub depth: f64,
    /// Pins asked for and none placed: why, and the pin that would stand.
    pub no_room: Option<NoRoom>,
    pins: Pins,
}

impl Plan {
    /// The turn that takes Y to the plane's normal (columns u, n, u x n): a box's and a
    /// cylinder's own frame onto the cut's.
    pub fn turn(&self) -> [Vec3d; 3] {
        let p = &self.plane;
        [p.u, p.n, p.u.cross(p.n)]
    }

    /// The rods `piece` gains (pegs, dowel pins) and loses (holes).
    pub fn rods(&self, piece: Piece) -> (Vec<Rod>, Vec<Rod>) {
        let n = self.plane.n;
        let (r, l, pins) = (self.radius, self.depth, &self.pins);
        let hole = r + pins.clearance;
        let deep = l + pins.clearance_end;
        // How far a rod reaches past the cut into the other side, so it never meets a face
        // flush.
        let over = (0.5 * r).min(0.5);
        let rod = |s: Vec3d, from: f64, to: f64, radius: f64| Rod { a: s + n * from, b: s + n * to, radius };
        let mut gain = Vec::new();
        let mut lose = Vec::new();
        for s in &self.spots {
            match (piece, pins.dowels) {
                (Piece::Below, false) => gain.push(rod(*s, -over, l, r)),
                (Piece::Below, true) => lose.push(rod(*s, -deep, over, hole)),
                (Piece::Above, _) => lose.push(rod(*s, -over, deep, hole)),
                (Piece::Pins, true) => gain.push(rod(*s, -l, l, r)),
                (Piece::Pins, false) => {}
            }
        }
        (gain, lose)
    }
}

/// The split of `mesh` by the plane through `at` facing `normal`, with `pins` across it. The
/// pins stand where the section has room for a hole and its wall, spread as far apart as they
/// go, and reach `depth` into each half where both take it (else their radius); none stand
/// where no place has room ([`Plan::no_room`] says so). Fails when the plane misses the part.
pub fn plan(mesh: &TriMesh, at: [f64; 3], normal: [f64; 3], pins: &Pins) -> Result<Plan, String> {
    let plane = PlaneFrame::new(dvec3(at[0], at[1], at[2]), dvec3(normal[0], normal[1], normal[2])).ok_or("a split's normal must be a direction, not [0 0 0]")?;
    let regions = section_plane(mesh, &plane);
    if regions.is_empty() {
        return Err("the split's plane misses the part: put its center inside it".into());
    }
    // A cube that holds the whole part on the normal's side.
    let (mut lo, mut hi) = (dvec3(f64::INFINITY, f64::INFINITY, f64::INFINITY), dvec3(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
    for v in &mesh.vertices {
        lo = dvec3(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
        hi = dvec3(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
    }
    let reach = (plane.at - (lo + hi) * 0.5).length() + (hi - lo).length();
    let slab = 2.0 * reach + 2.0;
    let slab_center = plane.at + plane.n * (0.5 * slab);
    let mut out = Plan { plane, slab_center, slab, spots: Vec::new(), radius: 0.0, depth: 0.0, no_room: None, pins: *pins };
    if pins.count == 0 {
        return Ok(out);
    }
    let edges: Vec<(P2, P2)> = regions.iter().flat_map(|r| r.loops().flat_map(|l| (0..l.len()).map(move |i| (l[i], l[(i + 1) % l.len()])))).collect();
    let inside = |q: P2| regions.iter().any(|r: &Region| r.contains(q));
    let room = |q: P2| edges.iter().map(|(a, b)| seg_distance(q, *a, *b)).fold(f64::INFINITY, f64::min);
    // Candidate feet on a grid over the section, with the room each has to its outline.
    let (mut ulo, mut uhi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for r in &regions {
        let (a, b) = r.bounds();
        for k in 0..2 {
            ulo[k] = ulo[k].min(a[k]);
            uhi[k] = uhi[k].max(b[k]);
        }
    }
    const STEPS: usize = 48;
    let step = ((uhi[0] - ulo[0]).max(uhi[1] - ulo[1]) / STEPS as f64).max(1e-6);
    let (nu, nv) = (((uhi[0] - ulo[0]) / step).ceil() as usize, ((uhi[1] - ulo[1]) / step).ceil() as usize);
    let mut cands: Vec<(P2, f64)> = Vec::new();
    for i in 0..nu {
        for j in 0..nv {
            let q = [ulo[0] + (i as f64 + 0.5) * step, ulo[1] + (j as f64 + 0.5) * step];
            if inside(q) {
                cands.push((q, room(q)));
            }
        }
    }
    let widest = cands.iter().map(|c| c.1).fold(0.0, f64::max);
    let r = pins.radius.unwrap_or_else(|| (0.5 * widest - pins.clearance - pins.wall).clamp(1.0, 2.5));
    let need = r + pins.clearance + pins.wall;
    let bvh = Bvh::build(mesh);
    // How far a peg's foot reaches into the half below (a pin of radius r).
    let over_of = |r: f64| (0.5 * r).min(0.5);
    // How deep a pin can reach at each foot: into the half above (and below, for dowels), its
    // hole's floor and a wall short of the far side.
    let depth_at = |q: P2, over: f64| -> f64 {
        let s = plane.point(q);
        let up = bvh.ray(s, plane.n, f64::INFINITY, None).map_or(0.0, |h| h.0);
        let down = bvh.ray(s, -plane.n, f64::INFINITY, None).map_or(0.0, |h| h.0);
        let floor = pins.clearance_end + pins.wall;
        if pins.dowels {
            up.min(down) - floor
        } else if down > over {
            up - floor
        } else {
            0.0
        }
    };
    let roomy: Vec<(P2, f64)> = cands.iter().filter(|c| c.1 >= need).map(|c| (c.0, depth_at(c.0, over_of(r)))).collect();
    let want = pins.depth.unwrap_or((2.0 * r).max(2.0));
    let depth = if roomy.iter().any(|c| c.1 >= want) { want } else { r };
    let fit: Vec<P2> = roomy.iter().filter(|c| c.1 >= depth).map(|c| c.0).collect();
    if fit.is_empty() {
        // The largest pin that stands: at each foot its room to the outline less a hole's gap
        // and wall, as deep as the halves take it there (2 rays a foot, only for a split that
        // places none).
        let fits = cands
            .iter()
            .filter(|c| c.1 - pins.clearance - pins.wall > 0.0)
            .map(|c| {
                let r = c.1 - pins.clearance - pins.wall;
                r.min(depth_at(c.0, over_of(r)))
            })
            .fold(0.0, f64::max);
        out.no_room = Some(NoRoom { radius: r, fits: (fits >= 0.5 * pins.wall).then_some(fits) });
        return Ok(out);
    }
    // The first foot the farthest from the section's middle (one pin: the roomiest), each next
    // the farthest from those taken, none nearer another than two holes and a wall.
    let mid = cands.iter().fold([0.0; 2], |m, c| [m[0] + c.0[0], m[1] + c.0[1]]).map(|x| x / cands.len() as f64);
    let d2 = |a: P2, b: P2| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
    let first = if pins.count == 1 {
        let room_of = |q: P2| room(q);
        fit.iter().copied().max_by(|a, b| room_of(*a).total_cmp(&room_of(*b))).unwrap()
    } else {
        fit.iter().copied().max_by(|a, b| d2(*a, mid).total_cmp(&d2(*b, mid))).unwrap()
    };
    let apart = 2.0 * (r + pins.clearance) + pins.wall;
    let mut taken = vec![first];
    while taken.len() < pins.count as usize {
        let next = fit
            .iter()
            .map(|q| (*q, taken.iter().map(|t| d2(*q, *t)).fold(f64::INFINITY, f64::min)))
            .filter(|(_, d)| *d >= apart * apart)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match next {
            Some((q, _)) => taken.push(q),
            None => break,
        }
    }
    out.spots = taken.into_iter().map(|q| plane.point(q)).collect();
    out.radius = r;
    out.depth = depth;
    Ok(out)
}

fn seg_distance(p: P2, a: P2, b: P2) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
}
