use crate::font::{TTFFont, Glyph, HorizontalMetrics};
use crate::geometry::{Point, Rectangle};
use crate::path::PathCommand;
use std::result;

pub use ttf_parser::{Face, FaceParsingError};

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

pub fn from_ttf_parser_face(face: &Face<'_>) -> Result<TTFFont> {
    Ok(TTFFont {
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
        glyphs: (0..face.number_of_glyphs()).map(ttf_parser::GlyphId).map(|id| {
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
            Ok(Glyph {
                horizontal_metrics,
                bounds,
                outline: outline_builder.0,
            })
        }).collect::<Result<_>>()?,
    })
}
