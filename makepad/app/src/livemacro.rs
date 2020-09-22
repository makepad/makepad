use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::appstorage::*;
use crate::colorpicker::*;
use crate::floatslider::*;
use std::fmt;

pub enum LiveMacro {
    Pick {range: (usize, usize), hsva: Vec4, in_shader: bool},
    Slide {range: (usize, usize), value: f32, min: f32, max: f32, step: f32, in_shader: bool},
    Shader
}

pub struct LiveMacros {
    pub changed: Signal,
    pub macros: Vec<LiveMacro>
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
    pub bg: Quad,
    pub text: Text,
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
            float_sliders: Elements::new(FloatSlider::new(cx)),
            bg: Quad::new(cx),
            text: Text::new(cx),
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live!(cx, r#"
            self::layout_bg: Layout {
                align: all(0.5),
                walk: {
                    width: Fill,
                    height: Compute,
                    margin: all(1.0),
                },
                padding: {l: 16.0, t: 12.0, r: 16.0, b: 12.0},
            }
            
            self::text_style_caption: TextStyle {
                ..makepad_widget::widgetstyle::text_style_normal
            }
        "#)
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
                LiveMacro::Pick {range, hsva, ..} => {
                    range.0 = (range.0 as isize + range_delta) as usize;
                    range.1 = (range.1 as isize + range_delta) as usize;
                    if let Some(color_picker) = self.color_pickers.get(index) {
                        match color_picker.handle_color_picker(cx, event) {
                            ColorPickerEvent::Change {hsva: new_hsva} => {
                                any_changed = true;
                                let color = Color::from_hsva(new_hsva);
                                *hsva = new_hsva;
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
                LiveMacro::Slide {range, value, ..} => {
                    range.0 = (range.0 as isize + range_delta) as usize;
                    range.1 = (range.1 as isize + range_delta) as usize;
                    if let Some(float_slider) = self.float_sliders.get(index) {
                        match float_slider.handle_float_slider(cx, event) {
                            FloatSliderEvent::Change {scaled_value} => {
                                *value = scaled_value;
                                any_changed = true;
                                let new_string = format!("{}", PrettyPrintedFloat3Decimals(scaled_value));
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
            atb.parse_live_macros(cx);
        }
    }
    
    pub fn draw_heading(&mut self, _cx: &mut Cx) {
        
    }
    
    pub fn draw_live_macros(&mut self, cx: &mut Cx, atb: &mut AppTextBuffer, _text_editor: &mut TextEditor) {
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

pub struct PrettyPrintedFloat3Decimals(pub f32);

impl fmt::Display for PrettyPrintedFloat3Decimals {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.000", self.0)
        } else {
            write!(f, "{:.3}", self.0)
        }
    }
}


impl AppTextBuffer {
    pub fn parse_live_macros(&mut self, cx: &mut Cx) {
        let mut tp = TokenParser::new(&self.text_buffer.flat_text, &self.text_buffer.token_chunks);
        // lets reset the data
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    // we need to find our live!(cx, r#".."#)
                    if tp.eat("live")
                        && tp.eat("!") 
                        && tp.eat("(") 
                        && tp.eat_token(TokenType::Identifier) 
                        && tp.eat(",")
                        && tp.eat("r")
                        && tp.eat("#"){
                        // so on a remote target.. 
                        // maybe we should ask the remote target for values?
                        // and then we live edit it here and cycle through the remote parser?
                        // 
                        if tp.cur_type() == TokenType::ParenOpen {
                            //shader_end = tp.cur_pair_offset();
                            if let Some(_shader) = tp.cur_pair_as_string() {
                                let _lc = tp.cur_line_col();
                                /*
                                cx.recompile_shader_sub(
                                    &self.full_path["main/makepad/".len()..],
                                    lc.0 + 1,
                                    lc.1 - 8,
                                    shader
                                );*/
                                //println!("{} {}:{}",self.full_path, lc.0, lc.1);
                            }
                            
                            //tp.jump_to_pair();
                        }
                    }
                
                    /*
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
                    */
                },
                _ => ()
            }
        }
        cx.send_signal(self.live_macros.changed, LiveMacrosView::macro_changed());
    }
}

