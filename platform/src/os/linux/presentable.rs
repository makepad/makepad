use crate::{Cx, Texture, texture::{TextureFormat, TextureUpdated}};

impl Cx {
    /// The Linux shared-memory transport contains RGBA bytes in GL row order.
    /// Preserve rows for RunView's existing flip and convert to the renderer
    /// independent BGRA texture format. The texture handle stays stable.
    pub fn upload_presentable_image_software_buffer(
        &mut self, texture: &Texture, width: u32, height: u32, pixels: &[u8],
    ) {
        let Some(len) = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)) else {
            crate::error!("software frame dimensions overflow");
            return;
        };
        if len == 0 || pixels.len() < len {
            crate::error!("invalid software frame length: {} for {}x{}", pixels.len(), width, height);
            return;
        }
        let format = texture.get_format(self);
        if !matches!(format, TextureFormat::VecBGRAu8_32 { width: w, height: h, .. } if *w == width as usize && *h == height as usize) {
            *format = TextureFormat::VecBGRAu8_32 {
                width: width as usize, height: height as usize, data: Some(Vec::new()), updated: TextureUpdated::Full,
            };
        }
        let TextureFormat::VecBGRAu8_32 { data, updated, .. } = format else { unreachable!() };
        let data = data.get_or_insert_with(Vec::new);
        data.resize(len / 4, 0);
        for (dst, src) in data.iter_mut().zip(pixels[..len].chunks_exact(4)) {
            *dst = u32::from_le_bytes([src[2], src[1], src[0], src[3]]);
        }
        *updated = TextureUpdated::Full;
    }
}
