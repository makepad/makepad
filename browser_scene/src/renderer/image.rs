use makepad_widgets::makepad_draw::{ImageBuffer, Texture};
use makepad_widgets::Cx;

use super::MpSceneLowerer;

impl MpSceneLowerer<'_> {
    pub(super) fn ensure_image_texture(
        &mut self,
        image_key: crate::MpImageKey,
        image: &crate::MpImageResource,
    ) -> Texture {
        if let Some(texture) = self.image_textures.get(&image_key) {
            return texture.clone();
        }
        let width = image.size.x.max(0.0).round() as usize;
        let height = image.size.y.max(0.0).round() as usize;
        let data = rgba_to_bgra_u32(&image.rgba8);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let texture: Texture = ImageBuffer {
            width,
            height,
            data,
            animation: None,
        }
        .into_new_texture(&mut cx);
        self.image_textures.insert(image_key, texture.clone());
        texture
    }
}

pub(super) fn rgba_to_bgra_u32(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|px| {
            (px[2] as u32)
                | ((px[1] as u32) << 8)
                | ((px[0] as u32) << 16)
                | ((px[3] as u32) << 24)
        })
        .collect()
}
