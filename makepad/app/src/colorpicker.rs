use makepad_render::*;

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawColorPicker {
    #[default_shader(self::shader_wheel)]
    base: DrawQuad,
    hue: f32,
    sat: f32,
    val: f32,
    hover: f32,
    down: f32
}

pub enum ColorPickerEvent {
    Change {rgba: Vec4},
    DoneChanging,
    None
}

#[derive(Clone)]
pub struct ColorPicker {
    pub size: f32,
    pub hue: f32,
    pub sat: f32,
    pub val: f32,
    pub wheel: DrawColorPicker,
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
            wheel: DrawColorPicker::new(cx, default_shader!()),
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
                self.hue = vx.atan2(vy) / std::f32::consts::PI * 0.5 - 0.33333;
            },
            _ => ()
        }
        // lets update hue sat val directly
        let mut changed = false;
        if last_hue != self.hue {
            self.wheel.set_hue(cx, self.hue);
            changed = true;
        }
        if last_sat != self.sat {
            self.wheel.set_sat(cx, self.sat);
            changed = true;
        }
        if last_val != self.val {
            self.wheel.set_val(cx, self.val);
            changed = true;
        }
        if changed {
            ColorPickerEvent::Change {rgba: self.to_rgba()}
        }
        else {
            ColorPickerEvent::None
        }
    }
    
    pub fn to_rgba(&self)->Vec4{
        Vec4::from_hsva(Vec4{x: self.hue, y: self.sat, z: self.val, w: 1.0})
    }
    
    pub fn handle_color_picker(&mut self, cx: &mut Cx, event: &mut Event) -> ColorPickerEvent {
        if let Some(ae) = event.is_animate(cx, &self.animator) {
            self.wheel.animate(cx, &mut self.animator, ae.time);
        }

        match event.hits(cx, self.wheel.area(), HitOpt::default()) {
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
    
    pub fn draw_color_picker(&mut self, cx: &mut Cx, rgba:Vec4,  height_scale: f32) {
         if self.animator.need_init(cx) {
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.wheel.last_animate(&self.animator);
        }
        
        if self.drag_mode == ColorPickerDragMode::None {
            // lets convert to rgba
            let old_rgba  = self.to_rgba();
            if !rgba.is_equal_enough(&old_rgba){
                let hsva = rgba.to_hsva();
                self.hue = hsva.x;
                self.sat = hsva.y;
                self.val = hsva.z;
            }
        }
        //self.wheel.shader = live_shader!(cx, self::shader_wheel);
        // i wanna draw a wheel with 'width' set but height a fixed height.
        self.size = cx.get_turtle_rect().size.x;
        self.wheel.hue = self.hue;
        self.wheel.sat = self.sat;
        self.wheel.val = self.val;
        
        self.wheel.draw_quad_walk(cx, Walk {
            margin: Margin::bottom(10.*height_scale),
            width: Width::Fill,
            height: Height::Fix(self.size * height_scale)
        });
    }

    pub fn style(cx: &mut Cx) {
        self::DrawColorPicker::register_draw_input(cx);
        live_body!(cx, r#"
            self::anim_default: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::DrawColorPicker::hover, keys: {1.0: 0.0}},
                    Float {bind_to: self::DrawColorPicker::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_hover: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::DrawColorPicker::hover, keys: {0.0: 1.0}},
                    Float {bind_to: self::DrawColorPicker::down, keys: {1.0: 0.0}}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks: [
                    Float {bind_to: self::DrawColorPicker::hover, keys: {1.0: 1.0}},
                    Float {bind_to: self::DrawColorPicker::down, keys: {0.0: 0.0, 1.0: 1.0}}
                ]
            }
            
            self::shader_wheel: Shader {
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawColorPicker;
                
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
                    let w = rect_size.x;
                    let h = rect_size.y;
                    let df = Df::viewport(pos * vec2(w, h));
                    let cx = w * 0.5;
                    let cy = h * 0.5;
                    
                    let radius = w * 0.37;
                    let inner = w * 0.28;
                    
                    df.hexagon(cx, cy, w * 0.45);
                    df.hexagon(cx, cy, w * 0.4);
                    df.subtract();
                    let ang = atan(pos.x * w - cx, 0.0001 + pos.y * h - cy) / PI * 0.5 - 0.33333;
                    df.fill(Pal::hsv2rgb(vec4(ang, 1.0, 1.0, 1.0)));
                    
                    let rsize = inner / sqrt(2.0);
                    df.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
                    
                    let norm_rect = vec2(pos.x * w - (cx - inner), pos.y * h - (cy - inner)) / (2. * inner);
                    let circ = clamp(circ_to_rect(norm_rect.x * 2. - 1., norm_rect.y * 2. - 1.), vec2(-1.), vec2(1.));
                    
                    df.fill(Pal::hsv2rgb(vec4(hue, (circ.x * .5 + .5), 1. - (circ.y * .5 + .5), 1.)));
                    
                    let col_angle = (hue + .333333) * 2. * PI;
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

