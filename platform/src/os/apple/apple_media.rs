
use {
    std::sync::{Arc, Mutex},    
    crate::{
        makepad_live_id::*,
        makepad_error_log::*,
        cx::Cx,
        event::Event,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::apple::audio_unit::*,
        os::apple::core_midi::*
    }
};


impl CxMediaApi for Cx {
    fn handle_midi_port_list(&mut self, event: &Event) -> Vec<MidiPortId> {
        if let Event::Signal(se) = event {
            if se.signals.contains(&live_id!(CoreMidiInputsChanged).into()) {
                let core_midi = self.os.core_midi.as_mut().unwrap();
                let mut core_midi = core_midi.lock().unwrap();
                core_midi.update_port_list();
                return core_midi.get_ports()
            }
        }
        Vec::new()
    }
    
    fn midi_port_desc(&self, port: MidiPortId) -> Option<MidiPortDesc> {
        if let Some(core_midi) = &self.os.core_midi{
            core_midi.lock().unwrap().port_desc(port)
        }
        else{
            None
        }
    }
    
    fn midi_input(&mut self)->MidiInput{
        if self.os.core_midi.is_none(){
            self.os.core_midi = Some(Arc::new(Mutex::new(CoreMidiAccess::new().unwrap())));
        }
        MidiInput(OsMidiInput(self.os.core_midi.as_ref().unwrap().clone()))
    }
    
    fn midi_output(&mut self)->MidiOutput{
        if self.os.core_midi.is_none(){
            self.os.core_midi = Some(Arc::new(Mutex::new(CoreMidiAccess::new().unwrap())));
        }
        MidiOutput(OsMidiOutput(self.os.core_midi.as_ref().unwrap().clone()))
    }
    
    fn handle_audio_device_list(&mut self, _event:&Event)->Vec<AudioDevice>{
        Vec::new()
    }
    
    fn request_audio_device_list(&mut self){
    }
    
    fn start_audio_output<F>(&mut self, _device:Option<&AudioDevice>, f: F) where F: FnMut(AudioTime, &mut AudioBuffer) + Send + 'static {
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let out = &AudioUnitFactory::query_audio_units(AudioUnitType::DefaultOutput)[0];
            let fbox = fbox.clone();
            AudioUnitFactory::new_audio_unit(out, move | result | {
                match result {
                    Ok(audio_unit) => {
                        let fbox = fbox.clone();
                        audio_unit.set_input_callback(move | time, output | {
                            let mut fbox = fbox.lock().unwrap();
                            fbox(time, output);
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
    
    fn start_audio_input<F>(&mut self, _device:Option<&AudioDevice>, f: F) where F: FnMut(AudioTime, AudioBuffer)->AudioBuffer + Send + 'static {
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let out = &AudioUnitFactory::query_audio_units(AudioUnitType::DefaultInput)[0];
            let fbox = fbox.clone();
            AudioUnitFactory::new_audio_unit(out, move | result | {
                match result {
                    Ok(audio_unit) => { 
                        let fbox = fbox.clone();
                        audio_unit.set_output_callback(move | time, buffer | {
                            let mut fbox = fbox.lock().unwrap();
                            fbox(time, buffer)
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

