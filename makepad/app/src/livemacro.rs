use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::appstorage::*;

#[derive(Clone)]
pub struct LiveMacroView {
    pub scroll_view: ScrollView,
}

pub enum LiveMacroData {
    Color {hsva: Vec4, in_shader: bool},
    Shader
}

pub struct LiveMacros {
    _changed: Signal,
    macros: Vec<LiveMacro>
}

impl LiveMacros {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            _changed: cx.new_signal(),
            macros: Vec::new()
        }
    }
}

pub enum LiveMacroViewEvent {
    None
}

impl LiveMacroView {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            scroll_view: ScrollView::new(cx),
        }
    }
    
    pub fn handle_live_macros(&mut self, _cx: &mut Cx, _event: &mut Event, _atb: &mut AppTextBuffer) -> LiveMacroViewEvent {
        LiveMacroViewEvent::None
    }
    
    pub fn draw_live_macros(&mut self, _cx: &mut Cx, _atb: &mut AppTextBuffer) {
        //
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
                    if tp.eat("color") && tp.eat("!") && tp.eat("(") { 
                        let _in_shader = tp.cur_offset() < shader_end;
                        // ok so now we need to parse the color, and turn it to HSV
                        let _color = if tp.cur_type() == TokenType::Hash { // its a #color
                            tp.advance();
                            let color = Color::parse_hex(&tp.cur_as_string());
                            if let Ok(color) = color {color}else {Color::white()}
                        }
                        else { // its a named color
                            let color = Color::parse_name(&tp.cur_as_string());
                            if let Ok(color) = color {color}else {Color::white()}
                        };
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
    }
}

