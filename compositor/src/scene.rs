use crate::*;

pub type MpNodeId = usize;
pub type MpClipNodeId = MpNodeId;

pub struct MpScene {
    pub root: MpSceneRoot,
    pub nodes: Vec<MpNode>,
}

pub struct MpSceneRoot {
    pub host_rect: Rect,
    pub page_to_host: Mat4f,
    pub clip: Option<MpClipNodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpTransformStyle {
    Flat,
    Preserve3D,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpBackfaceVisibility {
    Visible,
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MpClipShape {
    Rect { rect: Rect },
    RoundedRect { rect: Rect, radius: Vec4f },
    ImageMask { rect: Rect },
    PlaneSet { planes: Vec<Vec4f> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MpFilterSet {
    pub entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MpBlendMode {
    Normal,
    Named(String),
}

impl Default for MpBlendMode {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Clone, Debug)]
pub enum MpMaskSource {
    Clip(MpClipNodeId),
    SurfaceTexture(Texture),
}

pub struct MpReferenceFrame {
    pub parent: Option<MpNodeId>,
    pub clip: Option<MpClipNodeId>,
    pub local_rect: Rect,
    pub transform: Mat4f,
    pub perspective: Option<Mat4f>,
    pub transform_style: MpTransformStyle,
    pub backface_visibility: MpBackfaceVisibility,
    pub flattens_descendants: bool,
}

pub struct MpClipNode {
    pub parent: Option<MpNodeId>,
    pub prev: Option<MpClipNodeId>,
    pub shape: MpClipShape,
}

pub enum MpSurfaceSource {
    Texture(Texture),
    SurfaceTexture(Texture),
}

pub struct MpSurfaceNode {
    pub parent: MpNodeId,
    pub clip: Option<MpClipNodeId>,
    pub local_rect: Rect,
    pub source: MpSurfaceSource,
    pub backface_visibility: MpBackfaceVisibility,
}

pub struct MpEffectNode {
    pub parent: MpNodeId,
    pub clip: Option<MpClipNodeId>,
    pub opacity: f32,
    pub filter: MpFilterSet,
    pub blend_mode: MpBlendMode,
    pub is_isolated: bool,
    pub mask: Option<MpMaskSource>,
}

pub struct MpEmbedNode {
    pub parent: MpNodeId,
    pub clip: Option<MpClipNodeId>,
    pub local_rect: Rect,
    pub child_scene: Box<MpScene>,
}

pub enum MpNode {
    ReferenceFrame(MpReferenceFrame),
    Clip(MpClipNode),
    Surface(MpSurfaceNode),
    Effect(MpEffectNode),
    Embed(MpEmbedNode),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MpProjectedPoint {
    pub screen_point: DVec2,
    pub depth: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MpHit {
    pub node_id: MpNodeId,
    pub local_point: DVec2,
    pub depth: f32,
    pub clip_hit: bool,
    pub backface_visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MpHitTestOptions {
    pub clip: bool,
    pub backface: bool,
}

impl Default for MpHitTestOptions {
    fn default() -> Self {
        Self {
            clip: true,
            backface: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MpProjectError {
    MissingNode(MpNodeId),
    WrongNodeKind(MpNodeId),
    NotInvertible(MpNodeId),
    NotProjectable(MpNodeId),
    BackfaceHidden(MpNodeId),
    Clipped(MpNodeId),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MpMaskKind {
    RoundedRect { rect: Rect, radius: Vec4f },
    ImageMask { rect: Rect },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MpEvaluatedMask {
    pub kind: MpMaskKind,
    pub clip_to_local: Mat4f,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MpMaskExec {
    pub masks: Vec<MpEvaluatedMask>,
}

#[derive(Clone, Debug)]
pub(crate) struct Mp3dClipState {
    pub local_planes: Vec<Vec4f>,
}

#[derive(Clone, Debug)]
pub(crate) struct Mp3dSurfaceExec {
    pub texture: Texture,
    pub rect: Rect,
    pub opacity: f32,
    pub transform_matrix: Mat4f,
    pub perspective_matrix: Mat4f,
    pub backface_visibility: MpBackfaceVisibility,
    pub clip: Mp3dClipState,
    pub mask: MpMaskExec,
}
