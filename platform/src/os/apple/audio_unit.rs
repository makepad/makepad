use {
    std::collections::HashSet,
    std::sync::{Arc, Mutex},
    crate::{
        makepad_live_id::*,
        thread::*,
        makepad_error_log::*,
        audio::*,
        midi::*,
        os::apple::apple_sys::*,
        os::apple::apple_classes::*,
        os::apple::apple_util::*,
        makepad_objc_sys::objc_block,
        makepad_objc_sys::objc_block_invoke,
    },
};


pub struct KeyValueObserver {
    _callback: Box<Box<dyn Fn() >>,
    observer: RcObjcId
}
unsafe impl Send for KeyValueObserver {}
unsafe impl Sync for KeyValueObserver {}

impl Drop for KeyValueObserver {
    fn drop(&mut self) {
        unsafe {
            (*self.observer.as_id()).set_ivar("callback", 0 as *mut c_void);
        }
    }
}

impl KeyValueObserver {
    pub fn new(target: ObjcId, name: ObjcId, callback: Box<dyn Fn()>) -> Self {
        unsafe {
            let double_box = Box::new(callback);
            //let cocoa_app = get_macos_app_global();
            let observer = RcObjcId::from_owned(msg_send![get_apple_class_global().key_value_observing_delegate, alloc]);
            
            (*observer.as_id()).set_ivar("callback", &*double_box as *const _ as *const c_void);
            
            let () = msg_send![
                target,
                addObserver: observer.as_id()
                forKeyPath: name
                options: 15u64 // if its not 1+2+4+8 it does nothing
                context: nil
            ];
            Self {
                _callback: double_box,
                observer
            }
        }
        
    }
}

pub fn define_key_value_observing_delegate() -> *const Class {
    
    extern fn observe_value_for_key_path(
        this: &Object,
        _: Sel,
        _key_path: ObjcId,
        _of_object: ObjcId,
        _change: ObjcId,
        _data: *mut std::ffi::c_void
    ) {
        unsafe {
            let ptr: *const c_void = *this.get_ivar("key_value_observer_callback");
            if ptr == 0 as *const c_void { // owner gone
                return
            }
            (*(ptr as *const Box<dyn Fn()>))();
        }
    }
    
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("KeyValueObservingDelegate", superclass).unwrap();
    
    // Add callback methods
    unsafe {
        decl.add_method(
            sel!(observeValueForKeyPath: ofObject: change: context:),
            observe_value_for_key_path as extern fn(&Object, Sel, ObjcId, ObjcId, ObjcId, *mut std::ffi::c_void)
        );
    }
    // Store internal state as user data
    decl.add_ivar::<*mut c_void>("key_value_observer_callback");
    
    return decl.register();
}


#[derive(Default)]
pub struct AudioUnitAccess {
    pub change_signal: Signal,
    pub device_descs: Vec<CoreAudioDeviceDesc>,
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn> > >;MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn> > >;MAX_AUDIO_DEVICE_INDEX],
    pub audio_inputs: Arc<Mutex<Vec<RunningAudioUnit >> >,
    pub audio_outputs: Arc<Mutex<Vec<RunningAudioUnit >> >,
    failed_devices: Arc<Mutex<HashSet<AudioDeviceId>>>,
}

#[derive(Debug)]
pub enum AudioError {
    System(String),
    NoDevice
}

pub struct CoreAudioDeviceDesc {
    pub core_device_id: AudioDeviceID,
    pub desc: AudioDeviceDesc
}

pub struct RunningAudioUnit {
    device_id: AudioDeviceId,
    audio_unit: Option<AudioUnit>
}

impl AudioUnitAccess {
    pub fn new(change_signal:Signal) -> Arc<Mutex<Self>> {
        Self::observe_route_changes(change_signal.clone());
        
        Arc::new(Mutex::new(Self{
            failed_devices: Default::default(),
            change_signal,
            device_descs: Default::default(),
            audio_input_cb: Default::default(),
            audio_output_cb:Default::default(),
            audio_inputs:Default::default(),
            audio_outputs:Default::default(),
        }))
    }
    
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        let _ = self.update_device_list();
        let mut out = Vec::new();
        for dev in &self.device_descs {
            out.push(dev.desc.clone());
        }
        out
    }
    
    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.retain_mut( | v | {
                if devices.contains(&v.device_id) {
                    true
                }
                else if let Some(audio_unit) = &mut v.audio_unit {
                    audio_unit.stop_hardware();
                    audio_unit.clear_input_handler();
                    audio_unit.release_audio_unit();
                    false
                }
                else {
                    false
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    new.push((index,*device_id))
                }
            }
            for (_,device_id) in &new {
                audio_inputs.push(RunningAudioUnit {
                    device_id: *device_id,
                    audio_unit: None
                });
            }
            new
        };
        for (index, device_id) in new {
            // lets create an audio input
            let unit_info = &AudioUnitAccess::query_audio_units(AudioUnitQuery::Input)[0];
            let audio_inputs = self.audio_inputs.clone();
            let audio_input_cb = self.audio_input_cb[index].clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();
            self.new_audio_io(self.change_signal.clone(), unit_info, device_id, move | result | {
                let mut audio_inputs = audio_inputs.lock().unwrap();
                match result {
                    Ok(audio_unit) => {
                        
                        let running = audio_inputs.iter_mut().find( | v | v.device_id == device_id).unwrap();
                        let audio_input_cb = audio_input_cb.clone();
                        audio_unit.set_input_handler(move | time, output | {
                            if let Some(audio_input_cb) = &mut *audio_input_cb.lock().unwrap() {
                                return audio_input_cb(AudioInfo{
                                    device_id, 
                                    time: Some(time)
                                }, output)
                            }
                        });
                        running.audio_unit = Some(audio_unit);
                    }
                    Err(err) => {
                        failed_devices.lock().unwrap().insert(device_id);
                        change_signal.set();
                        audio_inputs.retain( | v | v.device_id != device_id);
                        error!("spawn_audio_output Error {:?}", err)
                    }
                }
            })
        }
    }
    
    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.retain_mut( | v | {
                if devices.contains(&v.device_id) {
                    true
                }
                else if let Some(audio_unit) = &mut v.audio_unit {
                    audio_unit.stop_hardware();
                    audio_unit.clear_output_provider();
                    audio_unit.release_audio_unit();
                    false
                }
                else {
                    false
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs.iter().find( | v | v.device_id == *device_id).is_none() {
                    new.push((index,*device_id))
                }
            }
            for (_,device_id) in &new {
                audio_outputs.push(RunningAudioUnit {
                    device_id: *device_id,
                    audio_unit: None
                });
            }
            new
            
        };
        for (index, device_id) in new {
            let unit_info = &AudioUnitAccess::query_audio_units(AudioUnitQuery::Output)[0];
            let audio_outputs = self.audio_outputs.clone();
            let audio_output_cb = self.audio_output_cb[index].clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();
            self.new_audio_io(self.change_signal.clone(), unit_info, device_id, move | result | {
                let mut audio_outputs = audio_outputs.lock().unwrap();
                match result {
                    Ok(audio_unit) => {
                        
                        let running = audio_outputs.iter_mut().find( | v | v.device_id == device_id).unwrap();
                        let audio_output_cb = audio_output_cb.clone();
                        audio_unit.set_output_provider(move | time, output | {
                            if let Some(audio_output_cb) = &mut *audio_output_cb.lock().unwrap() {
                                audio_output_cb(AudioInfo{
                                    device_id, 
                                    time:Some(time)
                                }, output)
                            }
                        });
                        running.audio_unit = Some(audio_unit);
                    }
                    Err(err) => {
                        failed_devices.lock().unwrap().insert(device_id);
                        change_signal.set();
                        audio_outputs.retain( | v | v.device_id != device_id);
                        error!("spawn_audio_output Error {:?}", err)
                    }
                }
            })
        }
    }
    
    
    pub fn observe_route_changes(device_change:Signal) {
        let center: ObjcId = unsafe {msg_send![class!(NSNotificationCenter), defaultCenter]};
        unsafe {
            let mut err: ObjcId = nil;
            let audio_session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];
            let () = msg_send![audio_session, setCategory: AVAudioSessionCategoryMultiRoute error: &mut err];
            let () = msg_send![audio_session, setActive: true error: &mut err];
        }
        let block = objc_block!(move | _note: ObjcId | {
            device_change.set();
        });
        let () = unsafe {msg_send![
            center,
            addObserverForName: AVAudioSessionRouteChangeNotification
            object: nil
            queue: nil
            usingBlock: &block
        ]};
    }
    
    pub fn observe_audio_unit_termination(change_signal:Signal, core_device_id: AudioDeviceID) {
        
        #[allow(non_snake_case)]
        unsafe extern "system" fn listener_fn(
            _inObjectID: AudioObjectID,
            _inNumberAddresses: u32,
            _inAddresses: *const AudioObjectPropertyAddress,
            inClientData: *mut ()
        ) -> OSStatus {
            let x: *const Signal = inClientData as *const _;
            (*x).set();
            0
        }
        
        let prop_addr = AudioObjectPropertyAddress {
            mSelector: AudioObjectPropertySelector::DeviceIsAlive,
            mScope: AudioObjectPropertyScope::Global,
            mElement: AudioObjectPropertyElement::Master,
        };
        
        let result = unsafe {AudioObjectAddPropertyListener(
            core_device_id,
            &prop_addr,
            Some(listener_fn),
            Box::into_raw(Box::new(change_signal)) as *mut _,
        )};
        
        if result != 0 {
            println!("Error in adding DeviceIsAlive listener");
        }
    }
    
    pub fn update_device_list(&mut self) -> Result<(), OSError> {
        self.device_descs.clear();
        
        fn get_value<T: Sized>(device_id: AudioDeviceID, selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope, default: T) -> Result<T, OSError> {
            let prop_addr = AudioObjectPropertyAddress {
                mSelector: selector,
                mScope: scope,
                mElement: AudioObjectPropertyElement::Master
            };
            
            let value = default;
            let data_size = std::mem::size_of::<T>();
            OSError::from(unsafe {AudioObjectGetPropertyData(
                device_id,
                &prop_addr,
                0,
                std::ptr::null(),
                &data_size as *const _ as *mut _,
                &value as *const _ as *mut _,
            )}) ?;
            Ok(value)
        }
        
        fn get_array<T: Sized>(device_id: AudioDeviceID, selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope) -> Result<Vec<T>, OSError> {
            let prop_addr = AudioObjectPropertyAddress {
                mSelector: selector,
                mScope: scope,
                mElement: AudioObjectPropertyElement::Master,
            };
            let data_size = 0u32;
            OSError::from(unsafe {AudioObjectGetPropertyDataSize(
                device_id,
                &prop_addr as *const _,
                0,
                std::ptr::null(),
                &data_size as *const _ as *mut _,
            )}) ?;
            
            let mut buf: Vec<T> = vec![];
            let elements = data_size as usize / std::mem::size_of::<T>();
            buf.reserve_exact(elements);
            OSError::from(unsafe {AudioObjectGetPropertyData(
                device_id,
                &prop_addr as *const _,
                0,
                std::ptr::null(),
                &data_size as *const _ as *mut _,
                buf.as_mut_ptr() as *mut _,
            )}) ?;
            unsafe {buf.set_len(elements)};
            Ok(buf)
        }
        
        let default_input_id: AudioDeviceID = get_value(
            kAudioObjectSystemObject,
            AudioObjectPropertySelector::DefaultInputDevice,
            AudioObjectPropertyScope::Global,
            0
        ) .unwrap();
        let default_output_id: AudioDeviceID = get_value(
            kAudioObjectSystemObject,
            AudioObjectPropertySelector::DefaultOutputDevice,
            AudioObjectPropertyScope::Global,
            0
        ) .unwrap();
        
        let core_device_ids: Vec<AudioDeviceID> = get_array(
            kAudioObjectSystemObject,
            AudioObjectPropertySelector::Devices,
            AudioObjectPropertyScope::Global,
        ) ?;
        
        for core_device_id in core_device_ids {
            let device_name: Result<CFStringRef, OSError> = get_value(
                core_device_id,
                AudioObjectPropertySelector::DeviceNameCFString,
                AudioObjectPropertyScope::Output,
                std::ptr::null()
            );
            if device_name.is_err() {
                println!("Error getting name for device {}", core_device_id);
                continue;
            }
            let device_name = device_name.unwrap();
            let device_name = unsafe {cfstring_ref_to_string(device_name)};
            // lets probe input/outputness
            fn probe_channels(device_id: AudioDeviceID, scope: AudioObjectPropertyScope) -> Result<usize, OSError> {
                let buffer_data: Vec<u8> = get_array(
                    device_id,
                    AudioObjectPropertySelector::StreamConfiguration,
                    scope
                ) ?;
                let audio_buffer_list = unsafe {&*(buffer_data.as_ptr() as *const AudioBufferList)};
                if audio_buffer_list.mNumberBuffers == 0 {
                    return Ok(0);
                }
                let mut channels = 0;
                for i in 0..audio_buffer_list.mNumberBuffers as usize {
                    channels += audio_buffer_list.mBuffers[i].mNumberChannels as usize;
                }
                Ok(channels)
            }
            
            let input_channels = probe_channels(core_device_id, AudioObjectPropertyScope::Input) ?;
            let output_channels = probe_channels(core_device_id, AudioObjectPropertyScope::Output) ?;
            
            if input_channels>0 {
                let device_id = LiveId::from_str(&format!("{} {} input", device_name, core_device_id)).into();
                self.device_descs.push(CoreAudioDeviceDesc {
                    core_device_id,
                    desc: AudioDeviceDesc {
                        has_failed: self.failed_devices.lock().unwrap().contains(&device_id),
                        device_id,
                        device_type: AudioDeviceType::Input,
                        channel_count: input_channels,
                        is_default: core_device_id == default_input_id,
                        name: device_name.clone(),
                    }
                })
            }
            if output_channels>0 {
                let device_id = LiveId::from_str(&format!("{} {} output", device_name, core_device_id)).into();
                self.device_descs.push(CoreAudioDeviceDesc {
                    core_device_id,
                    desc: AudioDeviceDesc {
                        has_failed: self.failed_devices.lock().unwrap().contains(&device_id),
                        device_id,
                        device_type: AudioDeviceType::Output,
                        channel_count: input_channels,
                        is_default: core_device_id == default_output_id,
                        name: device_name.clone(),
                    }
                })
            }
        }
        Ok(())
    }
    
    pub fn query_audio_units(unit_query: AudioUnitQuery) -> Vec<AudioUnitInfo> {
        let desc = match unit_query {
            AudioUnitQuery::MusicDevice => {
                AudioComponentDescription::new_all_manufacturers(
                    AudioUnitType::MusicDevice,
                    AudioUnitSubType::Undefined,
                )
            }
            #[cfg(target_os = "macos")]
            AudioUnitQuery::Output => {
                AudioComponentDescription::new_all_manufacturers(
                    AudioUnitType::IO,
                    AudioUnitSubType::HalOutput,
                )
            }
            #[cfg(target_os = "ios")]
            AudioUnitQuery::Output => {
                AudioComponentDescription::new_all_manufacturers(
                    AudioUnitType::IO,
                    AudioUnitSubType::RemoteIO,
                )
            }
            AudioUnitQuery::Input => {
                AudioComponentDescription::new_all_manufacturers(
                    AudioUnitType::IO,
                    AudioUnitSubType::HalOutput,
                )
            }
            AudioUnitQuery::Effect => {
                AudioComponentDescription::new_all_manufacturers(
                    AudioUnitType::Effect,
                    AudioUnitSubType::Undefined,
                )
            }
        };
        unsafe {
            let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
            let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
            let count: usize = msg_send![components, count];
            let mut out = Vec::new();
            for i in 0..count {
                let component: ObjcId = msg_send![components, objectAtIndex: i];
                let name = nsstring_to_string(msg_send![component, name]);
                let desc: AudioComponentDescription = msg_send!(component, audioComponentDescription);
                out.push(AudioUnitInfo {unit_query, name, desc});
            }
            out
        }
    }
    
    pub fn new_audio_io<F: Fn(Result<AudioUnit, AudioError>) + Send + 'static>(
        &self,
        change_signal: Signal,
        unit_info: &AudioUnitInfo,
        device_id: AudioDeviceId,
        unit_callback: F,
    ) {
        let unit_query = unit_info.unit_query;
        
        let core_device_id = self.device_descs.iter().find( | v | v.desc.device_id == device_id).unwrap().core_device_id;
        
        let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
            let () = unsafe {msg_send![av_audio_unit, retain]};
            unsafe fn inner(change_signal: Signal, core_device_id: AudioDeviceID, av_audio_unit: ObjcId, error: ObjcId, unit_query: AudioUnitQuery) -> Result<AudioUnit, OSError> {
                OSError::from_nserror(error) ?;
                let au_audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
                let () = msg_send![av_audio_unit, retain];
                
                let mut err: ObjcId = nil;
                let () = msg_send![au_audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                OSError::from_nserror(err) ?;
                let mut render_block = None;
                
                let stream_desc = CAudioStreamBasicDescription {
                    mSampleRate: 48000.0,
                    mFormatID: AudioFormatId::LinearPCM,
                    mFormatFlags: LinearPcmFlags::IS_FLOAT as u32
                        | LinearPcmFlags::IS_NON_INTERLEAVED as u32
                        | LinearPcmFlags::IS_PACKED as u32,
                    mBytesPerPacket: 4,
                    mFramesPerPacket: 1,
                    mBytesPerFrame: 4,
                    mChannelsPerFrame: 2,
                    mBitsPerChannel: 32,
                    mReserved: 0
                };
                
                let av_audio_format: ObjcId = msg_send![class!(AVAudioFormat), alloc];
                let () = msg_send![av_audio_format, initWithStreamDescription: &stream_desc];
                
                match unit_query {
                    AudioUnitQuery::Output => {
                        let busses: ObjcId = msg_send![au_audio_unit, inputBusses];
                        let count: usize = msg_send![busses, count];
                        if count > 0 {
                            let bus: ObjcId = msg_send![busses, objectAtIndexedSubscript: 0];
                            let mut err: ObjcId = nil;
                            let () = msg_send![bus, setFormat: av_audio_format error: &mut err];
                            OSError::from_nserror(err) ?;
                        }
                        
                        let () = msg_send![au_audio_unit, setOutputEnabled: true];
                        let () = msg_send![au_audio_unit, setInputEnabled: false];
                        
                        #[cfg(target_os = "macos")]
                        {
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, setDeviceID: core_device_id error: &mut err];
                            OSError::from_nserror(err) ?;
                        }
                        
                        // lets hardcode the format to 44100 float
                        let mut err: ObjcId = nil;
                        let () = msg_send![au_audio_unit, startHardwareAndReturnError: &mut err];
                        OSError::from_nserror(err) ?;
                    }
                    AudioUnitQuery::Input => {
                        // lets hardcode the format to 44100 float
                        let busses: ObjcId = msg_send![au_audio_unit, outputBusses];
                        let count: usize = msg_send![busses, count];
                        if count > 1 {
                            let bus: ObjcId = msg_send![busses, objectAtIndexedSubscript: 1];
                            //let format: ObjcId = msg_send![bus, format];
                            //let format: *const CAudioStreamBasicDescription = msg_send![format, streamDescription];
                            let mut err: ObjcId = nil;
                            let () = msg_send![bus, setFormat: av_audio_format error: &mut err];
                            OSError::from_nserror(err) ?;
                            let () = msg_send![bus, setEnabled: true];
                        }
                        let () = msg_send![au_audio_unit, setOutputEnabled: false];
                        let () = msg_send![au_audio_unit, setInputEnabled: true];
                        
                        let mut err: ObjcId = nil;
                        let () = msg_send![au_audio_unit, setDeviceID: core_device_id error: &mut err];
                        OSError::from_nserror(err) ?;
                        
                        let mut err: ObjcId = nil;
                        let () = msg_send![au_audio_unit, startHardwareAndReturnError: &mut err];
                        OSError::from_nserror(err) ?;
                        
                        let block_ptr: ObjcId = msg_send![au_audio_unit, renderBlock];
                        let () = msg_send![block_ptr, retain];
                        render_block = Some(block_ptr);
                    }
                    _ => ()
                }
                AudioUnitAccess::observe_audio_unit_termination(change_signal, core_device_id);
                Ok(AudioUnit {
                    view_controller: Arc::new(Mutex::new(None)),
                    param_tree_observer: None,
                    render_block,
                    unit_query,
                    av_audio_unit,
                    au_audio_unit
                })
            }
            let change_signal = change_signal.clone();
            match unsafe {inner(change_signal, core_device_id, av_audio_unit, error, unit_query)} {
                Err(err) => unit_callback(Err(AudioError::System(format!("{:?}", err)))),
                Ok(device) => unit_callback(Ok(device))
            }
        });
        
        // Instantiate output audio unit
        let () = unsafe {msg_send![
            class!(AVAudioUnit),
            instantiateWithComponentDescription: unit_info.desc
            options: kAudioComponentInstantiation_LoadOutOfProcess
            completionHandler: &instantiation_handler
        ]};
    }
    
    pub fn new_audio_plugin<F: Fn(Result<AudioUnit, AudioError>) + Send + 'static>(
        unit_info: &AudioUnitInfo,
        unit_callback: F,
    ) {
        let unit_query = unit_info.unit_query;
        
        let instantiation_handler = objc_block!(move | av_audio_unit: ObjcId, error: ObjcId | {
            let () = unsafe {msg_send![av_audio_unit, retain]};
            unsafe fn inner(av_audio_unit: ObjcId, error: ObjcId, unit_query: AudioUnitQuery) -> Result<AudioUnit, OSError> {
                
                OSError::from_nserror(error) ?;
                let au_audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];
                
                let mut err: ObjcId = nil;
                let () = msg_send![au_audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                OSError::from_nserror(err) ?;
                
                let mut render_block = None;
                
                match unit_query {
                    AudioUnitQuery::MusicDevice => {
                        let block_ptr: ObjcId = msg_send![au_audio_unit, renderBlock];
                        let () = msg_send![block_ptr, retain];
                        render_block = Some(block_ptr);
                    }
                    AudioUnitQuery::Effect => {
                        let block_ptr: ObjcId = msg_send![au_audio_unit, renderBlock];
                        let input_busses: ObjcId = msg_send![au_audio_unit, inputBusses];
                        let count: usize = msg_send![input_busses, count];
                        if count > 0 {
                            // enable bus 0
                            let bus: ObjcId = msg_send![input_busses, objectAtIndexedSubscript: 0];
                            let () = msg_send![bus, setEnabled: true];
                        }
                        let () = msg_send![block_ptr, retain];
                        render_block = Some(block_ptr);
                    }
                    _ => ()
                }
                
                Ok(AudioUnit {
                    view_controller: Arc::new(Mutex::new(None)),
                    param_tree_observer: None,
                    render_block,
                    unit_query,
                    av_audio_unit,
                    au_audio_unit
                })
            }
            
            match unsafe {inner(av_audio_unit, error, unit_query)} {
                Err(err) => unit_callback(Err(AudioError::System(format!("{:?}", err)))),
                Ok(device) => unit_callback(Ok(device))
            }
        });
        
        // Instantiate output audio unit
        let () = unsafe {msg_send![
            class!(AVAudioUnit),
            instantiateWithComponentDescription: unit_info.desc
            options: kAudioComponentInstantiation_LoadOutOfProcess
            completionHandler: &instantiation_handler
        ]};
    }
}


#[derive(Copy, Clone, Debug)]
pub enum AudioUnitQuery {
    Output,
    Input,
    MusicDevice,
    Effect
}
unsafe impl Send for AudioUnitQuery {}
unsafe impl Sync for AudioUnitQuery {}

#[derive(Clone, Debug)]
pub struct AudioUnitInfo {
    pub name: String,
    pub unit_query: AudioUnitQuery,
    desc: AudioComponentDescription
}

unsafe impl Send for AudioUnit {}
unsafe impl Sync for AudioUnit {}
pub struct AudioUnit {
    param_tree_observer: Option<KeyValueObserver>,
    av_audio_unit: ObjcId,
    au_audio_unit: ObjcId,
    render_block: Option<ObjcId>,
    view_controller: Arc<Mutex<Option<ObjcId >> >,
    unit_query: AudioUnitQuery
}

unsafe impl Send for AudioUnitClone {}
unsafe impl Sync for AudioUnitClone {}
pub struct AudioUnitClone {
    av_audio_unit: ObjcId,
    _au_audio_unit: ObjcId,
    render_block: Option<ObjcId>,
    unit_query: AudioUnitQuery
}

impl AudioTime {
    fn to_audio_time_stamp(&self) -> AudioTimeStamp {
        AudioTimeStamp {
            mSampleTime: self.sample_time,
            mHostTime: self.host_time,
            mRateScalar: self.rate_scalar,
            mWordClockTime: 0,
            mSMPTETime: SMPTETime::default(),
            mFlags: 7,
            mReserved: 0
        }
    }
}

impl AudioBuffer {
    unsafe fn to_audio_buffer_list(&mut self) -> AudioBufferList {
        let mut ab = AudioBufferList {
            mNumberBuffers: self.channel_count.min(MAX_AUDIO_BUFFERS) as u32,
            mBuffers: [
                _AudioBuffer {
                    mNumberChannels: 0,
                    mDataByteSize: 0,
                    mData: 0 as *mut ::std::os::raw::c_void
                };
                MAX_AUDIO_BUFFERS
            ]
        };
        for i in 0..self.channel_count.min(MAX_AUDIO_BUFFERS) {
            ab.mBuffers[i] = _AudioBuffer {
                mNumberChannels: 0,
                mDataByteSize: (self.frame_count * 4) as u32,
                mData: self.channel_mut(i).as_ptr() as *mut ::std::os::raw::c_void,
            }
        }
        ab
    }
}
/*
fn print_hex(data: &[u8]) {
    // lets print hex data
    // we print 16 bytes per line
    let mut o = 0;
    while o < data.len() {
        let line = (data.len() - o).min(16);
        print!("{:08x} ", o);
        for i in o..o + line {
            print!("{:02x} ", data[i]);
        }
        for i in o..o + line {
            let c = data[i];
            if c>30 && c < 127 {
                print!("{}", c as char);
            }
            else {
                print!(".");
            }
        }
        println!("");
        o += line;
    }
}*/

#[derive(Default)]
pub struct AudioInstrumentState {
    manufacturer: u64,
    data: Vec<u8>,
    vstdata: Vec<u8>,
    subtype: u64,
    version: u64,
    ty: u64,
    name: String
}


impl AudioUnitClone {
    
    pub fn render_to_audio_buffer(&self, info: AudioInfo, outputs: &mut [&mut AudioBuffer], inputs: &[&AudioBuffer]) {
        match self.unit_query {
            AudioUnitQuery::MusicDevice => (),
            AudioUnitQuery::Effect => (),
            _ => panic!("render_to_audio_buffer not supported on this device")
        }
        if let Some(render_block) = self.render_block {
            let inputs_ptr = inputs.as_ptr() as *const *const AudioBuffer as u64;
            let inputs_len = inputs.len();
            let output_provider = objc_block!(
                move | _flags: *mut u32,
                _timestamp: *const AudioTimeStamp,
                frame_count: u32,
                input_bus: u64,
                buffers: *mut AudioBufferList |: i32 {
                    let inputs = unsafe {std::slice::from_raw_parts(
                        inputs_ptr as *const &AudioBuffer,
                        inputs_len as usize
                    )};
                    let buffers = unsafe {&*buffers};
                    let input_bus = input_bus as usize;
                    if input_bus >= inputs.len() {
                        error!("render_to_audio_buffer - input bus number > input len {} {}", input_bus, inputs.len());
                        return 0
                    }
                    // ok now..
                    if buffers.mNumberBuffers as usize != inputs[input_bus].channel_count {
                        error!("render_to_audio_buffer - input channel count doesnt match {} {}", buffers.mNumberBuffers, inputs[input_bus].channel_count);
                        return 0
                    }
                    if buffers.mNumberBuffers as usize > MAX_AUDIO_BUFFERS {
                        error!("render_to_audio_buffer - number of channels requested > MAX_AUDIO_BUFFER_LIST_SIZE");
                        return 0
                    }
                    if frame_count as usize != inputs[input_bus].frame_count {
                        error!("render_to_audio_buffer - frame count doesnt match {} {}", inputs[input_bus].frame_count, frame_count);
                        return 0
                    }
                    for i in 0..inputs[input_bus].channel_count {
                        // ok so. we have an inputs
                        let output_buffer = unsafe {std::slice::from_raw_parts_mut(
                            buffers.mBuffers[i].mData as *mut f32,
                            frame_count as usize
                        )};
                        let input_buffer = inputs[input_bus].channel(i);
                        output_buffer.copy_from_slice(input_buffer);
                    }
                    0
                }
            );
            // ok we need to construct all these things
            let mut flags: u32 = 0;
            let timestamp = info.time.unwrap().to_audio_time_stamp();
            for i in 0..outputs.len() {
                unsafe {
                    let mut buffer_list = outputs[i].to_audio_buffer_list();
                    objc_block_invoke!(render_block, invoke(
                        (&mut flags as *mut u32): *mut u32,
                        (&timestamp as *const AudioTimeStamp): *const AudioTimeStamp,
                        (outputs[i].frame_count as u32): u32,
                        (i as u64): u64,
                        (&mut buffer_list as *mut AudioBufferList): *mut AudioBufferList,
                        (&output_provider as *const _ as ObjcId): ObjcId
                        //(nil): ObjcId
                    ) -> i32);
                }
            }
        };
    }
    
    pub fn handle_midi_data(&self, event: MidiData) {
        match self.unit_query {
            AudioUnitQuery::MusicDevice => (),
            _ => panic!("send_midi_1_event not supported on this device")
        }
        unsafe {
            let () = msg_send![self.av_audio_unit, sendMIDIEvent: event.data[0] data1: event.data[1] data2: event.data[2]];
        }
    }
}

impl AudioUnit {
    
    pub fn clone(&self) -> AudioUnitClone {
        AudioUnitClone {
            av_audio_unit: self.av_audio_unit,
            _au_audio_unit: self.au_audio_unit,
            render_block: self.render_block,
            unit_query: self.unit_query,
        }
    }
    
    pub fn parameter_tree_changed(&mut self, callback: Box<dyn Fn() + Send>) {
        if self.param_tree_observer.is_some() {
            panic!();
        }
        let observer = KeyValueObserver::new(self.au_audio_unit, str_to_nsstring("deviceIsAlive"), callback);
        self.param_tree_observer = Some(observer);
    }
    
    pub fn dump_parameter_tree(&self) {
        match self.unit_query {
            AudioUnitQuery::MusicDevice => (),
            AudioUnitQuery::Effect => (),
            _ => panic!("dump_parameter_tree on this device")
        }
        unsafe {
            let root: ObjcId = msg_send![self.au_audio_unit, parameterTree];
            unsafe fn recur_walk_tree(node: ObjcId, depth: usize) {
                let children: ObjcId = msg_send![node, children];
                let count: usize = msg_send![children, count];
                let param_group: ObjcId = msg_send![class!(AUParameterGroup), class];
                for i in 0..count {
                    let node: ObjcId = msg_send![children, objectAtIndex: i];
                    let class: ObjcId = msg_send![node, class];
                    let display: ObjcId = msg_send![node, displayName];
                    for _ in 0..depth {
                        print!("|  ");
                    }
                    if class == param_group {
                        recur_walk_tree(node, depth + 1);
                    }
                    else {
                        let min: f32 = msg_send![node, minValue];
                        let max: f32 = msg_send![node, maxValue];
                        let value: f32 = msg_send![node, value];
                        log!("{} : min:{} max:{} value:{}", nsstring_to_string(display), min, max, value);
                    }
                }
            }
            recur_walk_tree(root, 0);
        }
    }
    
    pub fn get_instrument_state(&self) -> AudioInstrumentState {
        match self.unit_query {
            AudioUnitQuery::MusicDevice => (),
            _ => panic!("start_audio_output_with_fn on this device")
        }
        unsafe {
            let dict: ObjcId = msg_send![self.au_audio_unit, fullState];
            let all_keys: ObjcId = msg_send![dict, allKeys];
            let count: usize = msg_send![all_keys, count];
            let mut out_state = AudioInstrumentState::default();
            for i in 0..count {
                let key: ObjcId = msg_send![all_keys, objectAtIndex: i];
                let obj: ObjcId = msg_send![dict, objectForKey: key];
                //let class: ObjcId = msg_send![obj, class]; nsstring_to_string(NSStringFromClass(class)),
                let name = nsstring_to_string(key);
                match name.as_ref() {
                    "manufacturer" => out_state.manufacturer = msg_send![obj, unsignedLongLongValue],
                    "subtype" => out_state.subtype = msg_send![obj, unsignedLongLongValue],
                    "version" => out_state.version = msg_send![obj, unsignedLongLongValue],
                    "type" => out_state.ty = msg_send![obj, unsignedLongLongValue],
                    "name" => out_state.name = nsstring_to_string(obj),
                    "data" => {
                        let len: usize = msg_send![obj, length];
                        if len > 0 {
                            let bytes: *const u8 = msg_send![obj, bytes];
                            out_state.data.extend_from_slice(std::slice::from_raw_parts(bytes, len));
                        }
                    }
                    "vstdata" => {
                        let len: usize = msg_send![obj, length];
                        if len > 0 {
                            let bytes: *const u8 = msg_send![obj, bytes];
                            out_state.vstdata.extend_from_slice(std::slice::from_raw_parts(bytes, len));
                            log!("{}", out_state.vstdata.len());
                        }
                    }
                    _ => {
                        eprintln!("Unexpected key in state dictionary {}", name);
                    }
                }
            }
            out_state
        }
    }
    
    pub fn set_instrument_state(&self, in_state: &AudioInstrumentState) {
        match self.unit_query {
            AudioUnitQuery::MusicDevice => (),
            _ => panic!("start_audio_output_with_fn on this device")
        }
        unsafe {
            let dict: ObjcId = msg_send![class!(NSMutableDictionary), dictionary];
            let () = msg_send![dict, init];
            
            unsafe fn set_number(dict: ObjcId, name: &str, value: u64) {
                let id: ObjcId = str_to_nsstring(name);
                let num: ObjcId = msg_send![class!(NSNumber), numberWithLongLong: value];
                let () = msg_send![dict, setObject: num forKey: id];
            }
            unsafe fn set_string(dict: ObjcId, name: &str, value: &str) {
                let id: ObjcId = str_to_nsstring(name);
                let value: ObjcId = str_to_nsstring(value);
                let () = msg_send![dict, setObject: value forKey: id];
            }
            unsafe fn set_data(dict: ObjcId, name: &str, data: &[u8]) {
                let id: ObjcId = str_to_nsstring(name);
                let nsdata: ObjcId = msg_send![class!(NSData), dataWithBytes: data.as_ptr() length: data.len()];
                let () = msg_send![dict, setObject: nsdata forKey: id];
            }
            set_number(dict, "manufacturer", in_state.manufacturer);
            set_number(dict, "subtype", in_state.subtype);
            set_number(dict, "version", in_state.version);
            set_number(dict, "type", in_state.ty);
            set_string(dict, "name", &in_state.name);
            set_data(dict, "data", &in_state.data);
            set_data(dict, "vstdata", &in_state.vstdata);
            
            let () = msg_send![self.au_audio_unit, setFullState: dict];
        }
    }
    
    pub fn set_output_provider<F: Fn(AudioTime, &mut AudioBuffer) + Send + 'static>(&self, audio_callback: F) {
        match self.unit_query {
            AudioUnitQuery::Output => (),
            AudioUnitQuery::Effect => (),
            x => panic!("cannot call set_output_provider on this device {:?}", x)
        }
        
        let buffer = Arc::new(Mutex::new(AudioBuffer::default()));
        let output_provider = objc_block!(
            move | _flags: *mut u32,
            time_stamp: *const AudioTimeStamp,
            frame_count: u32,
            _input_bus_number: u64,
            buffers: *mut AudioBufferList |: i32 {
                let buffers_ref = unsafe {&*buffers};
                let channel_count = buffers_ref.mNumberBuffers as usize;
                let frame_count = frame_count as usize;
                let mut buffer = buffer.lock().unwrap();
                buffer.resize(frame_count, channel_count);
                audio_callback(
                    unsafe {AudioTime {
                        sample_time: (*time_stamp).mSampleTime,
                        host_time: (*time_stamp).mHostTime,
                        rate_scalar: (*time_stamp).mRateScalar
                    }},
                    &mut buffer
                );
                for i in 0..channel_count {
                    let out = unsafe {std::slice::from_raw_parts_mut(buffers_ref.mBuffers[i].mData as *mut f32, frame_count)};
                    out.copy_from_slice(buffer.channel(i));
                }
                
                0
            }
        );
        let () = unsafe {msg_send![self.au_audio_unit, setOutputProvider: &output_provider]};
    }
    
    pub fn clear_output_provider(&mut self) {
        unsafe {
            let () = msg_send![self.au_audio_unit, setOutputProvider: nil];
        }
    }
    
    pub fn set_input_handler<F: Fn(AudioTime, &AudioBuffer) + Send + Sync + 'static>(&self, audio_callback: F) {
        match self.unit_query {
            AudioUnitQuery::Input => (),
            x => panic!("cannot call set_input_handler on this device {:?}", x)
        }
        if let Some(render_block) = self.render_block {
            unsafe {
                let input_handler = objc_block!(
                    move | _flags: *mut u32,
                    time_stamp: *const AudioTimeStamp,
                    frame_count: u32,
                    _input_bus_number: u64 | {
                        let mut buffer = AudioBuffer::new_with_size(frame_count as usize, 2);
                        let mut flags = 0u32;
                        let mut buffer_list = buffer.to_audio_buffer_list();
                        
                        objc_block_invoke!(render_block, invoke(
                            (&mut flags as *mut u32): *mut u32,
                            (time_stamp): *const AudioTimeStamp,
                            (frame_count): u32,
                            (1): u64,
                            (&mut buffer_list as *mut AudioBufferList): *mut AudioBufferList,
                            (nil): ObjcId
                        ) -> i32);
                        // lets sleep
                        audio_callback(AudioTime {
                            sample_time: (*time_stamp).mSampleTime,
                            host_time: (*time_stamp).mHostTime,
                            rate_scalar: (*time_stamp).mRateScalar
                        }, &buffer);
                    }
                );
                let () = msg_send![self.au_audio_unit, setInputHandler: &input_handler];
            }
        }
    }
    
    pub fn clear_input_handler(&mut self) {
        unsafe {
            let () = msg_send![self.au_audio_unit, setInputHandler: nil];
        }
    }
    
    pub fn stop_hardware(&mut self) {
        unsafe {
            let () = msg_send![self.au_audio_unit, stopHardware];
        }
    }
    
    pub fn release_audio_unit(&mut self) {
        unsafe {
            let () = msg_send![self.av_audio_unit, release];
            self.av_audio_unit = nil;
            let () = msg_send![self.au_audio_unit, release];
            self.au_audio_unit = nil;
        }
    }
    
    pub fn request_ui<F: Fn() + Send + 'static>(&self, view_loaded: F) {
        match self.unit_query {
            AudioUnitQuery::MusicDevice => (),
            AudioUnitQuery::Effect => (),
            _ => panic!("request_ui not supported on this device")
        }
        
        let view_controller_arc = self.view_controller.clone();
        
        let view_controller_complete = objc_block!(move | view_controller: ObjcId | {
            *view_controller_arc.lock().unwrap() = Some(view_controller);
            view_loaded();
        });
        
        let () = unsafe {msg_send![self.au_audio_unit, requestViewControllerWithCompletionHandler: &view_controller_complete]};
    }
    
    pub fn open_ui(&self) {
        if let Some(_view_controller) = self.view_controller.lock().unwrap().as_ref() {
            /*let audio_view: ObjcId = unsafe {msg_send![*view_controller, view]};
            let cocoa_app = get_macos_app_global();
            let win_view = cocoa_app.cocoa_windows[0].1;
            let () = unsafe {msg_send![win_view, addSubview: audio_view]};*/
        }
    }
    
    pub fn send_mouse_down(&self) {
        if let Some(_view_controller) = self.view_controller.lock().unwrap().as_ref() {
            unsafe {
                log!("Posting a doubleclick");
                let source = CGEventSourceCreate(1);
                /*
                let pos = NSPoint {x: 600.0, y: 720.0};
                let event = CGEventCreateMouseEvent(source, kCGEventLeftMouseDown, pos, 0);
                CGEventSetIntegerValueField(event, kCGMouseEventClickState, 1);
                CGEventPost(0, event);
                let event = CGEventCreateMouseEvent(source, kCGEventLeftMouseUp, pos, 0);
                CGEventSetIntegerValueField(event, kCGMouseEventClickState, 1);
                CGEventPost(0, event);
                let event = CGEventCreateMouseEvent(source, kCGEventLeftMouseDown, pos, 0);
                CGEventSetIntegerValueField(event, kCGMouseEventClickState, 2);
                CGEventPost(0, event);
                let event = CGEventCreateMouseEvent(source, kCGEventLeftMouseUp, pos, 0);
                CGEventSetIntegerValueField(event, kCGMouseEventClickState, 2);
                CGEventPost(0, event);
                */
                let event = CGEventCreateScrollWheelEvent(source, 0, 1, -24, 0, 0);
                CGEventPost(0, event);
                //CGEventPostToPid(pid, event);
            }
        }
    }
    /*
    pub fn ocr_ui(&self) {
        unsafe {
            let cocoa_app = get_macos_app_global();
            let window = cocoa_app.cocoa_windows[0].0;
            let win_num: u32 = msg_send![window, windowNumber];
            let win_opt = kCGWindowListOptionIncludingWindow;
            let null_rect: NSRect = NSRect {origin: NSPoint {x: f64::INFINITY, y: f64::INFINITY}, size: NSSize {width: 0.0, height: 0.0}};
            let cg_image: ObjcId = CGWindowListCreateImage(null_rect, win_opt, win_num, 1);
            if cg_image == nil {
                error!("Please add 'screen capture' privileges to the compiled binary in the macos settings");
                return
            }
            let handler: ObjcId = msg_send![class!(VNImageRequestHandler), alloc];
            let handler: ObjcId = msg_send![handler, initWithCGImage: cg_image options: nil];
            let start_time = std::time::Instant::now();
            let completion = objc_block!(move | request: ObjcId, error: ObjcId | {
                
                log!("Profile time {}", (start_time.elapsed().as_nanos() as f64) / 1000000f64);
                
                if error != nil {
                    error!("text recognition failed")
                }
                let results: ObjcId = msg_send![request, results];
                let count: usize = msg_send![results, count];
                for i in 0..count {
                    let obj: ObjcId = msg_send![results, objectAtIndex: i];
                    let top_objs: ObjcId = msg_send![obj, topCandidates: 1];
                    let top_obj: ObjcId = msg_send![top_objs, objectAtIndex: 0];
                    let _value: ObjcId = msg_send![top_obj, string];
                    //println!("Found text in UI: {}", nsstring_to_string(value));
                }
            });
            
            let request: ObjcId = msg_send![class!(VNRecognizeTextRequest), alloc];
            let request: ObjcId = msg_send![request, initWithCompletionHandler: &completion];
            let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: request];
            let error: ObjcId = nil;
            let () = msg_send![handler, performRequests: array error: &error];
            if error != nil {
                error!("performRequests failed")
            }
        };
    }*/
    
}

