use makepad_render::*;

pub struct Splitter {
    axis: Axis,
    position: Position,
    computed_position: f32,
    split_bar: DrawColor,
    split_bar_size: f32,
}

impl Splitter {
    pub fn style(cx: &mut Cx) {
        live_body!(cx, {
            self::split_bar_size: 2.0;
            self::split_bar_color: #19;
        })
    }

    pub fn new(cx: &mut Cx) -> Splitter {
        Splitter {
            axis: Axis::Vertical,
            position: Position::Weighted(0.5),
            computed_position: 0.0,
            split_bar: DrawColor::new(cx, default_shader!()),
            split_bar_size: 0.0,
        }
    }

    pub fn begin(&mut self, cx: &mut Cx) {
        self.apply_style(cx);
        self.update_computed_position(cx.get_turtle_rect());
        cx.begin_turtle(self.layout(), Area::Empty);
    }

    pub fn middle(&mut self, cx: &mut Cx) {
        cx.end_turtle(Area::Empty);
        let rect = cx.get_turtle_rect();
        match self.axis {
            Axis::Horizontal => {
                self.split_bar.draw_quad_abs(cx, Rect {
                    pos: vec2(rect.pos.x, rect.pos.y + self.computed_position),
                    size: vec2(rect.size.x, self.split_bar_size)
                });
                cx.set_turtle_pos(Vec2 {
                    x: rect.pos.x,
                    y: rect.pos.y + self.computed_position + self.split_bar_size,
                });
            }
            Axis::Vertical => {
                self.split_bar.draw_quad_abs(cx, Rect {
                    pos: vec2(rect.pos.x + self.computed_position, rect.pos.y),
                    size: vec2(self.split_bar_size, rect.size.y)
                });
                cx.set_turtle_pos(Vec2 {
                    x: rect.pos.x + self.computed_position + self.split_bar_size,
                    y: rect.pos.y,
                });
            }
        }
        cx.begin_turtle(Layout::default(), Area::Empty);
    }

    pub fn end(&mut self, cx: &mut Cx) {
        cx.end_turtle(Area::Empty);
    }

    fn update_computed_position(&mut self, rect: Rect) {
        self.computed_position = match self.axis {
            Axis::Horizontal => match self.position {
                Position::FromStart(position) => position,
                Position::FromEnd(position) => rect.size.y - position,
                Position::Weighted(percentage) => percentage * rect.size.y,
            },
            Axis::Vertical => match self.position {
                Position::FromStart(position) => position,
                Position::FromEnd(position) => rect.size.x - position,
                Position::Weighted(percentage) => percentage * rect.size.x,
            }
        };
    }

    fn apply_style(&mut self, cx: &mut Cx) {
        self.split_bar_size = live_float!(cx, self::split_bar_size);
        self.split_bar.color = live_vec4!(cx, self::split_bar_color);
    }

    fn layout(&self) -> Layout {
        Layout {
            walk: match self.axis {
                Axis::Horizontal => Walk::wh(
                    Width::Fill,
                    Height::Fix(self.computed_position),
                ),
                Axis::Vertical => Walk::wh(
                    Width::Fix(self.computed_position),
                    Height::Fill,
                ),
            },
            ..Layout::default()
        }
    }
}

pub enum Position {
    FromStart(f32),
    FromEnd(f32),
    Weighted(f32),
}
