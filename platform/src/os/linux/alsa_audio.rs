use {
    crate::{
        audio::*, makepad_live_id::*, os::linux::alsa_sys::*, os::linux::libc_sys,
        thread::SignalToUI,
    },
    std::collections::HashSet,
    std::ffi::CStr,
    std::os::raw::{c_char, c_void},
    std::sync::{Arc, Mutex},
};

struct AlsaAudioDesc {
    name: String,
    desc: AudioDeviceDesc,
}

struct AlsaAudioDevice {
    device_handle: *mut snd_pcm_t,
    channel_count: usize,
    frame_count: usize,
    interleaved: Vec<f32>,
    _buffer_size: usize,
}

struct AlsaAudioDeviceRef {
    device_id: AudioDeviceId,
    is_terminated: bool,
}

pub struct AlsaAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn>>>; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn>>>; MAX_AUDIO_DEVICE_INDEX],
    audio_outputs: Arc<Mutex<Vec<AlsaAudioDeviceRef>>>,
    audio_inputs: Arc<Mutex<Vec<AlsaAudioDeviceRef>>>,
    device_descs: Vec<AlsaAudioDesc>,
    // input and output are tracked apart because both directions of one alsa pcm
    // share a device id: a card without a microphone must not disable playback
    failed_inputs: Arc<Mutex<HashSet<AudioDeviceId>>>,
    failed_outputs: Arc<Mutex<HashSet<AudioDeviceId>>>,
    /// Devices asked for by the last call, used to tell an explicit request
    /// apart from the automatic re-selection that drives the retry loop.
    last_input_request: Vec<AudioDeviceId>,
    last_output_request: Vec<AudioDeviceId>,
    change_signal: SignalToUI,
}

#[derive(Debug)]
pub struct AlsaError(String);

/// The identity of an enumeration, used to tell a real device change from a
/// repeated enumeration of the same devices.
fn device_set(descs: &[AlsaAudioDesc]) -> HashSet<(AudioDeviceId, AudioDeviceType)> {
    descs
        .iter()
        .map(|v| (v.desc.device_id, v.desc.device_type))
        .collect()
}

macro_rules! alsa_error {
    ( $ call: expr) => {
        AlsaError::from(stringify!($call), $call)
    };
}

impl AlsaAudioAccess {
    pub fn new(change_signal: SignalToUI) -> Arc<Mutex<Self>> {
        let change_signal_inner = change_signal.clone();
        std::thread::spawn(move || {
            let mut last_card_count = 0;
            loop {
                let mut card_count = 0;
                let mut card_num = -1;
                loop {
                    unsafe {
                        snd_card_next(&mut card_num);
                    }
                    if card_num < 0 {
                        break;
                    }
                    card_count += 1;
                }
                if card_count != last_card_count {
                    last_card_count = card_count;
                    change_signal_inner.set();
                }
                let _ = std::thread::sleep(std::time::Duration::new(1, 0));
            }
        });

        Arc::new(Mutex::new(AlsaAudioAccess {
            change_signal,
            failed_inputs: Default::default(),
            failed_outputs: Default::default(),
            last_input_request: Vec::new(),
            last_output_request: Vec::new(),
            audio_input_cb: Default::default(),
            audio_output_cb: Default::default(),
            device_descs: Default::default(),
            audio_inputs: Default::default(),
            audio_outputs: Default::default(),
        }))
    }

    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
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
                    let device_id = AudioDeviceId(LiveId::from_str(&name_str));
                    let desc = AudioDeviceDesc {
                        has_failed: false,
                        device_id,
                        device_type: AudioDeviceType::Input,
                        is_default: false,
                        channel_count: 2,
                        name: format!("[ALSA] {}", desc_str),
                    };
                    if ioid == "" || ioid == "Input" {
                        device_descs.push(AlsaAudioDesc {
                            name: name_str.clone(),
                            desc: AudioDeviceDesc {
                                has_failed: failed_inputs.contains(&device_id),
                                ..desc.clone()
                            },
                        });
                    }
                    if ioid == "" || ioid == "Output" {
                        device_descs.push(AlsaAudioDesc {
                            name: name_str,
                            desc: AudioDeviceDesc {
                                device_type: AudioDeviceType::Output,
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
                    .find(|v| v.desc.device_type.is_output() && v.name == "default")
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.device_type.is_output() && v.name.starts_with("plughw:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.device_type.is_output() && v.name.starts_with("dmix:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) =
                    descs.iter_mut().find(|v| v.desc.device_type.is_output())
                {
                    descs.desc.is_default = true;
                }
                if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.device_type.is_input() && v.name == "default")
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.device_type.is_input() && v.name.starts_with("plughw:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs
                    .iter_mut()
                    .find(|v| v.desc.device_type.is_input() && v.name.starts_with("dmix:"))
                {
                    descs.desc.is_default = true;
                } else if let Some(descs) = descs.iter_mut().find(|v| v.desc.device_type.is_input())
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
                    self.failed_inputs.lock().unwrap().clear();
                    self.failed_outputs.lock().unwrap().clear();
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

    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.rearm_on_explicit_request(devices, true);
        let new = {
            let failed_inputs = self.failed_inputs.lock().unwrap().clone();
            let mut audio_inputs = self.audio_inputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_inputs.iter_mut().for_each(|v| {
                if !devices.contains(&v.device_id) {
                    v.is_terminated = true;
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                // a device that already refused to open is reported back with
                // has_failed; opening it again on every device change would
                // spawn a thread per frame and never stop
                if failed_inputs.contains(device_id) {
                    continue;
                }
                if audio_inputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    if let Some(v) = self
                        .device_descs
                        .iter()
                        .find(|v| v.desc.device_id == *device_id)
                    {
                        new.push((index, *device_id, v.name.clone()))
                    }
                }
            }
            new
        };
        for (index, device_id, name) in new {
            let audio_input_cb = self.audio_input_cb[index].clone();
            let audio_inputs = self.audio_inputs.clone();
            let failed_inputs = self.failed_inputs.clone();
            let change_signal = self.change_signal.clone();
            // published before the open, see the matching comment in
            // use_audio_outputs
            self.audio_inputs.lock().unwrap().push(AlsaAudioDeviceRef {
                device_id,
                is_terminated: false,
            });
            std::thread::spawn(move || {
                if let Ok((mut device, _device_ref)) =
                    AlsaAudioDevice::new(&name, device_id, SND_PCM_STREAM_CAPTURE)
                {
                    let mut audio_buffer = device.allocate_matching_buffer();
                    loop {
                        if audio_inputs
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|v| v.device_id == device_id && v.is_terminated)
                            .is_some()
                        {
                            break;
                        }
                        match device.read_input_buffer(&mut audio_buffer) {
                            Err(e) => {
                                // see the matching comment in use_audio_outputs
                                crate::error!("ALSA: input device {} stopped: {}", name, e.0);
                                failed_inputs.lock().unwrap().insert(device_id);
                                change_signal.set();
                                break;
                            }
                            Ok(_) => (),
                        }
                        if let Some(fbox) = &mut *audio_input_cb.lock().unwrap() {
                            fbox(
                                AudioInfo {
                                    device_id,
                                    time: None,
                                    sample_rate: 48000.0,
                                },
                                &audio_buffer,
                            );
                        }
                    }
                    let mut audio_inputs = audio_inputs.lock().unwrap();
                    audio_inputs.retain(|v| v.device_id != device_id);
                } else {
                    crate::error!("ALSA: could not open input device {}", name);
                    audio_inputs
                        .lock()
                        .unwrap()
                        .retain(|v| v.device_id != device_id);
                    failed_inputs.lock().unwrap().insert(device_id);
                    change_signal.set();
                }
            });
        }
    }

    /// Gives the requested devices another chance when the app asks for a
    /// different set than last time.
    ///
    /// The retry loop this guards against is an app re-requesting the *same*
    /// devices on every device change. A changed request is an explicit choice -
    /// a user picking a device, or a widget toggling its microphone back on -
    /// and silently ignoring it because the device failed once would leave that
    /// device dead for the rest of the process.
    fn rearm_on_explicit_request(&mut self, devices: &[AudioDeviceId], is_input: bool) {
        let (last, failed) = if is_input {
            (&mut self.last_input_request, &self.failed_inputs)
        } else {
            (&mut self.last_output_request, &self.failed_outputs)
        };
        if last.as_slice() == devices {
            return;
        }
        *last = devices.to_vec();
        let mut failed = failed.lock().unwrap();
        for device_id in devices {
            failed.remove(device_id);
        }
    }

    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.rearm_on_explicit_request(devices, false);
        let new = {
            let failed_outputs = self.failed_outputs.lock().unwrap().clone();
            let mut audio_outputs = self.audio_outputs.lock().unwrap();
            // lets shut down the ones we dont use
            audio_outputs.iter_mut().for_each(|v| {
                if !devices.contains(&v.device_id) {
                    v.is_terminated = true;
                }
            });
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                // a device that already refused to open is reported back with
                // has_failed; opening it again on every device change would
                // spawn a thread per frame and never stop
                if failed_outputs.contains(device_id) {
                    continue;
                }
                if audio_outputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    if let Some(v) = self
                        .device_descs
                        .iter()
                        .find(|v| v.desc.device_id == *device_id)
                    {
                        new.push((index, *device_id, v.name.clone()))
                    }
                }
            }
            new
        };
        for (index, device_id, name) in new {
            let audio_output_cb = self.audio_output_cb[index].clone();
            let audio_outputs = self.audio_outputs.clone();
            let failed_outputs = self.failed_outputs.clone();
            let change_signal = self.change_signal.clone();
            // published before the open, not after it: opening takes ~200ms, and
            // the check above that decides to spawn reads this same list. two
            // device changes inside that window - the normal startup, where the
            // card watcher signals immediately after the first enumeration -
            // would otherwise spawn a second thread for the same exclusive pcm,
            // whose EBUSY would mark a device that is playing fine as failed.
            // it also means a terminate arriving mid-open is not lost.
            self.audio_outputs.lock().unwrap().push(AlsaAudioDeviceRef {
                device_id,
                is_terminated: false,
            });
            std::thread::spawn(move || {
                // this thing fails here. so how would we then drop down to a secondary
                // we could simply switch default
                if let Ok((mut device, _device_ref)) =
                    AlsaAudioDevice::new(&name, device_id, SND_PCM_STREAM_PLAYBACK)
                {
                    // lets allocate an output buffer
                    let mut audio_buffer = device.allocate_matching_buffer();
                    loop {
                        if audio_outputs
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|v| v.device_id == device_id && v.is_terminated)
                            .is_some()
                        {
                            break;
                        }
                        if let Some(fbox) = &mut *audio_output_cb.lock().unwrap() {
                            fbox(
                                AudioInfo {
                                    device_id,
                                    time: None,
                                    sample_rate: 48000.0,
                                },
                                &mut audio_buffer,
                            );
                        }
                        match device.write_output_buffer(&audio_buffer) {
                            Err(e) => {
                                // the stream died under us. mark it like a failed
                                // open and tell the app, so it moves to another
                                // device instead of silently losing audio - and
                                // so we do not respawn this thread on every
                                // device change from here on
                                crate::error!("ALSA: output device {} stopped: {}", name, e.0);
                                failed_outputs.lock().unwrap().insert(device_id);
                                change_signal.set();
                                break;
                            }
                            Ok(_) => (),
                        }
                    }
                    audio_outputs
                        .lock()
                        .unwrap()
                        .retain(|v| v.device_id != device_id);
                } else {
                    crate::error!("ALSA: could not open output device {}", name);
                    audio_outputs
                        .lock()
                        .unwrap()
                        .retain(|v| v.device_id != device_id);
                    failed_outputs.lock().unwrap().insert(device_id);
                    change_signal.set();
                }
            });
        }
    }
}

impl AlsaAudioDevice {
    fn new(
        device_name: &str,
        device_id: AudioDeviceId,
        direction: snd_pcm_stream_t,
    ) -> Result<(AlsaAudioDevice, AlsaAudioDeviceRef), AlsaError> {
        unsafe {
            let mut handle: *mut snd_pcm_t = 0 as *mut _;
            let mut hw_params: *mut snd_pcm_hw_params_t = 0 as *mut _;
            let name0 = format!("{}\0", device_name);
            let mut rate = 48000;
            alsa_error!(snd_pcm_open(&mut handle, name0.as_ptr(), direction, 0))?;
            alsa_error!(snd_pcm_hw_params_malloc(&mut hw_params))?;
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
            alsa_error!(snd_pcm_hw_params_set_rate_near(
                handle,
                hw_params,
                &mut rate,
                0 as *mut _
            ))?;
            alsa_error!(snd_pcm_hw_params_set_channels(handle, hw_params, 2))?;
            let mut periods = 2;
            let mut dir = 0;
            alsa_error!(snd_pcm_hw_params_set_periods_near(
                handle,
                hw_params,
                &mut periods,
                &mut dir
            ))?;
            let mut buffer_size = 512;
            alsa_error!(snd_pcm_hw_params_set_buffer_size_near(
                handle,
                hw_params,
                &mut buffer_size
            ))?;
            alsa_error!(snd_pcm_hw_params(handle, hw_params))?;
            alsa_error!(snd_pcm_hw_params_set_rate_resample(handle, hw_params, 1))?;
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
            snd_pcm_hw_params_free(hw_params);

            // alright device is prepared.
            Ok((
                Self {
                    interleaved: {
                        let mut n = Vec::new();
                        n.resize(frame_count as usize * channel_count as usize, 0.0);
                        n
                    },
                    device_handle: handle,
                    channel_count: channel_count as usize,
                    frame_count: frame_count as usize,
                    _buffer_size: buffer_size as usize,
                },
                AlsaAudioDeviceRef {
                    device_id,
                    is_terminated: false,
                },
            ))
        }
    }

    fn allocate_matching_buffer(&self) -> AudioBuffer {
        AudioBuffer::new_with_size(self.frame_count, self.channel_count)
    }

    fn write_output_buffer(&mut self, buffer: &AudioBuffer) -> Result<i32, AlsaError> {
        unsafe {
            // interleave the audio buffer
            buffer.copy_to_interleaved(&mut self.interleaved);
            let result = snd_pcm_writei(
                self.device_handle,
                self.interleaved.as_ptr() as *mut _,
                self.frame_count as _,
            );
            if result == -libc_sys::EPIPE as _ {
                snd_pcm_prepare(self.device_handle);
                return Ok(0);
            }
            //println!("buffer {:?}", buffer.data.as_ptr());
            AlsaError::from("snd_pcm_writei", result as _)
        }
    }

    fn read_input_buffer(&mut self, buffer: &mut AudioBuffer) -> Result<i32, AlsaError> {
        unsafe {
            // interleave the audio buffer
            let result = snd_pcm_readi(
                self.device_handle,
                self.interleaved.as_ptr() as *mut _,
                self.frame_count as _,
            );
            if result == -libc_sys::EPIPE as _ {
                snd_pcm_prepare(self.device_handle);
                return Ok(0);
            }
            buffer.copy_from_interleaved(self.channel_count, &self.interleaved);
            //println!("buffer {:?}", buffer.data.as_ptr());
            AlsaError::from("snd_pcm_writei", result as _)
        }
    }
}

impl AlsaError {
    pub fn from(prefix: &str, err: i32) -> Result<i32, Self> {
        if err < 0 {
            Err(AlsaError(format!("{} - {}", prefix, unsafe {
                CStr::from_ptr(snd_strerror(err))
                    .to_str()
                    .unwrap()
                    .to_string()
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
        let c = CStr::from_ptr(s).to_str().unwrap().to_string();
        libc_sys::free(s as *mut c_void);
        Some(c)
    }
}
