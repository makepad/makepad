pub mod font;
pub mod font_data;
pub mod font_face;
pub mod font_family;
pub mod font_loader;
pub mod fonts;
pub mod geometry;
pub mod image;
pub mod image_atlas;
pub mod layouter;
pub mod non_nan;
pub mod numeric;
pub mod outline;
pub mod pixels;
pub mod raster_image;
pub mod shaper;
pub mod substr;

#[cfg(test)]
mod tests {
    use {
        super::*,
        font_loader::FontDefinitions,
        layouter::{LayoutParams, LayoutSettings, LayoutSpan, LayoutStyle, Layouter},
        non_nan::NonNanF32,
        std::{fs::File, io::BufWriter},
    };

    #[test]
    fn test() {
        let mut layouter = Layouter::new(FontDefinitions::default());
        let rows = layouter.get_or_layout(&LayoutParams {
            settings: LayoutSettings {
                max_width_in_lpxs: NonNanF32::new(256.0).unwrap(),
            },
            spans: vec![LayoutSpan {
                style: LayoutStyle {
                    font_family_id: "Sans".into(),
                    font_size_in_lpxs: NonNanF32::new(16.0).unwrap(),
                },
                text: "繁😊😔 The quick brown fox jumps over the lazy dog".into(),
            }],
        });

        let font_family = layouter.get_or_load_font_family(&"Sans".into());
        for row in &*rows {
            for glyph in &row.glyphs {
                glyph.font.allocate_glyph(glyph.id, 64.0);
            }
        }
        let glyphs = font_family.get_or_shape("HalloRik!繁😊😔".into());
        for glyph in &*glyphs {
            glyph.font.allocate_glyph(glyph.id, 64.0);
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
