use makepad_component::*;
use makepad_platform::*;

live_register!{
    use FrameComponent::*;
    App: {{App}} {
        frame: {
            color: #3
            padding: 0
            width: 500
            height: 500
            align:{fx:0.5,fy:0.5}
            Frame {color: #0f0, width: 40, height: 40}
            Frame {color: #0ff, width: 40, height: 80}
            Frame {color: #f0f, width: 40, height: 60}
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    window: BareWindow,
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        println!("{}", std::mem::size_of::<Frame>());
        makepad_component::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        
        match event {
            Event::Construct => {
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2da::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2da) {
        if self.window.begin(cx).is_err() {
            return;
        }
        self.frame.draw(cx);
        self.window.end(cx); 
    }
}