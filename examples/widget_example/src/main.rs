use makepad_render::*;
use makepad_widget::*;

struct WidgetExampleApp {
    desktop_window: DesktopWindow, 
    menu: Menu,
    button:NormalButton,
    buttons:ElementsCounted<NormalButton>
}

fn main(){
    main_app!(WidgetExampleApp);
}
impl WidgetExampleApp {
    pub fn new(cx: &mut Cx) -> Self {
        set_widget_style(cx, &StyleOptions{scale:0.5,..StyleOptions::default()});
        Self {
            desktop_window: DesktopWindow::new(cx),
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
       
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        self.desktop_window.handle_desktop_window(cx, event);

        if let ButtonEvent::Clicked = self.button.handle_normal_button(cx,event){
            println!("CLICKED");  
        }
        for button in self.buttons.iter(){
            button.handle_normal_button(cx, event);
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
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
