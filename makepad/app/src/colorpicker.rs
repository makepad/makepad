use makepad_render::*;

pub enum ColorPickerEvent {
    Change {hsva: Vec4},
    DoneChanging,
    None
}

#[derive(Clone)]
pub struct ColorPicker {
    pub size: f32,
    pub hue: f32,
    pub sat: f32,
    pub val: f32,
    pub wheel: Quad,
    pub animator: Animator,
    pub drag_mode: ColorPickerDragMode
}


#[derive(Clone, Debug, PartialEq)]
pub enum ColorPickerDragMode {
    Wheel,
    Rect,
    None
}
impl ColorPicker {
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            hue: 0.0,
            sat: 0.0,
            val: 0.0,
            size: 0.0,
            animator: Animator::default(),
            wheel: Quad::new(cx),
            drag_mode: ColorPickerDragMode::None
        }
    }
    
    pub fn handle_finger(&mut self, cx: &mut Cx, rel: Vec2) -> ColorPickerEvent {
        fn clamp(x: f32, mi: f32, ma: f32) -> f32 {if x < mi {mi} else if x > ma {ma} else {x}};
        let vx = rel.x - 0.5 * self.size;
        let vy = rel.y - 0.5 * self.size;
        let rsize = (self.size * 0.28) / 2.0f32.sqrt();
        let last_hue = self.hue;
        let last_sat = self.sat;
        let last_val = self.val;
        match self.drag_mode {
            ColorPickerDragMode::Rect => {
                self.sat = clamp((vx + rsize) / (2.0 * rsize), 0.0, 1.0);
                self.val = 1.0 - clamp((vy + rsize) / (2.0 * rsize), 0.0, 1.0);
            },
            ColorPickerDragMode::Wheel => {
                self.hue = vx.atan2(vy) / std::f32::consts::PI * 0.5 + 0.5;
            },
            _ => ()
        }
        // lets update hue sat val directly
        let mut changed = false;
        if last_hue != self.hue {
            self.animator.area.write_float(cx, live_id!(self::shader_wheel::hue), self.hue);
            changed = true;
        }
        if last_sat != self.sat {
            self.animator.area.write_float(cx, live_id!(self::shader_wheel::sat), self.sat);
            changed = true;
        }
        if last_val != self.val {
            self.animator.area.write_float(cx, live_id!(self::shader_wheel::val), self.val);
            changed = true;
        }
        if changed {
            ColorPickerEvent::Change {hsva: Vec4 {x: self.hue, y: self.sat, z: self.val, w: 1.0}}
        }
        else {
            ColorPickerEvent::None
        }
    }
    
    pub fn handle_color_picker(&mut self, cx: &mut Cx, event: &mut Event) -> ColorPickerEvent {
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
                let rsize = (self.size * 0.28) / 2.0f32.sqrt();
                let vx = fe.rel.x - 0.5 * self.size;
                let vy = fe.rel.y - 0.5 * self.size;
                if vx >= -rsize && vx <= rsize && vy >= -rsize && vy <= rsize {
                    self.drag_mode = ColorPickerDragMode::Rect;
                }
                else if vx >= -0.5 * self.size && vx <= 0.5 * self.size && vy >= -0.5 * self.size && vy <= 0.5 * self.size {
                    self.drag_mode = ColorPickerDragMode::Wheel;
                }
                else {
                    self.drag_mode = ColorPickerDragMode::None;
                }
                return self.handle_finger(cx, fe.rel);
                // lets check where we clicked!
            },
            Event::FingerUp(fe) => {
                if fe.is_over {
                    if !fe.is_touch {
                        self.animator.play_anim(cx, live_anim!(cx, self::anim_hover));
                    }
                    else {
                        self.animator.play_anim(cx, live_anim!(cx, Self::anim_default));
                    }
                }
                else {
                    self.animator.play_anim(cx, live_anim!(cx, Self::anim_default));
                }
                self.drag_mode = ColorPickerDragMode::None;
                return ColorPickerEvent::DoneChanging;
            }
            Event::FingerMove(fe) => {
                return self.handle_finger(cx, fe.rel)
                
            },
            _ => ()
        }
        ColorPickerEvent::None
    }
    
    pub fn draw_color_picker(&mut self, cx: &mut Cx, hsva: Vec4) {
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        if self.drag_mode == ColorPickerDragMode::None {
            self.hue = hsva.x;
            self.sat = hsva.y;
            self.val = hsva.z;
        }
        self.wheel.shader = live_shader!(cx, self::shader_wheel);
        // i wanna draw a wheel with 'width' set but height a fixed height.
        self.size = cx.get_turtle_rect().w;
        let k = self.wheel.draw_quad(cx, Walk {
            margin: Margin::bottom(10.),
            width: Width::Fill,
            height: Height::Fix(self.size * 1.0)
        });
        // lets put a hsv int here
        k.push_float(cx, self.hue);
        k.push_float(cx, self.sat);
        k.push_float(cx, self.val);
        
        k.push_last_float(cx, &self.animator, live_id!(self::shader_wheel::hover()));
        k.push_last_float(cx, &self.animator, live_id!(self::shader_wheel::hover()));
        
        self.animator.set_area(cx, k.into());
    }

    pub fn style(cx: &mut Cx) {
        live!(cx, r#"
            self::anim_default: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {live_id: self::shader_wheel::hover, keys: {1.0: 0.0}},
                    Float {live_id: self::shader_wheel::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_hover: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {live_id: self::shader_wheel::hover, keys: {0.0: 1.0}},
                    Float {live_id: self::shader_wheel::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {live_id: self::shader_wheel::hover, keys: {1.0: 1.0}},
                    Float {live_id: self::shader_wheel::down, keys: {0.0: 0.0, 1.0: 1.0}}
                ]
            }
            
            self::shader_wheel: Shader {
                use makepad_render::quad::shader::*;
                
                instance hue: float;
                instance sat: float;
                instance val: float;
                instance hover: float;
                instance down: float;
                
                fn circ_to_rect(u: float, v: float) -> vec2 {
                    let u2 = u * u;
                    let v2 = v * v;
                    return vec2(
                        0.5 * sqrt(2. + 2. * sqrt(2.) * u + u2 - v2) -
                        0.5 * sqrt(2. - 2. * sqrt(2.) * u + u2 - v2),
                        0.5 * sqrt(2. + 2. * sqrt(2.) * v - u2 + v2) -
                        0.5 * sqrt(2. - 2. * sqrt(2.) * v - u2 + v2)
                    );
                }
                
                fn pixel() -> vec4 {
                    let rgbv = Pal::hsv2rgb(vec4(hue, sat, val, 1.));
                    
                    let df = Df::viewport(pos * vec2(w, h));
                    let cx = w * 0.5;
                    let cy = h * 0.5;
                    
                    let radius = w * 0.37;
                    let inner = w * 0.28;
                    
                    df.hexagon(cx, cy, w * 0.45);
                    df.hexagon(cx, cy, w * 0.4);
                    df.subtract();
                    let ang = atan(pos.x * w - cx, 0.0001 + pos.y * h - cy) / PI * 0.5 + 0.5;
                    df.fill(Pal::hsv2rgb(vec4(ang, 1.0, 1.0, 1.0)));
                    
                    let rsize = inner / sqrt(2.0);
                    df.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
                    
                    let norm_rect = vec2(pos.x * w - (cx - inner), pos.y * h - (cy - inner)) / (2. * inner);
                    let circ = clamp(circ_to_rect(norm_rect.x * 2. - 1., norm_rect.y * 2. - 1.), vec2(-1.), vec2(1.));
                    
                    df.fill(Pal::hsv2rgb(vec4(hue, (circ.x * .5 + .5), 1. - (circ.y * .5 + .5), 1.)));
                    
                    let col_angle = (hue - .5) * 2. * PI;
                    let circle_puk = vec2(sin(col_angle) * radius + cx, cos(col_angle) * radius + cy);
                    
                    let rect_puk = vec2(cx + sat * 2. * rsize - rsize, cy + (1. - val) * 2. * rsize - rsize);
                    
                    let color = mix(mix(#3, #E, hover), #F, down);
                    let puck_size = 0.1 * w;
                    df.circle(rect_puk.x, rect_puk.y, puck_size);
                    df.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
                    df.intersect();
                    df.fill(color);
                    df.circle(rect_puk.x, rect_puk.y, puck_size - 1. - 2. * hover + down);
                    df.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
                    df.intersect();
                    df.fill(rgbv);
                    
                    df.circle(circle_puk.x, circle_puk.y, puck_size);
                    df.fill(color);
                    df.circle(circle_puk.x, circle_puk.y, puck_size - 1. - 2. * hover + down);
                    df.fill(rgbv);
                    
                    return df.result;
                }
                
            }
        "#)
    }
}

