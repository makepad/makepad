pub mod faces;
pub mod font;
pub mod font_data;
pub mod font_family;
pub mod font_loader;
pub mod fonts;
pub mod geometry;
pub mod outline;
pub mod raster_image;
pub mod image;
pub mod image_atlas;
pub mod non_nan;
pub mod numeric;
pub mod pixels;
pub mod substr;
pub mod layouter;
pub mod shaper;

#[cfg(test)]
mod tests {
    use {
        super::*,
        font_loader::FontDefinitions,
        std::{fs::File, io::BufWriter},
        non_nan::NonNanF32,
        layouter::{Paragraph, Line, Span, Style, TextLayouter, LayoutParams, LayoutTextSettings},
    };

    #[test]
    fn test() {
        let mut layouter = TextLayouter::new(FontDefinitions::default());

        let mut paragraph = Paragraph::new();
        let mut line = Line::new();
        line.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap(),
            },
            text: "The quick brown fox jumps over the lazy dog".into(),
        });
        paragraph.push_line(line);
        let laidout_text = layouter.get_or_layout(&LayoutParams {
            settings: LayoutTextSettings {
                max_width_in_lpxs: NonNanF32::new(256.0).unwrap(),
            },
            paragraph
        });

        let font_family = layouter.get_or_load_font_family(&"Sans".into());
        let shaped_text = font_family.get_or_shape_text("HalloRik!繁😊😔".into());
        for glyph in &shaped_text.glyphs {
            println!("{:?}", glyph.id);
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
