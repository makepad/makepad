// Audio Tap for macOS - System audio loopback capture using ScreenCaptureKit
// Requires macOS 12.3+ for ScreenCaptureKit audio capture

#[cfg(target_os = "macos")]
use {
    crate::{
        audio::*, makepad_live_id::*, makepad_objc_sys::declare::ClassDecl,
        makepad_objc_sys::objc_block, makepad_objc_sys::runtime::Sel,
        os::apple::apple_classes::get_apple_class_global, os::apple::apple_sys::*,
        thread::SignalToUI,
    },
    std::ffi::c_void,
    std::sync::{Arc, Mutex},
};

// Loopback device ID - hardcoded for the default system output loopback
pub const LOOPBACK_DEVICE_ID: LiveId = LiveId(0xAD10_1004_BACD_BAC0);

#[cfg(target_os = "macos")]
pub struct AudioTapAccess {
    pub change_signal: SignalToUI,
    pub audio_input_cb: Arc<Mutex<Option<AudioInputFn>>>,
    stream: Arc<Mutex<Option<ObjcId>>>,
    delegate: Arc<Mutex<Option<ObjcId>>>,
    is_running: Arc<Mutex<bool>>,
}

#[cfg(target_os = "macos")]
unsafe impl Send for AudioTapAccess {}
#[cfg(target_os = "macos")]
unsafe impl Sync for AudioTapAccess {}

#[cfg(target_os = "macos")]
impl AudioTapAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            change_signal,
            audio_input_cb: Arc::new(Mutex::new(None)),
            stream: Arc::new(Mutex::new(None)),
            delegate: Arc::new(Mutex::new(None)),
            is_running: Arc::new(Mutex::new(false)),
        }))
    }

    /// Returns the loopback device descriptor
    pub fn get_loopback_desc() -> AudioDeviceDesc {
        AudioDeviceDesc {
            device_id: AudioDeviceId(LOOPBACK_DEVICE_ID),
            device_type: AudioDeviceType::Loopback,
            is_default: true,
            has_failed: false,
            channel_count: 2,
            name: "System Audio (Loopback)".to_string(),
        }
    }

    /// Check if a device_id is our loopback device
    pub fn is_loopback_device(device_id: AudioDeviceId) -> bool {
        device_id.0 == LOOPBACK_DEVICE_ID
    }

    /// Start capturing system audio via ScreenCaptureKit
    pub fn start_capture(&mut self, callback: AudioInputFn) {
        // Store the callback
        *self.audio_input_cb.lock().unwrap() = Some(callback);

        // Clone Arcs for the completion handler
        let is_running = self.is_running.clone();
        let stream_arc = self.stream.clone();
        let delegate_arc = self.delegate.clone();
        let audio_input_cb = self.audio_input_cb.clone();

        // ScreenCaptureKit requires async initialization
        unsafe {
            // Get shareable content (displays, windows, apps)
            let completion_handler = objc_block!(move |content: ObjcId, error: ObjcId| {
                if error != nil {
                    let desc: ObjcId = msg_send![error, localizedDescription];
                    let desc_str = nsstring_to_string(desc);
                    println!(
                        "ScreenCaptureKit: Failed to get shareable content: {}",
                        desc_str
                    );
                    return;
                }
                if content == nil {
                    println!("ScreenCaptureKit: Content is nil");
                    return;
                }

                // Get the displays array
                let displays: ObjcId = msg_send![content, displays];
                let display_count: usize = msg_send![displays, count];

                println!("ScreenCaptureKit: Found {} displays", display_count);

                if display_count == 0 {
                    println!("ScreenCaptureKit: No displays found");
                    return;
                }

                // Use the first display
                let display: ObjcId = msg_send![displays, objectAtIndex: 0usize];

                // Create a content filter for the display (captures all audio from this display)
                let filter: ObjcId = msg_send![class!(SCContentFilter), alloc];
                // initWithDisplay:excludingWindows: captures everything on the display
                let empty_array: ObjcId = msg_send![class!(NSArray), array];
                let filter: ObjcId =
                    msg_send![filter, initWithDisplay: display excludingWindows: empty_array];

                if filter == nil {
                    println!("ScreenCaptureKit: Failed to create content filter");
                    return;
                }

                // Create stream configuration
                let config: ObjcId = msg_send![class!(SCStreamConfiguration), alloc];
                let config: ObjcId = msg_send![config, init];

                // Configure for audio-only capture
                let () = msg_send![config, setCapturesAudio: YES];
                let () = msg_send![config, setExcludesCurrentProcessAudio: NO];
                let () = msg_send![config, setSampleRate: 48000i32];
                let () = msg_send![config, setChannelCount: 2i32];

                // Disable video capture to save resources (set minimal size)
                let () = msg_send![config, setWidth: 2u64];
                let () = msg_send![config, setHeight: 2u64];

                // Create the stream
                let stream: ObjcId = msg_send![class!(SCStream), alloc];
                let stream: ObjcId =
                    msg_send![stream, initWithFilter: filter configuration: config delegate: nil];

                if stream == nil {
                    println!("ScreenCaptureKit: Failed to create stream");
                    return;
                }

                println!("ScreenCaptureKit: Stream created successfully");

                // Create a dispatch queue for audio samples
                let queue_name = b"com.makepad.audio_tap\0";
                let queue = dispatch_queue_create(queue_name.as_ptr(), nil);

                // Clone for inner closures
                let audio_cb_for_delegate = audio_input_cb.clone();
                let is_running_for_start = is_running.clone();
                let stream_arc_for_store = stream_arc.clone();
                let delegate_arc_for_store = delegate_arc.clone();

                // Create the stream output delegate
                let delegate_class = get_apple_class_global().sc_stream_output_delegate;
                let delegate: ObjcId = msg_send![delegate_class, alloc];
                let delegate: ObjcId = msg_send![delegate, init];

                // Store the callback pointer in the delegate
                let callback_box = Box::new(audio_cb_for_delegate);
                let callback_ptr = Box::into_raw(callback_box);
                (*delegate).set_ivar("audio_callback", callback_ptr as *mut c_void);

                // Add stream output for audio (type 1 = SCStreamOutputTypeAudio)
                let mut add_error: ObjcId = nil;
                let success: bool = msg_send![stream, addStreamOutput: delegate
                    type: 1i64
                    sampleHandlerQueue: queue
                    error: &mut add_error];

                if !success || add_error != nil {
                    if add_error != nil {
                        let desc: ObjcId = msg_send![add_error, localizedDescription];
                        let desc_str = nsstring_to_string(desc);
                        println!(
                            "ScreenCaptureKit: Failed to add stream output: {}",
                            desc_str
                        );
                    } else {
                        println!("ScreenCaptureKit: Failed to add stream output (unknown error)");
                    }
                    // Clean up callback
                    let _ = Box::from_raw(callback_ptr);
                    return;
                }

                println!("ScreenCaptureKit: Stream output added");

                // Store delegate reference
                *delegate_arc_for_store.lock().unwrap() = Some(delegate);

                // Start the capture
                let start_handler = objc_block!(move |start_error: ObjcId| {
                    if start_error != nil {
                        let desc: ObjcId = msg_send![start_error, localizedDescription];
                        let desc_str = nsstring_to_string(desc);
                        println!("ScreenCaptureKit: Failed to start capture: {}", desc_str);
                    } else {
                        println!("ScreenCaptureKit: Audio capture started successfully!");
                        *is_running_for_start.lock().unwrap() = true;
                    }
                });

                let () = msg_send![stream, startCaptureWithCompletionHandler: &start_handler];

                // Store the stream reference
                *stream_arc_for_store.lock().unwrap() = Some(stream);
            });

            // Request shareable content
            println!("ScreenCaptureKit: Requesting shareable content...");
            let () = msg_send![
                class!(SCShareableContent),
                getShareableContentExcludingDesktopWindows: NO
                onScreenWindowsOnly: NO
                completionHandler: &completion_handler
            ];
        }
    }

    /// Stop capturing
    pub fn stop_capture(&mut self) {
        let stream = self.stream.lock().unwrap().take();
        let delegate = self.delegate.lock().unwrap().take();

        if let Some(stream) = stream {
            unsafe {
                let stop_handler = objc_block!(move |_error: ObjcId| {
                    println!("ScreenCaptureKit: Audio capture stopped");
                });
                let () = msg_send![stream, stopCaptureWithCompletionHandler: &stop_handler];
            }
        }

        // Clean up delegate
        if let Some(delegate) = delegate {
            unsafe {
                let callback_ptr: *mut c_void = *(*delegate).get_ivar("audio_callback");
                if !callback_ptr.is_null() {
                    let _ = Box::from_raw(callback_ptr as *mut Arc<Mutex<Option<AudioInputFn>>>);
                }
                let () = msg_send![delegate, release];
            }
        }

        *self.is_running.lock().unwrap() = false;
        *self.audio_input_cb.lock().unwrap() = None;
    }

    pub fn is_running(&self) -> bool {
        *self.is_running.lock().unwrap()
    }
}

#[cfg(target_os = "macos")]
impl Drop for AudioTapAccess {
    fn drop(&mut self) {
        self.stop_capture();
    }
}

// Define the SCStreamOutput delegate class
#[cfg(target_os = "macos")]
pub fn define_sc_stream_output_delegate() -> *const Class {
    extern "C" fn stream_did_output_sample_buffer(
        this: &Object,
        _sel: Sel,
        _stream: ObjcId,
        sample_buffer: ObjcId,
        output_type: i64,
    ) {
        // Only process audio samples (type 1)
        if output_type != 1 {
            return;
        }

        unsafe {
            let callback_ptr: *mut c_void = *this.get_ivar("audio_callback");
            if callback_ptr.is_null() {
                return;
            }

            let audio_cb = &*(callback_ptr as *const Arc<Mutex<Option<AudioInputFn>>>);
            handle_audio_sample(sample_buffer, audio_cb);
        }
    }

    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MakepadSCStreamOutputDelegate", superclass).unwrap();

    unsafe {
        decl.add_method(
            sel!(stream:didOutputSampleBuffer:ofType:),
            stream_did_output_sample_buffer as extern "C" fn(&Object, Sel, ObjcId, ObjcId, i64),
        );
    }

    decl.add_ivar::<*mut c_void>("audio_callback");

    decl.register()
}

/// Handle incoming audio samples from ScreenCaptureKit
#[cfg(target_os = "macos")]
unsafe fn handle_audio_sample(sample_buffer: ObjcId, audio_cb: &Arc<Mutex<Option<AudioInputFn>>>) {
    if sample_buffer == nil {
        return;
    }

    // Get format description first
    let format_desc: ObjcId = CMSampleBufferGetFormatDescription(sample_buffer);
    if format_desc == nil {
        return;
    }

    let asbd_ptr = CMAudioFormatDescriptionGetStreamBasicDescription(format_desc);
    if asbd_ptr.is_null() {
        return;
    }

    let asbd = &*asbd_ptr;
    let channel_count = asbd.mChannelsPerFrame as usize;
    let sample_rate = asbd.mSampleRate;
    let bits_per_channel = asbd.mBitsPerChannel as usize;

    if channel_count == 0 || bits_per_channel == 0 {
        return;
    }

    // Get the number of sample frames from the sample buffer
    let num_samples = CMSampleBufferGetNumSamples(sample_buffer);
    if num_samples <= 0 {
        return;
    }
    let frame_count = num_samples as usize;

    // Get the audio buffer list - this is the proper way for audio CMSampleBuffers
    let mut audio_buffer_list_size: usize = 0;
    let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sample_buffer,
        &mut audio_buffer_list_size,
        std::ptr::null_mut(),
        0,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        0,
        std::ptr::null_mut(),
    );

    if status != 0 || audio_buffer_list_size == 0 {
        return;
    }

    // Allocate the AudioBufferList
    let layout = std::alloc::Layout::from_size_align(audio_buffer_list_size, 8).unwrap();
    let abl_ptr = std::alloc::alloc(layout) as *mut AudioBufferList;
    if abl_ptr.is_null() {
        return;
    }

    let mut block_buffer: ObjcId = nil;
    let status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sample_buffer,
        std::ptr::null_mut(),
        abl_ptr,
        audio_buffer_list_size,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        0,
        &mut block_buffer,
    );

    if status != 0 {
        std::alloc::dealloc(abl_ptr as *mut u8, layout);
        return;
    }

    let abl = &*abl_ptr;

    // Check format flags
    let is_float = (asbd.mFormatFlags & 1) != 0; // kAudioFormatFlagIsFloat
    let is_non_interleaved = (asbd.mFormatFlags & 32) != 0; // kAudioFormatFlagIsNonInterleaved

    let mut audio_buffer = AudioBuffer::new_with_size(frame_count, channel_count);

    if is_non_interleaved {
        // Non-interleaved: each buffer in the list contains one channel
        for ch in 0..channel_count.min(abl.mNumberBuffers as usize) {
            let buf = &*abl.mBuffers.as_ptr().add(ch);
            if buf.mData.is_null() {
                continue;
            }

            let samples_in_buffer = buf.mDataByteSize as usize / (bits_per_channel / 8);
            let samples_to_copy = samples_in_buffer.min(frame_count);

            if is_float {
                let float_data =
                    std::slice::from_raw_parts(buf.mData as *const f32, samples_to_copy);
                audio_buffer.channel_mut(ch)[..samples_to_copy].copy_from_slice(float_data);
            } else {
                // 16-bit integer
                let i16_data = std::slice::from_raw_parts(buf.mData as *const i16, samples_to_copy);
                for (i, &sample) in i16_data.iter().enumerate() {
                    audio_buffer.channel_mut(ch)[i] = sample as f32 / 32768.0;
                }
            }
        }
    } else {
        // Interleaved: single buffer with interleaved channels
        if abl.mNumberBuffers >= 1 {
            let buf = &*abl.mBuffers.as_ptr();
            if !buf.mData.is_null() {
                let total_samples = buf.mDataByteSize as usize / (bits_per_channel / 8);
                let expected_samples = frame_count * channel_count;
                let samples_to_use = total_samples.min(expected_samples);

                if is_float {
                    let float_data =
                        std::slice::from_raw_parts(buf.mData as *const f32, samples_to_use);
                    // De-interleave
                    for frame in 0..(samples_to_use / channel_count) {
                        for ch in 0..channel_count {
                            audio_buffer.channel_mut(ch)[frame] =
                                float_data[frame * channel_count + ch];
                        }
                    }
                } else {
                    // 16-bit integer interleaved
                    let i16_data =
                        std::slice::from_raw_parts(buf.mData as *const i16, samples_to_use);
                    for frame in 0..(samples_to_use / channel_count) {
                        for ch in 0..channel_count {
                            let sample = i16_data[frame * channel_count + ch];
                            audio_buffer.channel_mut(ch)[frame] = sample as f32 / 32768.0;
                        }
                    }
                }
            }
        }
    }

    // Clean up
    if block_buffer != nil {
        CFRelease(block_buffer as *const c_void);
    }
    std::alloc::dealloc(abl_ptr as *mut u8, layout);

    // Call the callback - try_lock to avoid blocking the audio thread
    if let Ok(mut guard) = audio_cb.try_lock() {
        if let Some(cb) = &mut *guard {
            cb(
                AudioInfo {
                    device_id: AudioDeviceId(LOOPBACK_DEVICE_ID),
                    time: None,
                    sample_rate,
                },
                &audio_buffer,
            );
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn nsstring_to_string(ns_string: ObjcId) -> String {
    if ns_string == nil {
        return String::new();
    }
    let utf8_ptr: *const u8 = msg_send![ns_string, UTF8String];
    if utf8_ptr.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(utf8_ptr as *const i8)
        .to_string_lossy()
        .into_owned()
}

// CoreMedia/CoreAudio types needed for ScreenCaptureKit audio handling
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[allow(non_snake_case)]
pub struct AudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: u32,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[allow(non_snake_case)]
pub struct CoreAudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[allow(non_snake_case)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [CoreAudioBuffer; 1], // Variable length array, at least 1
}

#[cfg(target_os = "macos")]
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetFormatDescription(sbuf: ObjcId) -> ObjcId;
    fn CMSampleBufferGetNumSamples(sbuf: ObjcId) -> i64;
    fn CMAudioFormatDescriptionGetStreamBasicDescription(
        desc: ObjcId,
    ) -> *const AudioStreamBasicDescription;
    fn CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sbuf: ObjcId,
        buffer_list_size_needed_out: *mut usize,
        buffer_list_out: *mut AudioBufferList,
        buffer_list_size: usize,
        block_buffer_structure_allocator: ObjcId,
        block_buffer_block_allocator: ObjcId,
        flags: u32,
        block_buffer_out: *mut ObjcId,
    ) -> i32;
}

// CFRelease is imported from apple_sys
