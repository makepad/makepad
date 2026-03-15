use crate::math::{Pose, Vector};
#[cfg(feature = "alloc")]
use crate::query::{self, ContactManifold, TrackedContact};
use crate::shape::{PackedFeatureId, Segment};

/// A polygonal feature representing the local polygonal approximation of
/// a vertex, or face, of a convex shape.
#[derive(Debug)]
pub struct PolygonalFeature {
    /// Up to two vertices forming this polygonal feature.
    pub vertices: [Vector; 2],
    /// The feature IDs of this polygon's vertices.
    pub vids: [PackedFeatureId; 2],
    /// The feature ID of this polygonal feature.
    pub fid: PackedFeatureId,
    /// The number of vertices on this polygon (must be <= 4).
    pub num_vertices: usize,
}

impl Default for PolygonalFeature {
    fn default() -> Self {
        Self {
            vertices: [Vector::ZERO; 2],
            vids: [PackedFeatureId::UNKNOWN; 2],
            fid: PackedFeatureId::UNKNOWN,
            num_vertices: 0,
        }
    }
}

impl From<Segment> for PolygonalFeature {
    fn from(seg: Segment) -> Self {
        PolygonalFeature {
            vertices: [seg.a, seg.b],
            vids: PackedFeatureId::vertices([0, 2]),
            fid: PackedFeatureId::face(1),
            num_vertices: 2,
        }
    }
}

impl PolygonalFeature {
    /// Transforms the vertices of `self` by the given position `pos`.
    pub fn transform_by(&mut self, pos: &Pose) {
        self.vertices[0] = pos * self.vertices[0];
        self.vertices[1] = pos * self.vertices[1];
    }

    /// Computes the contacts between two polygonal features.
    #[cfg(feature = "alloc")]
    pub fn contacts<ManifoldData, ContactData: Default + Copy>(
        pos12: &Pose,
        pos21: &Pose,
        sep_axis1: Vector,
        sep_axis2: Vector,
        feature1: &Self,
        feature2: &Self,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
        flipped: bool,
    ) {
        match (feature1.num_vertices == 2, feature2.num_vertices == 2) {
            (true, true) => {
                Self::face_face_contacts(pos12, feature1, sep_axis1, feature2, manifold, flipped)
            }
            (true, false) => {
                Self::face_vertex_contacts(pos12, feature1, sep_axis1, feature2, manifold, flipped)
            }
            (false, true) => {
                Self::face_vertex_contacts(pos21, feature2, sep_axis2, feature1, manifold, !flipped)
            }
            (false, false) => unimplemented!(),
        }
    }

    /// Compute contacts points between a face and a vertex.
    ///
    /// This method assume we already know that at least one contact exists.
    #[cfg(feature = "alloc")]
    pub fn face_vertex_contacts<ManifoldData, ContactData: Default + Copy>(
        pos12: &Pose,
        face1: &Self,
        sep_axis1: Vector,
        vertex2: &Self,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
        flipped: bool,
    ) {
        let v2_1 = pos12 * vertex2.vertices[0];
        let tangent1 = face1.vertices[1] - face1.vertices[0];
        let normal1 = Vector::new(-tangent1.y, tangent1.x);
        let denom = -normal1.dot(sep_axis1);
        let dist = (face1.vertices[0] - v2_1).dot(normal1) / denom;
        let local_p2 = v2_1;
        let local_p1 = v2_1 - dist * normal1;

        let contact = TrackedContact::flipped(
            local_p1,
            pos12.inverse_transform_point(local_p2),
            face1.fid,
            vertex2.vids[0],
            dist,
            flipped,
        );

        manifold.points.push(contact);
    }

    /// Computes the contacts between two polygonal faces.
    #[cfg(feature = "alloc")]
    pub fn face_face_contacts<ManifoldData, ContactData: Default + Copy>(
        pos12: &Pose,
        face1: &Self,
        normal1: Vector,
        face2: &Self,
        manifold: &mut ContactManifold<ManifoldData, ContactData>,
        flipped: bool,
    ) {
        if let Some((clip_a, clip_b)) = query::details::clip_segment_segment_with_normal(
            (face1.vertices[0], face1.vertices[1]),
            (pos12 * face2.vertices[0], pos12 * face2.vertices[1]),
            normal1,
        ) {
            let fids1 = [face1.vids[0], face1.fid, face1.vids[1]];
            let fids2 = [face2.vids[0], face2.fid, face2.vids[1]];

            let dist = (clip_a.1 - clip_a.0).dot(normal1);
            let contact = TrackedContact::flipped(
                clip_a.0,
                pos12.inverse_transform_point(clip_a.1),
                fids1[clip_a.2],
                fids2[clip_a.3],
                dist,
                flipped,
            );
            manifold.points.push(contact);

            let dist = (clip_b.1 - clip_b.0).dot(normal1);
            let contact = TrackedContact::flipped(
                clip_b.0,
                pos12.inverse_transform_point(clip_b.1),
                fids1[clip_b.2],
                fids2[clip_b.3],
                dist,
                flipped,
            );
            manifold.points.push(contact);
        }
    }
}
