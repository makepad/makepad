use {
    super::{
        geom::{Point, Rect, Size},
        image::{Bgra, SubimageMut},
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
};

#[derive(Clone, Debug)]
pub struct GlyphRasterImage<'a> {
    origin_in_dpxs: Point<f32>,
    dpxs_per_em: f32,
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
        let decoder = png::Decoder::new(self.data);
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        Size {
            width: info.width as usize,
            height: info.height as usize,
        }
    }

    pub fn decode(&self, image: &mut SubimageMut<Bgra>) {
        match self.format {
            Format::Png => self.decode_png(image),
        }
    }

    fn decode_png(&self, image: &mut SubimageMut<Bgra>) {
        let decoder = png::Decoder::new(self.data);
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0; reader.output_buffer_size()];
        let output_info = reader.next_frame(&mut buffer).unwrap();
        let info = reader.info();
        let height = info.height as usize;
        let width = info.width as usize;
        match output_info.color_type {
            png::ColorType::Indexed => {
                let palette = info.palette.as_ref().unwrap();
                let trns = info.trns.as_ref();
                let mut set_pixel = |x, y, index| {
                    let base = index * 3;
                    let r = palette[base + 0];
                    let g = palette[base + 1];
                    let b = palette[base + 2];
                    let a = trns.map_or(255, |trns| trns.get(index).copied().unwrap_or(255));
                    image[Point::new(x, y)] = Bgra::new(b, g, r, a);
                };
                match output_info.bit_depth {
                    png::BitDepth::Four => {
                        let bytes_per_row = (width + 1) / 2;
                        for y in 0..height {
                            for x in 0..width {
                                let byte = buffer[y * bytes_per_row + x / 2];
                                set_pixel(
                                    x,
                                    y,
                                    if x % 2 == 0 { byte >> 4 } else { byte & 0x0F } as usize,
                                );
                            }
                        }
                    }
                    png::BitDepth::Eight => {
                        for y in 0..height as usize {
                            for x in 0..width as usize {
                                set_pixel(x, y, buffer[y * width + x] as usize);
                            }
                        }
                    }
                    _ => println!("WARNING: encountered rasterized glyph with unsupported bit depth"),
                }
            }
            _ => println!("WARNING: encountered rasterized glyph with unsupported color type"),
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
