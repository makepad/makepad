use makepad_render::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawFloatSlider {
    #[default_shader(self::shader_slider)]
    base: DrawQuad,
    norm_value: f32,
    hover: f32,
    down: f32
}

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
    pub slider: DrawFloatSlider,
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
            slider: DrawFloatSlider::new(cx, default_shader!()),
            dragging: false
        }
    }
    
    pub fn style(cx: &mut Cx) {
        self::DrawFloatSlider::register_draw_input(cx);
        live_body!(cx, r#"
            self::anim_default: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::DrawFloatSlider::hover, keys: {1.0: 0.0}},
                    Float {bind_to: self::DrawFloatSlider::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_hover: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::DrawFloatSlider::hover, keys: {0.0: 1.0}},
                    Float {bind_to: self::DrawFloatSlider::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::DrawFloatSlider::hover, keys: {1.0: 1.0}},
                    Float {bind_to: self::DrawFloatSlider::down, keys: {0.0: 0.0, 1.0: 1.0}}
                ]
            }
            
            self::shader_slider: Shader {
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawFloatSlider;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * rect_size);
                    
                    let cy = rect_size.y * 0.5;
                    let height = 2.;
                    df.box(1., cy - 0.5 * height, rect_size.x - 1., height, 1.);
                     
                    df.fill(#4);
                    
                    let bheight = 15.;
                    let bwidth = 7.;
                    
                    df.box((rect_size.x - bwidth) * norm_value, cy - 0.5 * bheight, bwidth, bheight, 1.);
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
            self.slider.set_norm_value(cx, self.norm_value);
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
        if let Some(ae) = event.is_animate(cx, &self.animator) {
            self.slider.animate(cx, &mut self.animator, ae.time);
        }

        match event.hits(cx, self.slider.area(), HitOpt::default()) {
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
                self.slider.set_norm_value(cx, self.norm_value);
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
         if self.animator.need_init(cx) {
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.slider.last_animate(&self.animator);
        }

        if !self.dragging {
            self.scaled_value = scaled_value;
            self.min = min;
            self.max = max;
            self.step = step;
            self.norm_value = (scaled_value - min.unwrap_or(0.0)) / (max.unwrap_or(1.0) - min.unwrap_or(0.0));
        }
        
        let pad = 10.;
        
        self._size = cx.get_turtle_rect().size.x - 2. * pad;
        self.slider.norm_value = self.norm_value;
        self.slider.draw_quad_walk(cx, Walk {
            margin: Margin::left(pad),
            width: Width::FillPad(pad),
            height: Height::Fix(35.0 * height_scale)
        });
    }
    
}

