use makepad_render::*;

live_register!{
    use makepad_render::shader_std::*;
    
    Splitter: {{Splitter}} {
        split_bar_size: 2.0
        split_bar:{
            color:#19
        }
    }
}

#[derive(Live, LiveHook)]
pub struct Splitter {
    #[rust(Axis::Horizontal)] pub axis: Axis,
    #[rust(AlignPosition::Weighted(0.5))] pub align_position: AlignPosition,
    #[rust] pub rect: Rect,
    #[rust] pub position: f32,
    #[rust] pub live_ptr: Option<LivePtr>,
    #[rust] pub animator: Animator,
    #[live] pub layout: Layout,
    #[rust] pub drag_start_align_position: Option<AlignPosition>,

    #[live] pub state_default: Option<LivePtr>,

    #[live] pub split_bar: DrawColor,
    #[live] pub split_bar_size: f32,
}

impl Splitter {

    pub fn begin(&mut self, cx: &mut Cx) {
        self.rect = cx.get_turtle_rect();
        self.position = self.align_position.to_position(self.axis, self.rect);
        cx.begin_turtle(self.layout(), Area::Empty);
    }

    pub fn middle(&mut self, cx: &mut Cx) {
        cx.end_turtle(Area::Empty);
        match self.axis {
            Axis::Horizontal => {
                self.split_bar.draw_quad_abs(
                    cx,
                    Rect {
                        pos: vec2(self.rect.pos.x + self.position, self.rect.pos.y),
                        size: vec2(self.split_bar_size, self.rect.size.y),
                    },
                );
                cx.set_turtle_pos(Vec2 {
                    x: self.rect.pos.x + self.position + self.split_bar_size,
                    y: self.rect.pos.y,
                });
            }
            Axis::Vertical => {
                self.split_bar.draw_quad_abs(
                    cx,
                    Rect {
                        pos: vec2(self.rect.pos.x, self.rect.pos.y + self.position),
                        size: vec2(self.rect.size.x, self.split_bar_size),
                    },
                );
                cx.set_turtle_pos(Vec2 {
                    x: self.rect.pos.x,
                    y: self.rect.pos.y + self.position + self.split_bar_size,
                });
            }
        }
        cx.begin_turtle(Layout::default(), Area::Empty);
    }

    pub fn end(&mut self, cx: &mut Cx) {
        cx.end_turtle(Area::Empty);
    }

    fn layout(&self) -> Layout {
        Layout {
            walk: match self.axis {
                Axis::Horizontal => Walk::wh(Width::Fixed(self.position), Height::Filled),
                Axis::Vertical => Walk::wh(Width::Filled, Height::Fixed(self.position)),
            },
            ..self.layout
        }
    }

    pub fn axis(&self) -> Axis {
        self.axis
    }

    pub fn set_axis(&mut self, axis: Axis) {
        self.axis = axis;
    }

    pub fn align_position(&self) -> AlignPosition {
        self.align_position
    }

    pub fn set_align_position(&mut self, align_position: AlignPosition) {
        self.align_position = align_position;
    }

    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, SplitterAction),
    ) {
        match event.hits(
            cx,
            self.split_bar.draw_vars.area,
            HitOpt {
                margin: Some(self.margin()),
                ..HitOpt::default()
            },
        ) {
            Event::FingerHover(_) => match self.axis {
                Axis::Horizontal => cx.set_hover_mouse_cursor(MouseCursor::ColResize),
                Axis::Vertical => cx.set_hover_mouse_cursor(MouseCursor::RowResize),
            },
            Event::FingerDown(_) => {
                self.drag_start_align_position = Some(self.align_position);
            }
            Event::FingerUp(_) => {
                self.drag_start_align_position = None;
            }
            Event::FingerMove(event) => {
                if let Some(drag_start_align_position) = self.drag_start_align_position {
                    let delta = match self.axis {
                        Axis::Horizontal => event.abs.x - event.abs_start.x,
                        Axis::Vertical => event.abs.y - event.abs_start.y,
                    };
                    let new_position =
                        drag_start_align_position.to_position(self.axis, self.rect) + delta;
                    self.align_position = match self.axis {
                        Axis::Horizontal => {
                            let center = self.rect.size.x / 2.0;
                            if new_position < center - 30.0 {
                                AlignPosition::FromStart(new_position)
                            } else if new_position > center + 30.0 {
                                AlignPosition::FromEnd(self.rect.size.x - new_position)
                            } else {
                                AlignPosition::Weighted(new_position / self.rect.size.x)
                            }
                        }
                        Axis::Vertical => {
                            let center = self.rect.size.y / 2.0;
                            if new_position < center - 30.0 {
                                AlignPosition::FromStart(new_position)
                            } else if new_position > center + 30.0 {
                                AlignPosition::FromEnd(self.rect.size.y - new_position)
                            } else {
                                AlignPosition::Weighted(new_position / self.rect.size.y)
                            }
                        }
                    };
                    cx.redraw_view_of(self.split_bar.draw_vars.area);
                    dispatch_action(cx, SplitterAction::Changed);
                }
            }
            _ => {}
        }
    }

    fn margin(&self) -> Margin {
        match self.axis {
            Axis::Horizontal => Margin {
                l: 3.0,
                t: 0.0,
                r: 7.0,
                b: 0.0,
            },
            Axis::Vertical => Margin {
                l: 0.0,
                t: 3.0,
                r: 0.0,
                b: 7.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AlignPosition {
    FromStart(f32),
    FromEnd(f32),
    Weighted(f32),
}

impl AlignPosition {
    fn to_position(self, axis: Axis, rect: Rect) -> f32 {
        match axis {
            Axis::Horizontal => match self {
                AlignPosition::FromStart(position) => position,
                AlignPosition::FromEnd(position) => rect.size.x - position,
                AlignPosition::Weighted(weight) => weight * rect.size.x,
            },
            Axis::Vertical => match self {
                AlignPosition::FromStart(position) => position,
                AlignPosition::FromEnd(position) => rect.size.y - position,
                AlignPosition::Weighted(weight) => weight * rect.size.y,
            },
        }
    }
}

pub enum SplitterAction {
    Changed,
}
