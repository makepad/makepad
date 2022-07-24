use {
    crate::{
        audio::*,
        makepad_platform::platform::apple::audio_unit::*,
        makepad_platform::*
    },
};

live_register!{
    AudioUnitEffect: {{AudioUnitEffect}} {
        plugin: "FM8"
    }
}

enum ToUI {
    NewAudioUnit(AudioUnit)
}

enum FromUI {
    NewAudioUnit(AudioUnitClone)
}

#[derive(Live)]
#[live_register(audio_component_factory!(AudioUnitEffect))]
struct AudioUnitEffect {
    plugin: String,
    preset_data: String,
    input: AudioComponentRef,
    #[rust] audio_unit: Option<AudioUnit>,
    #[rust] from_ui: FromUISender<FromUI>,
    #[rust] to_ui: ToUIReceiver<ToUI>,
}

struct Node {
    from_ui: FromUIReceiver<FromUI>,
    audio_unit: Option<AudioUnitClone>
}

impl AudioGraphNode for Node {
    fn handle_midi_1_data(&mut self, _data: Midi1Data) {
    }
    
    fn render_to_audio_buffer(&mut self, time: AudioTime, outputs: &mut [&mut AudioBuffer], inputs: &[&AudioBuffer]) {
        while let Ok(msg) = self.from_ui.try_recv() {
            match msg {
                FromUI::NewAudioUnit(audio_unit) => {
                    self.audio_unit = Some(audio_unit);
                }
            }
        }
        if let Some(audio_unit) = &self.audio_unit {
            audio_unit.render_to_audio_buffer(time, outputs, inputs);
        }
    }
}

impl LiveHook for AudioUnitEffect {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.load_audio_unit();
    }
}

impl AudioUnitEffect {
    fn load_audio_unit(&mut self) {
        // alright lets create an audio device
        let list = AudioUnitFactory::query_audio_units(AudioUnitType::Effect);
        let sender = self.to_ui.sender();
        if let Some(info) = list.iter().find( | item | item.name == self.plugin) {
            AudioUnitFactory::new_audio_unit(info, move | result | {
                match result {
                    Ok(audio_unit) => {
                        sender.send(ToUI::NewAudioUnit(audio_unit)).unwrap()
                    }
                    Err(err) => println!("Error {:?}", err)
                }
            })
        }
        else {
            println!("Cannot find effect {}", self.plugin);
            for item in &list {
                println!("Effects: {}", item.name);
            }
        }
    }
}

impl AudioComponent for AudioUnitEffect {
    
    fn get_graph_node(&mut self, _cx: &mut Cx) -> Box<dyn AudioGraphNode + Send> {
        self.from_ui.new_channel();
        Box::new(Node {
            from_ui: self.from_ui.receiver(),
            audio_unit: if let Some(audio_unit) = &self.audio_unit {Some(audio_unit.clone())}else {None}
        })
    }
    
    fn handle_event(&mut self, _cx: &mut Cx, event: &mut Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)) {
        while let Ok(to_ui) = self.to_ui.try_recv(event) {
            match to_ui {
                ToUI::NewAudioUnit(audio_unit) => {
                    self.from_ui.send(FromUI::NewAudioUnit(audio_unit.clone())).unwrap();
                    self.audio_unit = Some(audio_unit);
                }
            }
        }
    }
    
    fn audio_query(&mut self, query: &AudioQuery, callback: &mut Option<&mut dyn FnMut(&mut Box<dyn AudioComponent >)>) -> AudioResult {
        self.input.audio_query(query, callback)
    }
}


