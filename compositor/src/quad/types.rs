use crate::scene::MpMaskExec;
use crate::*;

#[derive(Clone, Debug)]
pub struct MpCompositedQuad {
    pub texture: Texture,
    pub local_rect: Rect,
    pub uv_rect: Rect,
    pub transform: Mat4f,
    pub opacity: f32,
    pub premultiplied: bool,
    pub backface_visible: bool,
    pub depth_write: bool,
    pub clip_planes: Vec<Vec4f>,
    pub mask: MpMaskExec,
}

impl MpCompositedQuad {
    pub fn new(texture: Texture, local_rect: Rect) -> Self {
        Self {
            texture,
            local_rect,
            uv_rect: Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(1.0, 1.0),
            },
            transform: Mat4f::identity(),
            opacity: 1.0,
            premultiplied: true,
            backface_visible: true,
            depth_write: true,
            clip_planes: Vec::new(),
            mask: MpMaskExec::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MpInternal3dQuad {
    pub(crate) texture: Texture,
    pub(crate) rect: Rect,
    pub(crate) transform_matrix: Mat4f,
    pub(crate) perspective_matrix: Mat4f,
    pub(crate) opacity: f32,
    pub(crate) backface_visible: bool,
    pub(crate) depth_write: bool,
    pub(crate) clip_planes: Vec<Vec4f>,
    pub(crate) mask: MpMaskExec,
}

#[derive(Clone, Debug)]
pub(crate) struct MpInternal3dIsland {
    pub(crate) quads: Vec<MpInternal3dQuad>,
}

#[derive(Clone, Debug)]
pub(crate) struct MpInternal3dBatch {
    pub(crate) islands: Vec<MpInternal3dIsland>,
}

pub(crate) fn make_3d_quad(
    texture: Texture,
    rect: Rect,
    transform_matrix: Mat4f,
    perspective_matrix: Mat4f,
    opacity: f32,
    backface_visible: bool,
    depth_write: bool,
    clip_planes: Vec<Vec4f>,
    mask: MpMaskExec,
) -> MpInternal3dQuad {
    MpInternal3dQuad {
        texture,
        rect,
        transform_matrix,
        perspective_matrix,
        opacity,
        backface_visible,
        depth_write,
        clip_planes,
        mask,
    }
}

pub(crate) fn make_3d_island(
    quads: Vec<MpInternal3dQuad>,
    _clip_rect: Rect,
) -> MpInternal3dIsland {
    MpInternal3dIsland { quads }
}

pub(crate) fn make_3d_batch(islands: Vec<MpInternal3dIsland>) -> MpInternal3dBatch {
    MpInternal3dBatch { islands }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_constructor_defaults_match_compositor_expectations() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let texture = Texture::new(&mut cx);
        let quad = MpCompositedQuad::new(
            texture,
            Rect {
                pos: dvec2(0.0, 0.0),
                size: dvec2(10.0, 20.0),
            },
        );

        assert_eq!(quad.uv_rect.pos, dvec2(0.0, 0.0));
        assert_eq!(quad.uv_rect.size, dvec2(1.0, 1.0));
        assert_eq!(quad.opacity, 1.0);
        assert!(quad.premultiplied);
        assert!(quad.backface_visible);
        assert!(quad.depth_write);
        assert!(quad.clip_planes.is_empty());
        assert!(quad.mask.masks.is_empty());
    }
}
