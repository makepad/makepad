use {
    std::ptr,
    std::mem,
    std::sync::{Arc, Mutex},
    crate::{
        platform::apple::cocoa_delegate::*,
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

impl Into<Midi1Data> for Midi1Note {
    fn into(self) -> Midi1Data {
        Midi1Data {
            input: 0,
            data0: (if self.is_on {0x9}else {0x8} << 4) | self.channel,
            data1: self.note_number,
            data2: self.velocity
        }
    }
}

impl Midi1Data {
    pub fn decode(&self) -> Midi1Event {
        let status = self.data0 >> 4;
        let channel = self.data0 & 0xf;
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
    param_tree_observer: Option<KeyValueObserver>,
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
    
    
}

impl AudioDevice {
    
    pub fn parameter_tree_changed(&mut self, callback: Box<dyn Fn() + Send>) {
        if self.param_tree_observer.is_some() {
            panic!();
        }
        let observer = KeyValueObserver::new(self.au_audio_unit, "parameterTree", callback);
        self.param_tree_observer = Some(observer);
    }
    
    pub fn dump_parameter_tree(&self) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("start_audio_output_with_fn on this device")
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
                        println!("{} : min:{} max:{} value:{}", nsstring_to_string(display), min, max, value);
                    }
                }
            }
            recur_walk_tree(root, 0);
        }
    }
    
    pub fn dump_full_state(&self) {
        match self.device_type {
            AudioDeviceType::Music => (),
            _ => panic!("start_audio_output_with_fn on this device")
        }
        unsafe {
            let state: ObjcId = msg_send![self.au_audio_unit, fullStateForDocument];
            let all_keys: ObjcId = msg_send![state, allKeys];
            let count: usize = msg_send![all_keys, count];
            
            for i in 0..count {
                let key: ObjcId = msg_send![all_keys, objectAtIndex: i];
                let obj: ObjcId = msg_send![state, objectForKey: key];
                let class: ObjcId = msg_send![obj, class];
                let name = nsstring_to_string(key);
                if name == "name" {
                    /*
                    println!(
                        "HERE! {} - {} - {}",
                        nsstring_to_string(key),
                        nsstring_to_string(NSStringFromClass(class)),
                        nsstring_to_string(obj),
                    )*/
                }
            }
        }
    }
    
    
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
    
    pub fn ocr_ui(&self) {
        unsafe {
            let cocoa_app = get_cocoa_app_global();
            let window = cocoa_app.cocoa_windows[0].0;
            let win_num: u32 = msg_send![window, windowNumber];
            let win_opt = kCGWindowListOptionIncludingWindow;
            let null_rect: NSRect = NSRect{origin:NSPoint{x:f64::INFINITY, y:f64::INFINITY}, size:NSSize{width:0.0, height:0.0}};
            let cg_image: ObjcId = CGWindowListCreateImage(null_rect, win_opt, win_num, 1);
            if cg_image == nil{
                println!("Please add 'screen capture' privileges to the compiled binary in the macos settings");
                return
            }
            let handler: ObjcId = msg_send![class!(VNImageRequestHandler), alloc];
            let handler: ObjcId = msg_send![handler, initWithCGImage: cg_image options: nil];
            /*
            let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring("/Users/admin/makepad/test.png")];
            let dst: ObjcId = CGImageDestinationCreateWithURL(url, kUTTypePNG, 1, nil);
            println!("{} {}", dst as u64, nsstring_to_string(kUTTypePNG));
            CGImageDestinationAddImage(dst, cg_image, nil);
            if !CGImageDestinationFinalize(dst) {
                println!("FAILED TO WRITE IMAGE");
            }
            */
            let completion = objc_block!(move | request: ObjcId, error: ObjcId | {
                if error != nil {
                    println!("ERROR")
                }
                let results: ObjcId = msg_send![request, results];
                let count: usize = msg_send![results, count];
                for i in 0..count {
                    let obj: ObjcId = msg_send![results, objectAtIndex: i];
                    let top_objs: ObjcId = msg_send![obj, topCandidates: 1];
                    let top_obj: ObjcId = msg_send![top_objs, objectAtIndex: 0];
                    let value: ObjcId = msg_send![top_obj, string];
                   // println!("{}", nsstring_to_string(value));
                }
            });
            
            let request: ObjcId = msg_send![class!(VNRecognizeTextRequest), alloc];
            let request: ObjcId = msg_send![request, initWithCompletionHandler: &completion];
            let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: request];
            let error: ObjcId = nil;
            let () = msg_send![handler, performRequests: array error: &error];
            if error != nil {
                println!("ERROR")
            }
        };
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
                        param_tree_observer: None,
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