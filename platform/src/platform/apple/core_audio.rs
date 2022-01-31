use {
    std::ptr,
    std::mem,
    std::sync::{Arc, Mutex},
    crate::{
        platform::apple::frameworks::*,
        platform::apple::cocoa_app::*,
        platform::apple::apple_util::*,
        objc_block,
    },
};

pub struct MidiEndpoint {
    pub id: i32,
    pub name: String,
    pub manufacturer: String,
    endpoint: MIDIEndpointRef
}

#[derive(Clone, Copy, Debug)]
pub struct Midi1Data {
    pub input: usize,
    pub data0: u8,
    pub data1: u8,
    pub data2: u8
}

#[derive(Clone, Copy, Debug)]
pub struct Midi1Note {
    pub is_on: bool,
    pub channel: u8,
    pub note_number: u8,
    pub velocity: u8,
}


#[derive(Clone, Copy, Debug)]
pub enum Midi1Event {
    Note(Midi1Note),
    Unknown
}

impl Into<Midi1Data> for Midi1Note{
    fn into(self)->Midi1Data{
        Midi1Data{
            input: 0,
            data0: (if self.is_on{0x9}else{0x8}<<4) | self.channel,
            data1: self.note_number,
            data2: self.velocity
        }
    }
}

impl Midi1Data {
    pub fn decode(&self) -> Midi1Event {
        let status = self.data0>>4;
        let channel = self.data0&0xf;
        match status {
            0x8 | 0x9 => Midi1Event::Note(Midi1Note {is_on: status == 0x9, channel, note_number: self.data1, velocity: self.data2}),
            _ => Midi1Event::Unknown
        }
    }
}

pub struct Instrument {
    object: ObjcId
}

pub struct Midi {
    pub sources: Vec<MidiEndpoint>,
    pub destinations: Vec<MidiEndpoint>
}

impl MidiEndpoint {
    unsafe fn new(endpoint: MIDIEndpointRef) -> Result<Self,
    OSError> {
        let mut manufacturer = 0 as CFStringRef;
        let mut name = 0 as CFStringRef;
        let mut id = 0i32;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyManufacturer, &mut manufacturer)) ?;
        OSError::from(MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut name)) ?;
        OSError::from(MIDIObjectGetIntegerProperty(endpoint, kMIDIPropertyUniqueID, &mut id)) ?;
        Ok(Self {
            id,
            name: cfstring_ref_to_string(name),
            manufacturer: cfstring_ref_to_string(manufacturer),
            endpoint
        })
    }
}

impl Midi {
    pub fn new_midi_1_input<F: Fn(Midi1Data) + Send + 'static>(message_callback: F) -> Result<Self,
    OSError> {
        let mut midi_notify = objc_block!(move | _notification: &MIDINotification | {
            println!("Midi device added/removed");
        });
        
        let mut midi_receive = objc_block!(move | event_list: &MIDIEventList, user_data: u64 | {
            let packets = unsafe {std::slice::from_raw_parts(event_list.packet.as_ptr(), event_list.numPackets as usize)};
            for packet in packets {
                for i in 0 .. packet.wordCount {
                    let ump = packet.words[i as usize];
                    let ty = ((ump >> 28) & 0xf) as u8;
                    let _group = ((ump >> 24) & 0xf) as u8;
                    let data0 = ((ump >> 16) & 0xff) as u8;
                    let data1 = ((ump >> 8) & 0xff) as u8;
                    let data2 = (ump & 0xff) as u8;
                    if ty == 0x02 { // midi 1.0 channel voice
                        message_callback(Midi1Data {
                            input: user_data as usize,
                            data0,
                            data1,
                            data2
                        })
                    }
                }
            }
        });
        
        let mut midi_client = 0 as MIDIClientRef;
        let mut midi_in_port = 0 as MIDIPortRef;
        let mut midi_out_port = 0 as MIDIPortRef;
        let mut destinations = Vec::new();
        let mut sources = Vec::new();
        unsafe {
            OSError::from(MIDIClientCreateWithBlock(
                ccfstr_from_str("Makepad"),
                &mut midi_client,
                &mut midi_notify as *mut _ as ObjcId
            )) ?;
            
            OSError::from(MIDIInputPortCreateWithProtocol(
                midi_client,
                ccfstr_from_str("MIDI Input"),
                kMIDIProtocol_1_0,
                &mut midi_in_port,
                &mut midi_receive as *mut _ as ObjcId
            )) ?;
            
            OSError::from(MIDIOutputPortCreate(
                midi_client,
                ccfstr_from_str("MIDI Output"),
                &mut midi_out_port
            )) ?;
            
            for i in 0..MIDIGetNumberOfDestinations() {
                if let Ok(ep) = MidiEndpoint::new(MIDIGetDestination(i)) {
                    destinations.push(ep);
                }
            }
            for i in 0..MIDIGetNumberOfSources() {
                if let Ok(ep) = MidiEndpoint::new(MIDIGetSource(i)) {
                    MIDIPortConnectSource(midi_in_port, ep.endpoint, i as *mut _);
                    sources.push(ep);
                }
            }
        }
        
        Ok(Self {
            sources,
            destinations
        })
    }
}

pub struct Audio {}

#[derive(Clone)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub device_type: AudioDeviceType,
    desc: AudioComponentDescription
}

#[derive(Copy, Clone)]
pub enum AudioDeviceType {
    DefaultOutput,
    Music
}

unsafe impl Send for AudioDevice {}
pub struct AudioDevice {
    av_audio_unit: ObjcId,
    au_audio_unit: ObjcId,
    render_block: Option<ObjcId>,
    view_controller: Arc<Mutex<Option<ObjcId >> >,
    device_type: AudioDeviceType
}

pub struct AudioBuffer<'a> {
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
    
    buffers: *mut AudioBufferList,
    flags: *mut u32,
    timestamp: *const AudioTimeStamp,
    frame_count: u32,
    input_bus_number: u64,
}

impl AudioDevice {
    pub fn start_output<F: Fn(&mut AudioBuffer) + Send + 'static>(&self, audio_callback: F) {
        match self.device_type {
            AudioDeviceType::DefaultOutput => (),
            _ => panic!("start_audio_output_with_fn on this device")
        }
        unsafe {
            let output_provider = objc_block!(
                move | flags: *mut u32,
                timestamp: *const AudioTimeStamp,
                frame_count: u32,
                input_bus_number: u64,
                buffers: *mut AudioBufferList |: i32 {
                    let buffers_ref = &*buffers;
                    audio_callback(&mut AudioBuffer {
                        left: std::slice::from_raw_parts_mut(
                            buffers_ref.mBuffers[0].mData as *mut f32,
                            frame_count as usize
                        ),
                        right: std::slice::from_raw_parts_mut(
                            buffers_ref.mBuffers[1].mData as *mut f32,
                            frame_count as usize
                        ),
                        buffers,
                        flags,
                        timestamp,
                        frame_count,
                        input_bus_number
                    });
                    0
                }
            );
            let () = msg_send![self.au_audio_unit, setOutputProvider: &output_provider];
        }
    }
    
    pub fn render_to_audio_buffer(&self, buffer: &mut AudioBuffer) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("render_to_audio_buffer not supported on this device")
        }
        if let Some(render_block) = self.render_block {
            unsafe {objc_block_invoke!(render_block, invoke(
                (buffer.flags): *mut u32,
                (buffer.timestamp): *const AudioTimeStamp,
                (buffer.frame_count): u32,
                (buffer.input_bus_number): u64,
                (buffer.buffers): *mut AudioBufferList,
                (nil): ObjcId
            ) -> i32)};
        }
    }
    
    
    pub fn request_ui<F: Fn() + Send + 'static>(&self, view_loaded: F) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("request_ui not supported on this device")
        }
        
        let view_controller_arc = self.view_controller.clone();
        unsafe {
            let view_controller_complete = objc_block!(move | view_controller: ObjcId | {
                *view_controller_arc.lock().unwrap() = Some(view_controller);
                view_loaded();
            });
            
            let () = msg_send![self.au_audio_unit, requestViewControllerWithCompletionHandler: &view_controller_complete];
        }
    }
    
    pub fn open_ui(&self) {
        if let Some(view_controller) = self.view_controller.lock().unwrap().as_ref() {
            unsafe {
                let audio_view: ObjcId = msg_send![*view_controller, view];
                let cocoa_app = get_cocoa_app_global();
                let win_view = cocoa_app.cocoa_windows[0].1;
                let () = msg_send![win_view, addSubview: audio_view];
            }
        }
    }
    
    pub fn send_midi_1_event(&self, event: Midi1Data) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("send_midi_1_event not supported on this device")
        }
        unsafe {
            let () = msg_send![self.av_audio_unit, sendMIDIEvent: event.data0 data1: event.data1 data2: event.data2];
        }
    }
}

#[derive(Debug)]
pub enum AudioError {
    System(String),
    NoDevice
}

impl Audio {
    
    pub fn query_devices(device_type: AudioDeviceType) -> Vec<AudioDeviceInfo> {
        unsafe {
            let desc = match device_type {
                AudioDeviceType::Music => {
                    AudioComponentDescription::new_all_manufacturers(
                        AudioUnitType::MusicDevice,
                        AudioUnitSubType::Undefined,
                    )
                }
                AudioDeviceType::DefaultOutput => {
                    AudioComponentDescription::new_apple(
                        AudioUnitType::IO,
                        AudioUnitSubType::DefaultOutput,
                    )
                }
            };
            
            let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
            let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
            let count: usize = msg_send![components, count];
            let mut out = Vec::new();
            for i in 0..count {
                let component: ObjcId = msg_send![components, objectAtIndex: i];
                let name = nsstring_to_string(msg_send![component, name]);
                let desc: AudioComponentDescription = msg_send!(component, audioComponentDescription);
                out.push(AudioDeviceInfo {device_type, name, desc});
            }
            out
        }
    }
    
    pub fn new_device<F: Fn(Result<AudioDevice, AudioError>) + Send + 'static>(
        device_info: &AudioDeviceInfo,
        device_callback: F,
    ) {
        unsafe {
            let device_type = device_info.device_type;
            let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
                let () = msg_send![av_audio_unit, retain];
                unsafe fn inner(av_audio_unit: ObjcId, error: ObjcId, device_type: AudioDeviceType) -> Result<AudioDevice, OSError> {
                    OSError::from_nserror(error) ?;
                    let au_audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
                    
                    let mut err: ObjcId = nil;
                    let () = msg_send![au_audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                    OSError::from_nserror(err) ?;
                    let mut render_block = None;
                    match device_type {
                        AudioDeviceType::DefaultOutput => {
                            let () = msg_send![au_audio_unit, setOutputEnabled: true];
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, startHardwareAndReturnError: &mut err];
                            OSError::from_nserror(err) ?;
                        }
                        AudioDeviceType::Music => {
                            let block_ptr: ObjcId = msg_send![au_audio_unit, renderBlock];
                            let () = msg_send![block_ptr, retain];
                            render_block = Some(block_ptr);
                        }
                    }
                    
                    Ok(AudioDevice {
                        view_controller: Arc::new(Mutex::new(None)),
                        render_block,
                        device_type,
                        av_audio_unit,
                        au_audio_unit
                    })
                }
                
                match inner(av_audio_unit, error, device_type) {
                    Err(err) => device_callback(Err(AudioError::System(format!("{:?}", err)))),
                    Ok(device) => device_callback(Ok(device))
                }
            });
            
            // Instantiate output audio unit
            let () = msg_send![
                class!(AVAudioUnit),
                instantiateWithComponentDescription: device_info.desc
                options: kAudioComponentInstantiation_LoadOutOfProcess
                completionHandler: &instantiation_handler
            ];
        }
    }
}