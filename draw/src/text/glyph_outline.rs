use {
    super::{
        geom::{Point, Rect, Size, Transform},
        image::{SubimageMut, R},
        num::Zero,
    },
    makepad_rustybuzz as rustybuzz,
    rustybuzz::ttf_parser,
};

#[derive(Clone, Debug)]
pub struct GlyphOutline {
    bounds: Rect<f32>,
    units_per_em: f32,
    commands: Vec<Command>,
}

impl GlyphOutline {
    pub fn origin_in_dpxs(&self, dpxs_per_em: f32) -> Point<f32> {
        self.bounds
            .origin
            .apply_transform(Transform::from_scale_uniform(
                dpxs_per_em / self.units_per_em,
            ))
    }

    pub fn size_in_dpxs(&self, dpxs_per_em: f32) -> Size<f32> {
        self.bounds
            .size
            .apply_transform(Transform::from_scale_uniform(
                dpxs_per_em / self.units_per_em,
            ))
    }

    pub fn bounds_in_dpxs(&self, dpxs_per_em: f32) -> Rect<f32> {
        Rect::new(
            self.origin_in_dpxs(dpxs_per_em),
            self.size_in_dpxs(dpxs_per_em),
        )
    }

    pub fn image_size(&self, dpxs_per_em: f32) -> Size<usize> {
        let size = self.size_in_dpxs(dpxs_per_em);
        Size::new(size.width.ceil() as usize, size.height.ceil() as usize)
    }

    pub fn rasterize(&self, dpxs_per_em: f32, output: &mut SubimageMut<R>) {
        use ab_glyph_rasterizer::Rasterizer;

        fn to_ab_glyph(p: Point<f32>) -> ab_glyph_rasterizer::Point {
            ab_glyph_rasterizer::point(p.x, p.y)
        }

        let output_size = output.bounds().size;
        let mut rasterizer = Rasterizer::new(output_size.width, output_size.height);
        let origin = self.bounds.origin;
        let transform = Transform::from_translate(-origin.x, -origin.y)
            .scale_uniform(dpxs_per_em / self.units_per_em);
        let mut last = Point::ZERO;
        let mut last_move = None;
        for command in self.commands.iter().copied() {
            match command {
                Command::MoveTo(p) => {
                    last = p;
                    last_move = Some(p);
                }
                Command::LineTo(p) => {
                    rasterizer.draw_line(
                        to_ab_glyph(last.apply_transform(transform)),
                        to_ab_glyph(p.apply_transform(transform)),
                    );
                    last = p;
                }
                Command::QuadTo(p1, p) => {
                    rasterizer.draw_quad(
                        to_ab_glyph(last.apply_transform(transform)),
                        to_ab_glyph(p1.apply_transform(transform)),
                        to_ab_glyph(p.apply_transform(transform)),
                    );
                    last = p;
                }
                Command::CurveTo(p1, p2, p) => {
                    rasterizer.draw_cubic(
                        to_ab_glyph(last.apply_transform(transform)),
                        to_ab_glyph(p1.apply_transform(transform)),
                        to_ab_glyph(p2.apply_transform(transform)),
                        to_ab_glyph(p.apply_transform(transform)),
                    );
                    last = p;
                }
                Command::Close => {
                    if let Some(last_move) = last_move.take() {
                        rasterizer.draw_line(
                            to_ab_glyph(last.apply_transform(transform)),
                            to_ab_glyph(last_move.apply_transform(transform)),
                        );
                        last = last_move;
                    }
                }
            }
        }
        rasterizer.for_each_pixel_2d(|x, y, a| {
            let point = Point::new(x as usize, output_size.height - 1 - y as usize);
            let pixel = R::new((a * 255.0) as u8);
            output[point] = pixel;
        });
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    MoveTo(Point<f32>),
    LineTo(Point<f32>),
    QuadTo(Point<f32>, Point<f32>),
    CurveTo(Point<f32>, Point<f32>, Point<f32>),
    Close,
}

#[derive(Debug)]
pub struct Builder {
    commands: Vec<Command>,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn finish(self, bounds: Rect<f32>, units_per_em: f32) -> GlyphOutline {
        GlyphOutline {
            bounds,
            units_per_em,
            commands: self.commands,
        }
    }
}

impl ttf_parser::OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::MoveTo(Point::new(x, y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::LineTo(Point::new(x, y)));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.commands
            .push(Command::QuadTo(Point::new(x1, y1), Point::new(x, y)));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.commands.push(Command::CurveTo(
            Point::new(x1, y1),
            Point::new(x2, y2),
            Point::new(x, y),
        ));
    }

    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}
