//! System-audio loopback capture with ScreenCaptureKit (macOS 12.3+). Moved from
//! os/apple/audio_tap.rs: the capture feeds the input entry registered for the loopback device
//! (see `audio::AudioUnitAccess`), through a callback of planar samples.

use super::audio::Planar;
use crate::media::{device_kind, AudioDeviceInfo};
use makepad_apple_sys::*;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};

/// Receives the planar samples of a block and the sample rate.
pub type TapFn = Box<dyn FnMut(&Planar, f64) + Send>;

// Loopback device ID - hardcoded for the default system output loopback
pub const LOOPBACK_DEVICE_ID: u64 = 0xAD10_1004_BACD_BAC0;

/// The channels the capture is configured for (`setChannelCount`).
const TAP_CHANNELS: usize = 2;
/// The frames the scratch is allocated for. ScreenCaptureKit names no maximum and hands over
/// about 1024 frames per buffer at the configured 48 kHz; a larger buffer grows the scratch
/// rather than lose the block.
const TAP_FRAMES: usize = 4096;
/// Room for a sample buffer's AudioBufferList, in u64s for its 8-byte alignment: the list header
/// and up to 15 buffers (one per channel when the samples are not interleaved).
const TAP_LIST_WORDS: usize = 32;

/// What a capture block uses: the app's callback and the scratch the block is converted into,
/// allocated when the capture starts, so a block does not allocate. A replaced or stopped
/// capture's run comes back from the slot to the thread that replaced it and is dropped there.
struct TapRun {
    callback: TapFn,
    planar: Planar,
    buffer_list: Vec<u64>,
}

type TapSlot = makepad_lock_free::Slot<TapRun>;

impl TapRun {
    fn new(callback: TapFn) -> TapRun {
        TapRun { callback, planar: Planar::new(TAP_FRAMES, TAP_CHANNELS), buffer_list: vec![0; TAP_LIST_WORDS] }
    }
}

pub struct AudioTapAccess {
    /// The capture's run; the stream's delegate holds a count of it (`Arc::into_raw`).
    run: Arc<TapSlot>,
    stream: Arc<Mutex<Option<ObjcId>>>,
    delegate: Arc<Mutex<Option<ObjcId>>>,
    is_running: Arc<Mutex<bool>>,
}

unsafe impl Send for AudioTapAccess {}
unsafe impl Sync for AudioTapAccess {}

impl AudioTapAccess {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            run: Arc::new(TapSlot::default()),
            stream: Arc::new(Mutex::new(None)),
            delegate: Arc::new(Mutex::new(None)),
            is_running: Arc::new(Mutex::new(false)),
        }))
    }

    /// Returns the loopback device descriptor
    pub fn get_loopback_desc() -> AudioDeviceInfo {
        AudioDeviceInfo {
            id: LOOPBACK_DEVICE_ID,
            kind: device_kind::LOOPBACK,
            // NEVER a default: system-audio capture needs screen-recording
            // privileges and must only ever be an explicit app choice.
            is_default: false,
            has_failed: false,
            channels: 2,
            name: "System Audio (Loopback)".to_string(),
        }
    }

    /// Check if a device_id is our loopback device
    pub fn is_loopback_device(device_id: u64) -> bool {
        device_id == LOOPBACK_DEVICE_ID
    }

    /// Start capturing system audio via ScreenCaptureKit
    pub fn start_capture(&mut self, callback: TapFn) {
        // the capture's run, allocated here; a replaced one is dropped here, not on the queue
        drop(self.run.replace(Some(TapRun::new(callback))));

        // Clone Arcs for the completion handler
        let is_running = self.is_running.clone();
        let stream_arc = self.stream.clone();
        let delegate_arc = self.delegate.clone();
        let run = self.run.clone();

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
                let () = msg_send![config, setChannelCount: TAP_CHANNELS as i32];

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
                let run_for_delegate = run.clone();
                let is_running_for_start = is_running.clone();
                let stream_arc_for_store = stream_arc.clone();
                let delegate_arc_for_store = delegate_arc.clone();

                // Create the stream output delegate
                let delegate_class = delegate_class();
                let delegate: ObjcId = msg_send![delegate_class, alloc];
                let delegate: ObjcId = msg_send![delegate, init];

                // The delegate holds a count of the run's slot, which `AudioTapAccess` also
                // holds, so the slot outlives a stop that races a block on the queue.
                let callback_ptr = Arc::into_raw(run_for_delegate);
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
                    drop(Arc::from_raw(callback_ptr));
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
        // No block uses the run once this returns; it is dropped here.
        drop(self.run.replace(None));
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
                    drop(Arc::from_raw(callback_ptr as *const TapSlot));
                }
                let () = msg_send![delegate, release];
            }
        }

        *self.is_running.lock().unwrap() = false;
    }

    pub fn is_running(&self) -> bool {
        *self.is_running.lock().unwrap()
    }
}

impl Drop for AudioTapAccess {
    fn drop(&mut self) {
        self.stop_capture();
    }
}

// Define the SCStreamOutput delegate class
fn delegate_class() -> *const Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| define_sc_stream_output_delegate() as usize) as *const Class
}

fn define_sc_stream_output_delegate() -> *const Class {
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

            let slot = &*(callback_ptr as *const TapSlot);
            if sample_buffer != nil {
                // a block while the run is replaced or stopped is skipped
                slot.with(|run| run.block(sample_buffer));
            }
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

impl TapRun {
    /// Converts one ScreenCaptureKit sample buffer into the planar scratch and hands it to the
    /// callback.
    unsafe fn block(&mut self, sample_buffer: ObjcId) {
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

        // The AudioBufferList goes into the room set aside at start; past it, grow rather than
        // lose the block.
        let words = audio_buffer_list_size.div_ceil(8);
        if words > self.buffer_list.len() {
            self.buffer_list.resize(words, 0);
        }
        let abl_ptr = self.buffer_list.as_mut_ptr() as *mut AudioBufferList;

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
            return;
        }

        let abl = &*abl_ptr;

        // Check format flags
        let is_float = (asbd.mFormatFlags & 1) != 0; // kAudioFormatFlagIsFloat
        let is_non_interleaved = (asbd.mFormatFlags & 32) != 0; // kAudioFormatFlagIsNonInterleaved

        // The block goes into the scratch: past its room, grow rather than lose the block.
        let samples = frame_count * channel_count;
        if samples > self.planar.data.len() {
            self.planar.data.resize(samples, 0.0);
        }
        self.planar.frames = frame_count;
        self.planar.channels = channel_count;
        self.planar.data[..samples].fill(0.0);
        let audio_buffer = &mut self.planar;

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

        (self.callback)(&self.planar, sample_rate);
    }
}


// CoreMedia/CoreAudio types needed for ScreenCaptureKit audio handling
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

#[repr(C)]
#[allow(non_snake_case)]
pub struct CoreAudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

#[repr(C)]
#[allow(non_snake_case)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [CoreAudioBuffer; 1], // Variable length array, at least 1
}

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
