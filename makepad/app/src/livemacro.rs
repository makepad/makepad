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
    
    pub fn handle_live_macros(&mut self, cx: &mut Cx, event: &mut Event, atb: &mut AppTextBuffer, text_editor: &mut TextEditor){
        self.scroll_view.handle_scroll_view(cx, event);
        for (index, color_picker) in self.color_pickers.enumerate() {
            match color_picker.handle_color_picker(cx, event) {
                ColorPickerEvent::Change {hsva} => {
                    
                    // ok now what. now we serialize out hsva into the textbuffer
                    if let LiveMacro::Color{range,..} = &mut atb.live_macros.macros[*index]{
                        // and let the things work out
                        let color = Color::from_hsva(hsva);
                        // we have a range, now lets set the cursors to that range
                        // and replace shit.
                        let new_string = format!("#{}",color.to_hex());
                        text_editor.handle_live_replace(cx, *range, &new_string, &mut atb.text_buffer, 0);
                        *range = (range.0, range.0 + new_string.len());
                    }
                },
                _ => ()
            }
        }
    }
    
    pub fn draw_live_macros(&mut self, cx: &mut Cx, atb: &mut AppTextBuffer, _text_editor: &mut TextEditor) {
        // alright so we have a list of macros..
        // now we have to draw them.
        if self.scroll_view.begin_view(cx, Layout{
            direction:Direction::Down,
            ..Layout::default()
        }).is_ok(){
            for (index, m) in atb.live_macros.macros.iter_mut().enumerate() {
                match m {
                    LiveMacro::Color {hsva, ..} => {
                        self.color_pickers.get_draw(cx, index, | _, t | t.clone()).draw_color_picker(cx, *hsva);
                    },
                    _ => ()
                }
                
            }
            self.scroll_view.end_view(cx);
        }
    }
}


pub enum ColorPickerEvent {
    Change {hsva: Vec4},
    None
}

#[derive(Clone)]
pub struct ColorPicker {
    pub size: f32,
    pub hue: f32,
    pub sat: f32,
    pub val: f32,
    pub wheel: Quad,
    pub wheel_area: Area,
    pub drag_mode: ColorPickerDragMode
}


#[derive(Clone, Debug, PartialEq)]
pub enum ColorPickerDragMode {
    Wheel,
    Rect,
    None
}
impl ColorPicker {
    
    pub fn wheel() -> ShaderId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            hue: 0.0,
            sat: 0.0,
            val: 0.0,
            size: 0.0,
            wheel: Quad::new(cx),
            wheel_area: Area::Empty,
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
            self.wheel_area.write_float(cx, Self::hue(), self.hue);
            changed = true;
        }
        if last_sat != self.sat {
            self.wheel_area.write_float(cx, Self::sat(), self.sat);
            changed = true;
        }
        if last_val != self.val {
            self.wheel_area.write_float(cx, Self::val(), self.val);
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
        match event.hits(cx, self.wheel_area, HitOpt::default()) {
            Event::FingerHover(_fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Arrow);
            },
            Event::FingerDown(fe) => {
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
            Event::FingerUp(_fe)=>{
                self.drag_mode = ColorPickerDragMode::None;
            }
            Event::FingerMove(fe) => {
                return self.handle_finger(cx, fe.rel)
                
            },
            _ => ()
        }
        ColorPickerEvent::None
    }
    
    pub fn draw_color_picker(&mut self, cx: &mut Cx, hsva: Vec4) {
        if self.drag_mode == ColorPickerDragMode::None{
            self.hue = hsva.x;
            self.sat = hsva.y;
            self.val = hsva.z;
        }
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
        
        self.wheel_area = cx.update_area_refs(self.wheel_area, k.into());
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
                
                df.circle(rect_puk.x, rect_puk.y, 8.);
                df.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
                df.intersect();
                df.fill(color!(white));
                df.circle(rect_puk.x, rect_puk.y, 7.);
                df.rect(cx - rsize, cy - rsize, rsize * 2.0, rsize * 2.0);
                df.intersect();
                df.fill(rgbv);
                
                df.circle(circle_puk.x, circle_puk.y, 11.);
                df.fill(color!(white));
                df.circle(circle_puk.x, circle_puk.y, 10.);
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
                            let color = if tp.cur_type() == TokenType::Color { // its a #color
                                let color = Color::parse_hex(&tp.cur_as_string()[1..]);
                                if let Ok(color) = color {color}else {Color::white()}
                            }
                            else { // its a named color
                                let color = Color::parse_name(&tp.cur_as_string());
                                if let Ok(color) = color {color}else {Color::white()}
                            };
                            // ok lets turn color into HSV and store it in data
                            self.live_macros.macros.push(LiveMacro::Color {
                                in_shader,
                                range: (range.0+1,range.1),
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

