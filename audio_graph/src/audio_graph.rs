use {
    crate::{
        makepad_platform::thread::*,
        makepad_platform::*,
        makepad_platform::audio::*,
        makepad_platform::midi::*,
        audio_traits::*,
    },
    std::any::TypeId,
    std::sync::{Arc, Mutex},
};

live_design!{
    AudioGraph= {{AudioGraph}} {
    }
}

pub enum FromUI {
    AllNotesOff,
    MidiData(MidiData),
    NewRoot(Box<dyn AudioGraphNode + Send>),
    DisplayAudio(AudioBuffer),
}

pub enum AudioGraphAction<'a> {
    DisplayAudio {
        active: bool,
        voice: usize,
        buffer: &'a AudioBuffer
    },
    VoiceOff {voice: usize}
}

#[derive(Live)]
pub struct AudioGraph {
    root: AudioComponentRef,
    #[rust] from_ui: FromUISender<FromUI>,
    #[rust] to_ui: ToUIReceiver<ToUIDisplayMsg>,
}

impl LiveHook for AudioGraph {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        Self::start_audio_output(cx, self.from_ui.receiver(), self.to_ui.sender());
        // we should have a component
        
        if let Some(root) = self.root.as_mut() {
            let graph_node = root.get_graph_node(cx);
            let _ = self.from_ui.send(FromUI::NewRoot(graph_node));
        }
        
    }
}

struct Node {
    from_ui: FromUIReceiver<FromUI>,
    display_buffers: Vec<AudioBuffer>,
    root: Option<Box<dyn AudioGraphNode + Send >>
}

impl AudioGraph {
    
    pub fn by_type<T: 'static + AudioComponent>(&mut self) -> Option<&mut T> {
        if let Some(child) = self.root.audio_query(&AudioQuery::TypeId(TypeId::of::<T>()), &mut None).into_found() {
            return child.cast_mut::<T>()
        }
        None
    }
    
    pub fn send_midi_data(&self, data: MidiData) {
        let _ = self.from_ui.send(FromUI::MidiData(data));
    }
    
    
    pub fn all_notes_off(&self) {
        let _ =  self.from_ui.send(FromUI::AllNotesOff);
    }
     
    fn render_to_output_buffer(node: &mut Node, to_ui: &ToUISender<ToUIDisplayMsg>, info: AudioInfo, output: &mut AudioBuffer) {
        
        while let Ok(msg) = node.from_ui.try_recv() {
            match msg {
                FromUI::DisplayAudio(buf) => {
                    node.display_buffers.push(buf);
                    //log!("{}", node.display_buffers.len())
                }
                FromUI::NewRoot(new_root) => {
                    node.root = Some(new_root);
                }
                FromUI::MidiData(data) => {
                    //if data.channel() == 0{
                    if let Some(root) = node.root.as_mut() {
                        root.handle_midi_data(data);
                    }
                    // }
                }
                FromUI::AllNotesOff=>{
                    if let Some(root) = node.root.as_mut() {
                        root.all_notes_off();
                    }
                }
            }
        }
        if let Some(root) = node.root.as_mut() {
            // we should create a real output buffer
            //node.buffer.resize_like_output(output);
            let mut dg = DisplayAudioGraph {
                to_ui,
                buffers: &mut node.display_buffers
            };
            root.render_to_audio_buffer(info, &mut [output], &[], &mut dg);
            // lets output this buffer to the UI
            //if let Some(mut display_buffer) = dg.pop_buffer() {
            //    display_buffer.copy_from(&node.buffer);
            //   dg.send_buffer(0, display_buffer);
            //}
            //output.copy_from_buffer(&node.buffer);
        }
    }
    
    fn start_audio_output(cx: &mut Cx, from_ui: FromUIReceiver<FromUI>, to_ui: ToUISender<ToUIDisplayMsg>) {
        let mut buffers = Vec::new();
        for _ in 0..512 {
            buffers.push(AudioBuffer::new_with_size(512, 2));
        }
        
        let state = Arc::new(Mutex::new(Node {
            from_ui,
            display_buffers: buffers,
            root: None
        }));
        
        let to_ui = Arc::new(Mutex::new(to_ui));
        
        cx.audio_output(0, move | info, output_buffer | {
            let mut state = state.lock().unwrap();
            let to_ui = to_ui.lock().unwrap();
            Self::render_to_output_buffer(&mut state, &to_ui, info, output_buffer);
        });
    }
    
    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, AudioGraphAction)
    ) {
        if let Some(root) = self.root.as_mut() {
            root.handle_event_with(cx, event, &mut | _, _ | {});
        }
        
        while let Ok(to_ui) = self.to_ui.try_recv() {
            match to_ui {
                ToUIDisplayMsg::DisplayAudio {voice, buffer, active} => {
                    //log!("GOT DISPLAY AUDIO");
                    dispatch_action(cx, AudioGraphAction::DisplayAudio {buffer: &buffer, voice, active});
                    self.from_ui.send(FromUI::DisplayAudio(buffer)).unwrap();
                },
                ToUIDisplayMsg::VoiceOff {voice} => {
                    //log!("GOT DISPLAY AUDIO");
                    dispatch_action(cx, AudioGraphAction::VoiceOff {voice});
                },
                ToUIDisplayMsg::OutOfBuffers => { // inject some new buffers
                }
            }
        }
    }
}

