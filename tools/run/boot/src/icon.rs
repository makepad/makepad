//! Decode once on the host's loading thread; only a texture handle reaches frames.
use makepad_hal::{format, usage, Attr, CmdWriter, Id};
use makepad_zune_png::{makepad_zune_core::bytestream::ZCursor, PngDecoder};

// Texture ids have their own pool. The HAL lends its glyph atlas a different id.
pub const TEXTURE: Id = Id::new(0, 1);

pub struct Icon {
    size: [u32; 2],
    rgba: Vec<u8>,
}

impl Icon {
    pub fn decode(png: &[u8]) -> Option<Self> {
        let mut decoder = PngDecoder::new(ZCursor::new(png));
        decoder.decode_headers().ok()?;
        let (width, height) = decoder.dimensions()?;
        let components = decoder.colorspace()?.num_components();
        let decoded = decoder.decode().ok()?;
        let src = decoded.u8()?;
        let pixels = width.checked_mul(height)?;
        if pixels == 0
            || !(1..=4).contains(&components)
            || src.len() < pixels.checked_mul(components)?
        {
            return None;
        }
        let mut rgba = Vec::with_capacity(pixels.checked_mul(4)?);
        for c in src.chunks_exact(components).take(pixels) {
            match components {
                4 => rgba.extend_from_slice(c),
                3 => rgba.extend_from_slice(&[c[0], c[1], c[2], 255]),
                2 => rgba.extend_from_slice(&[c[0], c[0], c[0], c[1]]),
                1 => rgba.extend_from_slice(&[c[0], c[0], c[0], 255]),
                _ => return None,
            }
        }
        Some(Self {
            size: [width.try_into().ok()?, height.try_into().ok()?],
            rgba,
        })
    }

    pub(crate) fn upload(&self, writer: &mut CmdWriter) {
        writer.texture_create(
            TEXTURE,
            usage::SAMPLED,
            format::RGBA8,
            self.size[0],
            self.size[1],
            [1, 1],
            Attr::default(),
        );
        writer.texture_update(
            TEXTURE,
            0,
            [0, 0, self.size[0], self.size[1]],
            0,
            &self.rgba,
        );
    }

    pub(crate) fn aspect(&self) -> f32 {
        self.size[0] as f32 / self.size[1] as f32
    }
}
