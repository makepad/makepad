pub mod font;
pub mod font_atlas;
pub mod font_data;
pub mod font_face;
pub mod font_family;
pub mod font_loader;
pub mod fonts;
pub mod geom;
pub mod glyph_outline;
pub mod glyph_raster_image;
pub mod image;
pub mod layouter;
pub mod non_nan;
pub mod num;
pub mod sdfer;
pub mod shaper;
pub mod substr;
pub mod text;

#[cfg(test)]
mod tests {
    #[test]
    fn test() {
        use {
            super::{
                font_loader::FontDefinitions,
                layouter::{LayoutOptions, LayoutParams, Layouter, Settings},
                non_nan::NonNanF32,
                text::{Span, Style, Text},
            },
            std::{fs::File, io::BufWriter, rc::Rc},
        };

        let mut layouter = Layouter::new(FontDefinitions::default(), Settings::default());
        let mut text = Text::default();
        text.push_span(Span {
            style: Style {
                font_family_id: "Sans".into(),
                font_size_in_lpxs: NonNanF32::new(16.0).unwrap(),
            },
            text: "繁😊😔 The Xuick brown fox jumps over the lazy dog".into(),
        });
        let text = layouter.get_or_layout(LayoutParams {
            options: LayoutOptions {
                max_width_in_lpxs: Some(NonNanF32::new(256.0).unwrap()),
            },
            text: Rc::new(text),
        });
        for row in &*text {
            for glyph in &row.glyphs {
                glyph.font.rasterize_glyph(glyph.id, 64.0);
            }
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
        let pixels = atlas.image().as_pixels();
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
        let pixels = atlas.image().as_pixels();
        let data =
            unsafe { std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4) };
        writer.write_image_data(&data).unwrap();
    }
}
