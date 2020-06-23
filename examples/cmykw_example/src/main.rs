use makepad_render::*;
use makepad_widget::*;
//use tiff::decoder::{Decoder, DecodingResult};
//use tiff::ColorType;

use std::fs::File;

struct App {
    desktop_window: DesktopWindow, 
    menu: Menu,
    button:NormalButton,
}

fn main(){
    main_app!(App);
}

impl App {
    pub fn new(cx: &mut Cx) -> Self {
        set_widget_style(cx, &StyleOptions{..StyleOptions::default()});
        Self {
            desktop_window: DesktopWindow::new(cx),
            button: NormalButton::new(cx),
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
            // lets convert a file
            
        }
    }
    
    fn draw_app(&mut self, cx: &mut Cx) {
        if self.desktop_window.begin_desktop_window(cx, Some(&self.menu)).is_err() {
            return
        };
        self.button.draw_normal_button(cx, "Convert");
        self.desktop_window.end_desktop_window(cx);
    }
}
