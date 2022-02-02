pub use makepad_component::{self, *};
use makepad_platform::*;
use makepad_platform::platform::apple::audio_unit::*;
use makepad_platform::platform::apple::core_midi::*;
use std::sync::{Arc, Mutex};
mod piano;
use crate::piano::*;

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

#[derive(Clone, Copy)]
enum UISend {
    InstrumentUIReady,
    Midi(Midi1Data)
}

#[derive(Live, LiveHook)]
pub struct App {
    
    piano: Piano,
    frame: Frame,
    frame_component_registry: FrameComponentRegistry,
    desktop_window: DesktopWindow,
    scroll_view: ScrollView,
    #[rust] instrument_state: Option<AudioInstrumentState>,
    #[rust] midi: Option<Midi>,
    #[rust] instrument: Arc<Mutex<Option<AudioDevice >> >,
    
    #[rust(UIReceiver::new(cx))] ui_receiver: UIReceiver<UISend>,
    #[rust] offset: u64
}

impl App {
    pub fn live_register(cx: &mut Cx) {
        makepad_component::live_register(cx);
        crate::piano::live_register(cx);
    }
    
    pub fn new_app(cx: &mut Cx) -> Self {
        Self::new_as_main_module(cx, &module_path!(), id!(App)).unwrap()
    }
    
    pub fn run_audio_system(&mut self) {
        println!("UI THREAD ID: {:?}", std::thread::current().id());
        // listen to midi inputs
        let instrument = self.instrument.clone();
        let ui_sender = self.ui_receiver.sender();
        self.midi = Some(Midi::new_midi_1_input(move | event | {
            println!("MIDI ID: {:?}", std::thread::current().id());
            if let Some(instrument) = instrument.lock().unwrap().as_ref() {
                instrument.send_midi_1_event(event);
                //instrument.dump_full_state();
                ui_sender.send(UISend::Midi(event)).unwrap();
                // instrument.render_to_audio_buffer(buffer);
            }
            //}
            //   instrument.send_midi_1_event(event);
            //instrument.dump_full_state();
            //   ui_sender.send(UISend::Midi(event)).unwrap();
            //  }
        }).unwrap());
        
        let list = Audio::query_devices(AudioDeviceType::MusicDevice);
        //for item in &list {println!("{}", item.name)}
        
        // find an audio unit instrument and start it
        //let list = Audio::query_devices(AudioDeviceType::MusicDevice);
        //for item in &list{println!("{}", item.name)}
        if let Some(info) = list.iter().find( | item | item.name == "FM8") {
            let instrument = self.instrument.clone();
            let ui_sender = self.ui_receiver.sender();
            Audio::new_device(info, move | result | {
                //println!("NEW INSTRUMENT: {:?}", std::thread::current().id());
                match result {
                    Ok(mut new_instrument) => {
                        let ui_sender = ui_sender.clone();
                        let outer_instrument = instrument.clone();
                        
                        //new_instrument.set_input_callback(move | buffer | {
                        //    println!("WHOO")
                        //});
                        //new_instrument.dump_parameter_tree();
                        new_instrument.parameter_tree_changed(Box::new(move || {
                            println!("PARAM TREE ID: {:?}", std::thread::current().id());
                            //if let Some(instrument) = outer_instrument.lock().unwrap().as_ref() {
                            //println!("Tree chaNGE")
                            //instrument.dump_parameter_tree();
                            //instrument.dump_full_state();
                            //}
                        }));
                        
                        new_instrument.request_ui(move || { // happens on UI thread
                            println!("REQ UI ID: {:?}", std::thread::current().id());
                            ui_sender.send(UISend::InstrumentUIReady).unwrap();
                        });
                        //new_instrument.dump_full_state();
                        
                        *instrument.lock().unwrap() = Some(new_instrument);
                        /*
                        let instrument = instrument.clone();
                        std::thread::spawn(move || {
                            println!("REQ2 UI ID: {:?}", std::thread::current().id());
                            if let Some(instrument) = instrument.lock().unwrap().as_ref() {
                                instrument.request_ui(move || { // happens on UI thread
                                    println!("REQ UI ID: {:?}", std::thread::current().id());
                                    ui_sender.send(UISend::InstrumentUIReady).unwrap();
                                });
                            }
                        });*/
                        
                        
                    }
                    Err(err) => println!("Error {:?}", err)
                }
            })
        }
        
        // start the audio output thread
        let instrument = self.instrument.clone();
        std::thread::spawn(move || {
            println!("AUDIO CREATE ID: {:?}", std::thread::current().id());
            let out = &Audio::query_devices(AudioDeviceType::DefaultOutput)[0];
            Audio::new_device(out, move | result | {
                println!("AUDIO NEW DEVICE ID: {:?}", std::thread::current().id());
                match result {
                    Ok(device) => {
                        let instrument = instrument.clone();
                        let doonce = std::cell::Cell::new(false);
                        device.set_input_callback(move | buffer | {
                            
                            if !doonce.get() {
                                println!("AUDIO OUTPUT ID: {:?}", std::thread::current().id());
                                doonce.set(true);
                            }
                            
                            if let Ok(lock_instr) = instrument.try_lock() {
                                if let Some(instrument) = lock_instr.as_ref() {
                                    instrument.render_to_audio_buffer(buffer);
                                }
                            }
                            else {
                                buffer.zero();
                            }
                        });
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                    Err(err) => println!("Error {:?}", err)
                }
            });
        });
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event) {
        
        self.desktop_window.handle_event(cx, event);
        self.scroll_view.handle_event(cx, event);
        
        let instrument = self.instrument.clone();
        for action in self.piano.handle_event(cx, event) {
            match action {
                PianoAction::Note {is_on, note_number, velocity} => {
                    if let Some(instrument) = instrument.lock().unwrap().as_ref() {
                        instrument.send_midi_1_event(Midi1Note {
                            is_on,
                            note_number,
                            channel: 0,
                            velocity
                        }.into());
                    }
                }
            }
        };
        
        match event {
            Event::KeyDown(ke) => {
                if let KeyCode::F1 = ke.key_code {
                    if let Some(instrument) = self.instrument.lock().unwrap().as_ref() {
                        //instrument.ocr_ui();
                        self.instrument_state = Some(instrument.get_instrument_state());
                        //instrument.send_mouse_down();
                        //instrument.dump_parameter_tree();
                    }
                }
                if let KeyCode::Escape = ke.key_code {
                    if let Some(instrument) = self.instrument.lock().unwrap().as_ref() {
                        if let Some(state) = &self.instrument_state {
                            instrument.set_instrument_state(state);
                        }
                        //instrument.ocr_ui();
                        //let state = instrument.get_instrument_state();
                        //instrument.send_mouse_down();
                        //instrument.dump_parameter_tree();
                    }
                }
                
            }
            Event::Signal(se) => while let Ok(send) = self.ui_receiver.try_recv(se) {
                match send {
                    UISend::InstrumentUIReady => {
                        if let Some(instrument) = self.instrument.lock().unwrap().as_ref() {
                            instrument.open_ui();
                        }
                    }
                    UISend::Midi(me) => match me.decode() {
                        Midi1Event::Note(note) => {
                            self.piano.set_note(cx, note.is_on, note.note_number)
                        }
                        _ => ()
                    }
                }
            }
            Event::Construct => {
                self.run_audio_system();
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
                self.piano.set_key_focus(cx);
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
            self.piano.draw(cx);
            
            //if let Some(button) = get_component!(id!(b1), Button, self.frame) {
            //    button.label = "Btn1 label override".to_string();
            // }
            //cx.profile_start(1);
            //self.frame.draw(cx);
            //cx.profile_end(1);
            //cx.set_turtle_bounds(Vec2{x:10000.0,y:10000.0});
            self.scroll_view.end(cx);
        }
        
        self.desktop_window.end(cx);
        //cx.debug_draw_tree(false);
    }
}