use {
    super::{
        geom::{Point, Rect, Size},
        image::{Bgra, SubimageMut},
    },
    rustybuzz,
    rustybuzz::ttf_parser,
};

use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
use makepad_zune_png::PngDecoder;

#[derive(Clone, Debug)]
pub struct GlyphRasterImage<'a> {
    origin_in_dpxs: Point<f32>,
    dpxs_per_em: f32,
    #[allow(dead_code)]
    format: Format,
    data: &'a [u8],
}

impl<'a> GlyphRasterImage<'a> {
    pub fn from_raster_glyph_image(image: ttf_parser::RasterGlyphImage<'a>) -> Option<Self> {
        Some(Self {
            origin_in_dpxs: Point::new(image.x as f32, image.y as f32),
            dpxs_per_em: image.pixels_per_em as f32,
            format: Format::from_raster_image_format(image.format)?,
            data: image.data,
        })
    }

    pub fn origin_in_dpxs(&self) -> Point<f32> {
        self.origin_in_dpxs
    }

    pub fn size_in_dpxs(&self) -> Size<f32> {
        let size = self.decode_size();
        Size::new(size.width as f32, size.height as f32)
    }

    pub fn bounds_in_dpxs(&self) -> Rect<f32> {
        Rect::new(self.origin_in_dpxs(), self.size_in_dpxs())
    }

    pub fn dpxs_per_em(&self) -> f32 {
        self.dpxs_per_em
    }

    pub fn decode_size(&self) -> Size<usize> {
        match self.format {
            Format::Png => self.decode_size_png(),
        }
    }

    fn decode_size_png(&self) -> Size<usize> {
        let cursor = ZCursor::new(self.data);
        let mut decoder = PngDecoder::new(cursor);
        if decoder.decode_headers().is_err() {
            return Size {
                width: 0,
                height: 0,
            };
        }
        decoder
            .dimensions()
            .map(|(w, h)| Size {
                width: w,
                height: h,
            })
            .unwrap_or(Size {
                width: 0,
                height: 0,
            })
    }

    pub fn decode(&self, image: &mut SubimageMut<Bgra>) {
        match self.format {
            Format::Png => self.decode_png(image),
        }
    }

    fn decode_png(&self, image: &mut SubimageMut<Bgra>) {
        let cursor = ZCursor::new(self.data);
        let mut decoder = PngDecoder::new(cursor);
        if decoder.decode_headers().is_err() {
            return;
        }

        let (width, height) = match decoder.dimensions() {
            Some(dims) => dims,
            None => return,
        };

        let colorspace = match decoder.colorspace() {
            Some(cs) => cs,
            None => return,
        };

        let decoded = match decoder.decode() {
            Ok(d) => d,
            Err(_) => return,
        };

        let buffer = match decoded.u8() {
            Some(b) => b,
            None => return,
        };

        let num_components = colorspace.num_components();

        match num_components {
            4 => {
                // RGBA
                for y in 0..height {
                    for x in 0..width {
                        let i = (y * width + x) * 4;
                        let r = buffer[i];
                        let g = buffer[i + 1];
                        let b = buffer[i + 2];
                        let a = buffer[i + 3];
                        image[Point::new(x, y)] = Bgra::new(b, g, r, a);
                    }
                }
            }
            3 => {
                // RGB
                for y in 0..height {
                    for x in 0..width {
                        let i = (y * width + x) * 3;
                        let r = buffer[i];
                        let g = buffer[i + 1];
                        let b = buffer[i + 2];
                        image[Point::new(x, y)] = Bgra::new(b, g, r, 255);
                    }
                }
            }
            2 => {
                // Grayscale + Alpha
                for y in 0..height {
                    for x in 0..width {
                        let i = (y * width + x) * 2;
                        let gray = buffer[i];
                        let a = buffer[i + 1];
                        image[Point::new(x, y)] = Bgra::new(gray, gray, gray, a);
                    }
                }
            }
            1 => {
                // Grayscale
                for y in 0..height {
                    for x in 0..width {
                        let gray = buffer[y * width + x];
                        image[Point::new(x, y)] = Bgra::new(gray, gray, gray, 255);
                    }
                }
            }
            _ => {
                println!("WARNING: encountered rasterized glyph with unsupported color type");
            }
        }
    }

    /// Decode the PNG into a contiguous BGRA buffer at its native size.
    fn decode_native_bgra(&self) -> Option<(usize, usize, Vec<Bgra>)> {
        let cursor = ZCursor::new(self.data);
        let mut decoder = PngDecoder::new(cursor);
        decoder.decode_headers().ok()?;
        let (width, height) = decoder.dimensions()?;
        let colorspace = decoder.colorspace()?;
        let decoded = decoder.decode().ok()?;
        let buffer = decoded.u8()?;
        let num_components = colorspace.num_components();
        let mut out = Vec::with_capacity(width.saturating_mul(height));
        match num_components {
            4 => {
                for i in (0..width * height * 4).step_by(4) {
                    out.push(Bgra::new(buffer[i + 2], buffer[i + 1], buffer[i], buffer[i + 3]));
                }
            }
            3 => {
                for i in (0..width * height * 3).step_by(3) {
                    out.push(Bgra::new(buffer[i + 2], buffer[i + 1], buffer[i], 255));
                }
            }
            2 => {
                for i in (0..width * height * 2).step_by(2) {
                    let gray = buffer[i];
                    out.push(Bgra::new(gray, gray, gray, buffer[i + 1]));
                }
            }
            1 => {
                for i in 0..width * height {
                    let gray = buffer[i];
                    out.push(Bgra::new(gray, gray, gray, 255));
                }
            }
            _ => return None,
        }
        Some((width, height, out))
    }

    /// Decode the raster image, box-downsampling (alpha-weighted) to fit `image`'s size — so large
    /// emoji strikes shrink to ~display size here instead of looking blocky when minified at draw time.
    /// If the destination is >= the source, this is a straight copy.
    pub fn decode_scaled(&self, image: &mut SubimageMut<Bgra>) {
        let Some((sw, sh, src)) = self.decode_native_bgra() else {
            return;
        };
        let dst = image.size();
        let (dw, dh) = (dst.width, dst.height);
        if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
            return;
        }
        if dw >= sw && dh >= sh {
            for y in 0..sh.min(dh) {
                for x in 0..sw.min(dw) {
                    image[Point::new(x, y)] = src[y * sw + x];
                }
            }
            return;
        }
        // Alpha-weighted box filter: premultiply RGB by alpha while accumulating so that
        // fully-transparent texels don't bleed their (undefined) color into the average.
        for dy in 0..dh {
            let sy0 = dy * sh / dh;
            let sy1 = (((dy + 1) * sh / dh).max(sy0 + 1)).min(sh);
            for dx in 0..dw {
                let sx0 = dx * sw / dw;
                let sx1 = (((dx + 1) * sw / dw).max(sx0 + 1)).min(sw);
                let (mut acc_b, mut acc_g, mut acc_r, mut acc_a, mut count) =
                    (0u32, 0u32, 0u32, 0u32, 0u32);
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let p = src[sy * sw + sx];
                        let a = p.a() as u32;
                        acc_b += p.b() as u32 * a;
                        acc_g += p.g() as u32 * a;
                        acc_r += p.r() as u32 * a;
                        acc_a += a;
                        count += 1;
                    }
                }
                let out = if count == 0 || acc_a == 0 {
                    Bgra::new(0, 0, 0, (acc_a / count.max(1)) as u8)
                } else {
                    Bgra::new(
                        (acc_b / acc_a) as u8,
                        (acc_g / acc_a) as u8,
                        (acc_r / acc_a) as u8,
                        (acc_a / count) as u8,
                    )
                };
                image[Point::new(dx, dy)] = out;
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Format {
    Png,
}

impl Format {
    pub fn from_raster_image_format(format: ttf_parser::RasterImageFormat) -> Option<Self> {
        match format {
            ttf_parser::RasterImageFormat::PNG => Some(Self::Png),
            _ => None,
        }
    }
}
