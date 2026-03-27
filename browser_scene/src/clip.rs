use makepad_widgets::{Rect, Vec4f};

use crate::{MpPerCornerRadius, MpSpatialId};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpClipId(pub usize);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpClipChainId(pub usize);

#[derive(Clone, Debug)]
pub struct MpClipNode {
    pub spatial_id: MpSpatialId,
    pub kind: MpClipKind,
}

#[derive(Clone, Debug)]
pub enum MpClipKind {
    Rect { rect: Rect },
    RoundedRect { rect: Rect, radius: MpPerCornerRadius },
    ImageMask { rect: Rect },
    PlaneSet { planes: Vec<Vec4f> },
}

#[derive(Clone, Debug)]
pub struct MpClipChain {
    pub parent: Option<MpClipChainId>,
    pub clips: Vec<MpClipId>,
}

impl MpClipChain {
    pub fn root() -> Self {
        Self {
            parent: None,
            clips: Vec::new(),
        }
    }
}
