
use {
    std::sync::{Arc, Mutex},
    crate::{
        makepad_live_id::*,
        cx::Cx,
        event::Event,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::mswindows::win32_midi::{Win32MidiAccess,OsMidiInput,OsMidiOutput},
        os::mswindows::wasapi::{WasapiOutput, WasapiInput}
    }
};

impl CxMediaApi for Cx {
    fn handle_midi_port_list(&mut self, event: &Event) -> Vec<MidiPortId> {
        if let Event::Signal(se) = event {
            if se.signals.contains(&live_id!(Win32MidiInputsChanged).into()) {
                let win32_midi = self.os.win32_midi.as_mut().unwrap();
                let mut win32_midi = win32_midi.lock().unwrap();
                win32_midi.update_port_list();
                return win32_midi.get_ports()
            }
        }
        Vec::new()
    }
    
    fn midi_port_desc(&self, port: MidiPortId) -> Option<MidiPortDesc> {
        if let Some(win32_midi) = &self.os.win32_midi{
            win32_midi.lock().unwrap().port_desc(port)
        }
        else{
            None
        }
    }
    
    fn midi_input(&mut self)->MidiInput{
        if self.os.win32_midi.is_none(){
            self.os.win32_midi = Some(Arc::new(Mutex::new(Win32MidiAccess::new().unwrap())));
        }
        MidiInput(OsMidiInput(self.os.win32_midi.as_ref().unwrap().clone()))
    }
    
    fn midi_output(&mut self)->MidiOutput{
        if self.os.win32_midi.is_none(){
            self.os.win32_midi = Some(Arc::new(Mutex::new(Win32MidiAccess::new().unwrap())));
        }
        MidiOutput(OsMidiOutput(self.os.win32_midi.as_ref().unwrap().clone()))
    }

    fn handle_audio_device_list(&mut self, _event:&Event)->Vec<AudioDevice>{
        Vec::new()
    }

    fn request_audio_device_list(&mut self){}

    
    fn start_audio_output<F>(&mut self, _device:Option<&AudioDevice>, f: F) where F: FnMut(AudioTime, &mut AudioBuffer) + Send + 'static {
        
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let mut wasapi = WasapiOutput::new();
            let sample_time = 0f64;
            let host_time = 0u64;
            let rate_scalar = 44100f64;
            loop {
                let mut buffer = wasapi.wait_for_buffer().unwrap();
                let mut fbox = fbox.lock().unwrap();
                fbox(AudioTime {
                    sample_time,
                    host_time,
                    rate_scalar
                }, &mut buffer.audio_buffer);
                wasapi.release_buffer(buffer);
            }
        });
    }
    
    fn start_audio_input<F>(&mut self, _device:Option<&AudioDevice>, f: F) where F: FnMut(AudioTime, AudioBuffer)->AudioBuffer + Send + 'static {
        let fbox = std::sync::Arc::new(std::sync::Mutex::new(Box::new(f)));
        std::thread::spawn(move || {
            let mut wasapi = WasapiInput::new();
            let sample_time = 0f64;
            let host_time = 0u64;
            let rate_scalar = 44100f64;
            loop {
                let buffer = wasapi.wait_for_buffer().unwrap();
                
                let mut fbox = fbox.lock().unwrap();
                let ret_buffer = fbox(AudioTime {
                    sample_time,
                    host_time,
                    rate_scalar
                }, buffer);
                
                wasapi.release_buffer(ret_buffer);
            }
        });
    }
}


