use crate::makepad_platform::*;

live_register!{
    use makepad_platform::shader::std::*;
    use makepad_component::theme::*;
    
    DrawSplitter: {{DrawSplitter}} {
        const BORDER_RADIUS: 1.0
        const SPLITER_PAD: 1.0
        const SPLITER_GRABBER:110.0
        instance pressed: 0.0
        instance hover: 0.0
        
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.clear(COLOR_BG_APP);
            if self.is_vertical > 0.5 {
                sdf.box(
                    SPLITER_PAD,
                    self.rect_size.y * 0.5 - SPLITER_GRABBER *0.5,
                    self.rect_size.x-2.0*SPLITER_PAD,
                    SPLITER_GRABBER,
                    BORDER_RADIUS
                );
            }
            else {
                sdf.box(
                    self.rect_size.x*0.5 - SPLITER_GRABBER *0.5,
                    SPLITER_PAD,
                    SPLITER_GRABBER,
                    self.rect_size.y-2.0*SPLITER_PAD,
                    BORDER_RADIUS
                );
            }
            return sdf.fill_keep(mix(
                COLOR_BG_APP,
                mix(
                    COLOR_CONTROL_HOVER,
                    COLOR_CONTROL_PRESSED,
                    self.pressed
                ),
                self.hover
            )); 
        }
    }
    
    Splitter: {{Splitter}} {
        split_bar_size: (DIM_SPLITTER_SIZE)
        min_horizontal: (DIM_SPLITTER_MIN_HORIZONTAL)
        max_horizontal: (DIM_SPLITTER_MAX_HORIZONTAL)
        min_vertical: (DIM_SPLITTER_MIN_VERTICAL)
        max_vertical: (DIM_SPLITTER_MAX_VERTICAL)
        default_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply: {
                bar_quad: {pressed: 0.0, hover: 0.0}
            }
        }
        
        hover_state: {
            from: {
                all: Play::Forward {duration: 0.1}
                state_down: Play::Forward {duration: 0.01}
            }
            apply: {
                bar_quad: {
                    pressed: 0.0,
                    hover: [{time: 0.0, value: 1.0}],
                }
            }
        }
        
        pressed_state: {
            from: {all: Play::Forward {duration: 0.1}}
            apply: {
                bar_quad: {
                    pressed: [{time: 0.0, value: 1.0}],
                    hover: 1.0,
                }
            }
        }
    }
}


#[derive(Live, LiveHook)]
#[repr(C)]
pub struct DrawSplitter {
    deref_target: DrawQuad,
    is_vertical: f32,
}

#[derive(Live, LiveHook)]
pub struct Splitter {
    #[rust(Axis::Horizontal)] pub axis: Axis,
    #[rust(SplitterAlign::Weighted(0.5))] pub align: SplitterAlign,
    #[rust] rect: Rect,
    #[rust] position: f32,
    #[rust] drag_start_align: Option<SplitterAlign>,
    #[state(default_state)] pub animator: Animator,
    
    default_state: Option<LivePtr>,
    hover_state: Option<LivePtr>,
    pressed_state: Option<LivePtr>,
    
    min_vertical: f32,
    max_vertical: f32,
    min_horizontal: f32,
    max_horizontal: f32,
    
    layout: Layout,
    bar_quad: DrawSplitter,
    split_bar_size: f32,
}

impl Splitter {
    
    pub fn begin(&mut self, cx: &mut Cx2d) {
        self.rect = cx.get_turtle_padded_rect();
        self.position = self.align.to_position(self.axis, self.rect);
        cx.begin_turtle(self.layout());
    }
    
    pub fn middle(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
        match self.axis {
            Axis::Horizontal => {
               self.bar_quad.is_vertical = 1.0;
               self.bar_quad.draw_abs(
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
                self.bar_quad.is_vertical = 0.0;
                self.bar_quad.draw_abs(
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
        cx.begin_turtle(Layout::default());
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle();
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
    
    pub fn align(&self) -> SplitterAlign {
        self.align
    }
    
    pub fn set_align(&mut self, align: SplitterAlign) {
        self.align = align;
    }
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, SplitterAction),
    ) {
        self.animator_handle_event(cx, event);
        match event.hits_with_options(
            cx,
            self.bar_quad.draw_vars.area,
            HitOptions {
                margin: Some(self.margin()),
                ..HitOptions::default()
            },
        ) {
            HitEvent::FingerHover(f) => {
                match self.axis {
                    Axis::Horizontal => cx.set_hover_mouse_cursor(MouseCursor::ColResize),
                    Axis::Vertical => cx.set_hover_mouse_cursor(MouseCursor::RowResize),
                }
                match f.hover_state {
                    HoverState::In => if !f.any_down {
                        self.animate_to(cx, self.hover_state);
                    },
                    HoverState::Out => if !f.any_down{
                        self.animate_to(cx, self.default_state);
                    },
                    _ => ()
                }
            },
            HitEvent::FingerDown(_) => {
                 match self.axis {
                    Axis::Horizontal => cx.set_down_mouse_cursor(MouseCursor::ColResize),
                    Axis::Vertical => cx.set_down_mouse_cursor(MouseCursor::RowResize),
                } 
                self.animate_to(cx, self.pressed_state);
                self.drag_start_align = Some(self.align);
            }
            HitEvent::FingerUp(f) => {
                self.drag_start_align = None;
                if f.is_over {
                    if f.input_type.has_hovers() {
                        self.animate_to(cx, self.hover_state);
                    }
                    else {
                        self.animate_to(cx, self.default_state);
                    }
                }
                else {
                    self.animate_to(cx, self.default_state);
                }
            }
            HitEvent::FingerMove(f) => {
                if let Some(drag_start_align) = self.drag_start_align {
                    let delta = match self.axis {
                        Axis::Horizontal => f.abs.x - f.abs_start.x,
                        Axis::Vertical => f.abs.y - f.abs_start.y,
                    };
                    let new_position =
                    drag_start_align.to_position(self.axis, self.rect) + delta;
                    self.align = match self.axis {
                        Axis::Horizontal => {
                            let center = self.rect.size.x / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromStart(new_position.max(self.min_vertical))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromEnd((self.rect.size.x - new_position).max(self.max_vertical))
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.x)
                            }
                        }
                        Axis::Vertical => {
                            let center = self.rect.size.y / 2.0;
                            if new_position < center - 30.0 {
                                SplitterAlign::FromStart(new_position.max(self.min_horizontal))
                            } else if new_position > center + 30.0 {
                                SplitterAlign::FromEnd((self.rect.size.y - new_position).max(self.max_horizontal))
                            } else {
                                SplitterAlign::Weighted(new_position / self.rect.size.y)
                            }
                        }
                    };
                    cx.redraw_area(self.bar_quad.draw_vars.area);
                    dispatch_action(cx, SplitterAction::Changed {axis: self.axis, align: self.align});
                }
            }
            _ => {}
        }
    }
    
    fn margin(&self) -> Margin {
        match self.axis {
            Axis::Horizontal => Margin {
                left: 3.0,
                top: 0.0,
                right: 3.0,
                bottom: 0.0,
            },
            Axis::Vertical => Margin {
                left: 0.0,
                top: 3.0,
                right: 0.0,
                bottom: 3.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SplitterAlign {
    FromStart(f32),
    FromEnd(f32),
    Weighted(f32),
}

impl SplitterAlign {
    fn to_position(self, axis: Axis, rect: Rect) -> f32 {
        match axis {
            Axis::Horizontal => match self {
                Self::FromStart(position) => position,
                Self::FromEnd(position) => rect.size.x - position,
                Self::Weighted(weight) => weight * rect.size.x,
            },
            Axis::Vertical => match self {
                Self::FromStart(position) => position,
                Self::FromEnd(position) => rect.size.y - position,
                Self::Weighted(weight) => weight * rect.size.y,
            },
        }
    }
}

pub enum SplitterAction {
    Changed {axis: Axis, align: SplitterAlign},
}
