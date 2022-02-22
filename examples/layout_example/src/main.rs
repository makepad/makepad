use makepad_component::*;
use makepad_platform::*;

live_register!{
    use FrameComponent::*;
    App: {{App}} {
        frame: {
            color: #3
            padding: 30
            width: Size::Fill
            height: Size::Fill
            align: {fx: 0.0, fy: 0.0}
            spacing: 30.,
            Frame {color: #0f0, width: Size::Fill, height: 40}
            Frame {
                color: #0ff,
                padding: 10,
                flow: Flow::Down,
                width: Size2::Fit,
                height: 300
                spacing: 10
                Frame {color: #00f, width: 40, height: Size::Fill}
                Frame {color: #f00, width: 40, height: 40}
                Frame {color: #00f, width: 40, height: 40}
            }
            Frame {color: #f00, width: 40, height: 40}
            Frame {color: #f0f, width: Size::Fill, height: 60}
            Frame {color: #f00, width: 40, height: 40}
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
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx).is_err() {
            return;
        }
        self.frame.draw(cx, self.frame.walk);
        self.window.end(cx);
    }
}