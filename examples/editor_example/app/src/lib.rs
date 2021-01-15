use makepad_render::*;
use makepad_widget::*;
use makepad_code_editor::*;

pub struct EditorExampleApp {
    desktop_window: DesktopWindow, 
    menu: Menu,
    code_editor:CodeEditor,
}

impl EditorExampleApp {
    pub fn new(cx: &mut Cx) -> Self {
        
        Self { 
            desktop_window: DesktopWindow::new(cx)
            .with_inner_layout(Layout{
                line_wrap: LineWrap::NewLine,
                ..Layout::default()
            }),
            code_editor: CodeEditor::new(cx),
            menu:Menu::main(vec![
                Menu::sub("Example", vec![
                    Menu::line(),
                    Menu::item("Quit Example",  Cx::command_quit()),
                ]),
            ])
        }
    }
    
    pub fn style(cx: &mut Cx){
        set_widget_style(cx);
        set_code_editor_style(cx);
    }
       
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);
        self.code_editor.handle_code_editor(cx,event);
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, Some(&self.menu)).is_err() {
            return
        };
        
        self.code_editor.draw_code_editor(cx);
        self.desktop_window.end_desktop_window(cx);
    }
}
