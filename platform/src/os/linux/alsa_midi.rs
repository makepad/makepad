 use {
    std::sync::{Arc, Mutex, mpsc},
    crate::{
        midi::*,
        thread::Signal,
    }
};
 
 
#[derive(Clone)]
pub struct OsMidiOutput(pub (crate) Arc<Mutex<AlsaMidiAccess >>);

impl OsMidiOutput {
    pub fn send(&self, port_id: Option<MidiPortId>, d: MidiData) {
        let _ =  self.0.lock().unwrap().event_sender.send(AlsaMidiEvent::SendMidi(port_id, d));
    }
}

type InputSenders = Arc<Mutex<Vec<mpsc::Sender<(MidiPortId, MidiData) >> >>;

#[derive(Clone)]
pub struct AlsaMidiPort {
    _desc: MidiPortDesc
}

#[derive(Clone)]
pub struct AlsaMidiInput {
    _port_id: MidiPortId,
}

#[derive(Clone)]
pub struct AlsaMidiOutput {
}

pub struct AlsaMidiAccess {
    input_senders: InputSenders,
    event_sender: mpsc::Sender<AlsaMidiEvent>,
    descs: Vec<MidiPortDesc>,
}

#[derive(Clone)]
enum AlsaMidiEvent {
    UpdateDevices,
    SendMidi(Option<MidiPortId>, MidiData),
    UseMidiInputs(Vec<MidiPortId>),
    UseMidiOutputs(Vec<MidiPortId>),
}

impl AlsaMidiAccess {
    
    pub fn new(change_signal:Signal) -> Arc<Mutex<Self >> {
        let (watch_sender, watch_receiver) = mpsc::channel();
        let input_senders = InputSenders::default();
        
        let midi_access = Arc::new(Mutex::new(Self {
            descs: Vec::new(),
            event_sender: watch_sender.clone(),
            input_senders,
        }));

        let _midi_access_clone = midi_access.clone();
        let change_signal_clone = change_signal.clone();
        
        std::thread::spawn(move || {
            
            while let Ok(msg) = watch_receiver.recv() {
                match msg {
                    AlsaMidiEvent::UpdateDevices => {
                        //midi_access_clone.lock().unwrap().descs = descs;
                        change_signal_clone.set();
                    }
                    AlsaMidiEvent::UseMidiOutputs(_ports) => {

                    }
                    AlsaMidiEvent::UseMidiInputs(_ports) => {
                        // find all ports we want enabled
                        
                    }
                    AlsaMidiEvent::SendMidi(_port_id, _midi_data)=>{
                      
                    }                    
                }
            }
        });
        //output_watcher.Start().unwrap();
        // alrighty lets initialize midi.
        change_signal.set();
        midi_access
    }
    
    pub fn create_midi_input(&self) -> MidiInput {
        let senders = self.input_senders.clone();
        let (send, recv) = mpsc::channel();
        senders.lock().unwrap().push(send);
        MidiInput(Some(recv))
    }
    
    pub fn midi_reset(&self){
        self.event_sender.send(AlsaMidiEvent::UseMidiOutputs(vec![])).unwrap();
        self.event_sender.send(AlsaMidiEvent::UseMidiInputs(vec![])).unwrap();
        self.event_sender.send(AlsaMidiEvent::UpdateDevices).unwrap();
    }
    
    pub fn use_midi_outputs(&mut self, ports: &[MidiPortId]) {
        self.event_sender.send(AlsaMidiEvent::UseMidiOutputs(ports.to_vec())).unwrap();
    }
    
    pub fn use_midi_inputs(&mut self, ports: &[MidiPortId]) {
        self.event_sender.send(AlsaMidiEvent::UseMidiInputs(ports.to_vec())).unwrap();
    }
    
    pub fn get_descs(&self) -> Vec<MidiPortDesc> {
        self.descs.clone()
    }
    
}
