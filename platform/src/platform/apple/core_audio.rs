use crate::platform::apple::frameworks::*;
use std::ptr;
use std::mem;

pub struct AudioOutput{
    instance: AudioUnit
}

impl AudioOutput{

    pub fn new()->Result<AudioOutput, AudioError>{
        let desc = AudioComponentDescription {
            componentType: AudioUnitType::IO,
            componentSubType: AudioUnitSubType::DefaultOutput,
            componentManufacturer: kAudioUnitManufacturer_Apple,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        
        unsafe {

            let component = AudioComponentFindNext(ptr::null_mut(), &desc as *const _);

            if component.is_null() {
                return Err(AudioError::NoMatchingDefaultAudioUnitFound);
            }

            // Create an instance of the default audio unit using the component.
            let mut instance_uninit = mem::MaybeUninit::<AudioUnit>::uninit();
            AudioError::result(AudioComponentInstanceNew(
                component,
                instance_uninit.as_mut_ptr() as *mut AudioUnit
            ))?;
            let instance: AudioUnit = instance_uninit.assume_init();

            // Initialise the audio unit!
            AudioError::result(AudioUnitInitialize(instance))?;
            
            Ok(AudioOutput {
                instance,
            })
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

