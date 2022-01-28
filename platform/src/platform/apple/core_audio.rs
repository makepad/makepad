use {
    std::ptr,
    std::mem,
    crate::{
        platform::apple::frameworks::*,
        platform::apple::cocoa_app::*,
        platform::apple::apple_util::nsstring_to_string,
        objc_block,
    },
};

pub struct CoreAudio {
    instance: AudioUnit
}

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
            AudioError::ns_error_as_result(error).expect("instantiateWithComponentDescription");
            let audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
            
            let mut err: ObjcId = nil;
            let () = msg_send![audio_unit, allocateRenderResourcesAndReturnError: &mut err];
            AudioError::ns_error_as_result(err).expect("allocateRenderResourcesAndReturnError");
            
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
    
    pub unsafe fn new_audio_output(audio: Box<dyn Fn(&mut [f32], &mut [f32]) -> Option<u64 >>) {
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
                let block_ptr = audio(left_chan, right_chan);
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
            AudioError::ns_error_as_result(error).expect("instantiateWithComponentDescription");
            
            let audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
            
            let () = msg_send![audio_unit, setOutputProvider: &output_provider];
            let () = msg_send![audio_unit, setOutputEnabled: true];
            
            let mut err: ObjcId = nil;
            let () = msg_send![audio_unit, allocateRenderResourcesAndReturnError: &mut err];
            AudioError::ns_error_as_result(err).expect("allocateRenderResourcesAndReturnError");
            
            let mut err: ObjcId = nil;
            let () = msg_send![audio_unit, startHardwareAndReturnError: &mut err];
            AudioError::ns_error_as_result(err).expect("startHardwareAndReturnError");
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