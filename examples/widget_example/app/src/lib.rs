use makepad_render::*;
use makepad_widget::*;

pub struct WidgetExampleApp {
    desktop_window: DesktopWindow, 
    menu: Menu,
    button:NormalButton,
    buttons:ElementsCounted<NormalButton>
}

impl WidgetExampleApp {
    pub fn new(cx: &mut Cx) -> Self {
        
        Self { 
            desktop_window: DesktopWindow::new(cx)
            .with_inner_layout(Layout{
                line_wrap: LineWrap::NewLine,
                ..Layout::default()
            }),
            button: NormalButton::new(cx),
            buttons:ElementsCounted::new(NormalButton::new(cx)),
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
    }
       
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);

        if let ButtonEvent::Clicked = self.button.handle_normal_button(cx,event){
            log!("CLICKED Hello");
        }
        for (index,button) in self.buttons.iter().enumerate(){
            if let ButtonEvent::Clicked = button.handle_normal_button(cx, event){
                log!("CLICKED {}", index);
            }
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, Some(&self.menu)).is_err() {
            return
        };
        self.button.draw_normal_button(cx, "Hello");
        for i in 0..1000{  
            self.buttons.get_draw(cx).draw_normal_button(cx, &format!("{}",i));
        }
        self.desktop_window.end_desktop_window(cx);
    }
}
