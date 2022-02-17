use makepad_component::*;
use makepad_platform::*;

live_register!{
    use FrameComponent::*;
    App: {{App}} {
        scroll_view: {}
        frame: {
            Quad{color:#0f0, width:80}
            Quad{color:#0ff}
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    desktop_window: DesktopWindow,
    scroll_view: ScrollView,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        
        self.desktop_window.handle_event(cx, event);
        self.scroll_view.handle_event(cx, event);
        
        match event {
            Event::Construct => {  
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.desktop_window.begin(cx, None).is_err() {
            return;
        }
        if self.scroll_view.begin(cx).is_ok() {
            self.frame.draw(cx);
            self.scroll_view.end(cx);
        }
        
        self.desktop_window.end(cx);
    }
}