
use{
    std::cell::RefCell,
    std::sync::{Arc,Mutex},
    crate::{
        makepad_platform::*,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::apple::audio_unit::*,
        os::apple::core_midi::*   
    }
};

pub fn live_design(_cx:&Cx){
}

#[derive(Default)]
struct CxMediaApple{
    pub midi_access: Option<CoreMidiAccess>,
    pub midi_input_data: Arc<Mutex<RefCell<Vec<MidiInputData>>>>,    
}

impl CxMediaApi for Cx{
    
    fn send_midi_data(&mut self, data:MidiData){
         let media = self.get_global::<CxMediaApple>();
         media.midi_access.as_ref().unwrap().send_midi_1_data(data);
    }
    
    fn handle_midi_received(&mut self, event:&Event)->Vec<MidiInputData>{
        if let Event::Signal(se) = event{
            if se.signals.contains(&live_id!(CoreMidiInputData).into()) {
                let media = self.get_global::<CxMediaApple>();
                let out_data = if let Ok(data) = media.midi_input_data.lock() {
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
                let media = self.get_global::<CxMediaApple>();
                let inputs = media.midi_access.as_ref().unwrap().connect_all_inputs();
                media.midi_access.as_mut().unwrap().update_destinations();
                return inputs
            }
        }
        Vec::new()
    }
    
    fn start_midi_input(&mut self) {
        if !self.has_global::<CxMediaApple>() {
            let mut media = CxMediaApple::default();
            let midi_input_data = media.midi_input_data.clone();
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
                media.midi_access = Some(ma);
            }
            self.set_global(media);
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

