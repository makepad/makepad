#![allow(dead_code)] // TODO: remove this once we support polygons.

use crate::geometry::contact_generator::PrimitiveContactGenerationContext;
use crate::geometry::{sat, Contact, ContactData, ContactManifold, ContactManifoldData, Polygon};
use crate::math::{Pose, Vector, Real};
#[cfg(feature = "dim2")]
use crate::{math::Vector, utils};
use parry::query;

pub fn generate_contacts_polygon_polygon(_ctxt: &mut PrimitiveContactGenerationContext) {
    unimplemented!()
    // if let (Shape::Polygon(polygon1), Shape::Polygon(polygon2)) = (ctxt.shape1, ctxt.shape2) {
    //     generate_contacts(
    //         polygon1,
    //         &ctxt.position1,
    //         polygon2,
    //         &ctxt.position2,
    //         ctxt.manifold,
    //     );
    //     ContactManifoldData::update_warmstart_multiplier(ctxt.manifold);
    // } else {
    //     unreachable!()
    // }
    //
}

fn generate_contacts<'a>(
    mut p1: &'a Polygon,
    mut m1: &'a Pose,
    mut p2: &'a Polygon,
    mut m2: &'a Pose,
    manifold: &'a mut ContactManifold,
) {
    let mut m12 = m1.inv_mul(&m2);
    let mut m21 = m12.inverse();

    if manifold.try_update_contacts(&m12) {
        return;
    }

    let mut sep1 = sat::polygon_polygon_compute_separation_features(p1, p2, &m12);
    if sep1.0 > 0.0 {
        manifold.points.clear();
        return;
    }

    let mut sep2 = sat::polygon_polygon_compute_separation_features(p2, p1, &m21);
    if sep2.0 > 0.0 {
        manifold.points.clear();
        return;
    }

    let mut swapped = false;
    if sep2.0 > sep1.0 {
        core::mem::swap(&mut sep1, &mut sep2);
        core::mem::swap(&mut m1, &mut m2);
        core::mem::swap(&mut p1, &mut p2);
        core::mem::swap(&mut m12, &mut m21);
        manifold.swap_identifiers();
        swapped = true;
    }

    let support_face1 = sep1.1;
    let local_n1 = p1.normals[support_face1];
    let local_n2 = m21.rotation * -local_n1;
    let support_face2 = p2.support_face(&local_n2);
    let len1 = p1.vertices.len();
    let len2 = p2.vertices.len();

    let seg1 = (
        p1.vertices[support_face1],
        p1.vertices[(support_face1 + 1) % len1],
    );
    let seg2 = (
        m12 * p2.vertices[support_face2],
        m12 * p2.vertices[(support_face2 + 1) % len2],
    );
    if let Some((clip_a, clip_b)) = query::details::clip_segment_segment(seg1, seg2) {
        let dist_a = (clip_a.1 - clip_a.0).dot(local_n1);
        let dist_b = (clip_b.1 - clip_b.0).dot(local_n1);

        let mut data_a = ContactData::default();
        let mut data_b = ContactData::default();

        let fids_a = (
            ((support_face1 * 2 + clip_a.2) % (len1 * 2)) as u8,
            ((support_face2 * 2 + clip_a.3) % (len2 * 2)) as u8,
        );

        let fids_b = (
            ((support_face1 * 2 + clip_b.2) % (len1 * 2)) as u8,
            ((support_face2 * 2 + clip_b.3) % (len2 * 2)) as u8,
        );

        if !manifold.points.is_empty() {
            assert_eq!(manifold.points.len(), 2);

            // We already had 2 points in the previous iteration.
            // Match the features to see if we keep the cached impulse.
            let original_fids_a;
            let original_fids_b;

            // NOTE: the previous manifold may have its bodies swapped wrt. our new manifold.
            // So we have to adjust accordingly the features we will be comparing.
            if swapped {
                original_fids_a = (manifold.points[0].fid1, manifold.points[0].fid2);
                original_fids_b = (manifold.points[1].fid1, manifold.points[1].fid2);
            } else {
                original_fids_a = (manifold.points[0].fid2, manifold.points[0].fid1);
                original_fids_b = (manifold.points[1].fid2, manifold.points[1].fid1);
            }

            if fids_a == original_fids_a {
                data_a = manifold.points[0].data;
            } else if fids_a == original_fids_b {
                data_a = manifold.points[1].data;
            }

            if fids_b == original_fids_a {
                data_b = manifold.points[0].data;
            } else if fids_b == original_fids_b {
                data_b = manifold.points[1].data;
            }
        }

        manifold.points.clear();
        manifold.points.push(Contact {
            local_p1: clip_a.0,
            local_p2: m21 * clip_a.1,
            fid1: fids_a.0,
            fid2: fids_a.1,
            dist: dist_a,
            data: data_a,
        });

        manifold.points.push(Contact {
            local_p1: clip_b.0,
            local_p2: m21 * clip_b.1,
            fid1: fids_b.0,
            fid2: fids_b.1,
            dist: dist_b,
            data: data_b,
        });

        manifold.local_n1 = local_n1;
        manifold.local_n2 = local_n2;
    } else {
        manifold.points.clear();
    }
}
