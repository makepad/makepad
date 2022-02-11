#![allow(unused_variables)]
use {
    crate::{
        audio::*,
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
    use AudioComponent::*;
    AudioGraph: {{AudioGraph}} {
        root: BasicSynth {
        }
    }
}

pub enum FromUI {
    Midi1Data(Midi1Data),
    NewRoot(Box<dyn AudioGraphNode + Send>)
}

#[derive(Clone)]
pub enum ToUI {
    Midi1Data(Midi1Data),
}

pub enum AudioGraphAction {
    Midi1Data(Midi1Data),
}

#[derive(Live)]
pub struct AudioGraph {
    root: AudioComponentRef,
    #[rust] from_ui: FromUISender<FromUI>,
    #[rust(ToUIReceiver::new(cx))] to_ui: ToUIReceiver<ToUI>,
}

impl LiveHook for AudioGraph {
    fn after_apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) {
        // we should have a component
        if let Some(root) = self.root.as_mut() {
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

struct Node {
    from_ui: FromUIReceiver<FromUI>,
    buffer: AudioBuffer,
    root: Option<Box<dyn AudioGraphNode + Send >>
}

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
    
    fn render_to_output_buffer(node: &mut Node, time:AudioTime, output:&mut AudioOutputBuffer) {
        while let Ok(msg) = node.from_ui.try_recv() {
            match msg {
                FromUI::NewRoot(new_root) => {
                    node.root = Some(new_root);
                }
                FromUI::Midi1Data(data) => {
                    if let Some(root) = node.root.as_mut() {
                        root.handle_midi_1_data(data);
                    }
                }
            }
        }
        if let Some(root) = node.root.as_mut() {
            // we should create a real output buffer
            node.buffer.resize_like_output(output);
            root.render_to_audio_buffer(time, &mut [&mut node.buffer], &[]);
            output.copy_from_buffer(&node.buffer);
        }
    }
    
    fn start_audio_output(from_ui: FromUIReceiver<FromUI>, to_ui: ToUISender<ToUI>) {
        let state = Arc::new(Mutex::new(Node {from_ui, buffer:AudioBuffer::default(), root: None}));
        std::thread::spawn(move || {
            let out = &AudioFactory::query_devices(AudioDeviceType::DefaultOutput)[0];
            AudioFactory::new_device(out, move | result | {
                match result {
                    Ok(device) => {
                        let state = state.clone();
                        device.set_input_callback(move | time, output_buffer | {
                            // the core of the audio flow..
                            let mut state = state.lock().unwrap();
                            Self::render_to_output_buffer(&mut state, time, output_buffer);
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
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event)->Vec<AudioGraphAction> {
        let mut a = Vec::new(); self.handle_event_with_fn(cx, event, &mut |_,ac| a.push(ac)); a
    }
    
    pub fn handle_event_with_fn(&mut self, cx: &mut Cx, event: &mut Event, dispatch_action: &mut dyn FnMut(&mut Cx, AudioGraphAction)) {
        if let Some(root) = self.root.as_mut() {
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
                        dispatch_action(cx, AudioGraphAction::Midi1Data(data))
                    },
                }
                // ok something sent us a signal.
            }
            _ => ()
        }
    }
}

