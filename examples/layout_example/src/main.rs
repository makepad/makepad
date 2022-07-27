use makepad_component::*;
use makepad_platform::*;

live_register!{
    use makepad_component::frame::*;
    use FrameComponent::*;
    App: {{App}} {
        frame: {
            width: Fill
            height: Fill
            layout:{align: {x: 0.0, y: 0.5}, padding: 30,spacing: 30.}
            Solid {bg:{color: #0f0}, width: Fill, height: 40}
            Solid {
                bg:{color: #0ff},
                layout:{padding: 10, flow: Down, spacing: 10},
                width: Fit,
                height: 300
                Solid {bg:{color: #00f}, width: 40, height: Fill}
                Solid {bg:{color: #f00}, width: 40, height: 40}
                Solid {bg:{color: #00f}, width: 40, height: 40}
            }
            Solid {bg:{color: #f00}, width: 40, height: 40}
            Solid {bg:{color: #f0f}, width: Fill, height: 60}
            Solid {bg:{color: #f00}, width: 40, height: 40}
        }
    }
}
main_app!(App);

#[derive(Clone, Debug)]
pub enum ToUI {
    TestMessage(Vec<u32>),
}

#[derive(Clone, Debug)]
pub enum FromUI {
    TestMessage(Vec<u32>),
}


#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    window: DesktopWindow,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        self.window.handle_event(cx, event);
        
        match event {
            Event::Construct => {
                // lets draw the animation curve we use everywhere
                
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx, None).not_redrawing() {
            return;
        }
        while self.frame.draw(cx).not_done(){
            
        };
        self.window.end(cx);
    }
}