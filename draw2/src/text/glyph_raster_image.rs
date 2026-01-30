use {
    super::{
        geom::{Point, Rect, Size},
        image::{Bgra, SubimageMut},
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
};

#[cfg(feature = "png")]
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

    #[cfg(feature = "png")]
    pub fn decode_size(&self) -> Size<usize> {
        match self.format {
            Format::Png => self.decode_size_png(),
        }
    }

    #[cfg(not(feature = "png"))]
    pub fn decode_size(&self) -> Size<usize> {
        // Without PNG support, return zero size
        let _ = self.data;
        Size { width: 0, height: 0 }
    }

    #[cfg(feature = "png")]
    fn decode_size_png(&self) -> Size<usize> {
        let mut decoder = PngDecoder::new(self.data);
        if decoder.decode_headers().is_err() {
            return Size { width: 0, height: 0 };
        }
        decoder.get_dimensions()
            .map(|(w, h)| Size { width: w, height: h })
            .unwrap_or(Size { width: 0, height: 0 })
    }

    #[cfg(feature = "png")]
    pub fn decode(&self, image: &mut SubimageMut<Bgra>) {
        match self.format {
            Format::Png => self.decode_png(image),
        }
    }

    #[cfg(not(feature = "png"))]
    pub fn decode(&self, _image: &mut SubimageMut<Bgra>) {
        // PNG decoding not available without the png feature
    }

    #[cfg(feature = "png")]
    fn decode_png(&self, image: &mut SubimageMut<Bgra>) {
        let mut decoder = PngDecoder::new(self.data);
        if decoder.decode_headers().is_err() {
            return;
        }
        
        let (width, height) = match decoder.get_dimensions() {
            Some(dims) => dims,
            None => return,
        };
        
        let colorspace = match decoder.get_colorspace() {
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
