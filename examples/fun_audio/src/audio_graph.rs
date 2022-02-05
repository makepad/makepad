#![allow(unused_variables)]
use {
    crate::{
        audio_registry::*,
        makepad_platform::*,
        makepad_platform::platform::apple::{
            audio_unit::*,
        },
    },
    std::sync::{Arc, Mutex}
};

pub use crate::makepad_platform::platform::apple::core_midi::*;

// lets give this a stable pointer for the UI
live_register!{
    //use crate::plugin_music_device::PluginMusicDevice;
    
    AudioGraph: {{AudioGraph}} {
        root: BasicSynth {
            prop: 3.0
        }
        /*
        root: PluginMusicDevice {
            plugin: "AUMIDISynth"
            preset_data: "21adslkfjalkwqwe"
        }
        root: Mixer {
            Instrument {
                key_range: {start: 34, end: 47 shift: 30}
                PluginEffect {
                    plugin: "AUReverb"
                }
                PluginMusicDevice {
                    plugin: "FM8"
                    preset_data: "21adslkfjalkwqwe"
                }
            }
        }*/
    }
}

pub enum AudioComponentAction {}

pub trait AudioComponent: LiveApply {
    fn handle_event_with_fn(&mut self, _cx: &mut Cx, event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction));
    fn type_id(&self) -> LiveType;
    fn get_graph_node(&mut self) -> Box<dyn AudioGraphNode + Send>;
}

pub trait AudioGraphNode {
    fn handle_midi_1_data(&mut self, data: Midi1Data);
    fn render_to_audio_buffer(&mut self, buffer: &mut AudioBuffer);
}

pub enum FromUI {
    Midi1Data(Midi1Data),
    NewRoot(Box<dyn AudioGraphNode + Send>)
}

#[derive(Clone)]
pub enum ToUI {
    Midi1Data(Midi1Data),
}

pub enum AudioEngineAction {}

#[derive(Live)]
pub struct AudioGraph {
    registry: AudioComponentRegistry,
    root: AudioComponentOption,
    
    #[rust(FromUISender::new())] from_ui: FromUISender<FromUI>,
    #[rust(ToUIReceiver::new(cx))] to_ui: ToUIReceiver<ToUI>,
}

impl LiveHook for AudioGraph {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        // we should have a component
        if let Some(root) = self.root.component() {
            let graph_node = root.get_graph_node();
            self.from_ui.send(FromUI::NewRoot(graph_node)).unwrap();
        }
        //println!("{}", nodes.to_string(index,100))
    }
    
    fn after_new(&mut self, _cx: &mut Cx) {
        Self::start_midi_input(self.from_ui.sender(), self.to_ui.sender());
        Self::start_audio_output(self.from_ui.receiver(), self.to_ui.sender());
    }
}

struct AudioGraphState {
    from_ui: FromUIReceiver<FromUI>,
    root: Option<Box<dyn AudioGraphNode + Send >>
}
// ok so. how do we deal with this
impl AudioGraph {
    fn start_midi_input(from_ui: FromUISender<FromUI>, to_ui: ToUISender<ToUI>) {
        Midi::new_midi_1_input(move | data | {
            let _ = from_ui.send(FromUI::Midi1Data(data));
            let _ = to_ui.send(ToUI::Midi1Data(data));
            
        }).unwrap();
    }
    
    pub fn send_midi_1_data(&self, data: Midi1Data) {
        self.from_ui.send(FromUI::Midi1Data(data)).unwrap();
    }
    
    fn render_to_audio_buffer(state: &mut AudioGraphState, buffer: &mut AudioBuffer) {
        while let Ok(msg) = state.from_ui.try_recv() {
            match msg {
                FromUI::NewRoot(new_root) => {
                    state.root = Some(new_root);
                }
                FromUI::Midi1Data(data) => {
                    if let Some(root) = state.root.as_mut() {
                        root.handle_midi_1_data(data);
                    }
                }
            }
        }
        if let Some(root) = state.root.as_mut() {
            root.render_to_audio_buffer(buffer);
        }
    }
    
    fn start_audio_output(from_ui: FromUIReceiver<FromUI>, to_ui: ToUISender<ToUI>) {
        let state = Arc::new(Mutex::new(AudioGraphState {from_ui, root: None}));
        std::thread::spawn(move || {
            let out = &Audio::query_devices(AudioDeviceType::DefaultOutput)[0];
            Audio::new_device(out, move | result | {
                match result {
                    Ok(device) => {
                        let state = state.clone();
                        device.set_input_callback(move | buffer | {
                            // the core of the audio flow..
                            let mut state = state.lock().unwrap();
                            Self::render_to_audio_buffer(&mut state, buffer);
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
    
    pub fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioEngineAction)) {
        if let Some(root) = self.root.component() {
            root.handle_event_with_fn(cx, event, &mut | _cx, _action | {
            });
        }
        match event {
            Event::KeyDown(ke) => {
                if let KeyCode::F1 = ke.key_code {
                }
                if let KeyCode::Escape = ke.key_code {
                }
            }
            Event::Signal(se) => while let Ok(to_ui) = self.to_ui.try_recv(se) {
                match to_ui {
                    ToUI::Midi1Data(data) => {
                    },
                }
                // ok something sent us a signal.
            }
            _ => ()
        }
    }
}

