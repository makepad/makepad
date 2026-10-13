//! CoreAudio devices and streams: the device list, input and output units, and the realtime
//! callbacks, which call the entry the app registered for the slot (`AudioEntry`) with one
//! planar block (`media::AudioBlock`).
//!
//! On macOS the devices are CoreAudio's (`AudioObject`s, each opened through the HAL output
//! unit) plus the ScreenCaptureKit loopback; on iOS and tvOS there is one default input and one
//! default output through the shared `AVAudioSession` (RemoteIO / VoiceProcessingIO units). iOS
//! configures the session lazily: playback-only until an input is used, then PlayAndRecord.
//!
//! Moved from os/apple/audio_unit.rs. The program owns the render function: the HAL
//! owns device enumeration, opening and closing. The unit hosting of audio plug-ins
//! (`new_audio_plugin`, instrument state, plug-in UI) has no caller in the tree and is not part
//! of the move. Callbacks hold their slot's lock while they run (as the closure slots did), so
//! replacing an entry returns only when the old one is no longer running.

#[cfg(target_os = "macos")]
use super::audio_tap::AudioTapAccess;
#[cfg(target_os = "macos")]
use super::util::cfstring_ref_to_string;
use super::util::OSError;
use super::Signal;
use crate::media::{audio_option, device_kind, name_hash, AudioBlock, AudioDeviceInfo, AudioFn};
use makepad_apple_sys::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Slots per direction (the index of `AudioEntry`).
pub const MAX_AUDIO_DEVICE_INDEX: usize = 32;

/// A registered entry.
#[derive(Clone, Copy)]
pub struct Entry {
    pub f: AudioFn,
    pub ctx: usize,
}

/// The entry of one device slot. Locked while the entry runs.
pub type Slot = Arc<Mutex<Option<Entry>>>;

/// Planar samples: `channels` runs of `frames` floats.
pub struct Planar {
    pub data: Vec<f32>,
    pub frames: usize,
    pub channels: usize,
}

impl Planar {
    pub fn new(frames: usize, channels: usize) -> Planar {
        Planar { data: vec![0.0; frames * channels], frames, channels }
    }
    pub fn channel_mut(&mut self, c: usize) -> &mut [f32] {
        &mut self.data[c * self.frames..(c + 1) * self.frames]
    }
}

/// Calls the input entry of `slot` with `buffer`, when there is one.
pub fn call_input(slot: &Slot, device: u64, time: Option<(f64, u64, f64)>, sample_rate: f64, buffer: &Planar) {
    let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = *guard else { return };
    let mut planes: Vec<*mut f32> = (0..buffer.channels).map(|c| buffer.data[c * buffer.frames..].as_ptr() as *mut f32).collect();
    let (has_time, sample_time, host_time, rate_scalar) = match time {
        Some((s, h, r)) => (1, s, h, r),
        None => (0, 0.0, 0, 0.0),
    };
    let mut block = AudioBlock { device, sample_rate, sample_time, host_time, rate_scalar, has_time, frames: buffer.frames as u32, channels: buffer.channels as u32, planes: planes.as_mut_ptr() };
    (entry.f)(entry.ctx as *mut std::ffi::c_void, &mut block);
}

pub struct AudioUnitAccess {
    pub change_signal: Signal,
    pub device_descs: Vec<CoreAudioDeviceDesc>,
    pub input_cb: [Slot; MAX_AUDIO_DEVICE_INDEX],
    pub output_cb: [Slot; MAX_AUDIO_DEVICE_INDEX],
    pub audio_inputs: Arc<Mutex<Vec<RunningAudioUnit>>>,
    pub audio_outputs: Arc<Mutex<Vec<RunningAudioUnit>>>,
    failed_devices: Arc<Mutex<HashSet<u64>>>,
    /// Set when a VoiceProcessingIO input unit fails to initialize: later echo-cancellation
    /// requests degrade to plain capture instead of repeatedly failing (the DEVICE is fine,
    /// the unit type is not).
    voice_input_unusable: Arc<AtomicBool>,
    /// Options the running inputs were created with; a change rebuilds them.
    last_input_options: u32,
    #[cfg(target_os = "macos")]
    audio_tap: Option<Arc<Mutex<AudioTapAccess>>>,
    #[cfg(target_os = "ios")]
    ios_session_state: IosSessionState,
}

/// How the shared `AVAudioSession` is configured on iOS: set (or upgraded) only when the app
/// uses an output or an input, so a playback-only app never gets PlayAndRecord (which prompts
/// for the microphone and forces voice-chat mode).
#[cfg(target_os = "ios")]
#[derive(Default, Clone, Copy, PartialEq)]
enum IosSessionState {
    #[default]
    Inactive,
    PlaybackOnly,
    PlayAndRecord,
}

pub struct CoreAudioDeviceDesc {
    pub core_device_id: AudioDeviceID,
    pub desc: AudioDeviceInfo,
}

pub struct RunningAudioUnit {
    device_id: u64,
    audio_unit: Option<AudioUnit>,
}

impl AudioUnitAccess {
    pub fn new(change_signal: Signal) -> Arc<Mutex<Self>> {
        Self::observe_route_changes(change_signal.clone());
        Arc::new(Mutex::new(Self {
            failed_devices: Default::default(),
            voice_input_unusable: Default::default(),
            last_input_options: 0,
            change_signal,
            device_descs: Default::default(),
            input_cb: Default::default(),
            output_cb: Default::default(),
            audio_inputs: Default::default(),
            audio_outputs: Default::default(),
            #[cfg(target_os = "macos")]
            audio_tap: Some(AudioTapAccess::new()),
            #[cfg(target_os = "ios")]
            ios_session_state: IosSessionState::Inactive,
        }))
    }

    /// The device list as the system has it now, with the loopback device on macOS.
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceInfo> {
        let _ = self.update_device_list();
        #[allow(unused_mut)]
        let mut out: Vec<AudioDeviceInfo> = self.device_descs.iter().map(|d| d.desc.clone()).collect();
        #[cfg(target_os = "macos")]
        out.push(AudioTapAccess::get_loopback_desc());
        out
    }

    /// iOS: speaker unless something external (headphones, Bluetooth, AirPlay) is connected, so
    /// PlayAndRecord does not play through the earpiece.
    #[cfg(target_os = "ios")]
    fn update_ios_audio_route() {
        unsafe {
            let session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];
            let current_route: ObjcId = msg_send![session, currentRoute];
            let outputs: ObjcId = msg_send![current_route, outputs];
            let count: usize = msg_send![outputs, count];
            let mut has_external_audio = false;
            for i in 0..count {
                let output: ObjcId = msg_send![outputs, objectAtIndex: i];
                let port_type: ObjcId = msg_send![output, portType];
                let port_type = nsstring_to_string(port_type);
                if matches!(port_type.as_str(), "BluetoothA2DPOutput" | "BluetoothHFP" | "BluetoothLE" | "HeadsetMic" | "Headphones" | "AirPlay") {
                    has_external_audio = true;
                    break;
                }
            }
            let mut error: ObjcId = nil;
            // AVAudioSessionPortOverrideNone / AVAudioSessionPortOverrideSpeaker ('spkr')
            let port: u32 = if has_external_audio { 0 } else { 1936747378 };
            let () = msg_send![session, overrideOutputAudioPort: port error: &mut error];
        }
    }

    /// iOS: configures and activates the shared `AVAudioSession` for output (`for_input`
    /// false) or input. Never downgrades a PlayAndRecord session.
    #[cfg(target_os = "ios")]
    fn ensure_ios_session(&mut self, for_input: bool) {
        let target = if for_input { IosSessionState::PlayAndRecord } else { IosSessionState::PlaybackOnly };
        if self.ios_session_state == IosSessionState::PlayAndRecord || self.ios_session_state == target {
            return;
        }
        unsafe {
            let session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];
            let mut error: ObjcId = nil;
            if for_input {
                let _: BOOL = msg_send![
                    session,
                    setCategory: AVAudioSessionCategoryPlayAndRecord
                    withOptions: AVAudioSessionCategoryOption::DefaultToSpeaker as usize | AVAudioSessionCategoryOption::AllowBluetooth as usize
                    error: &mut error
                ];
                // voice-chat mode pairs with VoiceProcessingIO (echo cancellation, AGC, NS)
                let mode: ObjcId = str_to_nsstring("AVAudioSessionModeVoiceChat");
                let _: BOOL = msg_send![session, setMode: mode error: &mut error];
            } else {
                // playback only: no microphone, no permission prompt
                let _: BOOL = msg_send![session, setCategory: AVAudioSessionCategoryPlayback withOptions: 0usize error: &mut error];
            }
            // 48 kHz avoids 48k/44.1k drift on AirPods (the system may still pick another rate)
            let _: BOOL = msg_send![session, setPreferredSampleRate: 48000.0f64 error: &mut error];
            let _: BOOL = msg_send![session, setPreferredIOBufferDuration: 0.005f64 error: &mut error];
            let _: BOOL = msg_send![session, setActive: YES error: &mut error];
        }
        Self::update_ios_audio_route();
        self.ios_session_state = target;
    }

    /// Opens the input devices in `devices` and closes the other inputs. `options` is
    /// `audio_option::*`.
    pub fn use_audio_inputs(&mut self, devices: &[u64], options: u32) {
        #[cfg(target_os = "ios")]
        self.ensure_ios_session(true);

        // Capture options changed (e.g. echo cancellation toggled while the
        // assistant speaks): tear down running inputs so they rebuild with
        // the new unit type. Keyed globally — captures share one options set.
        if self.last_input_options != options && !devices.is_empty() {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            for input in audio_inputs.iter_mut() {
                if let Some(audio_unit) = &mut input.audio_unit {
                    audio_unit.stop_hardware();
                    audio_unit.clear_input_handler();
                    audio_unit.release_audio_unit();
                }
            }
            audio_inputs.clear();
        }
        self.last_input_options = options;

        // the loopback device is ScreenCaptureKit's (macOS)
        #[cfg(target_os = "macos")]
        {
            let has_loopback = devices
                .iter()
                .any(|d| AudioTapAccess::is_loopback_device(*d));
            if let Some(ref audio_tap) = self.audio_tap {
                let mut tap = audio_tap.lock().unwrap();
                if has_loopback && !tap.is_running() {
                    // Find the index for loopback in the devices array
                    if let Some((index, _)) = devices
                        .iter()
                        .enumerate()
                        .find(|(_, d)| AudioTapAccess::is_loopback_device(**d))
                    {
                        let slot = self.input_cb[index].clone();
                        tap.start_capture(Box::new(move |buffer, sample_rate| {
                            call_input(&slot, super::audio_tap::LOOPBACK_DEVICE_ID, None, sample_rate, buffer);
                        }));
                    }
                } else if !has_loopback && tap.is_running() {
                    tap.stop_capture();
                }
            }
        }

        #[cfg(target_os = "macos")]
        let devices: Vec<u64> = devices
            .iter()
            .filter(|d| !AudioTapAccess::is_loopback_device(**d))
            .copied()
            .collect();

        let new = {
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.retain_mut(|v| {
                if devices.contains(&v.device_id) {
                    true
                } else if let Some(audio_unit) = &mut v.audio_unit {
                    audio_unit.stop_hardware();
                    audio_unit.clear_input_handler();
                    audio_unit.release_audio_unit();
                    false
                } else {
                    false
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_inputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    new.push((index, *device_id))
                }
            }
            for (_, device_id) in &new {
                audio_inputs.push(RunningAudioUnit {
                    device_id: *device_id,
                    audio_unit: None,
                });
            }
            new
        };
        for (index, device_id) in new {
            // lets create an audio input
            let use_voice = options & audio_option::ECHO_CANCELLATION != 0
                && !self.voice_input_unusable.load(Ordering::Relaxed);
            let query = if use_voice {
                AudioUnitQuery::VoiceInput
            } else {
                AudioUnitQuery::Input
            };
            let unit_info = &AudioUnitAccess::query_audio_units(query)[0];
            let audio_inputs = self.audio_inputs.clone();
            let slot = self.input_cb[index].clone();
            let failed_devices = self.failed_devices.clone();
            let voice_input_unusable = self.voice_input_unusable.clone();
            let change_signal = self.change_signal.clone();
            self.new_audio_io(
                self.change_signal.clone(),
                unit_info,
                device_id,
                move |result| {
                    let mut audio_inputs = audio_inputs.lock().unwrap();
                    match result {
                        Ok(audio_unit) => {
                            let running = audio_inputs
                                .iter_mut()
                                .find(|v| v.device_id == device_id)
                                .unwrap();
                            let sample_rate = audio_unit.sample_rate;
                            audio_unit.set_input_handler(slot.clone(), device_id, sample_rate);
                            running.audio_unit = Some(audio_unit);
                        }
                        Err(err) => {
                            if use_voice {
                                // The voice-processing unit failed, not the
                                // device: degrade to plain capture. The
                                // change signal re-runs device selection,
                                // which recreates this input without VPIO.
                                voice_input_unusable.store(true, Ordering::Relaxed);
                                audio_inputs.retain(|v| v.device_id != device_id);
                                change_signal.set();
                                eprintln!(
                                    "voice-processing input failed ({:?}); falling back to plain capture",
                                    err
                                );
                            } else {
                                failed_devices.lock().unwrap().insert(device_id);
                                change_signal.set();
                                audio_inputs.retain(|v| v.device_id != device_id);
                                eprintln!("spawn_audio_input Error {:?}", err)
                            }
                        }
                    }
                },
            )
        }
    }


    pub fn use_audio_outputs(&mut self, devices: &[u64]) {
        #[cfg(target_os = "ios")]
        self.ensure_ios_session(false);
        let new = {
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.retain_mut(|v| {
                if devices.contains(&v.device_id) {
                    true
                } else if let Some(audio_unit) = &mut v.audio_unit {
                    audio_unit.stop_hardware();
                    audio_unit.clear_output_provider();
                    audio_unit.release_audio_unit();
                    false
                } else {
                    false
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                if audio_outputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    new.push((index, *device_id))
                }
            }
            for (_, device_id) in &new {
                audio_outputs.push(RunningAudioUnit {
                    device_id: *device_id,
                    audio_unit: None,
                });
            }
            new
        };
        for (index, device_id) in new {
            let unit_info = &AudioUnitAccess::query_audio_units(AudioUnitQuery::Output)[0];
            let audio_outputs = self.audio_outputs.clone();
            let slot = self.output_cb[index].clone();
            let failed_devices = self.failed_devices.clone();
            let change_signal = self.change_signal.clone();
            self.new_audio_io(
                self.change_signal.clone(),
                unit_info,
                device_id,
                move |result| {
                    let mut audio_outputs = audio_outputs.lock().unwrap();
                    match result {
                        Ok(audio_unit) => {
                            let running = audio_outputs
                                .iter_mut()
                                .find(|v| v.device_id == device_id)
                                .unwrap();
                            let sample_rate = audio_unit.sample_rate;
                            audio_unit.set_output_provider(slot.clone(), device_id, sample_rate);
                            running.audio_unit = Some(audio_unit);
                        }
                        Err(_err) => {
                            failed_devices.lock().unwrap().insert(device_id);
                            change_signal.set();
                            audio_outputs.retain(|v| v.device_id != device_id);
                            //eprintln!("spawn_audio_output Error {:?}", err)
                        }
                    }
                },
            )
        }
    }


    pub fn observe_route_changes(device_change: Signal) {
        let center: ObjcId = unsafe { msg_send![class!(NSNotificationCenter), defaultCenter] };
        unsafe {
            let mut err: ObjcId = nil;
            let audio_session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];
            // Do not force MultiRoute; keep the app's configured category/mode active
            let () = msg_send![audio_session, setActive: true error: &mut err];
        }
        let block = objc_block!(move |_note: ObjcId| {
            #[cfg(target_os = "ios")]
            AudioUnitAccess::update_ios_audio_route();
            device_change.set();
        });
        let () = unsafe {
            msg_send![
                center,
                addObserverForName: AVAudioSessionRouteChangeNotification
                object: nil
                queue: nil
                usingBlock: &block
            ]
        };
    }

    #[cfg(target_os = "macos")]
    pub fn observe_audio_unit_termination(
        change_signal: Signal,
        core_device_id: AudioDeviceID,
    ) {
        #[allow(non_snake_case)]
        unsafe extern "system" fn listener_fn(
            _inObjectID: AudioObjectID,
            _inNumberAddresses: u32,
            _inAddresses: *const AudioObjectPropertyAddress,
            inClientData: *mut (),
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

        let result = unsafe {
            AudioObjectAddPropertyListener(
                core_device_id,
                &prop_addr,
                Some(listener_fn),
                Box::into_raw(Box::new(change_signal)) as *mut _,
            )
        };

        if result != 0 {
            println!("Error in adding DeviceIsAlive listener");
        }
    }


    /// iOS / tvOS: the session's default input (mono) and default output.
    #[cfg(any(target_os = "ios", target_os = "tvos"))]
    pub fn update_device_list(&mut self) -> Result<(), OSError> {
        self.device_descs.clear();
        let failed = self.failed_devices.lock().unwrap_or_else(|e| e.into_inner());
        for (name, id, kind, channels) in [("Default Input", "default_input", device_kind::INPUT, 1), ("Default Output", "default_output", device_kind::OUTPUT, 2)] {
            let id = name_hash(id);
            self.device_descs.push(CoreAudioDeviceDesc {
                core_device_id: 0,
                desc: AudioDeviceInfo { has_failed: failed.contains(&id), id, kind, channels, is_default: true, name: name.to_string() },
            });
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn update_device_list(&mut self) -> Result<(), OSError> {
        self.device_descs.clear();

        fn get_value<T: Sized>(
            device_id: AudioDeviceID,
            selector: AudioObjectPropertySelector,
            scope: AudioObjectPropertyScope,
            default: T,
        ) -> Result<T, OSError> {
            let prop_addr = AudioObjectPropertyAddress {
                mSelector: selector,
                mScope: scope,
                mElement: AudioObjectPropertyElement::Master,
            };

            let value = default;
            let data_size = std::mem::size_of::<T>();
            OSError::from(unsafe {
                AudioObjectGetPropertyData(
                    device_id,
                    &prop_addr,
                    0,
                    std::ptr::null(),
                    &data_size as *const _ as *mut _,
                    &value as *const _ as *mut _,
                )
            })?;
            Ok(value)
        }

        fn get_array<T: Sized>(
            device_id: AudioDeviceID,
            selector: AudioObjectPropertySelector,
            scope: AudioObjectPropertyScope,
        ) -> Result<Vec<T>, OSError> {
            let prop_addr = AudioObjectPropertyAddress {
                mSelector: selector,
                mScope: scope,
                mElement: AudioObjectPropertyElement::Master,
            };
            let data_size = 0u32;
            OSError::from(unsafe {
                AudioObjectGetPropertyDataSize(
                    device_id,
                    &prop_addr as *const _,
                    0,
                    std::ptr::null(),
                    &data_size as *const _ as *mut _,
                )
            })?;

            let mut buf: Vec<T> = vec![];
            let elements = data_size as usize / std::mem::size_of::<T>();
            buf.reserve_exact(elements);
            OSError::from(unsafe {
                AudioObjectGetPropertyData(
                    device_id,
                    &prop_addr as *const _,
                    0,
                    std::ptr::null(),
                    &data_size as *const _ as *mut _,
                    buf.as_mut_ptr() as *mut _,
                )
            })?;
            unsafe { buf.set_len(elements) };
            Ok(buf)
        }

        let default_input_id: AudioDeviceID = get_value(
            kAudioObjectSystemObject,
            AudioObjectPropertySelector::DefaultInputDevice,
            AudioObjectPropertyScope::Global,
            0,
        )
        .unwrap();
        let default_output_id: AudioDeviceID = get_value(
            kAudioObjectSystemObject,
            AudioObjectPropertySelector::DefaultOutputDevice,
            AudioObjectPropertyScope::Global,
            0,
        )
        .unwrap();

        let core_device_ids: Vec<AudioDeviceID> = get_array(
            kAudioObjectSystemObject,
            AudioObjectPropertySelector::Devices,
            AudioObjectPropertyScope::Global,
        )?;

        for core_device_id in core_device_ids {
            let device_name: Result<CFStringRef, OSError> = get_value(
                core_device_id,
                AudioObjectPropertySelector::DeviceNameCFString,
                AudioObjectPropertyScope::Output,
                std::ptr::null(),
            );
            if device_name.is_err() {
                println!("Error getting name for device {}", core_device_id);
                continue;
            }

            let device_name = device_name.unwrap();
            let device_name = unsafe { cfstring_ref_to_string(device_name) };

            // lets probe input/outputness
            fn probe_channels(
                device_id: AudioDeviceID,
                scope: AudioObjectPropertyScope,
            ) -> Result<usize, OSError> {
                let buffer_data: Vec<u8> = get_array(
                    device_id,
                    AudioObjectPropertySelector::StreamConfiguration,
                    scope,
                )?;
                let audio_buffer_list =
                    unsafe { &*(buffer_data.as_ptr() as *const AudioBufferList) };
                if audio_buffer_list.mNumberBuffers == 0 {
                    return Ok(0);
                }
                let mut channels = 0;
                for i in 0..audio_buffer_list.mNumberBuffers as usize {
                    channels += audio_buffer_list.mBuffers[i].mNumberChannels as usize;
                }
                Ok(channels)
            }

            let input_channels = probe_channels(core_device_id, AudioObjectPropertyScope::Input)?;
            let output_channels = probe_channels(core_device_id, AudioObjectPropertyScope::Output)?;

            if input_channels > 0 {
                let device_id = name_hash(&format!("{} {} input", device_name, core_device_id));
                self.device_descs.push(CoreAudioDeviceDesc {
                    core_device_id,
                    desc: AudioDeviceInfo {
                        has_failed: self.failed_devices.lock().unwrap().contains(&device_id),
                        id: device_id,
                        kind: device_kind::INPUT,
                        channels: input_channels as u32,
                        is_default: core_device_id == default_input_id,
                        name: device_name.clone(),
                    },
                })
            }
            if output_channels > 0 {
                let device_id = name_hash(&format!("{} {} output", device_name, core_device_id));
                self.device_descs.push(CoreAudioDeviceDesc {
                    core_device_id,
                    desc: AudioDeviceInfo {
                        has_failed: self.failed_devices.lock().unwrap().contains(&device_id),
                        id: device_id,
                        kind: device_kind::OUTPUT,
                        channels: input_channels as u32,
                        is_default: core_device_id == default_output_id,
                        name: device_name.clone(),
                    },
                })
            }
        }
        Ok(())
    }
    pub fn query_audio_units(unit_query: AudioUnitQuery) -> Vec<AudioUnitInfo> {
        let desc = match unit_query {
            #[cfg(target_os = "macos")]
            AudioUnitQuery::Output | AudioUnitQuery::Input => AudioComponentDescription::new_all_manufacturers(AudioUnitType::IO, AudioUnitSubType::HalOutput),
            // iOS plays through the voice-processing unit too (the session's voice-chat mode)
            #[cfg(target_os = "ios")]
            AudioUnitQuery::Output => AudioComponentDescription::new_all_manufacturers(AudioUnitType::IO, AudioUnitSubType::VoiceProcessingIO),
            #[cfg(target_os = "tvos")]
            AudioUnitQuery::Output => AudioComponentDescription::new_all_manufacturers(AudioUnitType::IO, AudioUnitSubType::RemoteIO),
            #[cfg(any(target_os = "ios", target_os = "tvos"))]
            AudioUnitQuery::Input => AudioComponentDescription::new_all_manufacturers(AudioUnitType::IO, AudioUnitSubType::VoiceProcessingIO),
            // Voice-processing capture (echo cancellation): on macOS it swaps HAL for VPIO, which
            // cancels device playback out of the mic system-wide.
            AudioUnitQuery::VoiceInput => AudioComponentDescription::new_all_manufacturers(AudioUnitType::IO, AudioUnitSubType::VoiceProcessingIO),
        };
        unsafe {
            let manager: ObjcId = msg_send![class!(AVAudioUnitComponentManager), sharedAudioUnitComponentManager];
            let components: ObjcId = msg_send![manager, componentsMatchingDescription: desc];
            let count: usize = msg_send![components, count];
            let mut out = Vec::new();
            for i in 0..count {
                let component: ObjcId = msg_send![components, objectAtIndex: i];
                let desc: AudioComponentDescription = msg_send!(component, audioComponentDescription);
                out.push(AudioUnitInfo { unit_query, desc });
            }
            out
        }
    }

    pub fn new_audio_io<F: Fn(Result<AudioUnit, String>) + Send + 'static>(
        &self,
        change_signal: Signal,
        unit_info: &AudioUnitInfo,
        device_id: u64,
        unit_callback: F,
    ) {
        let unit_query = unit_info.unit_query;

        let core_device_id = self
            .device_descs
            .iter()
            .find(|v| v.desc.id == device_id)
            .unwrap()
            .core_device_id;

        let instantiation_handler = objc_block!(move |av_audio_unit: ObjcId, error: ObjcId| {
            let () = unsafe { msg_send![av_audio_unit, retain] };
            unsafe fn inner(
                _change_signal: Signal,
                core_device_id: AudioDeviceID,
                av_audio_unit: ObjcId,
                error: ObjcId,
                unit_query: AudioUnitQuery,
            ) -> Result<AudioUnit, OSError> {
                OSError::from_nserror(error)?;
                let au_audio_unit: ObjcId = msg_send![av_audio_unit, AUAudioUnit];

                // Configure stream format before allocating resources
                let mut render_block = None;
                // the iOS / tvOS microphone is mono
                let mut input_channels: usize = if cfg!(target_os = "macos") { 2 } else { 1 };

                let mut stream_desc = CAudioStreamBasicDescription {
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
                    mReserved: 0,
                };

                // the device's nominal sample rate (macOS), the session's rate (iOS / tvOS)
                #[cfg(any(target_os = "ios", target_os = "tvos"))]
                {
                    let _ = core_device_id;
                    let session: ObjcId = msg_send![class!(AVAudioSession), sharedInstance];
                    let sample_rate: f64 = msg_send![session, sampleRate];
                    if sample_rate > 0.0 {
                        stream_desc.mSampleRate = sample_rate;
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    let prop_addr = AudioObjectPropertyAddress {
                        mSelector: AudioObjectPropertySelector::NominalSampleRate,
                        mScope: AudioObjectPropertyScope::Global,
                        mElement: AudioObjectPropertyElement::Master,
                    };
                    let mut sample_rate: f64 = 48000.0;
                    let data_size = std::mem::size_of::<f64>();
                    let result = unsafe {
                        AudioObjectGetPropertyData(
                            core_device_id,
                            &prop_addr,
                            0,
                            std::ptr::null(),
                            &data_size as *const _ as *mut _,
                            &mut sample_rate as *mut f64 as *mut _,
                        )
                    };
                    if result == 0 && sample_rate > 0.0 {
                        stream_desc.mSampleRate = sample_rate;
                    }
                }

                let av_audio_format: ObjcId = msg_send![class!(AVAudioFormat), alloc];
                let () = msg_send![av_audio_format, initWithStreamDescription: &stream_desc];

                match unit_query {
                    AudioUnitQuery::Output => {
                        let mut err: ObjcId = nil;
                        let () = msg_send![au_audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                        OSError::from_nserror(err)?;
                        let busses: ObjcId = msg_send![au_audio_unit, inputBusses];
                        let count: usize = msg_send![busses, count];
                        if count > 0 {
                            let bus: ObjcId = msg_send![busses, objectAtIndexedSubscript: 0];
                            let mut err: ObjcId = nil;
                            let () = msg_send![bus, setFormat: av_audio_format error: &mut err];
                            OSError::from_nserror(err)?;
                            let () = msg_send![bus, setEnabled: true];
                        }

                        let () = msg_send![au_audio_unit, setOutputEnabled: false];
                        let () = msg_send![au_audio_unit, setInputEnabled: true];
                        #[cfg(target_os = "macos")]
                        {
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, setDeviceID: core_device_id error: &mut err];
                            OSError::from_nserror(err)?;
                        }

                        let () = msg_send![au_audio_unit, setOutputEnabled: true];
                        let () = msg_send![au_audio_unit, setInputEnabled: false];
                        #[cfg(target_os = "macos")]
                        {
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, setDeviceID: core_device_id error: &mut err];
                            OSError::from_nserror(err)?;
                        }
                        // Ensure final state for output units before allocation
                        let () = msg_send![au_audio_unit, setOutputEnabled: true];
                        let () = msg_send![au_audio_unit, setInputEnabled: false];
                        #[cfg(target_os = "macos")]
                        {
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, setDeviceID: core_device_id error: &mut err];
                            OSError::from_nserror(err)?;
                        }
                        let () = msg_send![au_audio_unit, startHardwareAndReturnError: &mut err];
                        OSError::from_nserror(err)?;
                    }
                    AudioUnitQuery::Input | AudioUnitQuery::VoiceInput => {
                        // the iOS / tvOS microphone is mono
                        if cfg!(not(target_os = "macos")) {
                            stream_desc.mChannelsPerFrame = 1;
                        }
                        // The macOS voice-processing unit REJECTS any custom
                        // bus format (allocate fails -10875): leave its
                        // native format untouched and read it back after
                        // allocation instead. iOS / tvOS take the format.
                        let skip_format = cfg!(target_os = "macos") && matches!(unit_query, AudioUnitQuery::VoiceInput);
                        if !skip_format {
                            // Recreate AVAudioFormat with possibly adjusted channel count
                            let av_audio_format: ObjcId = msg_send![class!(AVAudioFormat), alloc];
                            let () =
                                msg_send![av_audio_format, initWithStreamDescription: &stream_desc];

                            let busses: ObjcId = msg_send![au_audio_unit, outputBusses];
                            let count: usize = msg_send![busses, count];
                            if count > 0 {
                                // RemoteIO/VoiceProcessingIO uses bus 1 for input on iOS/tvOS
                                let bus_index = 1;
                                if count > bus_index {
                                    let bus: ObjcId =
                                        msg_send![busses, objectAtIndexedSubscript: bus_index];
                                    let mut err: ObjcId = nil;
                                    let () = msg_send![bus, setFormat: av_audio_format error: &mut err];
                                    OSError::from_nserror(err)?;
                                    let () = msg_send![bus, setEnabled: true];
                                }
                            }
                        }
                        let () = msg_send![au_audio_unit, setOutputEnabled: false];
                        let () = msg_send![au_audio_unit, setInputEnabled: true];

                        // The voice-processing unit manages its own device
                        // aggregation and fails initialization when pinned to
                        // a HAL device — it follows the system default input.
                        #[cfg(target_os = "macos")]
                        if !matches!(unit_query, AudioUnitQuery::VoiceInput) {
                            let mut err: ObjcId = nil;
                            let () = msg_send![au_audio_unit, setDeviceID: core_device_id error: &mut err];
                            OSError::from_nserror(err)?;
                        }

                        // Allocate after configuration
                        let mut err: ObjcId = nil;
                        let () = msg_send![au_audio_unit, allocateRenderResourcesAndReturnError: &mut err];
                        OSError::from_nserror(err)?;
                        // VPIO kept its native bus format — read back the
                        // ACTUAL sample rate + channel count so the input
                        // callback and downstream resamplers see the truth.
                        if matches!(unit_query, AudioUnitQuery::VoiceInput) {
                            let busses: ObjcId = msg_send![au_audio_unit, outputBusses];
                            let count: usize = msg_send![busses, count];
                            if count > 1 {
                                let bus: ObjcId = msg_send![busses, objectAtIndexedSubscript: 1usize];
                                let format: ObjcId = msg_send![bus, format];
                                if format != nil {
                                    let sample_rate: f64 = msg_send![format, sampleRate];
                                    let channel_count: u32 = msg_send![format, channelCount];
                                    if sample_rate > 0.0 {
                                        stream_desc.mSampleRate = sample_rate;
                                    }
                                    if channel_count > 0 {
                                        input_channels = channel_count as usize;
                                    }
                                    // eprintln!(
                                    //     "voice input: native format {}Hz {}ch",
                                    //     stream_desc.mSampleRate,
                                    //     input_channels
                                    // );
                                }
                            }
                        }

                        // VPIO defaults to voice-chat mode, which DUCKS all
                        // other app audio (music drops to a whisper). Request
                        // advanced ducking at the minimum level — AEC keeps
                        // working, other audio stays near full volume.
                        // macOS 14+/iOS 17+; older systems return an error we
                        // ignore (they keep the default ducking).
                        if matches!(unit_query, AudioUnitQuery::VoiceInput) {
                            #[repr(C)]
                            struct DuckingConfig {
                                enable_advanced_ducking: u8,
                                _pad: [u8; 3],
                                ducking_level: u32,
                            }
                            const OTHER_AUDIO_DUCKING_CONFIGURATION: u32 = 2108;
                            const DUCKING_LEVEL_MIN: u32 = 10;
                            const SCOPE_GLOBAL: u32 = 0;
                            #[link(name = "AudioToolbox", kind = "framework")]
                            extern "C" {
                                fn AudioUnitSetProperty(
                                    unit: *mut std::ffi::c_void,
                                    property_id: u32,
                                    scope: u32,
                                    element: u32,
                                    data: *const std::ffi::c_void,
                                    data_size: u32,
                                ) -> i32;
                            }
                            let raw_unit: *mut std::ffi::c_void =
                                msg_send![av_audio_unit, audioUnit];
                            if !raw_unit.is_null() {
                                // Standard (non-advanced) ducking at Min is
                                // the gentlest combination — advanced mode
                                // dips other audio hard on voice activity.
                                // MAKEPAD_VPIO_DUCKING=min|mid|max|default
                                // and MAKEPAD_VPIO_ADVANCED=1 override for
                                // experimentation.
                                let level = match std::env::var("MAKEPAD_VPIO_DUCKING")
                                    .unwrap_or_default()
                                    .as_str()
                                {
                                    "mid" => 20,
                                    "max" => 30,
                                    "default" => 0,
                                    _ => DUCKING_LEVEL_MIN,
                                };
                                let advanced =
                                    std::env::var("MAKEPAD_VPIO_ADVANCED").is_ok_and(|v| v == "1");
                                let config = DuckingConfig {
                                    enable_advanced_ducking: advanced as u8,
                                    _pad: [0; 3],
                                    ducking_level: level,
                                };
                                let status = AudioUnitSetProperty(
                                    raw_unit,
                                    OTHER_AUDIO_DUCKING_CONFIGURATION,
                                    SCOPE_GLOBAL,
                                    0,
                                    (&config as *const DuckingConfig).cast(),
                                    std::mem::size_of::<DuckingConfig>() as u32,
                                );
                                if status == 0 {
                                    // eprintln!(
                                    //     "voice input: other-audio ducking level {level} advanced {advanced}"
                                    // );
                                } else {
                                    eprintln!(
                                        "voice input: ducking config not applied (OSStatus {status})"
                                    );
                                }
                            }
                        }

                        let () = msg_send![au_audio_unit, startHardwareAndReturnError: &mut err];
                        OSError::from_nserror(err)?;

                        let block_ptr: ObjcId = msg_send![au_audio_unit, renderBlock];
                        let () = msg_send![block_ptr, retain];
                        render_block = Some(block_ptr);
                    }
                }
                #[cfg(target_os = "macos")]
                AudioUnitAccess::observe_audio_unit_termination(_change_signal, core_device_id);
                // Retain the AU handle only on SUCCESS (balanced by
                // release_audio_unit) — retaining earlier leaked it on any
                // error return. NOT av again: that historic double retain
                // made every IO unit immortal, so failed/stopped
                // VoiceProcessingIO units kept system-wide ducking engaged
                // until process exit.
                let () = msg_send![au_audio_unit, retain];
                Ok(AudioUnit {
                    render_block,
                    unit_query,
                    av_audio_unit,
                    au_audio_unit,
                    sample_rate: stream_desc.mSampleRate,
                    input_channels,
                })
            }
            let change_signal = change_signal.clone();
            match unsafe {
                inner(
                    change_signal,
                    core_device_id,
                    av_audio_unit,
                    error,
                    unit_query,
                )
            } {
                Err(err) => {
                    // Release the instantiated unit on failure or it leaks
                    // FOREVER — a failed-but-alive VoiceProcessingIO unit
                    // keeps the system-wide voice-chat ducking engaged until
                    // the process dies.
                    if av_audio_unit != nil {
                        let () = unsafe { msg_send![av_audio_unit, release] };
                    }
                    unit_callback(Err(format!("{:?}", err)))
                }
                Ok(device) => unit_callback(Ok(device)),
            }
        });

        // Instantiate output audio unit
        let () = unsafe {
            msg_send![
                class!(AVAudioUnit),
                instantiateWithComponentDescription: unit_info.desc
                options: kAudioComponentInstantiation_LoadOutOfProcess
                completionHandler: &instantiation_handler
            ]
        };
    }
}

#[derive(Copy, Clone, Debug)]
pub enum AudioUnitQuery {
    Output,
    Input,
    /// Input through the OS voice-processing unit (echo cancellation).
    VoiceInput,
}
unsafe impl Send for AudioUnitQuery {}
unsafe impl Sync for AudioUnitQuery {}

#[derive(Clone, Debug)]
pub struct AudioUnitInfo {
    pub unit_query: AudioUnitQuery,
    desc: AudioComponentDescription,
}

unsafe impl Send for AudioUnit {}
unsafe impl Sync for AudioUnit {}
pub struct AudioUnit {
    av_audio_unit: ObjcId,
    au_audio_unit: ObjcId,
    render_block: Option<ObjcId>,
    unit_query: AudioUnitQuery,
    sample_rate: f64,
    /// Channel count the input render block delivers (VPIO native is mono).
    input_channels: usize,
}

impl AudioUnit {
    /// Output: the unit's provider calls the slot's entry with the block to fill.
    pub fn set_output_provider(&self, slot: Slot, device: u64, sample_rate: f64) {
        match self.unit_query {
            AudioUnitQuery::Output => (),
            x => panic!("cannot call set_output_provider on this device {:?}", x),
        }
        let output_provider = objc_block!(
            move | _flags: *mut u32,
            time_stamp: *const AudioTimeStamp,
            frame_count: u32,
            _input_bus_number: u64,
            buffers: *mut AudioBufferList |: i32 {
                let buffers_ref = unsafe {&*buffers};
                let frames = frame_count as usize;
                let channels = (buffers_ref.mNumberBuffers as usize).min(MAX_AUDIO_BUFFERS);
                let mut planes = [std::ptr::null_mut::<f32>(); MAX_AUDIO_BUFFERS];
                for i in 0..channels {
                    let plane = buffers_ref.mBuffers[i].mData as *mut f32;
                    // an entry that writes less than a block leaves silence
                    unsafe { std::ptr::write_bytes(plane, 0, frames) };
                    planes[i] = plane;
                }
                let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = *guard {
                    let (sample_time, host_time, rate_scalar) = unsafe { ((*time_stamp).mSampleTime, (*time_stamp).mHostTime, (*time_stamp).mRateScalar) };
                    let mut block = AudioBlock { device, sample_rate, sample_time, host_time, rate_scalar, has_time: 1, frames: frame_count, channels: channels as u32, planes: planes.as_mut_ptr() };
                    (entry.f)(entry.ctx as *mut std::ffi::c_void, &mut block);
                }
                0
            }
        );
        let () = unsafe { msg_send![self.au_audio_unit, setOutputProvider: &output_provider] };
    }

    pub fn clear_output_provider(&mut self) {
        unsafe {
            let () = msg_send![self.au_audio_unit, setOutputProvider: nil];
        }
    }

    /// Input: the handler renders the captured block and calls the slot's entry with it.
    pub fn set_input_handler(&self, slot: Slot, device: u64, sample_rate: f64) {
        match self.unit_query {
            AudioUnitQuery::Input | AudioUnitQuery::VoiceInput => (),
            x => panic!("cannot call set_input_handler on this device {:?}", x),
        }
        if let Some(render_block) = self.render_block {
            unsafe {
                let input_channels = self.input_channels.max(1);
                // the captured block lives here between calls: it grows when a block is larger
                let scratch = Mutex::new(Planar::new(0, input_channels));
                let input_handler = objc_block!(move |_flags: *mut u32, time_stamp: *const AudioTimeStamp, frame_count: u32, _input_bus_number: u64| {
                    let mut buffer = scratch.lock().unwrap_or_else(|e| e.into_inner());
                    let frames = frame_count as usize;
                    if buffer.frames != frames {
                        *buffer = Planar::new(frames, input_channels);
                    }
                    let mut flags = 0u32;
                    let mut buffer_list = AudioBufferList { mNumberBuffers: input_channels.min(MAX_AUDIO_BUFFERS) as u32, mBuffers: [_AudioBuffer { mNumberChannels: 0, mDataByteSize: 0, mData: 0 as *mut ::std::os::raw::c_void }; MAX_AUDIO_BUFFERS] };
                    for i in 0..input_channels.min(MAX_AUDIO_BUFFERS) {
                        buffer_list.mBuffers[i] = _AudioBuffer { mNumberChannels: 0, mDataByteSize: (frames * 4) as u32, mData: buffer.channel_mut(i).as_mut_ptr() as *mut ::std::os::raw::c_void };
                    }
                    // For RemoteIO/VoiceProcessingIO input, use bus 1
                    let bus_index = 1u64;
                    objc_block_invoke!(render_block, invoke(
                        (&mut flags as *mut u32): *mut u32,
                        (time_stamp): *const AudioTimeStamp,
                        (frame_count): u32,
                        (bus_index): u64,
                        (&mut buffer_list as *mut AudioBufferList): *mut AudioBufferList,
                        (nil): ObjcId
                    ) -> i32);
                    let time = Some(((*time_stamp).mSampleTime, (*time_stamp).mHostTime, (*time_stamp).mRateScalar));
                    call_input(&slot, device, time, sample_rate, &buffer);
                });
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
            // Full teardown, not just refcounts: a VoiceProcessingIO unit keeps the system-wide
            // voice-chat session (and its ducking of other apps' audio) alive until its render
            // resources are deallocated, and the retained render_block otherwise pins the unit
            // for the life of the process.
            let () = msg_send![self.au_audio_unit, stopHardware];
            let () = msg_send![self.au_audio_unit, deallocateRenderResources];
            if let Some(render_block) = self.render_block.take() {
                let () = msg_send![render_block, release];
            }
            let () = msg_send![self.av_audio_unit, release];
            self.av_audio_unit = nil;
            let () = msg_send![self.au_audio_unit, release];
            self.au_audio_unit = nil;
        }
    }
}

impl Drop for AudioUnitAccess {
    fn drop(&mut self) {
        // streams stop before their slots go
        for list in [&self.audio_inputs, &self.audio_outputs] {
            let mut units = list.lock().unwrap_or_else(|e| e.into_inner());
            for running in units.iter_mut() {
                if let Some(unit) = &mut running.audio_unit {
                    unit.stop_hardware();
                    unit.clear_input_handler();
                    unit.clear_output_provider();
                    unit.release_audio_unit();
                }
            }
            units.clear();
        }
    }
}
