//! Edge selectors for fillets and chamfers: which of a mesh's sharp edges an op
//! rounds or bevels.

use crate::hash::Hasher;
use crate::{Key, P3};
use makepad_csg_cad as cad;
use makepad_csg_math::dvec3;
use makepad_csg_mesh::mesh::TriMesh;

/// The kind of a sharp edge chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Line,
    Circle,
    Curve,
}

impl EdgeKind {
    fn name(self) -> &'static str {
        match self {
            EdgeKind::Line => "line",
            EdgeKind::Circle => "circle",
            EdgeKind::Curve => "curve",
        }
    }
}

/// One filter: every condition it sets must hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeFilter {
    /// On the top (bottom) of the object: every point at its highest (lowest) Y.
    pub top: bool,
    pub bottom: bool,
    /// Every point's Y within this range.
    pub y: Option<(f64, f64)>,
    /// Every point inside this box.
    pub within: Option<(P3, P3)>,
    /// Straight and along this direction (within 2 degrees).
    pub dir: Option<P3>,
    pub kind: Option<EdgeKind>,
    pub convex: bool,
    pub concave: bool,
    pub min_length: Option<f64>,
    /// The one chain nearest this point (after the other conditions).
    pub near: Option<P3>,
}

/// A face of an exact body by name: the step that made it (its key) and its role there. Names
/// never come from ids or order: a generator names its fresh faces by role, a modifying step
/// carries its inputs' names to the faces that come from them and names the faces it makes by
/// what they come from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceName {
    pub origin: Key,
    pub role: Role,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// A generator's own face.
    Named(Face),
    /// The side an extrude, revolve or sweep made from sketch segment `i`.
    Side(u32),
    /// A shell's inner face: every one is `s.inner`, and each also `s.inner(<outer>)`, the
    /// outer face it is offset from.
    Inner(Option<Box<FaceName>>),
    /// A fillet's or chamfer's face along the edge between two named faces, the two in order
    /// (`r.face(edge(a, b))`); every piece of a split edge has it.
    Blend(Box<FaceName>, Box<FaceName>),
    /// The patch where several blends of one step meet: the input faces round it, in order
    /// (`r.corner(a, b, c)`).
    Corner(Vec<FaceName>),
}

/// A generator's faces: `top bottom left right front back` (box), `wall top bottom` (cylinder,
/// cone), `wall` (sphere, torus), `top bottom` (extrude), `start end` (revolve, sweep, loft),
/// `cut` (a split's halves).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Face {
    Top,
    Bottom,
    Left,
    Right,
    Front,
    Back,
    Wall,
    Start,
    End,
    /// The faces a split's plane cut.
    Cut,
}

impl Face {
    pub const ALL: [Face; 10] = [Face::Top, Face::Bottom, Face::Left, Face::Right, Face::Front, Face::Back, Face::Wall, Face::Start, Face::End, Face::Cut];

    pub fn name(self) -> &'static str {
        match self {
            Face::Top => "top",
            Face::Bottom => "bottom",
            Face::Left => "left",
            Face::Right => "right",
            Face::Front => "front",
            Face::Back => "back",
            Face::Wall => "wall",
            Face::Start => "start",
            Face::End => "end",
            Face::Cut => "cut",
        }
    }

    pub fn from_name(name: &str) -> Option<Face> {
        Face::ALL.into_iter().find(|f| f.name() == name)
    }
}

impl FaceName {
    pub(crate) fn hash(&self, h: &mut Hasher) {
        h.bytes(&self.origin);
        match &self.role {
            Role::Named(f) => {
                h.u(0);
                h.u(*f as u64);
            }
            Role::Side(i) => {
                h.u(1);
                h.u(*i as u64);
            }
            Role::Inner(of) => {
                h.u(2);
                match of {
                    Some(n) => {
                        h.u(1);
                        n.hash(h);
                    }
                    None => h.u(0),
                }
            }
            Role::Blend(a, b) => {
                h.u(3);
                a.hash(h);
                b.hash(h);
            }
            Role::Corner(round) => {
                h.u(4);
                h.u(round.len() as u64);
                for n in round {
                    n.hash(h);
                }
            }
        }
    }
}

/// An edge by name: where a face named `a` meets one named `b`, every piece of it; `near` keeps
/// the piece nearest a point; `chain` adds the edges continuing it tangent.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeName {
    pub a: FaceName,
    pub b: FaceName,
    pub near: Option<P3>,
    pub chain: bool,
}

/// Edges picked by any of its filters or names; neither: every sharp edge. Names pick only on
/// an exact body.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeSel {
    pub any: Vec<EdgeFilter>,
    pub named: Vec<EdgeName>,
}

impl EdgeSel {
    pub fn all() -> EdgeSel {
        EdgeSel::default()
    }

    /// No filter and no name: every sharp edge.
    pub fn is_all(&self) -> bool {
        self.any.is_empty() && self.named.is_empty()
    }

    pub(crate) fn hash(&self, h: &mut Hasher) {
        h.u(self.named.len() as u64);
        for n in &self.named {
            n.a.hash(h);
            n.b.hash(h);
            match n.near {
                Some(p) => {
                    h.u(1);
                    h.p3(p);
                }
                None => h.u(0),
            }
            h.u(n.chain as u64);
        }
        h.u(self.any.len() as u64);
        for f in &self.any {
            h.u(f.top as u64);
            h.u(f.bottom as u64);
            match f.y {
                Some((a, b)) => {
                    h.u(1);
                    h.f(a);
                    h.f(b);
                }
                None => h.u(0),
            }
            match f.within {
                Some((a, b)) => {
                    h.u(1);
                    h.p3(a);
                    h.p3(b);
                }
                None => h.u(0),
            }
            match f.dir {
                Some(d) => {
                    h.u(1);
                    h.p3(d);
                }
                None => h.u(0),
            }
            h.u(f.kind.map_or(0, |k| k as u64 + 1));
            h.u(f.convex as u64);
            h.u(f.concave as u64);
            h.opt(f.min_length);
            match f.near {
                Some(p) => {
                    h.u(1);
                    h.p3(p);
                }
                None => h.u(0),
            }
        }
    }

    /// The chains of `mesh` it picks; an error lists the edges there are when none match.
    pub fn select<'a>(&self, mesh: &TriMesh, chains: &'a [cad::EdgeChain]) -> Result<Vec<&'a cad::EdgeChain>, String> {
        if !self.named.is_empty() {
            return Err("an edge named by its faces (m.edge(a, b)) picks only on an exact body (m.brep)".into());
        }
        if self.any.is_empty() {
            return Ok(chains.iter().collect());
        }
        let bb = mesh.bounding_box();
        let eps = 1e-3 * (bb.max - bb.min).length().max(1.0);
        let one = |s: &EdgeFilter| -> Vec<&'a cad::EdgeChain> {
            let mut picked: Vec<&cad::EdgeChain> = chains
                .iter()
                .filter(|c| {
                    let (lo, hi) = c.points.iter().map(|p| p.y).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), y| (a.min(y), b.max(y)));
                    if s.top && lo < bb.max.y - eps {
                        return false;
                    }
                    if s.bottom && hi > bb.min.y + eps {
                        return false;
                    }
                    if let Some((a, b)) = s.y {
                        if lo < a - eps || hi > b + eps {
                            return false;
                        }
                    }
                    if let Some((a, b)) = s.within {
                        let inside = c.points.iter().all(|p| {
                            (0..3).all(|k| {
                                let x = [p.x, p.y, p.z][k];
                                x >= a[k].min(b[k]) - eps && x <= a[k].max(b[k]) + eps
                            })
                        });
                        if !inside {
                            return false;
                        }
                    }
                    if let Some(d) = s.dir {
                        let axis = dvec3(d[0], d[1], d[2]);
                        if axis.length() < 1e-12 || !c.direction().is_some_and(|e| cad::edges::axis_angle_deg(e, axis.normalize()) < 2.0) {
                            return false;
                        }
                    }
                    if let Some(k) = s.kind {
                        if c.kind_name() != k.name() {
                            return false;
                        }
                    }
                    if s.convex && c.concave {
                        return false;
                    }
                    if s.concave && !c.concave {
                        return false;
                    }
                    if let Some(l) = s.min_length {
                        if c.length() < l {
                            return false;
                        }
                    }
                    true
                })
                .collect();
            if let Some(n) = s.near {
                let q = dvec3(n[0], n[1], n[2]);
                let dist = |c: &cad::EdgeChain| c.points.iter().map(|p| p.distance(q)).fold(f64::INFINITY, f64::min);
                picked.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
                picked.truncate(1);
            }
            picked
        };
        let mut out: Vec<&cad::EdgeChain> = Vec::new();
        for s in &self.any {
            for c in one(s) {
                if !out.iter().any(|o| std::ptr::eq(*o, c)) {
                    out.push(c);
                }
            }
        }
        if out.is_empty() {
            let mut found: Vec<String> = chains
                .iter()
                .take(12)
                .map(|c| {
                    let m = c.midpoint();
                    format!(
                        "{} {}{} at ({:.1}, {:.1}, {:.1}) {:.1} long",
                        if c.concave { "concave" } else { "convex" },
                        c.kind_name(),
                        c.direction().map(|d| format!(" along ({:.2}, {:.2}, {:.2})", d.x, d.y, d.z)).unwrap_or_default(),
                        m.x,
                        m.y,
                        m.z,
                        c.length()
                    )
                })
                .collect();
            if chains.len() > 12 {
                found.push(format!("... {} more", chains.len() - 12));
            }
            return Err(format!("the edge selector matches none of the object's {} sharp edges: {}", chains.len(), found.join("; ")));
        }
        Ok(out)
    }
}
