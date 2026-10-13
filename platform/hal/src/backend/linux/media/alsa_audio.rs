//! ALSA device enumeration and PCM workers, ported from the native Linux backend.
use super::{
    alsa_sys::*,
    audio::{Buffer, Slot},
    Signal,
};
use crate::media::{device_kind, name_hash, AudioDeviceInfo};
use std::{
    collections::HashSet,
    ffi::CStr,
    os::raw::{c_char, c_void},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
};

struct AlsaAudioDesc {
    name: String,
    desc: AudioDeviceInfo,
}
struct Stream {
    id: u64,
    slot: usize,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
pub struct AlsaAudioAccess {
    device_descs: Vec<AlsaAudioDesc>,
    failed_inputs: Arc<Mutex<HashSet<u64>>>,
    failed_outputs: Arc<Mutex<HashSet<u64>>>,
    streams: [Vec<Stream>; 2],
    last: [Vec<u64>; 2],
    change: Signal,
}
fn device_set(descs: &[AlsaAudioDesc]) -> HashSet<(u64, u32)> {
    descs.iter().map(|d| (d.desc.id, d.desc.kind)).collect()
}
#[derive(Debug)]
pub struct AlsaError(pub String);
macro_rules! alsa_error {
    ($call:expr) => {
        AlsaError::from(stringify!($call), $call)
    };
}
impl AlsaAudioAccess {
    pub fn new(change: Signal) -> Self {
        Self {
            device_descs: Vec::new(),
            failed_inputs: Default::default(),
            failed_outputs: Default::default(),
            streams: Default::default(),
            last: Default::default(),
            change,
        }
    }
    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceInfo> {
        // alright lets do it
        fn inner(alsa: &AlsaAudioAccess) -> Result<Vec<AlsaAudioDesc>, AlsaError> {
            let mut device_descs = Vec::new();
            let failed_inputs = alsa.failed_inputs.lock().unwrap().clone();
            let failed_outputs = alsa.failed_outputs.lock().unwrap().clone();
            unsafe {
                // -1 asks alsa for the whole system instead of one card. It is
                // what every other alsa client does, and it is the only way the
                // generic pcms show up at all - "default" above all, which
                // follows whatever the machine is configured to use and mixes
                // through the sound server. Asking per card only ever returned
                // raw device nodes like plughw:CARD=PCH,DEV=0, which demand
                // exclusive access and so fail whenever anything else is
                // playing. The whole-system list already contains every card's
                // own pcms, so nothing is lost by not walking the cards.
                let mut hints: *mut *mut c_void = 0 as *mut _;
                alsa_error!(snd_device_name_hint(-1, "pcm\0".as_ptr(), &mut hints))?;
                if hints.is_null() {
                    return Ok(device_descs);
                }

                let mut index = 0;
                while *hints.offset(index) != std::ptr::null_mut() {
                    let hint_ptr = *hints.offset(index);
                    index += 1;
                    let name_str =
                        from_alsa_string(snd_device_name_get_hint(hint_ptr, "NAME\0".as_ptr()))
                            .unwrap_or("".into());
                    // "null" accepts and discards everything, and it always
                    // opens - offering it would let the automatic fallback
                    // land on a device that looks like working audio and is
                    // silent
                    if name_str.is_empty() || name_str == "null" {
                        continue;
                    }
                    let desc_str =
                        from_alsa_string(snd_device_name_get_hint(hint_ptr, "DESC\0".as_ptr()))
                            .unwrap_or("".into())
                            .replace("\n", " ");
                    let ioid =
                        from_alsa_string(snd_device_name_get_hint(hint_ptr, "IOID\0".as_ptr()))
                            .unwrap_or("".into());
                    let device_id = name_hash(&name_str);
                    let desc = AudioDeviceInfo {
                        has_failed: false,
                        id: device_id,
                        kind: device_kind::INPUT,
                        is_default: false,
                        channels: 2,
                        name: format!("[ALSA] {}", desc_str),
                    };
                    if ioid == "" || ioid == "Input" {
                        device_descs.push(AlsaAudioDesc {
                            name: name_str.clone(),
                            desc: AudioDeviceInfo {
                                has_failed: failed_inputs.contains(&device_id),
                                ..desc.clone()
                            },
                        });
                    }
                    if ioid == "" || ioid == "Output" {
                        device_descs.push(AlsaAudioDesc {
                            name: name_str,
                            desc: AudioDeviceInfo {
                                kind: device_kind::OUTPUT,
                                has_failed: failed_outputs.contains(&device_id),
                                ..desc
                            },
                        });
                    }
                }
                snd_device_name_free_hint(hints);
            }
            Ok(device_descs)
        }
        // taken before the clear below: it is what the new enumeration is
        // compared against to decide whether anything actually changed
        let previous_devices = device_set(&self.device_descs);
        self.device_descs.clear();
        match inner(self) {
            Err(e) => {
                println!("ALSA ERROR {}", e.0)
            }
            Ok(mut descs) => {
                // pick a single default device. "default" first: it is the pcm
                // every other linux application plays through, it follows the
                // machine's configured device, and it shares the card instead of
                // seizing it. The raw nodes below it are the fallback for a
                // system without any alsa configuration at all.
                if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::OUTPUT && v.name == "default")
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::OUTPUT && v.name.starts_with("plughw:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::OUTPUT && v.name.starts_with("dmix:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::OUTPUT)
                {
                    descs.desc.is_default = true;
                }
                if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::INPUT && v.name == "default")
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::INPUT && v.name.starts_with("plughw:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.kind == device_kind::INPUT && v.name.starts_with("dmix:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) =
                    descs.iter_mut().find(|v| v.desc.kind == device_kind::INPUT)
                {
                    descs.desc.is_default = true;
                }

                // a device that failed to open is not tried again until
                // something changes - the alsa equivalent of a hotplug event is
                // the device list itself changing, so re-arm every failure then.
                // that is what lets a card which was merely busy come back.
                // compared as a set: a reordered enumeration is not a change,
                // and treating it as one would resurrect the retry loop.
                if device_set(&descs) != previous_devices {
                    self.failed_inputs
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clear();
                    self.failed_outputs
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clear();
                    for v in descs.iter_mut() {
                        v.desc.has_failed = false;
                    }
                }
                self.device_descs = descs;
            }
        }
        let mut out = Vec::new();
        for dev in &self.device_descs {
            out.push(dev.desc.clone());
        }
        out
    }

    pub fn use_devices(&mut self, dir: usize, devices: &[u64], slots: &[Arc<Slot>]) {
        let failed = if dir == 0 {
            self.failed_inputs.clone()
        } else {
            self.failed_outputs.clone()
        };
        if self.last[dir] != devices {
            for id in devices {
                failed.lock().unwrap().remove(id);
            }
            self.last[dir] = devices.to_vec();
        }
        self.streams[dir].retain(|s| {
            devices.get(s.slot) == Some(&s.id)
                && s.thread
                    .as_ref()
                    .is_some_and(|thread| !thread.is_finished())
        });
        for (index, &id) in devices.iter().enumerate() {
            if self.streams[dir]
                .iter()
                .any(|s| s.id == id && s.slot == index)
                || failed.lock().unwrap().contains(&id)
            {
                continue;
            }
            let Some(desc) = self
                .device_descs
                .iter()
                .find(|d| d.desc.id == id && d.desc.kind == dir as u32)
            else {
                continue;
            };
            let name = desc.name.clone();
            let slot = slots[index].clone();
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = stop.clone();
            let failed = failed.clone();
            let change = self.change.clone();
            let thread = std::thread::spawn(move || {
                let result = (|| {
                    let mut pcm = Pcm::new(
                        &name,
                        if dir == 0 {
                            SND_PCM_STREAM_CAPTURE
                        } else {
                            SND_PCM_STREAM_PLAYBACK
                        },
                    )?;
                    let mut planar = Buffer::new(pcm.frames);
                    while !stopped.load(Ordering::Acquire) {
                        if dir == 0 {
                            let n = unsafe {
                                snd_pcm_readi(
                                    pcm.handle,
                                    pcm.samples.as_mut_ptr().cast(),
                                    pcm.frames as _,
                                )
                            };
                            if n > 0 {
                                planar.read(&pcm.samples[..n as usize * 2]);
                                slot.call(id, pcm.rate, &mut planar);
                            } else if n == 0 || n == -11 {
                                std::thread::sleep(std::time::Duration::from_millis(1));
                            } else if n < 0 {
                                alsa_error!(unsafe { snd_pcm_recover(pcm.handle, n as i32, 1) })?;
                            }
                        } else {
                            planar.frames = pcm.frames;
                            planar.clear();
                            slot.call(id, pcm.rate, &mut planar);
                            planar.write(&mut pcm.samples);
                            let mut offset = 0;
                            while offset < pcm.frames && !stopped.load(Ordering::Acquire) {
                                let n = unsafe {
                                    snd_pcm_writei(
                                        pcm.handle,
                                        pcm.samples.as_ptr().add(offset * 2).cast_mut().cast(),
                                        (pcm.frames - offset) as _,
                                    )
                                };
                                if n > 0 {
                                    offset += n as usize
                                } else if n == 0 || n == -11 {
                                    std::thread::sleep(std::time::Duration::from_millis(1));
                                } else if n < 0 {
                                    alsa_error!(unsafe {
                                        snd_pcm_recover(pcm.handle, n as i32, 1)
                                    })?;
                                }
                            }
                        }
                    }
                    Ok::<_, AlsaError>(())
                })();
                if let Err(e) = result {
                    eprintln!("[hal-alsa] {name}: {}", e.0);
                    failed.lock().unwrap().insert(id);
                    change.set();
                }
            });
            self.streams[dir].push(Stream {
                id,
                slot: index,
                stop,
                thread: Some(thread),
            });
        }
    }
}
struct Pcm {
    handle: *mut snd_pcm_t,
    frames: usize,
    rate: f64,
    samples: Vec<f32>,
}
struct PcmGuard {
    handle: *mut snd_pcm_t,
    params: *mut snd_pcm_hw_params_t,
}
impl Drop for PcmGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.params.is_null() {
                snd_pcm_hw_params_free(self.params)
            }
            if !self.handle.is_null() {
                snd_pcm_close(self.handle);
            }
        }
    }
}
impl Drop for Pcm {
    fn drop(&mut self) {
        unsafe {
            snd_pcm_drop(self.handle);
            snd_pcm_close(self.handle);
        }
    }
}
impl Pcm {
    fn new(device_name: &str, direction: snd_pcm_stream_t) -> Result<Self, AlsaError> {
        unsafe {
            let mut handle: *mut snd_pcm_t = 0 as *mut _;
            let mut hw_params: *mut snd_pcm_hw_params_t = 0 as *mut _;
            let name0 = format!("{}\0", device_name);
            let mut rate = 48000;
            alsa_error!(snd_pcm_open(&mut handle, name0.as_ptr(), direction, 1))?;
            let mut guard = PcmGuard {
                handle,
                params: std::ptr::null_mut(),
            };
            alsa_error!(snd_pcm_hw_params_malloc(&mut hw_params))?;
            guard.params = hw_params;
            alsa_error!(snd_pcm_hw_params_any(handle, hw_params))?;
            alsa_error!(snd_pcm_hw_params_set_access(
                handle,
                hw_params,
                SND_PCM_ACCESS_RW_INTERLEAVED
            ))?;
            alsa_error!(snd_pcm_hw_params_set_format(
                handle,
                hw_params,
                SND_PCM_FORMAT_FLOAT_LE
            ))?;
            // Enable ALSA/plugin resampling before committing params. Calling this
            // after snd_pcm_hw_params() is a no-op and left us stuck with whatever
            // rate_near picked without soft-resample fallbacks.
            alsa_error!(snd_pcm_hw_params_set_rate_resample(handle, hw_params, 1))?;
            alsa_error!(snd_pcm_hw_params_set_rate_near(
                handle,
                hw_params,
                &mut rate,
                0 as *mut _
            ))?;
            alsa_error!(snd_pcm_hw_params_set_channels(handle, hw_params, 2))?;
            // ~40–80ms of buffering. The old 512-frame / 2-period setup (~5–10ms)
            // underran constantly under soft video decode (PipeWire + VM), which
            // surfaces as crackle / static in the FFmpeg MediaPlugin path.
            let mut periods = 4;
            let mut dir = 0;
            alsa_error!(snd_pcm_hw_params_set_periods_near(
                handle,
                hw_params,
                &mut periods,
                &mut dir
            ))?;
            let mut buffer_size = 4096;
            alsa_error!(snd_pcm_hw_params_set_buffer_size_near(
                handle,
                hw_params,
                &mut buffer_size
            ))?;
            alsa_error!(snd_pcm_hw_params(handle, hw_params))?;
            let mut buffer_size = 0;
            alsa_error!(snd_pcm_hw_params_get_buffer_size(
                hw_params,
                &mut buffer_size
            ))?;
            let mut channel_count = 0;
            alsa_error!(snd_pcm_hw_params_get_channels(
                hw_params,
                &mut channel_count
            ))?;
            let mut frame_count = 0;
            alsa_error!(snd_pcm_hw_params_get_period_size(
                hw_params,
                &mut frame_count,
                0 as *mut _
            ))?;
            let mut actual_rate = 0u32;
            // Prefer the negotiated rate; fall back to the value rate_near wrote.
            if snd_pcm_hw_params_get_rate(hw_params, &mut actual_rate, 0 as *mut _) == 0
                && actual_rate > 0
            {
                rate = actual_rate;
            }
            if channel_count != 2 || frame_count == 0 {
                return Err(AlsaError("ALSA negotiated an invalid stereo period".into()));
            }
            snd_pcm_hw_params_free(hw_params);
            guard.params = std::ptr::null_mut();
            guard.handle = std::ptr::null_mut();

            Ok(Self {
                handle,
                frames: frame_count as usize,
                rate: rate as f64,
                samples: vec![0.0; frame_count as usize * 2],
            })
        }
    }
}
impl AlsaError {
    pub fn from(prefix: &str, err: i32) -> Result<i32, Self> {
        if err < 0 {
            Err(AlsaError(format!("{} - {}", prefix, unsafe {
                CStr::from_ptr(snd_strerror(err))
                    .to_string_lossy()
                    .into_owned()
            })))
        } else {
            Ok(err)
        }
    }
}

fn from_alsa_string(s: *mut c_char) -> Option<String> {
    if s.is_null() {
        return None;
    };
    unsafe {
        let c = CStr::from_ptr(s).to_string_lossy().into_owned();
        super::libc_sys::free(s as *mut c_void);
        Some(c)
    }
}
