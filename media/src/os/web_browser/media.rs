
use {
    crate::{
        makepad_platform::makepad_wasm_bridge::*,
        makepad_platform::*,
        audio::*,
        midi::*,
        media_api::CxMediaApi,
        os::web_browser::web_audio::*,
    }
};

// WebAudio API
#[derive(FromWasm)]
pub struct FromWasmStartMidiInput {
}

#[derive(FromWasm)]
pub struct FromWasmSpawnAudioOutput {
    pub closure_ptr: u32,
}

#[derive(ToWasm)]
pub struct ToWasmMidiInputData {
    pub input_id: u32,
    pub data: u32,
}

impl Into<Midi1InputData> for ToWasmMidiInputData {
    fn into(self) -> Midi1InputData {
        Midi1InputData {
            input_id: self.input_id as usize,
            data: Midi1Data {
                data0: ((self.data >> 16) & 0xff) as u8,
                data1: ((self.data >> 8) & 0xff) as u8,
                data2: ((self.data >> 0) & 0xff) as u8,
            }
        }
    }
}

#[derive(ToWasm)]
pub struct WMidiInputInfo {
    pub manufacturer: String,
    pub name: String,
    pub uid: String,
}


#[derive(ToWasm)]
pub struct ToWasmMidiInputList {
    pub inputs: Vec<WMidiInputInfo>
}

impl Into<MidiInputInfo> for WMidiInputInfo {
    fn into(self) -> MidiInputInfo {
        MidiInputInfo {
            manufacturer: self.manufacturer,
            name: self.name,
            uid: self.uid
        }
    }
}

pub fn live_register(cx: &mut Cx) {
    cx.os.append_to_wasm_js(&[
        ToWasmMidiInputData::to_string(),
        ToWasmMidiInputList::to_string(),
    ]);
    
     cx.os.append_from_wasm_js(&[
        FromWasmStartMidiInput::to_string(),
        FromWasmSpawnAudioOutput::to_string(),
    ]);
}

impl CxMediaApi for Cx {
    
    fn on_midi_1_input_data(&mut self, event: &Event) -> Vec<Midi1InputData> {
        if let Event::ToWasmMsg(event) = event {
            match event.id{
                id!(ToWasmMidiInputData)=>{
                    let tw = ToWasmMidiInputData::read_to_wasm(&mut event.as_ref());
                    return vec![tw.into()]
                },
                _=>()
            }
        }
        Vec::new()
    }
    
    fn on_midi_input_list(&mut self, event: &Event) -> Vec<MidiInputInfo> {
        if let Event::ToWasmMsg(event) = event {
            match event.id{
                id!(ToWasmMidiInputList)=>{
                    let tw = ToWasmMidiInputList::read_to_wasm(&mut event.as_ref());
                    let mut ret = Vec::new();
                    for input in tw.inputs{
                        ret.push(input.into())
                    }
                    return ret
                },
                _=>()
            }
        }
        Vec::new()
    }
    
    fn start_midi_input(&mut self) {
        self.os.from_wasm(FromWasmStartMidiInput {
        });
    }
    
    fn start_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static {
        let closure_ptr = Box::into_raw(Box::new(WebAudioOutputClosure {
            callback: Box::new(f),
            output_buffer: WebAudioOutputBuffer::default()
        }));
        self.os.from_wasm(FromWasmSpawnAudioOutput {closure_ptr: closure_ptr as u32});
    }
}
