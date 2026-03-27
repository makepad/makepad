use makepad_widgets::{Rect, Vec2f, Vec4f};

use crate::{
    MpClipChainId, MpEffectId, MpGlyphRunKey, MpHitTestTag, MpImageKey, MpSpatialId,
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MpPrimitiveId(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MpPerCornerRadius {
    pub tl: f32,
    pub tr: f32,
    pub br: f32,
    pub bl: f32,
}

impl MpPerCornerRadius {
    pub fn uniform(radius: f32) -> Self {
        Self {
            tl: radius,
            tr: radius,
            br: radius,
            bl: radius,
        }
    }

    pub fn max(&self) -> f32 {
        self.tl.max(self.tr).max(self.br).max(self.bl)
    }

    pub fn outset(&self, delta: f32) -> Self {
        Self {
            tl: (self.tl + delta).max(0.0),
            tr: (self.tr + delta).max(0.0),
            br: (self.br + delta).max(0.0),
            bl: (self.bl + delta).max(0.0),
        }
    }
}

impl From<f32> for MpPerCornerRadius {
    fn from(radius: f32) -> Self {
        Self::uniform(radius)
    }
}

#[derive(Clone, Debug)]
pub struct MpPrimitive {
    pub id: MpPrimitiveId,
    pub spatial_id: MpSpatialId,
    pub clip_chain_id: MpClipChainId,
    pub effect_id: Option<MpEffectId>,
    pub bounds: Rect,
    pub kind: MpPrimitiveKind,
    pub hit_test_tag: Option<MpHitTestTag>,
}

#[derive(Clone, Debug)]
pub enum MpPrimitiveKind {
    SolidRect(MpSolidRect),
    RoundedRect(MpRoundedRect),
    Border(MpBorder),
    TextRun(MpTextRun),
    Image(MpImage),
    RepeatingImage(MpRepeatingImage),
    LinearGradient(MpLinearGradient),
    RadialGradient(MpRadialGradient),
    ConicGradient(MpConicGradient),
    BoxShadow(MpBoxShadow),
    LineDecoration(MpLineDecoration),
}

#[derive(Clone, Debug)]
pub struct MpSolidRect {
    pub color: Vec4f,
}

#[derive(Clone, Debug)]
pub struct MpRoundedRect {
    pub color: Vec4f,
    pub radius: MpPerCornerRadius,
}

#[derive(Clone, Debug)]
pub struct MpBorder {
    pub color: Vec4f,
    pub width: f32,
    pub radius: MpPerCornerRadius,
}

#[derive(Clone, Debug)]
pub struct MpTextRun {
    pub glyph_run_key: MpGlyphRunKey,
    pub color: Vec4f,
}

#[derive(Clone, Debug)]
pub struct MpImage {
    pub image_key: MpImageKey,
}

#[derive(Clone, Debug)]
pub struct MpRepeatingImage {
    pub image_key: MpImageKey,
}

#[derive(Clone, Debug)]
pub struct MpLinearGradient {
    pub start: Vec2f,
    pub end: Vec2f,
    pub repeating: bool,
    pub stops: Vec<MpGradientStop>,
}

#[derive(Clone, Debug)]
pub struct MpRadialGradient {
    pub center: Vec2f,
    pub radius: Vec2f,
    pub repeating: bool,
    pub stops: Vec<MpGradientStop>,
}

#[derive(Clone, Debug)]
pub struct MpConicGradient {
    pub center: Vec2f,
    pub start_angle_rad: f32,
    pub repeating: bool,
    pub stops: Vec<MpGradientStop>,
}

#[derive(Clone, Debug)]
pub struct MpGradientStop {
    pub offset: f32,
    pub color: Vec4f,
}

#[derive(Clone, Debug)]
pub struct MpBoxShadow {
    pub color: Vec4f,
    pub box_offset: Vec2f,
    pub box_size: Vec2f,
    pub sigma: f32,
    pub corner_radius_px: f32,
    pub inset: bool,
}

#[derive(Clone, Debug)]
pub struct MpLineDecoration {
    pub color: Vec4f,
    pub thickness: f32,
}

impl MpPrimitive {
    pub fn solid_rect(
        id: MpPrimitiveId,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
        bounds: Rect,
        color: Vec4f,
    ) -> Self {
        Self {
            id,
            spatial_id,
            clip_chain_id,
            effect_id: None,
            bounds,
            kind: MpPrimitiveKind::SolidRect(MpSolidRect { color }),
            hit_test_tag: None,
        }
    }

    pub fn rounded_rect(
        id: MpPrimitiveId,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
        bounds: Rect,
        color: Vec4f,
        radius: impl Into<MpPerCornerRadius>,
    ) -> Self {
        Self {
            id,
            spatial_id,
            clip_chain_id,
            effect_id: None,
            bounds,
            kind: MpPrimitiveKind::RoundedRect(MpRoundedRect {
                color,
                radius: radius.into(),
            }),
            hit_test_tag: None,
        }
    }

    pub fn border(
        id: MpPrimitiveId,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
        bounds: Rect,
        color: Vec4f,
        width: f32,
        radius: impl Into<MpPerCornerRadius>,
    ) -> Self {
        Self {
            id,
            spatial_id,
            clip_chain_id,
            effect_id: None,
            bounds,
            kind: MpPrimitiveKind::Border(MpBorder {
                color,
                width,
                radius: radius.into(),
            }),
            hit_test_tag: None,
        }
    }

    pub fn text_run(
        id: MpPrimitiveId,
        spatial_id: MpSpatialId,
        clip_chain_id: MpClipChainId,
        bounds: Rect,
        glyph_run_key: MpGlyphRunKey,
        color: Vec4f,
    ) -> Self {
        Self {
            id,
            spatial_id,
            clip_chain_id,
            effect_id: None,
            bounds,
            kind: MpPrimitiveKind::TextRun(MpTextRun {
                glyph_run_key,
                color,
            }),
            hit_test_tag: None,
        }
    }
}
