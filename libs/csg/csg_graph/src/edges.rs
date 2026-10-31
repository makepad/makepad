//! Edge selectors for fillets and chamfers: which of a mesh's sharp edges an op
//! rounds or bevels.

use crate::hash::Hasher;
use crate::P3;
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

/// Edges picked by any of its filters; no filters: every sharp edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeSel {
    pub any: Vec<EdgeFilter>,
}

impl EdgeSel {
    pub fn all() -> EdgeSel {
        EdgeSel::default()
    }

    pub(crate) fn hash(&self, h: &mut Hasher) {
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
