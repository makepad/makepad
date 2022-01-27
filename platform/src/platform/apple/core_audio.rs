use crate::platform::apple::frameworks::*;
use crate::objc_block;
use std::ptr;
use std::mem;

pub struct AudioOutput {
    instance: AudioUnit
}

impl AudioOutput {
    
    pub fn new() /*-> Result<AudioOutput, AudioError>*/ {
        let desc = AudioComponentDescription {
            componentType: AudioUnitType::IO,
            componentSubType: AudioUnitSubType::DefaultOutput,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        
        unsafe {
            let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
            let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
            let count: usize = msg_send![components, count];
            if count != 1{
                panic!();
            }

            let component: ObjcId = msg_send![components, objectAtIndex: 0];
            let desc: AudioComponentDescription = msg_send![component, audioComponentDescription];
            
            let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
                // lets spawn a thread
                if error != nil {
                    let code: i32 = msg_send![error, code];
                    panic!("Error constructing {:?}", AudioError::result(code))
                }
                
                let audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
                
                let () = msg_send![audio_unit, setOutputProvider: &objc_block!(
                    move | _flags: *mut u32,
                    _timestamp: *const AudioTimeStamp,
                    _frame_count: u32,
                    _input_bus_number: u64,
                    buffers: *mut AudioBufferList |: i32 {

                        let buffers = &*buffers;
                        let _left_chan = std::slice::from_raw_parts_mut(
                            buffers.mBuffers[0].mData as *mut f32,
                            (buffers.mBuffers[0].mDataByteSize >> 2) as usize
                        );
                        let _right_chan = std::slice::from_raw_parts_mut(
                            buffers.mBuffers[1].mData as *mut f32,
                            (buffers.mBuffers[1].mDataByteSize >> 2) as usize
                        );
                        // output beep here!
                        //for i in 0..left_chan.len(){
                            //left_chan[i] = (i as f32*0.01).sin();
                            //right_chan[i] = (i as f32*0.01).sin();
                        //}
                        0
                    }
                )];
                
                let () = msg_send![audio_unit, setOutputEnabled: true];
                
                let mut err: ObjcId = nil;
                let ret: bool = msg_send![audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                if !ret {
                    let code: i32 = msg_send![error, code];
                    panic!("allocateRenderResourcesAndReturnError failed {:?}", AudioError::result(code))
                }
                
                let mut err: ObjcId = nil;
                let ret: bool = msg_send![audio_unit, startHardwareAndReturnError: &mut err];
                if !ret {
                    let code: i32 = msg_send![error, code];
                    panic!("startHardwareAndReturnError failed {:?}", AudioError::result(code))
                }
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
                completionHandler:&instantiation_handler
            ];
        }
    }
}