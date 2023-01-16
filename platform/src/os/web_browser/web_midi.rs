use {
    std::sync::{mpsc,mpsc::TryRecvError},
    crate::{
        //makepad_wasm_bridge::*,
        midi::*,
        thread::Signal,
        os::web_browser::CxOs,
        //event::Event,
        //os::web_browser::{
            //to_wasm::{ToWasmMidiInputList, ToWasmMidiInputData},
            //from_wasm::{FromWasmStartMidiInput, FromWasmSpawnAudioOutput}
        //},
       // media_api::CxMediaApi,
       // os::web_browser::web_audio::*,
    }
};

pub struct OsMidiOutput{
    sender: mpsc::Sender<(Option<MidiPortId>, MidiData)>
}

impl OsMidiOutput{
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        let _ = self.sender.send((port_id, d));
        Signal::set_ui_signal();
        // lets send this midi to the JS side
        // sooo the problem is if we are not in the UI thread.
        // so how do we deal with that.
        // the audio worklet thread has signal blocks
    }
} 

#[derive(Default)]
pub struct WebMidiAccess{
    pub (crate) output_receivers: Vec<mpsc::Receiver<(Option<MidiPortId>, MidiData)>>,
    pub (crate) input_senders: MidiInputSenders,
}

impl WebMidiAccess{ 
    pub fn create_midi_input(&mut self)->MidiInput{
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn create_midi_output(&mut self)->MidiOutput{
        let (send, _recv) = mpsc::channel();
        MidiOutput(Some(OsMidiOutput{
            sender: send
        }))
    }
}

impl CxOs{
    pub fn handle_web_midi_signals(&mut self){
        // lets receive output and send
        let _from_wasm = self.from_wasm.as_mut().unwrap();
        self.web_midi_access.output_receivers.retain(|recv|{
            loop{
                match recv.try_recv(){
                    Ok((_midi_port_id, _data))=>{
                        //from_wasm.from_wasm(FromWasmMidiOutput{
                            
                        //})
                    },
                    Err(TryRecvError::Empty)=>{
                        return true
                    },
                    Err(TryRecvError::Disconnected)=>{
                        return false
                    }
                }
            }
        })
    }
}
