
use{
    crate::{
        makepad_live_id::*,
        makepad_error_log::*,
        cx::Cx,
        cx_api::CxOsApi,
        event::Event,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::apple::audio_unit::*,
        os::apple::core_midi::*   
    }
};

impl CxMediaApi for Cx{
    
    fn send_midi_data(&mut self, data:MidiData){
         self.os.midi_access.as_ref().unwrap().send_midi_1_data(data);
    }
    
    fn handle_midi_received(&mut self, event:&Event)->Vec<MidiInputData>{
        if let Event::Signal(se) = event{
            if se.signals.contains(&live_id!(CoreMidiInputData).into()) {
               let out_data = if let Ok(data) = self.os.midi_input_data.lock() {
                    let mut data = data.borrow_mut();
                    let out_data = data.clone();
                    data.clear();
                    out_data
                }
                else {
                    panic!();
                };
                return out_data;
            }
        }
        Vec::new()
    }
    
    fn handle_midi_inputs(&mut self, event:&Event)->Vec<MidiInputInfo>{
        if let Event::Signal(se) = event{
            if se.signals.contains(&live_id!(CoreMidiInputsChanged).into()) {
                let inputs = self.os.midi_access.as_ref().unwrap().connect_all_inputs();
                self.os.midi_access.as_mut().unwrap().update_destinations();
                return inputs
            }
        }
        Vec::new()
    }
    
    fn start_midi_input(&mut self) {
        let midi_input_data = self.os.midi_input_data.clone();
        if let Ok(ma) = CoreMidiAccess::new_midi_input(
            move | datas | {
                if let Ok(midi_input_data) = midi_input_data.lock() {
                    let mut midi_input_data = midi_input_data.borrow_mut();
                    midi_input_data.extend_from_slice(&datas);
                    Cx::post_signal(live_id!(CoreMidiInputData).into());
                }
            },
            move || {
                Cx::post_signal(live_id!(CoreMidiInputsChanged).into());
            }
        ) {
            self.os.midi_access = Some(ma);
        }
        Cx::post_signal(live_id!(CoreMidiInputsChanged).into());
    }
    
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static {
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let out = &AudioUnitFactory::query_audio_units(AudioUnitType::DefaultOutput)[0];
            let fbox = fbox.clone();
            AudioUnitFactory::new_audio_unit(out, move | result | {
                match result {
                    Ok(audio_unit) => {
                        let fbox = fbox.clone();
                        audio_unit.set_input_callback(move | time, output | {
                            if let Ok(mut fbox) = fbox.lock() {
                                fbox(time, output);
                            }
                        });
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                    Err(err) => error!("spawn_audio_output Error {:?}", err)
                }
            });
        });
    }
}

