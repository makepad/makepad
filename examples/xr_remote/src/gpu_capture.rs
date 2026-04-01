use crate::protocol::*;
use makepad_widgets::makepad_platform::{
    DrawPass, DrawPassClearColor, Texture, TextureFormat, TextureId, TextureSize,
};
use makepad_widgets::*;

pub struct EyeRenderTarget {
    pub pass: DrawPass,
    pub color_texture: Texture,
    pub draw_list: DrawList,
}

impl EyeRenderTarget {
    pub fn new(cx: &mut Cx, width: u32, height: u32) -> Self {
        let pass = DrawPass::new(cx);
        let color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: width as usize,
                    height: height as usize,
                },
                initial: true,
            },
        );
        pass.set_size(cx, dvec2(width as f64, height as f64));
        pass.set_color_texture(
            cx,
            &color_texture,
            DrawPassClearColor::ClearWith(vec4(0.02, 0.03, 0.06, 1.0)),
        );

        EyeRenderTarget {
            pass,
            color_texture,
            draw_list: DrawList::new(cx),
        }
    }

    pub fn texture_id(&self) -> TextureId {
        self.color_texture.texture_id()
    }

    pub fn texture(&self) -> Texture {
        self.color_texture.clone()
    }
}

pub struct GpuCapture {
    pub eyes: [Option<EyeRenderTarget>; 2],
    pub width: u32,
    pub height: u32,
}

impl Default for GpuCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl GpuCapture {
    pub fn new() -> Self {
        GpuCapture {
            eyes: [None, None],
            width: XR_REMOTE_STREAM_WIDTH,
            height: XR_REMOTE_STREAM_HEIGHT,
        }
    }

    pub fn ensure_targets(&mut self, cx: &mut Cx) {
        for eye in XrRemoteEye::ALL {
            if self.eyes[eye.index()].is_none() {
                self.eyes[eye.index()] = Some(EyeRenderTarget::new(cx, self.width, self.height));
            }
        }
    }

    pub fn eye_target(&self, eye: XrRemoteEye) -> Option<&EyeRenderTarget> {
        self.eyes[eye.index()].as_ref()
    }

    #[allow(dead_code)]
    pub fn eye_target_mut(&mut self, eye: XrRemoteEye) -> Option<&mut EyeRenderTarget> {
        self.eyes[eye.index()].as_mut()
    }
}
