use makepad_widgets::Rect;

use crate::{MpClipChainId, MpSpatialId};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpEffectId(pub usize);

#[derive(Clone, Debug)]
pub struct MpEffectNode {
    pub spatial_id: MpSpatialId,
    pub clip_chain_id: MpClipChainId,
    pub opacity: f32,
    pub filters: Vec<MpFilter>,
    pub blend_mode: MpBlendMode,
    pub isolation: MpIsolation,
    pub mask: Option<MpMask>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum MpBlendMode {
    #[default]
    Normal,
    Named(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum MpFilter {
    Blur(f32),
    Opacity(f32),
    Named(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MpIsolation {
    #[default]
    Auto,
    Isolate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MpMask {
    Rect { rect: Rect },
    RoundedRect { rect: Rect, radius: f32 },
    ImageMask { rect: Rect },
}

impl MpEffectNode {
    pub fn requires_isolation(&self) -> bool {
        self.opacity < 0.999
            || !self.filters.is_empty()
            || !matches!(self.blend_mode, MpBlendMode::Normal)
            || matches!(self.isolation, MpIsolation::Isolate)
            || self.mask.is_some()
    }
}
