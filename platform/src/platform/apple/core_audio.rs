use crate::platform::apple::frameworks::*;
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
            // oookaay so how do we access the pointerlist.
            let count: usize = msg_send![components, count];
            // lets access item 1
            let component: ObjcId = msg_send![components, objectAtIndex: 0];
            // ok now.
            let desc: AudioComponentDescription = msg_send![component, audioComponentDescription];
            
            #[repr(C)]
            struct BlockDescriptor {
                reserved: c_ulong,
                size: c_ulong,
                copy_helper: extern "C" fn(*mut c_void, *const c_void),
                dispose_helper: extern "C" fn(*mut c_void),
            }
            
            static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
                reserved: 0,
                size: mem::size_of::<BlockLiteral>() as c_ulong,
                copy_helper,
                dispose_helper,
            };
            
            extern "C" fn copy_helper(dst: *mut c_void, src: *const c_void) {
                unsafe {
                    /*ptr::write(
                        &mut (*(dst as *mut BlockLiteral)).inner as *mut _,
                        (&*(src as *const BlockLiteral)).inner.clone()
                    );*/
                }
            }
            
            extern "C" fn dispose_helper(src: *mut c_void) {
                unsafe {
                    ptr::drop_in_place(src as *mut BlockLiteral);
                }
            }
            
            #[repr(C)]
            struct BlockLiteral {
                isa: *const c_void,
                flags: i32,
                reserved: i32,
                invoke: extern "C" fn(*mut BlockLiteral, ObjcId, ObjcId),
                descriptor: *const BlockDescriptor,
            }
            
            let literal = BlockLiteral {
                isa: unsafe {_NSConcreteStackBlock.as_ptr() as *const c_void},
                flags: 1 << 25,
                reserved: 0,
                invoke,
                descriptor: &DESCRIPTOR,
            };
            
            extern "C" fn invoke(literal: *mut BlockLiteral, audio_unit: ObjcId, error: ObjcId) {
                let literal = unsafe {&mut *literal};
                println!("GOT INVOKED!");
                //drop(literal.inner.gpu_read_guards.lock().unwrap().take().unwrap());
            }
            
            // ok now instantiate the fucker
            let () = msg_send![
                class!(AVAudioUnit),
                instantiateWithComponentDescription: desc
                options: kAudioComponentInstantiation_LoadInProcess
                completionHandler: &literal
            ];
        }
    }
}
/*

pub fn get_property<T>(
    au: AudioUnit,
    id: u32,
    scope: Scope,
    elem: Element,
) -> Result<T, Error> {
    let scope = scope as c_uint;
    let elem = elem as c_uint;
    let mut size = ::std::mem::size_of::<T>() as u32;
    unsafe {
        let mut data_uninit = ::std::mem::MaybeUninit::<T>::uninit();
        let data_ptr = data_uninit.as_mut_ptr() as *mut _ as *mut c_void;
        let size_ptr = &mut size as *mut _;
        try_os_status!(sys::AudioUnitGetProperty(
            au, id, scope, elem, data_ptr, size_ptr
        ));
        let data: T = data_uninit.assume_init();
        Ok(data)
    }
}*/

