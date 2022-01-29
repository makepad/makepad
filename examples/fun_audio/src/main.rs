use makepad_component::*;
use makepad_platform::*;
use makepad_platform::platform::apple::core_audio::{Audio, AudioDevice, AudioDeviceType, Midi};
use std::sync::{Arc, Mutex};

live_register!{
    use makepad_component::frame::Frame;
    use makepad_component::button::Button;
    App: {{App}} {
        scroll_view: {
            h_show: true,
            v_show: true,
            view: {
                layout: {
                    line_wrap: LineWrap::NewLine
                }
            }
        }
        frame: {
        }
    }
}
main_app!(App);

#[derive(Live, LiveHook)]
pub struct App {
    frame: Frame,
    frame_component_registry: FrameComponentRegistry,
    desktop_window: DesktopWindow,
    scroll_view: ScrollView,
    
    #[rust] midi: Option<Midi>,
    #[rust] instrument: Arc<Mutex<Option<AudioDevice>>>,
    
    #[rust(cx.new_signal())] signal: Signal,
    #[rust] offset: u64
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
            Event::Signal(se) => {
                if let Some(data) = se.signals.get(&self.signal) {
                    self.instrument.lock().unwrap().as_ref().unwrap().open_ui();
                }
            }
            Event::Construct => {
                // ok lets list midi inputs
                self.midi = Some(Midi::new_midi_1_input(Box::new(move | _message | {
                    //println!("MIDI MESSAGE!");
                })).unwrap());
                
                //for source in &self.midi.as_ref().unwrap().sources{
                //println!("{} {}", source.name, source.manufacturer);
                //}
                
                //let signal = self.signal;
                // let block_ptr = Arc::new(Cell::new(None));
                let list = Audio::query_devices(AudioDeviceType::Music);
                //for item in &list {
                //println!("{}", item.name);
                //}
                if let Some(info) = list.iter().find( | item | item.name == "FM8") {
                    //let block_ptr = block_ptr.clone();≈
                    let instrument = self.instrument.clone();
                    let signal = self.signal;
                    Audio::new_device(info, Box::new(move | result | {
                        match result {
                            Ok(device) => {
                                device.request_ui(Box::new(move ||{
                                    Cx::post_signal(signal, 0);
                                }));
                                *instrument.lock().unwrap() = Some(device);
                            }
                            Err(err) => println!("Error {:?}", err)
                        }
                    }))
                }
                
                let instrument = self.instrument.clone();
                std::thread::spawn(move || {
                    let out = &Audio::query_devices(AudioDeviceType::DefaultOutput)[0];
                    Audio::new_device(out, Box::new(move | result | {
                        match result {
                            Ok(device) => {
                                let instrument = instrument.clone();
                                device.start_audio_output_with_fn(Box::new(move | buffer | {
                                    // now access here to the 'write buffer'
                                    if let Some(instrument) = instrument.lock().unwrap().as_ref(){
                                        instrument.render_to_audio_buffer(buffer);
                                    }
                                }));
                                
                                loop {
                                    std::thread::sleep(std::time::Duration::from_millis(100));
                                }
                            }
                            Err(err) => println!("Error {:?}", err)
                        }
                    }));
                });
                
                // spawn 1000 buttons into the live structure
                let mut out = Vec::new();
                out.open();
                for i in 0..1 {
                    out.push_live(live_object!{
                        [id_num!(btn, i)]: Button {
                            label: (format!("This is makepad metal UI{}", i + self.offset))
                        }
                    });
                }
                out.close();
                self.frame.apply_clear(cx, &out);
                
                //cx.new_next_frame();
                cx.redraw_all();
            }
            Event::Draw(draw_event) => {
                self.draw(&mut Cx2d::new(cx, draw_event));
            }
            _ => ()
        }
        
        for item in self.frame.handle_event(cx, event) {
            if let ButtonAction::IsPressed = item.action.cast() {
                println!("Clicked on button {}", item.id);
            }
        }
    }
    
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if self.desktop_window.begin(cx, None).is_err() {
            return;
        }
        if self.scroll_view.begin(cx).is_ok() {
            //if let Some(button) = get_component!(id!(b1), Button, self.frame) {
            //    button.label = "Btn1 label override".to_string();
            // }
            //cx.profile_start(1);
            self.frame.draw(cx);
            //cx.profile_end(1);
            //cx.set_turtle_bounds(Vec2{x:10000.0,y:10000.0});
            self.scroll_view.end(cx);
        }
        
        self.desktop_window.end(cx);
        //cx.debug_draw_tree(false);
    }
}