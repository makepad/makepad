//! The fork's section placement and corner-patch marks of the live session.
//!
//! Upstream: the fork source fork `fd3b6a7` (`SmFilletExecutive::SetBlendCurveAtThirds`)
//! and `60af574` (`SmFilletExecutive::SetMarkCornerPatches`, `SmFilletCorner::GetPatchFaces`,
//! the marking loop of `DoFilleting`); neither is in SMLib at the pin.
//!
//! | Rust | Upstream (fork) |
//! |------|-----------------|
//! | [`FilletSession::set_blend_curve_at_thirds`] | `SmFilletExecutive::SetBlendCurveAtThirds` |
//! | [`FilletSession::set_mark_corner_patches`] | `SmFilletExecutive::SetMarkCornerPatches` |
//! | `mark_corner_patches` (private) | the `m_bMarkCornerPatches` loop of `DoFilleting` |
//! | `patch_faces` (private) | `SmFilletCorner::GetPatchFaces` |
//! | [`FilletSession::corner_patch_faces`] | `round_edges` reading the `SM_AI_FILLET_CORNER_FACE` marks |
//!
//! Deviations: both settings are per session and off by default, so a session that
//! does not set them is the pinned C++'s. The marks live in the session's attribute
//! store (keyed by the intermediate faces, then by the target copies the merge makes,
//! as the fork's attributes travel with the copy); [`FilletSession::corner_patch_faces`]
//! reads them from the target faces, and they go with the session. No recursion.

use super::{invalid, FilletObject, FilletSession};
use crate::error::{EntityRef, Result, SolidError};
use crate::geom::attribute::{ids, Attribute, AttributeValue, Behavior, TagData};
use crate::topology::brep::p1;
use crate::topology::FaceId;

const OP: &str = "blend::fillet_executive::session::corner_marks";

impl FilletSession<'_> {
    /// `SetBlendCurveAtThirds` (the fork's, source fork `fd3b6a7`; off by
    /// default): the blend-curve generators the prepare calls create
    /// (`SetFilletParameters`) place each section's inner control points a third and
    /// two thirds of the way from each contact to where the contacts' tangent lines
    /// meet (the end derivatives scaled to `n/3` of that distance for degree `n`, the
    /// second and third derivatives by its square and cube). Set it before preparing
    /// the edges. Off, the sections are the pinned C++'s faired Hermite scaled by the
    /// thumbweight.
    pub fn set_blend_curve_at_thirds(&mut self, at_thirds: bool) {
        self.executive.set_blend_curve_at_thirds(at_thirds);
    }

    /// `SetMarkCornerPatches` (the fork's, source fork `60af574`; off by
    /// default): before the merge, `do_filleting` marks each face of every set-back
    /// corner's patch (the faces on the surfaces the corner made) with an
    /// `SM_AI_FILLET_CORNER_FACE` tag whose primary id is the corner's index in
    /// [`Self::fillet_corners`]; the mark travels with the copy the merge gives the
    /// target. Read the result with [`Self::corner_patch_faces`]. Set it before
    /// filleting.
    pub fn set_mark_corner_patches(&mut self, mark: bool) {
        self.executive.set_mark_corner_patches(mark);
    }

    /// The target faces carrying an `SM_AI_FILLET_CORNER_FACE` mark, in the target's
    /// face order, each with the index in [`Self::fillet_corners`] of the corner whose
    /// patch it belongs to (empty unless [`Self::set_mark_corner_patches`] was set
    /// before filleting).
    ///
    /// # Errors
    /// A mark that is not a tag attribute, or whose corner index is negative.
    pub fn corner_patch_faces(&self) -> Result<Vec<(usize, FaceId)>> {
        let mut out = Vec::new();
        for face in p1::faces(self.objects.target, None)? {
            let key = self.target_identity(EntityRef::Face(face)).key;
            let Some(handle) = self
                .objects
                .attributes
                .find_attribute(key, ids::FILLET_CORNER_FACE)
            else {
                continue;
            };
            let attribute = self
                .objects
                .attributes
                .get(handle)
                .ok_or_else(|| invalid("corner mark is not live"))?;
            let AttributeValue::Tag(tag) = attribute.value() else {
                return Err(invalid("corner mark is not a tag attribute"));
            };
            let index = usize::try_from(tag.primary_id)
                .map_err(|_| invalid("corner mark names a negative corner index"))?;
            out.push((index, face));
        }
        Ok(out)
    }

    /// The `m_bMarkCornerPatches` loop of the fork's `DoFilleting`, after the fillet
    /// faces are shrunk and before the merge: each face of a set-back corner's patch
    /// gets its own `SmTagAttribute(SM_AI_FILLET_CORNER_FACE, i)` (`SM_AB_COPY`).
    pub(super) fn mark_corner_patches(&mut self) -> Result<()> {
        if !self.executive.mark_corner_patches {
            return Ok(());
        }
        for (i, corner) in self.executive.corners.iter().enumerate() {
            let FilletObject::Corner(id) = *corner else {
                return Err(invalid("executive corner is not a session corner"));
            };
            let record = self
                .objects
                .corners
                .get(id.0 as usize)
                .ok_or_else(|| invalid("corner is not live in this session"))?;
            #[allow(clippy::neg_cmp_op_on_partial_ord)] // upstream `!(d > 0.0)`: NaN skips
            if !(record.set_back_dist > 0.0) {
                continue;
            }
            let patch = self.patch_faces(id.0 as usize)?;
            let primary_id =
                i64::try_from(i).map_err(|_| invalid("corner index exceeds the tag range"))?;
            for face in patch {
                let mark = Attribute::tag(
                    ids::FILLET_CORNER_FACE,
                    TagData {
                        primary_id,
                        ..TagData::default()
                    },
                    Behavior::Copy,
                );
                let handle = self.objects.attributes.insert(mark)?;
                let key = self
                    .objects
                    .intermediate_identity(EntityRef::Face(face))?
                    .key;
                self.objects.attributes.add_attribute(key, handle)?;
            }
        }
        Ok(())
    }

    /// `SmFilletCorner::GetPatchFaces`: the intermediate-body face on each surface in
    /// `m_vSurfaces`, in order; `NER` (`SM_ERR_NULL_POINTER`) for a surface without a
    /// live face.
    fn patch_faces(&self, corner: usize) -> Result<Vec<FaceId>> {
        let record = self
            .objects
            .corners
            .get(corner)
            .ok_or_else(|| invalid("corner is not live in this session"))?;
        let body = self
            .objects
            .intermediate
            .as_ref()
            .ok_or_else(|| invalid("intermediate body is not allocated"))?;
        let mut faces = Vec::new();
        for index in 0..record.surfaces.len() {
            let face = record
                .surface_faces
                .get(index)
                .copied()
                .filter(|&face| body.face(face).is_ok())
                .ok_or_else(|| SolidError::null_pointer(OP))?;
            faces.push(face);
        }
        Ok(faces)
    }
}
