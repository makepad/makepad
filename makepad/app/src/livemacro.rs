use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::appstorage::*;

pub enum LiveMacro {
    Color {range: (usize, usize), hsva: Vec4, in_shader: bool},
    Shader
}

pub struct LiveMacros {
    changed: Signal,
    macros: Vec<LiveMacro>
}

impl LiveMacros {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            changed: cx.new_signal(),
            macros: Vec::new()
        }
    }
}

#[derive(Clone)]
pub struct LiveMacrosView {
    pub scroll_view: ScrollView,
    pub color_pickers: Elements<usize, ColorPicker, ColorPicker>
}


impl LiveMacrosView {
    pub fn macro_changed() -> StatusId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            scroll_view: ScrollView::new(cx),
            color_pickers: Elements::new(ColorPicker::new(cx))
        }
    }
    
    pub fn handle_live_macros(&mut self, cx: &mut Cx, event: &mut Event, _atb: &mut AppTextBuffer) {
        for (_index, color_picker) in self.color_pickers.enumerate() {
            color_picker.handle_color_picker(cx, event);
        }
    }
    
    pub fn draw_live_macros(&mut self, cx: &mut Cx, atb: &mut AppTextBuffer) {
        // alright so we have a list of macros..
        // now we have to draw them.
        for (index, _m) in atb.live_macros.macros.iter_mut().enumerate() {
            self.color_pickers.get_draw(cx, index, | _, t | t.clone()).draw_color_picker(cx);
        }
    }
}


pub enum ColorPickerEvent {
    None
}

#[derive(Clone)]
pub struct ColorPicker {
    pub size: f32,
    pub hue:f32,
    pub sat:f32,
    pub val:f32,
    pub wheel: Quad,
    pub animator: Animator,
    pub wheel_area: Area,
    pub drag_mode: ColorPickerDragMode
}


#[derive(Clone)]
pub enum ColorPickerDragMode {
    Wheel,
    Rect,
    None
}
const TORAD:f32 = 0.017453292519943295;
impl ColorPicker {
    
    pub fn wheel() -> ShaderId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            hue:0.0,
            sat:0.0,
            val:0.0,
            size: 0.0,
            wheel: Quad::new(cx),
            animator: Animator::default(),
            wheel_area: Area::Empty,
            drag_mode: ColorPickerDragMode::None
        }
    }
    
    pub fn handle_color_picker(&mut self, cx: &mut Cx, event: &mut Event) -> ColorPickerEvent {
        match event.hits(cx, self.wheel_area, HitOpt::default()) {
            Event::FingerHover(_fe)=>{
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
            },
            Event::FingerDown(fe) => {
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                let rsize = (self.size * 0.28) / 2.0f32.sqrt();
                let cx = fe.rel.x - 0.5 * self.size;
                let cy = fe.rel.y - 0.5 * self.size;
                if cx >= -rsize && cx <= rsize && cy >= -rsize && cy <= rsize {
                    self.drag_mode = ColorPickerDragMode::Rect;
                }
                else if cx >= -0.5 * self.size && cx <= 0.5 * self.size && cy >= -0.5 * self.size && cy <= 0.5 * self.size {
                    self.drag_mode = ColorPickerDragMode::Wheel;
                }
                else {
                    self.drag_mode = ColorPickerDragMode::None;
                }
                
                // lets check where we clicked!
            },
            Event::FingerMove(fe) => {
                fn clamp(x:f32, mi:f32, ma:f32)->f32{ if x < mi{mi} else if x > ma{ma} else {x}};
                let vx = fe.rel.x - 0.5 * self.size;
                let vy = fe.rel.y - 0.5 * self.size;
                let rsize = (self.size * 0.28) / 2.0f32.sqrt();
                match self.drag_mode {
                    ColorPickerDragMode::Rect => {
                        self.sat = clamp((vx + rsize) / (2.0 * rsize), 0.0, 1.0);
                        self.val = 1.0 - clamp((vy + rsize) / (2.0 * rsize), 0.0, 1.0);
                    },
                    ColorPickerDragMode::Wheel => {
                        fn hexagon_side(x: f32, y: f32, r: f32) -> f32 {
                            let dx = x.abs() * 1.15;
                            let dy = y.abs();
                            if (dy + (60.0 * TORAD).cos() * dx - r).max(dx - r) < 0.0 {
                                return -1.0;
                            }
                            // alright check if we are either
                            let t1 = dy + 0.5 * r - 1.5 * (dx - r) - r;
                            let t2 = dy - 1.5 * (dx) - r;
                            let t3 = dy + 0.5 * r - r;
                            if t1 > 0.0 && t2 < 0.0 {return -1.0;}
                            if t3 < 0.0 {return -1.0;}
                            if t2 > 0.0 {
                                if y < 0.0 {return 1.0;}
                                else {return 0.5;}
                            }
                            if x > 0.0{
                                if y > 0.0{
                                    return 4.0/6.0;
                                }
                                else {
                                    return 5.0/6.0;
                                }
                            }
                            else{
                                if y > 0.0{
                                    return 2.0/6.0
                                }
                                else{
                                    return 1.0/6.0
                                }
                            }
                        }
                        
                        let side = hexagon_side(vx, vy, self.size * 0.5);
                        if side < 0.0 {
                            self.hue = vx.atan2(vy) / std::f32::consts::PI * 0.5 + 0.5;
                        }
                        else {
                            self.hue = side
                        }
                    },
                    _ => ()
                }
                // lets update hue sat val directly
                self.wheel_area.write_float(cx, Self::hue(), self.hue);
                self.wheel_area.write_float(cx, Self::sat(), self.sat);
                self.wheel_area.write_float(cx, Self::val(), self.val);
            },
            _ => ()
        }
        ColorPickerEvent::None
    }
    
    pub fn draw_color_picker(&mut self, cx: &mut Cx) {
        self.wheel.shader = Self::wheel().get(cx);
        // i wanna draw a wheel with 'width' set but height a fixed height.
        self.size = cx.get_turtle_rect().w;
        let k = self.wheel.draw_quad(cx, Walk {
            margin: Margin::zero(),
            width: Width::Fill,
            height: Height::Fix(self.size * 1.0)
        });
        // lets put a hsv int here
        k.push_float(cx, self.hue);
        k.push_float(cx, self.sat);
        k.push_float(cx, self.val);
        self.wheel_area = k.into();
    }
    
    pub fn hue() -> FloatId {uid!()}
    pub fn sat() -> FloatId {uid!()}
    pub fn val() -> FloatId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        Self::wheel().set(cx, Quad::def_quad_shader().compose(shader!{"
            
            instance hue: Self::hue();
            instance sat: Self::sat();
            instance val: Self::val();
            
            fn circ_to_rect(u: float, v: float) -> vec2 {
                let u2 = u * u;
                let v2 = v * v;
                return vec2(
                    0.5 * sqrt(2. + 2. * sqrt(2.) * u + u2 - v2) -
                    0.5 * sqrt(2. - 2. * sqrt(2.) * u + u2 - v2),
                    0.5 * sqrt(2. + 2. * v * sqrt(2.) - u2 + v2) -
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
                
                df.hexagon(cx, cy, w * 0.5);
                df.hexagon(cx, cy, w * 0.32);
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
                
                df.circle(rect_puk.x, rect_puk.y, 10.);
                df.fill(color!(white));
                df.circle(rect_puk.x, rect_puk.y, 9.);
                df.fill(rgbv);
                
                df.circle(circle_puk.x, circle_puk.y, 14.);
                df.fill(color!(white));
                df.circle(circle_puk.x, circle_puk.y, 13.);
                df.fill(rgbv);
                
                return df.result;
            }
        "}))
    }
}


impl AppTextBuffer {
    pub fn parse_live_macros(&mut self, cx: &mut Cx) {
        let mut tp = TokenParser::new(&self.text_buffer.flat_text, &self.text_buffer.token_chunks);
        // lets reset the data
        self.live_macros.macros.truncate(0);
        let mut shader_end = 0;
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    if tp.eat("color") && tp.eat("!") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            let range = tp.cur_pair_range();
                            tp.advance();
                            let in_shader = tp.cur_offset() < shader_end;
                            // ok so now we need to parse the color, and turn it to HSV
                            let color = if tp.cur_type() == TokenType::Hash { // its a #color
                                tp.advance();
                                let color = Color::parse_hex(&tp.cur_as_string());
                                if let Ok(color) = color {color}else {Color::white()}
                            }
                            else { // its a named color
                                let color = Color::parse_name(&tp.cur_as_string());
                                if let Ok(color) = color {color}else {Color::white()}
                            };
                            // ok lets turn color into HSV and store it in data
                            self.live_macros.macros.push(LiveMacro::Color {
                                in_shader,
                                range,
                                hsva: color.to_hsv()
                            })
                        }
                    }
                    else if tp.eat("shader") && tp.eat("!") && tp.eat("{") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            shader_end = tp.cur_pair_offset();
                            if let Some(shader) = tp.cur_pair_as_string() {
                                let lc = tp.cur_line_col();
                                
                                cx.recompile_shader_sub(
                                    &self.full_path["main/makepad/".len()..],
                                    lc.0 + 1,
                                    lc.1 - 8,
                                    shader
                                );
                                //println!("{} {}:{}",self.full_path, lc.0, lc.1);
                            }
                            
                            //tp.jump_to_pair();
                        }
                    }
                },
                _ => ()
            }
        }
        cx.send_signal(self.live_macros.changed, LiveMacrosView::macro_changed());
    }
}

