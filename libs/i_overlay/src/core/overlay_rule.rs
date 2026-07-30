use crate::segm::segment::{BOTH_BOTTOM, BOTH_TOP, CLIP_TOP, NONE, SUBJ_TOP, SegmentFill};
use core::fmt;

/// Defines the types of overlay/boolean operations that can be applied to shapes. For a visual description, see [Overlay Rules](https://ishape-rust.github.io/iShape-js/overlay/overlay_rules/overlay_rules.html).
/// - `Subject`: Processes the subject shape, useful for resolving self-intersections and degenerate cases within the subject itself.
/// - `Clip`: Similar to `Subject`, but for Clip shapes.
/// - `Intersect`: Finds the common area between the subject and clip shapes, effectively identifying where they overlap.
/// - `Union`: Combines the area of both subject and clip shapes into a single unified shape.
/// - `Difference`: Subtracts the area of the clip shape from the subject shape, removing the clip shape's area from the subject.
/// - `InverseDifference`: Subtracts the area of the subject shape from the clip shape, removing the subject shape's area from the clip.
/// - `Xor`: Produces a shape consisting of areas unique to each shape, excluding any parts where the subject and clip overlap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayRule {
    Subject,
    Clip,
    Intersect,
    Union,
    Difference,
    InverseDifference,
    Xor,
}

impl OverlayRule {
    #[inline(always)]
    pub(crate) fn is_fill_top(&self, fill: SegmentFill) -> bool {
        match self {
            OverlayRule::Subject => fill & SUBJ_TOP == SUBJ_TOP,
            OverlayRule::Clip => fill & CLIP_TOP == CLIP_TOP,
            OverlayRule::Intersect => fill & BOTH_TOP == BOTH_TOP,
            OverlayRule::Union => fill & BOTH_BOTTOM == NONE,
            OverlayRule::Difference => fill & BOTH_TOP == SUBJ_TOP,
            OverlayRule::InverseDifference => fill & BOTH_TOP == CLIP_TOP,
            OverlayRule::Xor => {
                let is_subject = fill & BOTH_TOP == SUBJ_TOP;
                let is_clip = fill & BOTH_TOP == CLIP_TOP;
                is_subject || is_clip
            }
        }
    }
}

impl fmt::Display for OverlayRule {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let text = match self {
            OverlayRule::Subject => "Subject",
            OverlayRule::Clip => "Clip",
            OverlayRule::Intersect => "Intersect",
            OverlayRule::Union => "Union",
            OverlayRule::Difference => "Difference",
            OverlayRule::InverseDifference => "InverseDifference",
            OverlayRule::Xor => "Xor",
        };

        write!(f, "{}", text)
    }
}
