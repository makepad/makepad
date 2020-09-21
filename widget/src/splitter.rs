use makepad_render::*;
use makepad_microserde::*;

#[derive(Clone)]
pub struct Splitter {
    pub axis: Axis,
    pub align: SplitterAlign,
    pub pos: f32,
    
    pub min_size: f32,
    pub split_size: f32,
    pub bg: Quad,
    pub animator: Animator,
    pub realign_dist: f32,
    pub split_view: View,
    pub _split_area: Area,
    pub _calc_pos: f32,
    pub _is_moving: bool,
    pub _drag_point: f32,
    pub _drag_pos_start: f32,
    pub _drag_max_pos: f32,
    pub _hit_state_margin: Option<Margin>,
}

#[derive(Clone, PartialEq, SerRon, DeRon)]
pub enum SplitterAlign {
    First,
    Last,
    Weighted
}

#[derive(Clone, PartialEq)]
pub enum SplitterEvent {
    None,
    Moving {new_pos: f32},
    MovingEnd {new_align: SplitterAlign, new_pos: f32}
}

impl Splitter {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            axis: Axis::Vertical,
            align: SplitterAlign::First,
            pos: 0.0,
            
            _split_area: Area::Empty,
            _calc_pos: 0.0,
            _is_moving: false,
            _drag_point: 0.,
            _drag_pos_start: 0.,
            _drag_max_pos: 0.0,
            _hit_state_margin: None,
            realign_dist: 30.,
            split_size: 2.0,
            min_size: 25.0,
            split_view: View::new(cx),
            bg: Quad::new(cx),
            animator: Animator::default(),
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        live!(cx, r#"
            
            self::color_bg: #19;
            self::color_over: #5;
            self::color_peak: #f;
            self::color_drag: #6;
            
            self::anim_default: Anim {
                play: Cut {duration: 0.5}
                tracks: [
                    Color {keys: {1.0: self::color_bg} live_id: makepad_render::quad::shader::color}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.05}
                tracks: [
                    Color {keys: {1.0: self::color_over}, live_id: makepad_render::quad::shader::color}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2}
                tracks: [
                    Color {keys: {0.0: self::color_peak, 1.0: self::color_drag}, live_id: makepad_render::quad::shader::color}
                ]
            }
            
            self::shader_bg: Shader {
                use makepad_render::quad::shader::*;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * vec2(w, h));
                    df.box(0., 0., w, h, 0.5);
                    return df.fill(color);
                }
            }
            
        "#);
    }
    
    pub fn handle_splitter(&mut self, cx: &mut Cx, event: &mut Event) -> SplitterEvent {
        match event.hits(cx, self._split_area, HitOpt {margin: self._hit_state_margin, ..Default::default()}) {
            Event::Animate(ae) => {
                self.animator.calc_area(cx, self._split_area, ae.time);
            },
            Event::AnimEnded(_) => self.animator.end(),
            Event::FingerDown(fe) => {
                self._is_moving = true;
                self.animator.play_anim(cx, live_anim!(cx, self::anim_down));
                match self.axis {
                    Axis::Horizontal => cx.set_down_mouse_cursor(MouseCursor::RowResize),
                    Axis::Vertical => cx.set_down_mouse_cursor(MouseCursor::ColResize)
                };
                self._drag_pos_start = self.pos;
                self._drag_point = match self.axis {
                    Axis::Horizontal => {fe.rel.y},
                    Axis::Vertical => {fe.rel.x}
                }
            },
            Event::FingerHover(fe) => {
                match self.axis {
                    Axis::Horizontal => cx.set_hover_mouse_cursor(MouseCursor::RowResize),
                    Axis::Vertical => cx.set_hover_mouse_cursor(MouseCursor::ColResize)
                };
                if !self._is_moving {
                    match fe.hover_state {
                        HoverState::In => {
                            self.animator.play_anim(cx, live_anim!(cx, self::anim_over));
                        },
                        HoverState::Out => {
                            self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                        },
                        _ => ()
                    }
                }
            },
            Event::FingerUp(fe) => {
                self._is_moving = false;
                if fe.is_over {
                    if !fe.is_touch {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_over));
                    }
                    else {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                    }
                }
                else {
                    self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                }
                // we should change our mode based on which edge we are closest to
                // the rule is center - 30 + 30
                let center = self._drag_max_pos * 0.5;
                if self._calc_pos > center - self.realign_dist &&
                self._calc_pos < center + self.realign_dist {
                    self.align = SplitterAlign::Weighted;
                    self.pos = self._calc_pos / self._drag_max_pos;
                }
                else if self._calc_pos < center - self.realign_dist {
                    
                    self.align = SplitterAlign::First;
                    self.pos = self._calc_pos;
                }
                else {
                    self.align = SplitterAlign::Last;
                    self.pos = self._drag_max_pos - self._calc_pos;
                }
                
                return SplitterEvent::MovingEnd {
                    new_align: self.align.clone(),
                    new_pos: self.pos
                }
            },
            Event::FingerMove(fe) => {
                let delta = match self.axis {
                    Axis::Horizontal => {
                        fe.abs_start.y - fe.abs.y
                    },
                    Axis::Vertical => {
                        fe.abs_start.x - fe.abs.x
                    }
                };
                let mut pos = match self.align {
                    SplitterAlign::First => self._drag_pos_start - delta,
                    SplitterAlign::Last => self._drag_pos_start + delta,
                    SplitterAlign::Weighted => self._drag_pos_start * self._drag_max_pos - delta
                };
                if pos > self._drag_max_pos - self.min_size {
                    pos = self._drag_max_pos - self.min_size
                }
                else if pos < self.min_size {
                    pos = self.min_size
                };
                let calc_pos = match self.align {
                    SplitterAlign::First => {
                        self.pos = pos;
                        pos
                    },
                    SplitterAlign::Last => {
                        self.pos = pos;
                        self._drag_max_pos - pos
                    },
                    SplitterAlign::Weighted => {
                        self.pos = pos / self._drag_max_pos;
                        pos
                    }
                };
                //log_str(&format!("CALC POS {}", calc_pos));
                // pixelsnap calc_pos
                if calc_pos != self._calc_pos {
                    self._calc_pos = calc_pos;
                    cx.redraw_child_area(self._split_area);
                    return SplitterEvent::Moving {new_pos: self.pos};
                }
            }
            _ => ()
        };
        SplitterEvent::None
    }
    
    pub fn set_splitter_state(&mut self, align: SplitterAlign, pos: f32, axis: Axis) {
        self.axis = axis;
        self.align = align;
        self.pos = pos;
        match self.axis {
            Axis::Horizontal => {
                self._hit_state_margin = Some(Margin {
                    l: 0.,
                    t: 3.,
                    r: 0.,
                    b: 7.,
                })
            },
            Axis::Vertical => {
                self._hit_state_margin = Some(Margin {
                    l: 3.,
                    t: 0.,
                    r: 7.,
                    b: 0.,
                })
            }
        }
    }
    
    pub fn begin_splitter(&mut self, cx: &mut Cx) {
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        let rect = cx.get_turtle_rect();
        self._calc_pos = match self.align {
            SplitterAlign::First => self.pos,
            SplitterAlign::Last => match self.axis {
                Axis::Horizontal => rect.h - self.pos,
                Axis::Vertical => rect.w - self.pos
            },
            SplitterAlign::Weighted => self.pos * match self.axis {
                Axis::Horizontal => rect.h,
                Axis::Vertical => rect.w
            }
        };
        let dpi_factor = cx.get_dpi_factor_of(&self._split_area);
        self._calc_pos -= self._calc_pos % (1.0 / dpi_factor);
        match self.axis {
            Axis::Horizontal => {
                cx.begin_turtle(Layout {
                    walk: Walk::wh(Width::Fill, Height::Fix(self._calc_pos)),
                    ..Layout::default()
                }, Area::Empty)
            },
            Axis::Vertical => {
                cx.begin_turtle(Layout {
                    walk: Walk::wh(Width::Fix(self._calc_pos), Height::Fill),
                    ..Layout::default()
                }, Area::Empty)
            }
        }
    }
    
    pub fn mid_splitter(&mut self, cx: &mut Cx) {
        cx.end_turtle(Area::Empty);
        let rect = cx.get_turtle_rect();
        let origin = cx.get_turtle_origin();
        self.bg.shader = live_shader!(cx, self::shader_bg);
        self.bg.color = self.animator.last_color(cx, live_id!(makepad_render::quad::shader::color));
        match self.axis {
            Axis::Horizontal => {
                cx.set_turtle_pos(Vec2 {x: origin.x, y: origin.y + self._calc_pos});
                if let Ok(_) = self.split_view.begin_view(cx, Layout {
                    walk: Walk::wh(Width::Fix(rect.w), Height::Fix(self.split_size)),
                    ..Layout::default()
                }) {
                    self._split_area = self.bg.draw_quad_rel(cx, Rect {x: 0., y: 0., w: rect.w, h: self.split_size}).into();
                    self.split_view.end_view(cx);
                }
                cx.set_turtle_pos(Vec2 {x: origin.x, y: origin.y + self._calc_pos + self.split_size});
            },
            Axis::Vertical => {
                cx.set_turtle_pos(Vec2 {x: origin.x + self._calc_pos, y: origin.y});
                if let Ok(_) = self.split_view.begin_view(cx, Layout {
                    walk: Walk::wh(Width::Fix(self.split_size), Height::Fix(rect.h)),
                    ..Layout::default()
                }) {
                    self._split_area = self.bg.draw_quad_rel(cx, Rect {x: 0., y: 0., w: self.split_size, h: rect.h}).into();
                    self.split_view.end_view(cx);
                }
            }
        };
        cx.begin_turtle(Layout::default(), Area::Empty);
    }
    
    pub fn end_splitter(&mut self, cx: &mut Cx) {
        cx.end_turtle(Area::Empty);
        // draw the splitter in the middle of the turtle
        let rect = cx.get_turtle_rect();
        
        match self.axis {
            Axis::Horizontal => {
                self._drag_max_pos = rect.h;
            },
            Axis::Vertical => {
                self._drag_max_pos = rect.w;
            }
        };
        
        self.animator.set_area(cx, self._split_area);
    }
}
