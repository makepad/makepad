use makepad_render::*;
use makepad_widget::*;

#[derive(Clone)]
pub struct CodeEditor {
    pub view: ScrollView,
    pub bg: DrawColor,
    pub text: DrawText,
}

pub enum CodeEditorEvent{
    None
}

impl CodeEditor {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: ScrollView::new_standard_hv(cx),
            bg: DrawColor::new(cx, default_shader!()),
            text: DrawText::new(cx, default_shader!()),
        }
    }
    pub fn style(cx: &mut Cx) {

        live_body!(cx, r#"
            self::layout_bg: Layout {}
            
            self::text_style_label: TextStyle {
                ..makepad_widget::widgetstyle::text_style_fixed
            }
        "#);
    }
    
    pub fn handle_code_editor(&mut self, cx: &mut Cx, event: &mut Event) -> CodeEditorEvent {
        self.view.handle_scroll_view(cx, event);
        CodeEditorEvent::None
    }
    
    pub fn begin_code_editor(&mut self, cx: &mut Cx) -> Result<(), ()> {
        self.view.begin_view(cx, live_layout!(cx, self::layout_bg)) ?;        
        self.bg.draw_quad_abs(cx, cx.get_turtle_rect());
        Ok(())
    }
    
    pub fn end_code_editor(&mut self, cx: &mut Cx){
        self.view.end_view(cx);
    }
    
    pub fn draw_code_editor(&mut self, cx: &mut Cx) {
        
        if !self.begin_code_editor(cx).is_ok(){
            return
        }
        
        self.text.text_style = live_text_style!(cx, self::text_style_label);
        self.text.color = Vec4::color("#7");
        for _j in 0..100{
            for _i in 0..100{
                self.text.draw_text_walk(cx, "Hello World! ");
            }
            cx.turtle_new_line(); 
        }

        self.end_code_editor(cx);
    }
}

pub fn set_code_editor_style(cx: &mut Cx) {
    CodeEditor::style(cx)
}

