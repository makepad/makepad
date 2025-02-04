use {
    super::{
        loader::{Options, Definitions, Loader},
        geometry::Point,
        pixels::Bgra,
    },
    makepad_platform::*,
};

#[derive(Debug)]
pub struct Layouter {
    loader: Loader
}

impl Layouter {
    pub fn new(loader: Loader) -> Self {
        Self {
            loader,
        }
    }

    pub fn loader(&self) -> &Loader {
        &self.loader
    }

    pub fn loader_mut(&mut self) -> &mut Loader {
        &mut self.loader
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{super::font_family::FontFamilyId, *},
        std::{fs::File, io::BufWriter},
    };

    #[test]
    fn test() {
        let loader = Loader::new(Options::default(), Definitions::default());
        let mut layouter = Layouter::new(loader);
        let font_family = layouter.loader_mut().font_family(&FontFamilyId::Sans);
        let glyphs = font_family.shape("HalloRik!繁😊😔");
        for glyph in &*glyphs {
            glyph.font.allocate_glyph(glyph.id, 64.0);
        }

        let file = File::create("/Users/ejpbruel/Desktop/grayscale.png").unwrap();
        let writer = BufWriter::new(file);
        let atlas = layouter.loader().grayscale_atlas().borrow();
        let mut encoder = png::Encoder::new(
            writer,
            atlas.size().width as u32,
            atlas.size().height as u32,
        );
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = atlas.image().pixels();
        let data =
            unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len()) };
        writer.write_image_data(&data).unwrap();

        let file = File::create("/Users/ejpbruel/Desktop/color.png").unwrap();
        let writer = BufWriter::new(file);
        let atlas = layouter.loader().color_atlas().borrow();
        let mut encoder = png::Encoder::new(
            writer,
            atlas.size().width as u32,
            atlas.size().height as u32,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        let pixels = atlas.image().pixels();
        let data =
            unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4) };
        writer.write_image_data(&data).unwrap();
    }
}
