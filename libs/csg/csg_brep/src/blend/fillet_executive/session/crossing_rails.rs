//! Opt-in refusal of rounds whose rails cross on a shared face (not upstream).
//!
//! `fixes.fillet_refuse_crossing_rails` (WR-460, source issue #1073): two
//! rounds on opposite edges of one face, together wider than the face, leave the
//! face with no room between their rails. `SmFilletExecutive::DoFilleting` builds
//! the open body anyway. After `CreateFilletBrep` and before the target is edited,
//! each rail of a round is compared with the strip the other round takes from the
//! same face: a rail lying inside that strip means the rounds do not fit.
//!
//! Deviations: not upstream. Rail positions are sampled, and the distance to the other
//! round's edge is a 3D distance (exact on planar faces, a lower bound on curved ones
//! where the face between the edges bends).
// ponytail: sampled 3D distances, no face-interior test; a C-shaped face whose two
// edges face each other across its notch would be refused though they fit. Upgrade to
// the surface distance within the face if that shows up.

use super::{FilletBody, FilletObject, FilletSession};
use crate::error::{EntityRef, Result, SolidError};
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::query::api_queries::edge_closest_point;
use crate::topology::edge;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::{EdgeId, FaceId};

const OP: &str = "blend::fillet_executive::session::crossing_rails";
/// Rail points compared per rail.
const SAMPLES: usize = 9;

/// One rail of a round on a face of the target.
struct Rail {
    /// The target edge the round is on.
    edge: EdgeId,
    face: FaceId,
    /// The least distance from the round's edge to a rail point: the strip it takes (the
    /// rail runs on past the edge's ends, where the distance grows).
    width: f64,
    points: Vec<Point3>,
}

impl FilletSession<'_> {
    /// Refuse the fillet when a rail of one round lies inside the strip another round,
    /// on an edge sharing no vertex with the first, takes from the same face.
    pub(super) fn refuse_crossing_rails(&mut self, k: &Kernel) -> Result<()> {
        let rails = self.collect_rails(k)?;
        let tolerance = self.objects.target.tolerance();
        let target = &*self.objects.target;
        for a in &rails {
            for b in &rails {
                if a.face != b.face || a.edge == b.edge || shares_vertex(target, a.edge, b.edge)? {
                    continue;
                }
                for &p in &a.points {
                    if edge_closest_point(k, target, b.edge, p)?.closest.distance + tolerance
                        < b.width
                    {
                        self.executive.error.code = 4; // SM_FILERR_EDGE_PROBLEM
                        self.executive.error.status = 2; // SM_FIL_FAILURE
                        self.executive.error.edge = Some(FilletObject::Topology(
                            FilletBody::Target,
                            EntityRef::Edge(a.edge),
                        ));
                        self.executive.error.comment =
                            "Fillet Solver Error: Rounds Of Two Edges Cross On A Face".into();
                        return Err(SolidError::Upstream {
                            operation: OP,
                            code: 1001,
                            entity: Some(EntityRef::Edge(a.edge)),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_rails(&self, k: &Kernel) -> Result<Vec<Rail>> {
        let target = &*self.objects.target;
        let pseudo = &self.objects.pseudo;
        let mut rails = Vec::new();
        for solver in &self.executive.solvers {
            let FilletObject::Solver(id) = *solver else {
                continue;
            };
            let base = self.objects.solver(id)?.base();
            let Some(eu) = base.edgeuse(0)? else {
                continue;
            };
            let Some(edge) = eu1::edge(target, eu)? else {
                continue;
            };
            for &geom in &base.fillet_geoms {
                for &rail in pseudo.geom(geom)?.rails.iter().flatten() {
                    let Some(face) = pseudo.fillet_edge(rail)?.orig_face else {
                        continue;
                    };
                    let record = pseudo.brep().edge(rail)?;
                    let Some(curve) = record.curve.as_ref() else {
                        continue;
                    };
                    // The rails are not trimmed yet: their interval is still
                    // the inside-out `Extent1d::new()`, whose samples lie ~1e21 away.
                    let span = if record.interval.is_init() {
                        curve.natural_interval()
                    } else {
                        record.interval
                    };
                    let (lo, hi) = (span.min(), span.max());
                    let mut points = Vec::with_capacity(SAMPLES);
                    let mut width = f64::INFINITY;
                    for i in 0..SAMPLES {
                        let t = lo + (hi - lo) * (i as f64 + 0.5) / SAMPLES as f64;
                        let p = curve.evaluate_point(k, t)?;
                        width = width.min(edge_closest_point(k, target, edge, p)?.closest.distance);
                        points.push(p);
                    }
                    rails.push(Rail {
                        edge,
                        face,
                        width,
                        points,
                    });
                }
            }
        }
        Ok(rails)
    }
}

fn shares_vertex(target: &crate::topology::Brep, a: EdgeId, b: EdgeId) -> Result<bool> {
    let (a0, a1) = edge::start_end_vertices(target, a)?;
    let (b0, b1) = edge::start_end_vertices(target, b)?;
    Ok([a0, a1]
        .into_iter()
        .flatten()
        .any(|v| Some(v) == b0 || Some(v) == b1))
}
