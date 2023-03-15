use {
    crate::{
        makepad_platform::audio::*,
        makepad_platform::midi::*,
        audio_component,
        audio_traits::*,
        makepad_platform::os::apple::audio_unit::*,
        makepad_platform::thread::*,
        makepad_platform::*
    },
};

live_design!{
    AudioUnitInstrument= {{AudioUnitInstrument}} {
        plugin: "FM8"
    }
}

enum ToUI {
    NewAudioUnit(AudioUnit),
    UIReady
}

enum FromUI {
    NewAudioUnit(AudioUnitClone)
}

#[derive(Live)]
#[live_design_with{
    audio_component!(cx, AudioUnitInstrument)
}]
struct AudioUnitInstrument {
    plugin: String,
    preset_data: String,
    #[rust] audio_unit: Option<AudioUnit>,
    #[rust] from_ui: FromUISender<FromUI>,
    #[rust] to_ui: ToUIReceiver<ToUI>,
}

struct Node {
    from_ui: FromUIReceiver<FromUI>,
    audio_unit: Option<AudioUnitClone>
}

impl AudioGraphNode for Node {
    fn handle_midi_data(&mut self, data: MidiData) {
        if let Some(audio_unit) = &self.audio_unit {
            audio_unit.handle_midi_data(data);
        }
    }

    fn all_notes_off(&mut self) {
    }
    
    fn render_to_audio_buffer(
        &mut self,
        info: AudioInfo,
        outputs: &mut [&mut AudioBuffer],
        inputs: &[&AudioBuffer],
        display: &mut DisplayAudioGraph
    ) {
        while let Ok(msg) = self.from_ui.try_recv() {
            match msg {
                FromUI::NewAudioUnit(audio_unit) => {
                    self.audio_unit = Some(audio_unit);
                }
            }
        }
        if let Some(audio_unit) = &self.audio_unit {
            audio_unit.render_to_audio_buffer(info, outputs, inputs);
            let display_buffer = display.pop_buffer_resize(outputs[0].frame_count(), outputs[0].channel_count());
            if let Some(mut buf) = display_buffer {
                buf.copy_from(&outputs[0]);
                display.send_buffer(true, 0, buf);
            }
            
        }
    }
}

impl LiveHook for AudioUnitInstrument {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.load_audio_unit();
    }
}

impl AudioUnitInstrument {
    fn load_audio_unit(&mut self) {
        // alright lets create an audio device
        
        let list = AudioUnitAccess::query_audio_units(AudioUnitQuery::MusicDevice);
        let sender = self.to_ui.sender();
        if let Some(info) = list.iter().find( | item | item.name == self.plugin) {
            AudioUnitAccess::new_audio_plugin(info, move | result | {
                match result {
                    Ok(audio_unit) => {
                        let sender2 = sender.clone();
                        audio_unit.request_ui(move || {
                            sender2.send(ToUI::UIReady).unwrap()
                        });
                        sender.send(ToUI::NewAudioUnit(audio_unit)).unwrap();
                    }
                    Err(err) => error!("Error {:?}", err)
                }
            })
        }
        else {
            error!("Cannot find music device {}", self.plugin);
            for item in &list {
                error!("MusicDevices: {}", item.name);
            }
        }
    }
}

impl AudioComponent for AudioUnitInstrument {
    fn get_graph_node(&mut self, _cx: &mut Cx) -> Box<dyn AudioGraphNode + Send> {
        self.from_ui.new_channel();
        Box::new(Node {
            from_ui: self.from_ui.receiver(),
            audio_unit: if let Some(audio_unit) = &self.audio_unit {Some(audio_unit.clone())}else {None}
        })
    }
    
    fn handle_event_with(&mut self, _cx: &mut Cx, _event: &Event, _dispatch_action: &mut dyn FnMut(&mut Cx, AudioComponentAction)) {
        // ui EVENT
        while let Ok(to_ui) = self.to_ui.try_recv() {
            match to_ui {
                ToUI::UIReady => {
                    if let Some(audio_unit) = &self.audio_unit {
                        audio_unit.open_ui();
                    }
                }
                ToUI::NewAudioUnit(audio_unit) => {
                    self.from_ui.send(FromUI::NewAudioUnit(audio_unit.clone())).unwrap();
                    self.audio_unit = Some(audio_unit);
                }
            }
        }
    }
    // we dont have inputs
    fn audio_query(&mut self, _query: &AudioQuery, _callback: &mut Option<AudioQueryCb>) -> AudioResult {
        AudioResult::not_found()
    }
}


