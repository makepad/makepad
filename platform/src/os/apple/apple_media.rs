
use {
    crate::{
        cx::Cx,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::apple::core_midi::*
    }
};


impl CxMediaApi for Cx {
    /*fn handle_midi_port_list(&mut self, event: &Event) -> Vec<MidiPortId> {
        if let Event::Signal(se) = event {
            if se.signals.contains(&live_id!(CoreMidiInputsChanged).into()) {
                let core_midi = self.os.core_midi.as_mut().unwrap();
                let mut core_midi = core_midi.lock().unwrap();
                core_midi.update_port_list();
                return core_midi.get_ports()
            }
        }
        Vec::new()
    }*/
    
    fn midi_input(&mut self) -> MidiInput {
        self.os.core_midi().lock().unwrap().create_midi_input()
    }
    
    fn midi_output(&mut self) -> MidiOutput {
        MidiOutput(Some(OsMidiOutput(self.os.core_midi())))
    }
    
    fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        let core_midi = self.os.core_midi();
        core_midi.clone().lock().unwrap().use_midi_inputs(ports);
    }
    
    fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        let core_midi = self.os.core_midi();
        core_midi.clone().lock().unwrap().use_midi_outputs(ports);
    }
    
    fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let audio_unit = self.os.audio_unit();
        audio_unit.clone().lock().unwrap().use_audio_inputs(devices);
    }
    
    fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let audio_unit = self.os.audio_unit();
        audio_unit.clone().lock().unwrap().use_audio_outputs(devices);
    }
    
    fn audio_output<F>(&mut self, f: F) where F: FnMut(usize, AudioDeviceId, AudioTime, &mut AudioBuffer) + Send + 'static {
        let audio_unit = self.os.audio_unit();
        *audio_unit.lock().unwrap().audio_output_cb.lock().unwrap() = Some(Box::new(f));
        /*
        //std::thread::spawn(move || {
        let out = &AudioUnitFactory::query_audio_units(AudioUnitSelect::DefaultOutput)[0];
        let fbox = fbox.clone();
        AudioUnitFactory::new_audio_unit(out, Some(device.clone()), move | result | {
            match result {
                Ok(mut audio_unit) => {
                    // lets observe device 
                    audio_device_change.lock().unwrap().observe_termination(device.os.0);
                    let fbox = fbox.clone();
                    audio_unit.set_input_callback(move | time, output | {
                        let mut fbox = fbox.lock().unwrap();
                        fbox(time, output);
                    });
                    // ok so we have to kinda terminate this loop here
                    //loop {
                        //println!("IN AUDIO UNIT LOOP");
                        //std::thread::sleep(std::time::Duration::from_millis(100));
                    //}
                }
                Err(err) => error!("spawn_audio_output Error {:?}", err)
            }
        });
        */
        //});
    }
    
    fn audio_input<F>(&mut self, f: F)
    where F: FnMut(usize, AudioDeviceId, AudioTime, AudioBuffer) -> AudioBuffer + Send + 'static {
        let audio_unit = self.os.audio_unit();
        *audio_unit.lock().unwrap().audio_input_cb .lock().unwrap() = Some(Box::new(f));
        /*
        //std::thread::spawn(move || {
        let out = &AudioUnitFactory::query_audio_units(AudioUnitSelect::DefaultInput)[0];
        let fbox = fbox.clone();
        AudioUnitFactory::new_audio_unit(out, Some(device), move | result | {
            match result {
                Ok(audio_unit) => { 
                    let fbox = fbox.clone();
                    audio_unit.set_output_callback(move | time, buffer | {
                        let mut fbox = fbox.lock().unwrap();
                        fbox(time, buffer)
                    });
                    //loop {
                    //    std::thread::sleep(std::time::Duration::from_millis(100));
                    //}
                }
                Err(err) => error!("spawn_audio_output Error {:?}", err)
            }
        });
        */
        // });
    }
    
}

