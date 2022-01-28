use {
    std::ptr,
    std::mem,
    crate::{
        platform::apple::frameworks::*,
        platform::apple::cocoa_app::*,
        platform::apple::apple_util::*,
        objc_block,
    },
};

pub struct CoreMidiEndpoint {
    pub id: i32,
    pub name: String,
    pub manufacturer: String,
    endpoint: MIDIEndpointRef
}

pub struct CoreMidi {
    pub inputs: Vec<CoreMidiEndpoint>,
    pub outputs: Vec<CoreMidiEndpoint>
}

impl CoreMidiEndpoint {
    unsafe fn new(endpoint: MIDIEndpointRef) -> Result<Self,  OSError> {
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

impl CoreMidi {
    pub unsafe fn new_midi_input(_message_callback: Box<dyn Fn(u64)>,) -> Result<Self, OSError> {
        let mut midi_notify = objc_block!(move | _notification: &MIDINotification | {
            println!("Midi device added/removed");
        });
        
        let mut midi_receive = objc_block!(move | _event_list: &MIDIEventList, _user_data: ObjcId | {
            println!("Midi data received!");
        });
        
        let mut midi_client = 0 as MIDIClientRef;
        OSError::from(MIDIClientCreateWithBlock(
            ccfstr_from_str("Makepad"),
            &mut midi_client,
            &mut midi_notify as *mut _ as ObjcId
        )) ?;
        
        let mut midi_in_port = 0 as MIDIPortRef;
        OSError::from(MIDIInputPortCreateWithProtocol(
            midi_client,
            ccfstr_from_str("MIDI Input"),
            kMIDIProtocol_1_0,
            &mut midi_in_port,
            &mut midi_receive as *mut _ as ObjcId
        )) ?;
        
        let mut midi_out_port = 0 as MIDIPortRef; 
        OSError::from(MIDIOutputPortCreate(
            midi_client,
            ccfstr_from_str("MIDI Output"),
            &mut midi_out_port
        )) ?;
        
        let mut outputs = Vec::new();
        for i in 0..MIDIGetNumberOfDestinations() {
            if let Ok(ep) = CoreMidiEndpoint::new(MIDIGetDestination(i)) {
                outputs.push(ep);
            }
        }
        let mut inputs = Vec::new();
        for i in 0..MIDIGetNumberOfSources() {
            if let Ok(ep) = CoreMidiEndpoint::new(MIDIGetSource(i)) {
                MIDIPortConnectSource(midi_in_port, ep.endpoint, 0 as *mut _);
                inputs.push(ep);
            }
        }
        
        Ok(Self {
            inputs,
            outputs
        })
    }
}

pub struct CoreAudio {}

pub struct AudioComponentInfo {
    pub name: String,
    pub desc: AudioComponentDescription
}

impl CoreAudio {
    
    pub unsafe fn open_view_controller(vc: u64) {
        let view_controller = vc as ObjcId;
        let audio_view: ObjcId = msg_send![view_controller, view];
        let cocoa_app = get_cocoa_app_global();
        let win_view = cocoa_app.cocoa_windows[0].1;
        let () = msg_send![win_view, addSubview: audio_view];
    }
    
    pub unsafe fn get_music_devices() -> Vec<AudioComponentInfo> {
        let desc = AudioComponentDescription::new_all_manufacturers(
            AudioUnitType::MusicDevice,
            AudioUnitSubType::Undefined,
        );
        let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
        let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
        let count: usize = msg_send![components, count];
        let mut out = Vec::new();
        for i in 0..count {
            let component: ObjcId = msg_send![components, objectAtIndex: i];
            let name = nsstring_to_string(msg_send![component, name]);
            let desc: AudioComponentDescription = msg_send!(component, audioComponentDescription);
            out.push(AudioComponentInfo {name, desc});
        }
        out
    }
    
    pub unsafe fn new_midi_instrument_from_desc(
        desc: AudioComponentDescription,
        vc: Box<dyn Fn(u64)>,
        render_block: Box<dyn Fn(u64)>
    ) {
        let view_controller_complete = objc_block!(move | view_controller: ObjcId | {
            vc(view_controller as u64);
        });
        
        let instantiation_complete = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
            OSError::from_nserror(error).expect("instantiateWithComponentDescription");
            let audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
            
            let mut err: ObjcId = nil;
            let () = msg_send![audio_unit, allocateRenderResourcesAndReturnError: &mut err];
            OSError::from_nserror(err).expect("allocateRenderResourcesAndReturnError");
            
            let block_ptr: ObjcId = msg_send![audio_unit, renderBlock];
            let () = msg_send![block_ptr, retain];
            render_block(block_ptr as u64);
            
            let () = msg_send![audio_unit, requestViewControllerWithCompletionHandler: &view_controller_complete];
        });
        
        // Instantiate output audio unit
        let () = msg_send![
            class!(AVAudioUnit),
            instantiateWithComponentDescription: desc
            options: kAudioComponentInstantiation_LoadOutOfProcess
            completionHandler: &instantiation_complete
        ];
    }
    
    pub unsafe fn new_audio_output(audio_callback: Box<dyn Fn(&mut [f32], &mut [f32]) -> Option<u64 >>) {
        let desc = AudioComponentDescription::new_apple(
            AudioUnitType::IO,
            AudioUnitSubType::DefaultOutput,
        );
        
        let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
        let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
        let count: usize = msg_send![components, count];
        if count != 1 {
            panic!();
        }
        
        let component: ObjcId = msg_send![components, objectAtIndex: 0];
        let desc: AudioComponentDescription = msg_send![component, audioComponentDescription];
        
        let output_provider = objc_block!(
            move | flags: *mut u32,
            timestamp: *const AudioTimeStamp,
            frame_count: u32,
            input_bus_number: u64,
            buffers: *mut AudioBufferList |: i32 {
                let buffers_ref = &*buffers;
                let left_chan = std::slice::from_raw_parts_mut(
                    buffers_ref.mBuffers[0].mData as *mut f32,
                    frame_count as usize
                );
                let right_chan = std::slice::from_raw_parts_mut(
                    buffers_ref.mBuffers[1].mData as *mut f32,
                    frame_count as usize
                );
                let block_ptr = audio_callback(left_chan, right_chan);
                if let Some(block_ptr) = block_ptr {
                    objc_block_invoke!(block_ptr, invoke(
                        flags: *mut u32,
                        timestamp: *const AudioTimeStamp,
                        frame_count: u32,
                        input_bus_number: u64,
                        buffers: *mut AudioBufferList,
                        nil: ObjcId
                    ) -> i32);
                }
                0
            }
        );
        
        let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
            // lets spawn a thread
            OSError::from_nserror(error).expect("instantiateWithComponentDescription");
            
            let audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
            
            let () = msg_send![audio_unit, setOutputProvider: &output_provider];
            let () = msg_send![audio_unit, setOutputEnabled: true];
            
            let mut err: ObjcId = nil;
            let () = msg_send![audio_unit, allocateRenderResourcesAndReturnError: &mut err];
            OSError::from_nserror(err).expect("allocateRenderResourcesAndReturnError");
            
            let mut err: ObjcId = nil;
            let () = msg_send![audio_unit, startHardwareAndReturnError: &mut err];
            OSError::from_nserror(err).expect("startHardwareAndReturnError");
            // stay in a waitloop so the audio output gets callbacks.
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        
        // Instantiate output audio unit
        let () = msg_send![
            class!(AVAudioUnit),
            instantiateWithComponentDescription: desc
            options: kAudioComponentInstantiation_LoadInProcess
            completionHandler: &instantiation_handler
        ];
    }
}