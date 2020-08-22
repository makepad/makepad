use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::appstorage::*;
use makepad_render::PrettyPrintedFloat;

pub enum LiveMacro {
    Pick {range: (usize, usize), hsva: Vec4, in_shader: bool},
    Slide {range: (usize, usize), value: f32, min: f32, max: f32, step: f32, in_shader: bool},
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
    pub undo_id: u64,
    pub color_pickers: Elements<usize, ColorPicker, ColorPicker>,
    pub float_sliders: Elements<usize, FloatSlider, FloatSlider>
}


impl LiveMacrosView {
    pub fn macro_changed() -> StatusId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            scroll_view: ScrollView::new(cx),
            undo_id: 0,
            color_pickers: Elements::new(ColorPicker::new(cx)),
            float_sliders: Elements::new(FloatSlider::new(cx))
        }
    }
    
    pub fn handle_live_macros(&mut self, cx: &mut Cx, event: &mut Event, atb: &mut AppTextBuffer, text_editor: &mut TextEditor) {
        match event {
            Event::Signal(se) => {
                // process network messages for hub_ui
                if let Some(_) = se.signals.get(&atb.live_macros.changed) {
                    self.scroll_view.redraw_view_area(cx);
                }
            },
            _ => ()
        }
        
        // instead of enumerating these things, i could enumerate the live_macros instead.
        // then look up the widget by ID, and process any changes.
        
        self.scroll_view.handle_scroll_view(cx, event);
        
        let mut range_delta: isize = 0;
        let mut any_changed = false;
        let mut all_in_place = true;
        for (index, live_macro) in atb.live_macros.macros.iter_mut().enumerate() {
            match live_macro {
                LiveMacro::Pick {range, ..} => {
                    range.0 = (range.0 as isize + range_delta) as usize;
                    range.1 = (range.1 as isize + range_delta) as usize;
                    if let Some(color_picker) = self.color_pickers.get(index) {
                        match color_picker.handle_color_picker(cx, event) {
                            ColorPickerEvent::Change {hsva} => {
                                any_changed = true;
                                let color = Color::from_hsva(hsva);
                                let new_string = format!("#{}", color.to_hex());
                                let in_place = text_editor.handle_live_replace(cx, *range, &new_string, &mut atb.text_buffer, self.undo_id);
                                if !in_place {
                                    range_delta += new_string.len() as isize - (range.1 - range.0) as isize;
                                    *range = (range.0, range.0 + new_string.len());
                                    all_in_place = false;
                                }
                            },
                            ColorPickerEvent::DoneChanging => {
                                self.undo_id += 1;
                            },
                            _ => ()
                        }
                    }
                },
                LiveMacro::Slide {range, ..} => {
                    range.0 = (range.0 as isize + range_delta) as usize;
                    range.1 = (range.1 as isize + range_delta) as usize;
                    if let Some(float_slider) = self.float_sliders.get(index) {
                        match float_slider.handle_float_slider(cx, event) {
                            FloatSliderEvent::Change {scaled_value} => {
                                any_changed = true;
                                let new_string = format!("{}", PrettyPrintedFloat(scaled_value));
                                let in_place = text_editor.handle_live_replace(cx, *range, &new_string, &mut atb.text_buffer, self.undo_id);
                                if !in_place {
                                    range_delta += new_string.len() as isize - (range.1 - range.0) as isize;
                                    *range = (range.0, range.0 + new_string.len());
                                    all_in_place = false;
                                }
                            }
                            FloatSliderEvent::DoneChanging => {
                                self.undo_id += 1;
                            },
                            _ => ()
                        }
                    }
                },
                _ => ()
            }
        }
        if any_changed && all_in_place {
            atb.text_buffer.mark_clean();
            atb.parse_live_macros(cx, true);
        }
    }
    
    pub fn draw_live_macros(&mut self, cx: &mut Cx, atb: &mut AppTextBuffer, _text_editor: &mut TextEditor) {
        // alright so we have a list of macros..
        // now we have to draw them.
        if self.scroll_view.begin_view(cx, Layout {
            direction: Direction::Down,
            ..Layout::default()
        }).is_ok() {
            for (index, m) in atb.live_macros.macros.iter_mut().enumerate() {
                match m {
                    LiveMacro::Pick {hsva, ..} => {
                        self.color_pickers.get_draw(cx, index, | _, t | t.clone()).draw_color_picker(cx, *hsva);
                    },
                    LiveMacro::Slide {value, min, max, step, ..} => {
                        self.float_sliders.get_draw(cx, index, | _, t | t.clone())
                            .draw_float_slider(cx, *value, *min, *max, *step);
                    }
                    _ => ()
                }
            }
            self.scroll_view.end_view(cx);
        }
    }
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
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub size: f32,
    pub slider: Quad,
    pub dragging: bool
}

impl FloatSlider {
    
    pub fn slider() -> ShaderId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            norm_value: 0.0,
            scaled_value: 0.0,
            animator: Animator::default(),
            min: 0.0,
            max: 1.0,
            step: 0.0,
            size: 0.0,
            slider: Quad::new(cx),
            dragging: false
        }
    }
    
    pub fn handle_finger(&mut self, cx: &mut Cx, rel: Vec2) -> FloatSliderEvent {
        let norm_value = (rel.x / self.size).max(0.0).min(1.0);
        let mut scaled_value = norm_value * (self.max - self.min) + self.min;
        if self.step > 0.0 {
            scaled_value = (scaled_value / self.step).round() * self.step;
        }
        let mut changed = false;
        if scaled_value != self.scaled_value {
            self.scaled_value = scaled_value;
            self.norm_value = norm_value;
            self.animator.area.write_float(cx, Self::norm_value(), self.norm_value);
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
                        self.animator.play_anim(cx, Self::anim_hover().get(cx));
                    },
                    HoverState::Out => {
                        self.animator.play_anim(cx, Self::anim_default().get(cx));
                    },
                    _ => ()
                }
            },
            Event::FingerDown(fe) => {
                self.animator.play_anim(cx, Self::anim_down().get(cx));
                cx.set_down_mouse_cursor(MouseCursor::Arrow);
                self.dragging = true;
                return self.handle_finger(cx, fe.rel);
                // lets check where we clicked!
            },
            Event::FingerUp(fe) => {
                if fe.is_over {
                    if !fe.is_touch {
                        self.animator.play_anim(cx, Self::anim_hover().get(cx));
                    }
                    else {
                        self.animator.play_anim(cx, Self::anim_default().get(cx));
                    }
                }
                else {
                    self.animator.play_anim(cx, Self::anim_default().get(cx));
                }
                self.dragging = false;
                return FloatSliderEvent::DoneChanging;
            }
            Event::FingerMove(fe) => {
                return self.handle_finger(cx, fe.rel)
                
            },
            _ => ()
        }
        FloatSliderEvent::None
    }
    
    pub fn draw_float_slider(&mut self, cx: &mut Cx, scaled_value: f32, min: f32, max: f32, step: f32) {
        self.animator.init(cx, | cx | Self::anim_default().get(cx));
        if !self.dragging {
            self.scaled_value = scaled_value;
            self.min = min;
            self.max = max;
            self.step = step;
            self.norm_value = (scaled_value - min) / (max - min);
        }
        
        self.slider.shader = Self::slider().get(cx);
        // i wanna draw a wheel with 'width' set but height a fixed height.
        
        let pad = 10.;
        
        self.size = cx.get_turtle_rect().w - 2. * pad;
        let k = self.slider.draw_quad(cx, Walk {
            margin: Margin::left(pad),
            width: Width::FillPad(pad),
            height: Height::Fix(20.0)
        });
        // lets put a hsv int here
        k.push_float(cx, self.norm_value);
        k.push_last_float(cx, &self.animator, Self::hover());
        k.push_last_float(cx, &self.animator, Self::down());
        self.animator.set_area(cx, k.into());
    }
    
    pub fn norm_value() -> FloatId {uid!()}
    pub fn hover() -> FloatId {uid!()}
    pub fn down() -> FloatId {uid!()}
    
    pub fn anim_default() -> AnimId {uid!()}
    pub fn anim_hover() -> AnimId {uid!()}
    pub fn anim_down() -> AnimId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        Self::anim_default().set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::hover(), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(Self::down(), Ease::Lin, vec![(1.0, 0.)]),
        ]));
        
        Self::anim_hover().set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::down(), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(Self::hover(), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
        ]));
        
        Self::anim_down().set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::down(), Ease::OutExp, vec![(0.0, 0.0), (1.0, 3.1415 * 0.5)]),
            Track::float(Self::hover(), Ease::Lin, vec![(1.0, 1.0)]),
        ]));
        
        Self::slider().set(cx, Quad::def_quad_shader().compose(shader!{"
            
            instance norm_value: Self::norm_value();
            instance hover: Self::hover();
            instance down: Self::down();
            
            fn pixel() -> vec4 {
                let df = Df::viewport(pos * vec2(w, h));
                
                let cy = h * 0.5;
                let height = 5.;
                df.box(4., cy - 0.5 * height, w - 10., height, 1.);
                
                df.fill(pick!(#4));
                
                let bheight = 15.;
                let bwidth = 10.;
                
                df.box((w - bwidth) * norm_value, cy - 0.5 * bheight, bwidth, bheight, 2.);
                ////
                let color = mix(mix(pick!(#5), pick!(#B), hover), pick!(#F), down);
                df.fill(color);
                
                return df.result;
            }
        "}))
    }
}


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
    
    pub fn wheel() -> ShaderId {uid!()}
    
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
            self.animator.area.write_float(cx, Self::hue(), self.hue);
            changed = true;
        }
        if last_sat != self.sat {
            self.animator.area.write_float(cx, Self::sat(), self.sat);
            changed = true;
        }
        if last_val != self.val {
            self.animator.area.write_float(cx, Self::val(), self.val);
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
                        self.animator.play_anim(cx, Self::anim_hover().get(cx));
                    },
                    HoverState::Out => {
                        self.animator.play_anim(cx, Self::anim_default().get(cx));
                    },
                    _ => ()
                }
            },
            Event::FingerDown(fe) => {
                self.animator.play_anim(cx, Self::anim_down().get(cx));
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
                        self.animator.play_anim(cx, Self::anim_hover().get(cx));
                    }
                    else {
                        self.animator.play_anim(cx, Self::anim_default().get(cx));
                    }
                }
                else {
                    self.animator.play_anim(cx, Self::anim_default().get(cx));
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
        self.animator.init(cx, | cx | Self::anim_default().get(cx));
        if self.drag_mode == ColorPickerDragMode::None {
            self.hue = hsva.x;
            self.sat = hsva.y;
            self.val = hsva.z;
        }
        self.wheel.shader = Self::wheel().get(cx);
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
        
        k.push_last_float(cx, &self.animator, Self::hover());
        k.push_last_float(cx, &self.animator, Self::down());
        
        self.animator.set_area(cx, k.into());
    }
    
    pub fn hue() -> FloatId {uid!()}
    pub fn sat() -> FloatId {uid!()}
    pub fn val() -> FloatId {uid!()}
    pub fn hover() -> FloatId {uid!()}
    pub fn down() -> FloatId {uid!()}
    
    pub fn anim_default() -> AnimId {uid!()}
    pub fn anim_hover() -> AnimId {uid!()}
    pub fn anim_down() -> AnimId {uid!()}
    
    pub fn style(cx: &mut Cx, _opt: &StyleOptions) {
        Self::anim_default().set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::hover(), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(Self::down(), Ease::Lin, vec![(1.0, 0.)]),
        ]));
        
        Self::anim_hover().set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::down(), Ease::Lin, vec![(1.0, 0.)]),
            Track::float(Self::hover(), Ease::Lin, vec![(0.0, 1.0), (1.0, 1.0)]),
        ]));
        
        Self::anim_down().set(cx, Anim::new(Play::Cut {duration: 0.2}, vec![
            Track::float(Self::down(), Ease::OutExp, vec![(0.0, 0.0), (1.0, 3.1415 * 0.5)]),
            Track::float(Self::hover(), Ease::Lin, vec![(1.0, 1.0)]),
        ]));
        
        Self::wheel().set(cx, Quad::def_quad_shader().compose(shader!{"
            
            instance hue: Self::hue();
            instance sat: Self::sat();
            instance val: Self::val();
            instance hover: Self::hover();
            instance down: Self::down();
            
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
                
                let color = mix(mix(pick!(#3), pick!(#E), hover), pick!(#F), down);
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
        "}))
    }
}


impl AppTextBuffer {
    pub fn parse_live_macros(&mut self, cx: &mut Cx, only_update_shaders:bool) {
        let mut tp = TokenParser::new(&self.text_buffer.flat_text, &self.text_buffer.token_chunks);
        // lets reset the data
        if !only_update_shaders{
            self.live_macros.macros.truncate(0);
        }
        let mut shader_end = 0;
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    if !only_update_shaders && tp.eat("pick") && tp.eat("!") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            let range = tp.cur_pair_range();
                            tp.advance();
                            let in_shader = tp.cur_offset() < shader_end;
                            
                            // TODO parse 1,2,3,4 number arg version of pick!
                            
                            // ok so now we need to parse the color, and turn it to HSV
                            let color = if tp.cur_type() == TokenType::Color { // its a #color
                                let color = Color::parse_hex_str(&tp.cur_as_string()[1..]);
                                if let Ok(color) = color {color}else {Color::white()}
                            }
                            else { // its a named color
                                let color = Color::parse_name(&tp.cur_as_string());
                                if let Ok(color) = color {color}else {Color::white()}
                            };
                            // ok lets turn color into HSV and store it in data
                            self.live_macros.macros.push(LiveMacro::Pick {
                                in_shader,
                                range: (range.0 + 1, range.1),
                                hsva: color.to_hsv()
                            })
                        }
                    }
                    else if !only_update_shaders && tp.eat("slide") && tp.eat("!") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            let in_shader = tp.cur_offset() < shader_end;
                            // now i just want the first number
                            let paren_range = tp.cur_pair_range();
                            tp.advance();
                            let mut value = 1.0;
                            let mut min = 0.0;
                            let mut max = 1.0;
                            let mut step = 0.0;
                            let range;
                            if tp.cur_type() == TokenType::Number {
                                // it could be followed by a min, max and step
                                value = if let Ok(v) = tp.cur_as_string().parse() {v}else {1.0};
                                range = tp.cur_range();
                                tp.advance();
                                if tp.eat(",") && tp.cur_type() == TokenType::Number {
                                    min = if let Ok(v) = tp.cur_as_string().parse() {v}else {0.0};
                                    tp.advance();
                                    if tp.eat(",") && tp.cur_type() == TokenType::Number {
                                        max = if let Ok(v) = tp.cur_as_string().parse() {v}else {1.0};
                                        tp.advance();
                                        if tp.eat(",") && tp.cur_type() == TokenType::Number {
                                            step = if let Ok(v) = tp.cur_as_string().parse() {v}else {0.0};
                                            tp.advance();
                                        }
                                    }
                                }
                            }
                            else {
                                range = (paren_range.0 + 1, paren_range.1);
                            }
                            self.live_macros.macros.push(LiveMacro::Slide {
                                in_shader,
                                value,
                                min,
                                max,
                                range,
                                step
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

