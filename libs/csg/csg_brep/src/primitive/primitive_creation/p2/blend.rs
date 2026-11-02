//! A blend between two face boundary edges.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_blend_primitive_between_faces`] | face/edge `CreateBlendPrimitive` |
//!
//! Deviations: face and edge pairs are typed ids of the target B-rep. The edgeuse
//! overload's status is ignored, as upstream; only `Unported` passes. No recursion.

use super::PrimitiveCreationAccess;
use crate::error::{ignore_status, Result, SolidError};
use crate::primitive::primitive_creation::{p1::PrimitiveCreation, p3::BlendPrimitiveOptions};
use crate::surface::surface::p2::{BlendEndType, CurvatureType, DerivSurfDefinition};
use crate::topology::{BrepId, EdgeId, FaceId};

const OP: &str = "primitive::primitive_creation::p2::create_blend_primitive_between_faces";

impl PrimitiveCreation {
    /// Blend from `first`'s edge on its face to `second`'s edge on its face
    /// (`CreateBlendPrimitive(pFace1, pEdge1, pFace2, pEdge2, ...)`). Without
    /// `options` the blend is G2 from the surfaces, uncusped, with unit tangent
    /// lengths. The end tangent length is `end_tan_scale * start_tan_length`.
    ///
    /// # Errors
    ///
    /// `SM_ERR_INVALID_INPUT` when either edge has no edge use on its face. The
    /// blend itself never fails this call (upstream ignores its status), except
    /// for `Unported`.
    pub fn create_blend_primitive_between_faces(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        first: (FaceId, EdgeId),
        second: (FaceId, EdgeId),
        same_direction_curves: bool,
        options: Option<&DerivSurfDefinition<'_>>,
        blend_face: &mut Option<FaceId>,
    ) -> Result<()> {
        // Get edgeuse connecting edge to face
        let first_edgeuse =
            crate::topology::edge::edgeuse_of_face(a.brep(brep)?, first.1, Some(first.0))?;
        let second_edgeuse =
            crate::topology::edge::edgeuse_of_face(a.brep(brep)?, second.1, Some(second.0))?;
        if first_edgeuse.is_none() || second_edgeuse.is_none() {
            return Err(SolidError::invalid_input(
                OP,
                "edge has no edge use on its face",
            ));
        }

        // extract values from the options
        let curvature = options.map_or(CurvatureType::G2FromSurface, |o| o.curvature_type);
        let ends = options.map_or(BlendEndType::NoCusp, |o| o.blend_end_type);
        let start_tangent_length = options.map_or(1.0, |o| o.start_tan_length);
        let end_tangent_length = options.map_or(1.0, |o| o.end_tan_scale * start_tangent_length);
        let curves_direction = if same_direction_curves { 1 } else { 2 };

        // Pass call along
        ignore_status(self.create_blend_primitive(
            a,
            brep,
            first_edgeuse,
            second_edgeuse,
            blend_face,
            BlendPrimitiveOptions {
                curvature,
                ends,
                curves_direction,
                start_tangent_length,
                end_tangent_length,
            },
        ))?;
        Ok(())
    }
}
