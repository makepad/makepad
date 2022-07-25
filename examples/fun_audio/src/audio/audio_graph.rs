use {
    crate::{
        audio::*,
        makepad_platform::thread::*,
        makepad_platform::audio::*,
        makepad_platform::midi::*,
        makepad_platform::*,
    },
    std::any::TypeId,
    std::sync::{Arc, Mutex},
};

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
}

pub enum AudioGraphAction {
}

#[derive(Live)]
pub struct AudioGraph {
    root: AudioComponentRef,
    #[rust] from_ui: FromUISender<FromUI>,
    #[rust] to_ui: ToUIReceiver<ToUI>,
}

impl LiveHook for AudioGraph {
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        cx.start_midi_input();
        Self::start_audio_output(cx, self.from_ui.receiver(), self.to_ui.sender());
        // we should have a component
        if let Some(root) = self.root.as_mut() {
            let graph_node = root.get_graph_node(cx);
            self.from_ui.send(FromUI::NewRoot(graph_node)).unwrap();
        }
    }
}

struct Node {
    from_ui: FromUIReceiver<FromUI>,
    buffer: AudioBuffer,
    root: Option<Box<dyn AudioGraphNode + Send >>
}

impl AudioGraph {
    
     pub fn by_type<T: 'static + AudioComponent>(&mut self) -> Option<&mut T> {
        if let AudioResult::Found(child) = self.root.audio_query(&AudioQuery::TypeId(TypeId::of::<T>()), &mut None) {
            return child.cast_mut::<T>()
        }
        None
    }
    
    pub fn send_midi_1_data(&self, data: Midi1Data) {
        self.from_ui.send(FromUI::Midi1Data(data)).unwrap();
    }
    
    fn render_to_output_buffer(node: &mut Node, time:AudioTime, output:&mut dyn AudioOutputBuffer) {

        while let Ok(msg) = node.from_ui.try_recv() {
            match msg {
                FromUI::NewRoot(new_root) => {
                    node.root = Some(new_root);
                }
                FromUI::Midi1Data(data) => {
                    //if data.channel() == 0{
                    if let Some(root) = node.root.as_mut() {
                        root.handle_midi_1_data(data);
                    }
                   // }
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
    
    fn start_audio_output(cx: &mut Cx, from_ui: FromUIReceiver<FromUI>, to_ui: ToUISender<ToUI>) {
        let state = Arc::new(Mutex::new(Node {from_ui, buffer:AudioBuffer::default(), root: None}));
        let _to_ui = Arc::new(Mutex::new(to_ui));
        cx.spawn_audio_output(move |time, output_buffer|{
            let mut state = state.lock().unwrap();
            Self::render_to_output_buffer(&mut state, time, output_buffer);
        });
    }
    
    pub fn handle_event_iter(&mut self, cx: &mut Cx, event: &mut Event)->Vec<AudioGraphAction> {
        let mut a = Vec::new(); self.handle_event(cx, event, &mut |_,ac| a.push(ac)); a
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioGraphAction)) {
        if let Some(root) = self.root.as_mut() {
            root.handle_event(cx, event, &mut | _cx, _action | {
            });
        }
        
        while let Ok(to_ui) = self.to_ui.try_recv(event) {
            match to_ui {
            }
        }

        match event {
            Event::Midi1InputData(inputs)=>{
                for input in inputs{
                    self.from_ui.send(FromUI::Midi1Data(input.data)).unwrap();
                }
            }
            Event::KeyDown(ke) => {
                if let KeyCode::F1 = ke.key_code {
                }
                if let KeyCode::Escape = ke.key_code {
                }
            }
            _ => ()
        }
    }
}

