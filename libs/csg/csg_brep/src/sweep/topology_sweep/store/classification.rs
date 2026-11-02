//! Borrowed, owner-local geometry for the primitive rotational-axis classifier.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`,
//! `source/SMLib/src/SmFace.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `axis_inside_face` | the `SmFace::CurveOnClassify` axis check in `SmPrimitiveCreation::CreateRotationalSweep` |
//! | `Geometry`, `Handle` | caller-owned object and topology resolution for classification |
//!
//! Deviations: borrowed typed handles replace object pointers. The classification
//! runs on the face's B-rep in place (`curve_on_classify_mut`); the gaps this
//! adapter answers through `ClassGeometry::vertex_face_gap` (a `&self` query) are
//! computed on a private copy of it, so those lazy gap updates are not kept. No classification algorithm or recursion is introduced; decisions
//! use the existing curve/face classifier and its interval midpoint classes.

use crate::curve::curve::Curve;
use crate::error::{EntityRef, Result};
use crate::geom::object::ObjectRef;
use crate::intersect::curve_class::p1::CurveClassification;
use crate::intersect::curve_class::p5::ClassGeometryMut;
use crate::intersect::point_class::{ClassGeometry, ClassObject, ClassObjectKind, PointClassType};
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::query::face::p2::{
    curve_on_classify_mut, CurveOnClassifyBrep, CurveOnClassifyObjects, FaceBrep,
};
use crate::surface::surface::Surface;
use crate::surface::volume::Volume;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, LoopId, VertexId, VertexuseId};

#[derive(Clone, Copy, PartialEq)]
enum Handle {
    Topology(EntityRef),
    Axis,
    EdgeCurve(EdgeId),
    Trim(EdgeuseId),
    Surface(FaceId),
}

impl ClassObject for Handle {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Topology(e) => e.class_kind(),
            Self::Axis | Self::EdgeCurve(_) | Self::Trim(_) => ClassObjectKind::Curve,
            Self::Surface(_) => ClassObjectKind::Surface,
        }
    }
}

struct Geometry<'a> {
    kernel: &'a Kernel,
    brep: &'a mut Brep,
    axis: &'a Curve,
}

impl ClassGeometry<Handle> for Geometry<'_> {
    fn curve(&self, o: Handle) -> Option<&Curve> {
        match o {
            Handle::Axis => Some(self.axis),
            Handle::EdgeCurve(e) => self.brep.edge(e).ok()?.curve.as_ref(),
            Handle::Trim(eu) => {
                crate::topology::edgeuse::p1::uv_trim_curve(&*self.brep, eu)
                    .ok()?
                    .0
            }
            _ => None,
        }
    }
    fn surface(&self, o: Handle) -> Option<&Surface> {
        if let Handle::Surface(f) = o {
            self.brep.face(f).ok()?.surface.as_ref()
        } else {
            None
        }
    }
    fn volume(&self, _: Handle) -> Option<&Volume> {
        None
    }
    fn point(&self, _: Handle) -> Option<Point3> {
        None
    }
    fn classification_vertex_point(&self, o: Handle) -> Option<Point3> {
        if let Handle::Topology(EntityRef::Vertex(v)) = o {
            Some(self.brep.vertex(v).ok()?.point)
        } else {
            None
        }
    }
    fn topology_zone_tol_3d(&self, o: Handle) -> Option<f64> {
        match o {
            Handle::Topology(EntityRef::Vertex(v)) => Some(self.brep.vertex(v).ok()?.tolerance),
            Handle::Topology(EntityRef::Edge(e)) => Some(self.brep.edge(e).ok()?.tolerance),
            Handle::Topology(EntityRef::Face(f)) => Some(self.brep.face(f).ok()?.tolerance),
            _ => None,
        }
    }
    fn topology_entity(&self, o: Handle) -> Option<(&Brep, EntityRef)> {
        if let Handle::Topology(e) = o {
            Some((&*self.brep, e))
        } else {
            None
        }
    }
    fn topology_handle(&self, e: EntityRef) -> Option<Handle> {
        Some(Handle::Topology(e))
    }
    fn owner_of(&self, owner: ObjectRef) -> Option<ObjectRef> {
        // Owner topology resolves directly through topology_entity/handle. Curves
        // passed here have no additional generating-curve owner chain.
        let _ = owner;
        None
    }
    fn vertex_face_gap(&self, vertex: Handle, _: &Surface, face: Option<Handle>) -> Option<f64> {
        let Handle::Topology(EntityRef::Vertex(v)) = vertex else {
            return None;
        };
        let Handle::Topology(EntityRef::Face(f)) = face? else {
            return None;
        };
        for &vu in &self.brep.vertex(v).ok()?.vertexuses {
            let Some(fu) = crate::topology::vertexuse::faceuse(&*self.brep, vu).ok()? else {
                continue;
            };
            if self.brep.faceuse(fu).ok()?.face != Some(f) {
                continue;
            }
            // As query::face's boundary views do, retain lazy gap mutations in a
            // private topology snapshot while evaluating under the real kernel.
            let mut snapshot = self.brep.clone();
            return crate::topology::vertexuse::vertex_face_gap(
                self.kernel,
                &mut snapshot,
                vu,
                false,
            )
            .ok()
            .map(|gap| gap.map_or(0.0, |g| g.gap_3d));
        }
        None
    }
}

impl CurveOnClassifyObjects<Handle> for Geometry<'_> {
    fn face(&self, f: FaceId) -> Handle {
        Handle::Topology(EntityRef::Face(f))
    }
    fn vertex(&self, v: VertexId) -> Handle {
        Handle::Topology(EntityRef::Vertex(v))
    }
    fn edge(&self, e: EdgeId) -> Handle {
        Handle::Topology(EntityRef::Edge(e))
    }
    fn vertexuse(&self, v: VertexuseId) -> Handle {
        Handle::Topology(EntityRef::Vertexuse(v))
    }
    fn edgeuse(&self, e: EdgeuseId) -> Handle {
        Handle::Topology(EntityRef::Edgeuse(e))
    }
    fn edge_curve(&self, e: EdgeId) -> Handle {
        Handle::EdgeCurve(e)
    }
    fn trim_curve(&self, e: EdgeuseId) -> Handle {
        Handle::Trim(e)
    }
    fn face_surface(&self, f: FaceId) -> Option<Handle> {
        self.brep
            .face(f)
            .ok()?
            .surface
            .as_ref()
            .map(|_| Handle::Surface(f))
    }
    fn loop_id(&self, o: Handle) -> Option<LoopId> {
        if let Handle::Topology(EntityRef::Loop(l)) = o {
            Some(l)
        } else {
            None
        }
    }
}

impl ClassGeometryMut<Handle> for Geometry<'_> {
    fn curve_mut(&mut self, _o: Handle) -> Option<&mut Curve> {
        // the axis is the caller's (read-only); ClassifyFaceInterval does not edit it
        None
    }
    fn topology_entity_mut(&mut self, o: Handle) -> Option<(&mut Brep, EntityRef)> {
        let (_, e) = self.topology_entity(o)?;
        Some((&mut *self.brep, e))
    }
}

impl FaceBrep for Geometry<'_> {
    fn brep(&self) -> &Brep {
        self.brep
    }
}

impl CurveOnClassifyBrep<Handle> for Geometry<'_> {
    fn brep_mut(&mut self) -> &mut Brep {
        self.brep
    }
}

pub(crate) fn axis_inside_face(
    k: &Kernel,
    b: &mut Brep,
    face: FaceId,
    axis: &Curve,
) -> Result<bool> {
    let tolerance = b.face(face)?.tolerance;
    let mut geometry = Geometry {
        kernel: k,
        brep: b,
        axis,
    };
    let mut cc = CurveClassification::with_curve_interval(
        k,
        Handle::Axis,
        axis.natural_interval(),
        tolerance,
    );
    curve_on_classify_mut(k, face, &mut geometry, true, &mut cc, false, true, true)?;
    Ok(cc
        .intervals()
        .iter()
        .any(|i| i.mid.point_class() == PointClassType::Face))
}
