
use {
    crate::{
        cx::Cx,
        event::Event,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::mswindows::wasapi::Wasapi
    }
};

impl CxMediaApi for Cx {
    
    fn send_midi_data(&mut self, _data: MidiData) {
        //         self.os.midi_access.as_ref().unwrap().send_midi_1_data(data);
    }
    
    fn handle_midi_received(&mut self, event: &Event) -> Vec<MidiInputData> {
        if let Event::Signal(_se) = event {
            /*if se.signals.contains(&live_id!(CoreMidiInputData).into()) {
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
            }*/
        }
        Vec::new()
    }
    
    fn handle_midi_inputs(&mut self, event: &Event) -> Vec<MidiInputInfo> {
        if let Event::Signal(_se) = event {
            /* if se.signals.contains(&live_id!(CoreMidiInputsChanged).into()) {
                let inputs = self.os.midi_access.as_ref().unwrap().connect_all_inputs();
                self.os.midi_access.as_mut().unwrap().update_destinations();
                return inputs
            }*/
        }
        Vec::new()
    }
    
    fn start_midi_input(&mut self) {
        /*let midi_input_data = self.os.midi_input_data.clone();
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
        Cx::post_signal(live_id!(CoreMidiInputsChanged).into());*/
    }
    
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static {
        
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let mut wasapi = Wasapi::new();
            let sample_time = 0f64;
            let host_time = 0u64;
            let rate_scalar = 44100f64;
            loop {
                if let Ok(mut buffer) = wasapi.wait_for_buffer() {
                    if let Ok(mut fbox) = fbox.lock() {
                        fbox(AudioTime {
                            sample_time,
                            host_time,
                            rate_scalar
                        }, &mut buffer);
                    }
                    wasapi.release_buffer(buffer);
                };
            }
        });
    }
}


