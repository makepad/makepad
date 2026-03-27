use makepad_widgets::{DVec2, Mat4f, Rect};

use crate::{MpBackfaceVisibility, MpTransformStyle};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpSpatialId(pub usize);

#[derive(Clone, Debug)]
pub struct MpSpatialNode {
    pub parent: Option<MpSpatialId>,
    pub kind: MpSpatialKind,
}

#[derive(Clone, Debug)]
pub enum MpSpatialKind {
    ReferenceFrame(MpReferenceFrame),
    ScrollFrame(MpScrollFrame),
    StickyFrame(MpStickyFrame),
    EmbedRoot(MpEmbedRoot),
}

#[derive(Clone, Debug)]
pub struct MpReferenceFrame {
    pub viewport_rect: Rect,
    pub placement_origin: DVec2,
    pub transform: Option<Mat4f>,
    pub perspective: Option<Mat4f>,
    pub transform_style: MpTransformStyle,
    pub backface_visibility: MpBackfaceVisibility,
    pub flattens_descendants: bool,
}

#[derive(Clone, Debug)]
pub struct MpScrollFrame {
    pub viewport_rect: Rect,
    pub content_rect: Rect,
    pub scroll_offset: DVec2,
}

#[derive(Clone, Debug)]
pub struct MpStickyFrame {
    pub frame_rect: Rect,
    pub containing_block_rect: Rect,
    pub margins: MpStickyOffsets,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MpStickyOffsets {
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct MpEmbedRoot {
    pub rect: Rect,
}

impl MpSpatialNode {
    pub fn root(viewport_rect: Rect) -> Self {
        Self {
            parent: None,
            kind: MpSpatialKind::ReferenceFrame(MpReferenceFrame {
                viewport_rect,
                placement_origin: viewport_rect.pos,
                transform: None,
                perspective: None,
                transform_style: MpTransformStyle::Flat,
                backface_visibility: MpBackfaceVisibility::Visible,
                flattens_descendants: true,
            }),
        }
    }
}
