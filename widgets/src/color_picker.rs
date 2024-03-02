use crate::makepad_draw::*;


live_design!{
    import makepad_draw::shader::std::*;
    
    DrawColorWheel= {{DrawColorWheel}} {
        instance hover: float
        instance pressed: float
        
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
        
        fn pixel(self) -> vec4 {
            
            let rgbv = Pal::hsv2rgb(vec4(self.hue, self.sat, self.val, 1.));
            let w = self.rect_size.x;
            let h = self.rect_size.y;
            let sdf = Sdf2d::viewport(self.pos * vec2(w, h));
            let cx = w * 0.5;
            let cy = h * 0.5;
            
            let radius = w * 0.37;
            let inner = w * 0.28;
            
            sdf.hexagon(cx, cy, w * 0.45);
            sdf.hexagon(cx, cy, w * 0.4);
            sdf.subtract();
            let ang = atan(self.pos.x * w - cx, 0.0001 + self.pos.y * h - cy) / PI * 0.5 - 0.33333;
            sdf.fill(Pal::hsv2rgb(vec4(ang, 1.0, 1.0, 1.0)));
            
            let rsize = inner / sqrt(2.0);
            sdf.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
            
            let norm_rect = vec2(self.pos.x * w - (cx - inner), self.pos.y * h - (cy - inner)) / (2. * inner);
            let circ = clamp(circ_to_rect(norm_rect.x * 2. - 1., norm_rect.y * 2. - 1.), vec2(-1.), vec2(1.));
            
            sdf.fill(Pal::hsv2rgb(vec4(self.hue, (circ.x * .5 + .5), 1. - (circ.y * .5 + .5), 1.)));
            
            let col_angle = (self.hue + .333333) * 2. * PI;
            let circle_puk = vec2(sin(col_angle) * radius + cx, cos(col_angle) * radius + cy);
            
            let rect_puk = vec2(cx + self.sat * 2. * rsize - rsize, cy + (1. - self.val) * 2. * rsize - rsize);
            
            let color = mix(mix(#3, #E, self.hover), #F, self.pressed);
            let puck_size = 0.1 * w;
            sdf.circle(rect_puk.x, rect_puk.y, puck_size);
            sdf.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
            sdf.intersect();
            sdf.fill(color);
            sdf.circle(rect_puk.x, rect_puk.y, puck_size - 1. - 2. * self.hover + self.pressed);
            sdf.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
            sdf.intersect();
            sdf.fill(rgbv);
            
            sdf.circle(circle_puk.x, circle_puk.y, puck_size);
            sdf.fill(color);
            sdf.circle(circle_puk.x, circle_puk.y, puck_size - 1. - 2. * self.hover + self.pressed);
            sdf.fill(rgbv);
            
            return sdf.result;
        }
    }
    
    ColorPicker= {{ColorPicker}} {
        
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_wheel: {pressed: 0.0, hover: 0.0}
                    }
                }
                
                on = {
                    cursor: Arrow,
                    from: {
                        all: Forward {duration: 0.1}
                        pressed: Forward {duration: 0.01}
                    }
                    apply: {
                        draw_wheel: {
                            pressed: 0.0,
                            hover: [{time: 0.0, value: 1.0}],
                        }
                    }
                }
                
                pressed = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.2}}
                    apply: {
                        draw_wheel: {
                            pressed: [{time: 0.0, value: 1.0}],
                            hover: 1.0,
                        }
                    }
                }
            }
        }
    }
}


#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawColorWheel {
    #[deref] draw_super: DrawQuad,
    #[live] hue: f32,
    #[live] sat: f32,
    #[live] val: f32,
}

#[derive(Live, LiveHook, LiveRegister)]
pub struct ColorPicker {
    #[live] draw_wheel: DrawColorWheel,
    
    #[animator] animator: Animator,
    
    #[rust] pub size: f64,
    #[rust] hue: f32,
    #[rust] sat: f32,
    #[rust] val: f32,
    #[rust(ColorPickerDragMode::None)] drag_mode: ColorPickerDragMode
}

pub enum ColorPickerAction {
    Change {rgba: Vec4},
    DoneChanging,
    None
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColorPickerDragMode {
    Wheel,
    Rect,
    None
}

impl ColorPicker {
    
    pub fn handle_finger(&mut self, cx: &mut Cx, rel: DVec2, dispatch_action: &mut dyn FnMut(&mut Cx, ColorPickerAction)) {
        
        fn clamp(x: f64, mi: f64, ma: f64) -> f64 {if x < mi {mi} else if x > ma {ma} else {x}}
        
        let vx = rel.x - 0.5 * self.size;
        let vy = rel.y - 0.5 * self.size;
        let rsize = (self.size * 0.28) / 2.0f64.sqrt();
        let last_hue = self.hue;
        let last_sat = self.sat;
        let last_val = self.val;
        
        match self.drag_mode {
            ColorPickerDragMode::Rect => {
                self.sat = clamp((vx + rsize) / (2.0 * rsize), 0.0, 1.0) as f32;
                self.val = 1.0 - clamp((vy + rsize) / (2.0 * rsize), 0.0, 1.0) as f32;
            },
            ColorPickerDragMode::Wheel => {
                self.hue = ((vx.atan2(vy) / std::f64::consts::PI * 0.5) - 0.33333 + 1.0) as f32;
            },
            _ => ()
        }
        // lets update hue sat val directly
        let mut changed = false;
        
        if last_hue != self.hue {
            self.draw_wheel.apply_over(cx, live!{hue: (self.hue)});
            changed = true;
        }
        if last_sat != self.sat {
            self.draw_wheel.apply_over(cx, live!{sat: (self.sat)});
            changed = true;
        }
        if last_val != self.val {
            self.draw_wheel.apply_over(cx, live!{val: (self.val)});
            changed = true;
        }
        if changed {
            dispatch_action(cx, ColorPickerAction::Change {rgba: self.to_rgba()})
        }
    }
    
    pub fn to_rgba(&self) -> Vec4 {
        Vec4::from_hsva(Vec4 {x: self.hue, y: self.sat, z: self.val, w: 1.0})
    }
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, ColorPickerAction)) {
        self.animator_handle_event(cx, event);
        
        match event.hits(cx, self.draw_wheel.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, id!(hover.on));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, id!(hover.off));
            },
            Hit::FingerDown(fe) => {
                self.animator_play(cx, id!(hover.pressed));
                let rsize = (self.size * 0.28) / 2.0f64.sqrt();
                let rel = fe.abs - fe.rect.pos;
                let vx = rel.x - 0.5 * self.size;
                let vy = rel.y - 0.5 * self.size;
                if vx >= -rsize && vx <= rsize && vy >= -rsize && vy <= rsize {
                    self.drag_mode = ColorPickerDragMode::Rect;
                }
                else if vx >= -0.5 * self.size && vx <= 0.5 * self.size && vy >= -0.5 * self.size && vy <= 0.5 * self.size {
                    self.drag_mode = ColorPickerDragMode::Wheel;
                }
                else {
                    self.drag_mode = ColorPickerDragMode::None;
                }
                return self.handle_finger(cx, rel, dispatch_action);
                // lets check where we clicked!
            },
            Hit::FingerUp(fe) => {
                if fe.is_over && fe.device.has_hovers() {
                    self.animator_play(cx, id!(hover.on));
                }
                else {
                    self.animator_play(cx, id!(hover.off));
                }
                self.drag_mode = ColorPickerDragMode::None;
                dispatch_action(cx, ColorPickerAction::DoneChanging)
            }
            Hit::FingerMove(fe) => {
                let rel = fe.abs - fe.rect.pos;
                return self.handle_finger(cx, rel, dispatch_action)
                
            },
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d, rgba: Vec4, height_scale: f64) {
        if self.drag_mode == ColorPickerDragMode::None {
            // lets convert to rgba
            let old_rgba = self.to_rgba();
            if !rgba.is_equal_enough(&old_rgba, 0.0001) {
                let hsva = rgba.to_hsva();
                self.hue = hsva.x;
                self.sat = hsva.y;
                self.val = hsva.z;
            }
        }
        //self.draw_wheel.shader = live_shader!(cx, self::shader_wheel);
        // i wanna draw a draw_wheel with 'width' set but height a fixed height.
        self.size = cx.turtle().rect().size.y;
        self.draw_wheel.hue = self.hue;
        self.draw_wheel.sat = self.sat;
        self.draw_wheel.val = self.val;
        self.draw_wheel.draw_walk(cx, Walk::fixed_size(dvec2(self.size * height_scale, self.size * height_scale)));
    }
}

