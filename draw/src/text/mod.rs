pub mod atlas;
pub mod faces;
pub mod font;
pub mod font_data;
pub mod font_family;
pub mod fonts;
pub mod geom;
pub mod image;
pub mod layouter;
pub mod loader;
pub mod non_nan;
pub mod num;
pub mod outline;
pub mod pixels;
pub mod raster_image;
pub mod shaper;
pub mod substr;

#[cfg(test)]
mod tests {
    use {
        super::*,
        layouter::Layouter,
        loader::Definitions,
        std::{fs::File, io::BufWriter},
    };

    #[test]
    fn test() {
        let mut layouter = Layouter::new(Definitions::default());

        let font_family = layouter.get_or_load_font_family(&"Sans".into());
        let output = font_family.get_or_shape("HalloRik!繁😊😔".into());
        for glyph in &output.glyphs {
            glyph.font.allocate_glyph(glyph.glyph_id, 64.0);
        }

        let file = File::create("/Users/ejpbruel/Desktop/grayscale.png").unwrap();
        let writer = BufWriter::new(file);
        let atlas = layouter.grayscale_atlas().borrow();
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
        let atlas = layouter.color_atlas().borrow();
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
