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
    #[rust(ToUIReceiver::new(cx))] to_ui: ToUIReceiver<ToUI>,
    #[rust] from_ui: FromUISender<FromUI>,
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
            Event::Signal(se)=>{
                if let Ok(data) = self.to_ui.try_recv(se){
                    console_log!("GOT DATA {:?}", data);
                    self.from_ui.send(FromUI::TestMessage(vec![4,5,6])).unwrap();
                }
            }
            Event::Construct => {
                // lets spawn up a thread
                let to_ui = self.to_ui.sender();
                let from_ui = self.from_ui.receiver();
                cx.spawn_thread(move ||{
                    to_ui.send(ToUI::TestMessage(vec![1,2,3])).unwrap();
                    loop{
                        if let Ok(data) = from_ui.try_recv(){
                            console_log!("GOT FROM UI {:?}", data);
                        }
                    }
                        //console_log!("Hi from wasm worker");
                    // lets post to our main thread
                    //Cx::post_signal(Signal{signal_id:1}, 0);
                });
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.window.begin(cx, None).is_err() {
            return;
        }
        while let Err(_child) = self.frame.draw(cx){
            
        };
        self.window.end(cx);
    }
}