use makepad_render::*;

pub enum FloatSliderEvent {
    Change {scaled_value: f32},
    DoneChanging,
    None
}

#[derive(Clone)]
pub struct FloatSlider {
    pub scaled_value: f32,
    pub norm_value: f32,
    pub animator: Animator,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
    pub _size: f32,
    pub slider: Quad,
    pub dragging: bool
}

impl FloatSlider {
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            norm_value: 0.0,
            scaled_value: 0.0,
            animator: Animator::default(),
            min: None,
            max: None,
            step: None,
            _size: 0.0,
            slider: Quad::new(cx),
            dragging: false
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::anim_default: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::shader_slider::hover, keys: {1.0: 0.0}},
                    Float {bind_to: self::shader_slider::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_hover: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::shader_slider::hover, keys: {0.0: 1.0}},
                    Float {bind_to: self::shader_slider::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::shader_slider::hover, keys: {1.0: 1.0}},
                    Float {bind_to: self::shader_slider::down, keys: {0.0: 0.0, 1.0: 1.0}}
                ]
            }
            
            self::shader_slider: Shader {
                use makepad_render::quad::shader::*;
                
                instance norm_value: float;
                instance hover: float;
                instance down: float;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * vec2(w, h));
                    
                    let cy = h * 0.5;
                    let height = 2.;
                    df.box(1., cy - 0.5 * height, w - 1., height, 1.);
                     
                    df.fill(#4);
                    
                    let bheight = 15.;
                    let bwidth = 7.;
                    
                    df.box((w - bwidth) * norm_value, cy - 0.5 * bheight, bwidth, bheight, 1.);
                    ////
                    let color = mix(mix(#7, #B, hover), #F, down);
                    df.fill(color);
                    
                    return df.result;
                }
            }
        "#)
    }
    
    pub fn handle_finger(&mut self, cx: &mut Cx, rel: Vec2) -> FloatSliderEvent {
        let norm_value = (rel.x / self._size).max(0.0).min(1.0);
        let mut scaled_value = norm_value * (self.max.unwrap_or(1.0) - self.min.unwrap_or(0.0)) + self.min.unwrap_or(0.0);
        if self.step.unwrap_or(0.0) > 0.0 {
            scaled_value = (scaled_value / self.step.unwrap_or(1.0)).round() * self.step.unwrap_or(1.0);
        }
        let mut changed = false;
        if scaled_value != self.scaled_value {
            self.scaled_value = scaled_value;
            self.norm_value = norm_value;
            self.animator.area.write_float(cx, live_item_id!(self::shader_slider::norm_value), self.norm_value);
            changed = true;
        }
        if changed {
            FloatSliderEvent::Change {scaled_value}
        }
        else {
            FloatSliderEvent::None
        }
    }
    
    pub fn handle_float_slider(&mut self, cx: &mut Cx, event: &mut Event) -> FloatSliderEvent {
        match event.hits(cx, self.animator.area, HitOpt::default()) {
            Event::Animate(ae) => {
                self.animator.calc_area(cx, self.animator.area, ae.time);
            },
            Event::AnimEnded(_) => self.animator.end(),
            Event::FingerHover(fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
                match fe.hover_state {
                    HoverState::In => {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_hover));
                    },
                    HoverState::Out => {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                    },
                    _ => ()
                }
            },
            Event::FingerDown(fe) => {
                self.animator.play_anim(cx, live_anim!(cx, self::anim_down));
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.dragging = true;
                return self.handle_finger(cx, fe.rel);
                // lets check where we clicked!
            },
            Event::FingerUp(fe) => {
                if fe.is_over {
                    if fe.input_type.has_hovers() {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_hover));
                    }
                    else {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                    }
                }
                else {
                    self.animator.play_anim(cx, live_anim!(cx, self::anim_default));
                }
                self.dragging = false;
                return FloatSliderEvent::DoneChanging;
            }
            Event::FingerMove(fe) => {
                return self.handle_finger(cx, fe.rel)
                
            },
            Event::FingerScroll(fs) => {
                self.norm_value += fs.scroll.x / 1000.0;
                self.norm_value = self.norm_value.min(1.0).max(0.0);
                self.scaled_value = self.norm_value * (self.max.unwrap_or(1.0) - self.min.unwrap_or(0.0)) + self.min.unwrap_or(0.0);
                self.animator.area.write_float(cx, live_item_id!(self::shader_slider::norm_value), self.norm_value);
                return FloatSliderEvent::Change {scaled_value: self.scaled_value};
            },
            _ => ()
        }
        FloatSliderEvent::None
    }
    
    pub fn draw_float_slider(
        &mut self,
        cx: &mut Cx,
        scaled_value: f32,
        min: Option<f32>,
        max: Option<f32>,
        step: Option<f32>,
        height_scale: f32
    ) {
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        if !self.dragging {
            self.scaled_value = scaled_value;
            self.min = min;
            self.max = max;
            self.step = step;
            self.norm_value = (scaled_value - min.unwrap_or(0.0)) / (max.unwrap_or(1.0) - min.unwrap_or(0.0));
        }
        
        self.slider.shader = live_shader!(cx, self::shader_slider);
        // i wanna draw a wheel with 'width' set but height a fixed height.
        
        let pad = 10.;
        
        self._size = cx.get_turtle_rect().w - 2. * pad;
        let k = self.slider.draw_quad(cx, Walk {
            margin: Margin::left(pad),
            width: Width::FillPad(pad),
            height: Height::Fix(35.0 * height_scale)
        });
        // lets put a hsv int here
        k.push_float(cx, self.norm_value);
        k.push_last_float(cx, &self.animator, live_item_id!(self::shader_slider::hover));
        k.push_last_float(cx, &self.animator, live_item_id!(self::shader_slider::down));
        self.animator.set_area(cx, k.into());
    }
    
}

