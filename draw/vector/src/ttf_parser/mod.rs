use crate::font::{TTFFont, Glyph, HorizontalMetrics};
use crate::geometry::{Point, Rectangle};
use crate::path::PathCommand;
use resvg::usvg::{Options, Tree};
use std::{result, rc::Rc};
use makepad_ttf_parser as ttf_parser;
pub use ttf_parser::{Face, FaceParsingError, GlyphId};

struct OutlineBuilder(Vec<PathCommand>);

impl ttf_parser::OutlineBuilder for OutlineBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(PathCommand::MoveTo(Point { x: x as f64, y: y as f64 }));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(PathCommand::LineTo(Point { x: x as f64, y: y as f64 }));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.push(PathCommand::QuadraticTo(
            Point { x: x1 as f64, y: y1 as f64 },
            Point { x: x as f64, y: y as f64 },
        ));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.push(PathCommand::CubicTo(
            Point { x: x1 as f64, y: y1 as f64 },
            Point { x: x2 as f64, y: y2 as f64 },
            Point { x: x as f64, y: y as f64 },
        ));
    }
    fn close(&mut self) {
        self.0.push(PathCommand::Close);
    }
}

pub type Result<T> = result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Error;

pub fn from_ttf_parser_face(face: &Face<'_>) -> TTFFont {
    TTFFont {
        units_per_em: face.units_per_em() as f64,
        ascender: face.ascender() as f64,
        descender: face.descender() as f64,
        line_gap: face.line_gap() as f64,
        bounds: {
            let ttf_parser::Rect { x_min, y_min, x_max, y_max } = face.global_bounding_box();
            Rectangle::new(
                Point::new(x_min as f64, y_min as f64),
                Point::new(x_max as f64, y_max as f64),
            )
        },
        cached_decoded_glyphs: vec![],
        cached_svg_images: vec![],
    }
}

impl TTFFont {
    pub fn get_glyph_by_id(&mut self, face: &Face<'_>, id: usize) -> Result<&Glyph> {
        if self.cached_decoded_glyphs.len() <= id {
            self.cached_decoded_glyphs.resize(id + 1, None);
        }
        if self.cached_svg_images.len() <= id {
            self.cached_svg_images.resize(id + 1, None);
        }
        let glyph_slot = &mut self.cached_decoded_glyphs[id];
        if glyph_slot.is_none() {
            let id = ttf_parser::GlyphId(u16::try_from(id).unwrap());

            if self.cached_svg_images[id.0 as usize].is_none() {
                match face.glyph_svg_image(id) {
                    Some(svg_image) => {
                        let opt = Options::default();
                        let tree = Rc::new(Tree::from_data(&svg_image.data, &opt).map_err(|_| Error)?);
                        for id in svg_image.start_glyph_id.0..svg_image.end_glyph_id.0 {
                            self.cached_svg_images[id as usize] = Some(Some(tree.clone()));
                        }
                    }
                    None => {
                        self.cached_svg_images[id.0 as usize] = Some(None);
                    }
                }
            }

            let horizontal_metrics = HorizontalMetrics {
                advance_width: face.glyph_hor_advance(id).ok_or(Error)? as f64,
                left_side_bearing: face.glyph_hor_side_bearing(id).ok_or(Error)? as f64,
            };
            let mut outline_builder = OutlineBuilder(vec![]);
            let bounds = face.outline_glyph(id, &mut outline_builder)
                .map(|ttf_parser::Rect { x_min, y_min, x_max, y_max }| {
                    Rectangle::new(
                        Point::new(x_min as f64, y_min as f64),
                        Point::new(x_max as f64, y_max as f64),
                    )
                })
                .unwrap_or_default();
            *glyph_slot = Some(Box::new(Glyph {
                horizontal_metrics,
                bounds,
                outline: outline_builder.0,
                svg_image: self.cached_svg_images[id.0 as usize].as_ref().unwrap().clone(),
            }));
        }
        Ok(glyph_slot.as_ref().unwrap())
    }
}
