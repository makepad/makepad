use {
    self::super::{
        from_wasm::{FromWasmQueryAudioDevices, FromWasmStartAudioOutput, FromWasmStopAudioOutput},
        to_wasm::ToWasmAudioDeviceList,
        web::CxOs,
    },
    crate::{audio::*, makepad_live_id::*, thread::SignalToUI},
    std::{
        cell::UnsafeCell,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex,
        },
    },
};

#[repr(C)]
pub struct WebAudioOutputClosure {
    pub callback: Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static>,
    pub device_id: AudioDeviceId,
    pub output_buffer: AudioBuffer,
}

pub struct WebAudioDevice {
    web_device_id: String,
    desc: AudioDeviceDesc,
}

/// What the page's output entry reaches through its context pointer, and nothing else: the
/// app's render function (replaced from the UI through a lock-free slot, the old one handed
/// back to drop there), the device it renders for, and the block buffer. The entry never
/// locks, so a UI thread busy with `WebAudioAccess` cannot hold the worklet up.
pub struct WebAudioOutput {
    render: makepad_lock_free::Slot<AudioOutputFn>,
    device_id: AtomicU64,
    /// Touched by the entry only (the page calls it for one block at a time).
    buffer: UnsafeCell<AudioBuffer>,
}

// SAFETY: `render` and `device_id` are thread-safe; `buffer` is reached by the entry only.
unsafe impl Sync for WebAudioOutput {}

impl WebAudioOutput {
    /// Install (or clear) the render function; the one it replaces drops here, on the caller's
    /// thread, after the block that may be running it has finished.
    pub fn set_render(&self, render: Option<AudioOutputFn>) {
        drop(self.render.replace(render));
    }
}

pub struct WebAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn>>>; MAX_AUDIO_DEVICE_INDEX],
    pub output: Arc<WebAudioOutput>,
    pub devices: Vec<WebAudioDevice>,
    change_signal: SignalToUI,
}

impl WebAudioAccess {
    pub fn new(os: &mut CxOs, change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        // ok lets request audio inputs and outputs
        os.from_wasm(FromWasmQueryAudioDevices {});
        Arc::new(Mutex::new(Self {
            audio_input_cb: Default::default(),
            output: Arc::new(WebAudioOutput {
                render: makepad_lock_free::Slot::default(),
                device_id: AtomicU64::new(0),
                // A worklet quantum is 128 frames, a script processor's up to 16384: this much
                // up front, so the entry's resize only changes the length (a larger one grows once).
                buffer: UnsafeCell::new(AudioBuffer::new_with_size(4096, 2)),
            }),
            devices: Default::default(),
            change_signal,
        }))
    }

    pub fn to_wasm_audio_device_list(&mut self, tw: ToWasmAudioDeviceList) {
        self.devices.clear();
        for device in tw.devices {
            let device_id = LiveId::from_str(&format!(
                "{} {} {}",
                device.web_device_id, device.label, device.is_output
            ))
            .into();
            self.devices.push(WebAudioDevice {
                web_device_id: device.web_device_id,
                desc: AudioDeviceDesc {
                    device_id,
                    device_type: if device.is_output {
                        AudioDeviceType::Output
                    } else {
                        AudioDeviceType::Input
                    },
                    is_default: false,
                    has_failed: false,
                    channel_count: 2,
                    name: device.label,
                },
            })
        }
        // lets pick some defaults
        // we need to find either 'default' or the first of either input or ouptput
        if let Some(device) = self
            .devices
            .iter_mut()
            .find(|v| v.desc.device_type.is_input() && v.web_device_id == "default")
        {
            device.desc.is_default = true;
        } else if let Some(device) = self
            .devices
            .iter_mut()
            .find(|v| v.desc.device_type.is_input())
        {
            device.desc.is_default = true;
        }
        if let Some(device) = self
            .devices
            .iter_mut()
            .find(|v| v.desc.device_type.is_output() && v.web_device_id == "default")
        {
            device.desc.is_default = true;
        } else if let Some(device) = self
            .devices
            .iter_mut()
            .find(|v| v.desc.device_type.is_output())
        {
            device.desc.is_default = true;
        }
        self.change_signal.set();
    }

    pub fn use_audio_inputs(&mut self, _os: &mut CxOs, _devices: &[AudioDeviceId]) {
        // TODO
    }

    pub fn use_audio_outputs(&mut self, os: &mut CxOs, devices: &[AudioDeviceId]) {
        // alright we're going to use audio outputs.
        // we can however only use one so we'll use the first one
        // and then we'll send over the device we want to the other side
        if devices.len() == 0 {
            os.from_wasm(FromWasmStopAudioOutput {});
            return;
        }
        if devices.len() > 1 {
            crate::log!("Web only supports a single audio device");
        }
        let web_device_id =
            if let Some(device) = self.devices.iter().find(|v| v.desc.device_id == devices[0]) {
                device.web_device_id.clone()
            } else {
                crate::log!("web audio: requested output device is unavailable; using browser default");
                "".to_string()
            };
        self.output.device_id.store(devices[0].0 .0, Ordering::Relaxed);
        // The page calls the entry with this pointer for as long as it plays: the output lives
        // as long as this access, which the app keeps for its whole life.
        os.from_wasm(FromWasmStartAudioOutput {
            web_device_id,
            context_ptr: Arc::as_ptr(&self.output) as u32,
        });
    }

    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        let mut desc = Vec::new();
        for device in &self.devices {
            desc.push(device.desc.clone())
        }
        desc
    }
}

/// One output block: the page asks for `frames` x `channels` planar samples (channel `c`
/// at `c * frames` from the returned address). `sample_time` is the block's first frame
/// on the output stream; `heard_at` is when that frame is heard, in seconds of the page's
/// monotonic clock (performance.now() / 1000), or NaN where the caller cannot tell (the
/// threaded worklet).
#[export_name = "wasm_audio_output_entrypoint"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_audio_output_entrypoint(
    context_ptr: u32,
    frames: u32,
    channels: u32,
    sample_rate: f64,
    sample_time: f64,
    heard_at: f64,
) -> u32 {
    // SAFETY: `context_ptr` is the `WebAudioOutput` `use_audio_outputs` gave the page, kept
    // alive for the app's life; the page calls this entry for one block at a time, so the buffer
    // is this call's alone
    let output = &*(context_ptr as *const WebAudioOutput);
    let buffer = &mut *output.buffer.get();
    buffer.clear_final_size();
    buffer.resize(frames as usize, channels as usize);
    buffer.set_final_size();
    let info = AudioInfo {
        device_id: AudioDeviceId(LiveId(output.device_id.load(Ordering::Relaxed))),
        time: heard_at.is_finite().then(|| AudioTime {
            sample_time,
            host_time: (heard_at.max(0.0) * 1e9) as u64,
            rate_scalar: 1.0,
        }),
        sample_rate,
    };
    // No render function, or one being replaced: this block is silence.
    if output.render.with(|render| render(info, &mut *buffer)).is_none() {
        buffer.zero();
    }
    let ptr = buffer.data.as_ptr();
    ptr as u32
}
