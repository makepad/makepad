use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::appstorage::*;

#[derive(Clone)]
pub struct LiveMacroView{
    pub scroll_view: ScrollView,
}

pub struct LiveMacroPick {
    _token: usize,
}

pub struct LiveMacroShader {
    _token: usize,
}

pub enum LiveMacro {
    Pick(LiveMacroPick),
    Shader(LiveMacroShader)
}

pub struct LiveMacros {
    changed: Signal,
    macros: Vec<LiveMacro>
}

impl LiveMacros{
    pub fn new(cx: &mut Cx)->Self{
        Self{
            changed: cx.new_signal(),
            macros: Vec::new()
        }
    }
}

pub enum LiveMacroViewEvent{
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
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    if tp.eat("color") && tp.eat("!") && tp.eat("("){
                       // lets add this thing to our macro widget list
                       // we also have to parse whats in it. AGAIN. ahwell
                       // lets parse it, 
                       // lets add the control
                    }
                    else if tp.eat("shader") && tp.eat("!") && tp.eat("{") {
                        if tp.cur_type() == TokenType::ParenOpen {
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

