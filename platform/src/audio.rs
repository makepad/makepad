use crate::makepad_live_id::{FromLiveId, LiveId};

pub const MAX_AUDIO_DEVICE_INDEX: usize = 32;

pub type AudioOutputFn = Box<dyn FnMut(AudioInfo, &mut AudioBuffer) + Send + 'static>;
pub type AudioInputFn = Box<dyn FnMut(AudioInfo, &AudioBuffer) + Send + 'static>;

#[derive(Clone, Debug, Default, Eq, Hash, Copy, PartialEq, FromLiveId)]
pub struct AudioDeviceId(pub LiveId);

/// Options for `use_audio_inputs_with_options`. Best-effort per platform:
/// a platform without the capability captures plain audio instead.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioInputOptions {
    /// Route capture through the OS voice-processing path (echo
    /// cancellation / feedback suppression), so device playback — e.g. the
    /// app's own TTS — is removed from the mic signal. Apple: the input
    /// unit becomes VoiceProcessingIO (system-wide AEC; typically mono).
    pub echo_cancellation: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioInfo {
    pub device_id: AudioDeviceId,
    pub time: Option<AudioTime>,
    pub sample_rate: f64,
}

#[derive(Clone, Debug)]
pub struct AudioDeviceDesc {
    pub device_id: AudioDeviceId,
    pub device_type: AudioDeviceType,
    pub is_default: bool,
    pub has_failed: bool,
    pub channel_count: usize,
    pub name: String,
}

impl std::fmt::Display for AudioDeviceDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_default {
            write!(f, "[Default ")?;
        } else {
            write!(f, "[")?;
        }
        match self.device_type {
            AudioDeviceType::Input => write!(f, "Input]")?,
            AudioDeviceType::Output => write!(f, "Output]")?,
            AudioDeviceType::Loopback => write!(f, "Loopback]")?,
        }
        write!(f, " {}", self.name)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AudioDevicesEvent {
    pub descs: Vec<AudioDeviceDesc>,
}

impl AudioDevicesEvent {
    /// The device to record from: the default device, or if that one failed to
    /// open, the default device anyway.
    ///
    /// Deliberately does *not* fall back to some other microphone the way
    /// [`Self::default_output`] falls back to another speaker. Capture devices
    /// are not interchangeable - the next input in the list is typically a
    /// monitor source, which is a loopback of everything the machine is
    /// playing, so silently recording from it instead would be a privacy
    /// breach. Which microphone to use when the default one is unavailable is
    /// the app's decision to make, from `descs`.
    pub fn default_input(&self) -> Vec<AudioDeviceId> {
        // Real microphones only: the loopback device captures SYSTEM AUDIO
        // (via screen-recording privileges on macOS) and must never be
        // selected implicitly — apps opt into it by explicit device id.
        for d in &self.descs {
            if d.is_default && matches!(d.device_type, AudioDeviceType::Input) && !d.has_failed {
                return vec![d.device_id];
            }
        }
        for d in &self.descs {
            if d.is_default && matches!(d.device_type, AudioDeviceType::Input) {
                return vec![d.device_id];
            }
        }
        Vec::new()
    }
    /// The device to play to, as a fallback chain: the default device, then any
    /// other device that has not failed to open, and only if everything has
    /// failed the default anyway.
    ///
    /// Handing back a device that is already known to have failed is what makes
    /// an app ask for it again on every device change, so it is the last resort
    /// rather than the first fallback.
    pub fn default_output(&self) -> Vec<AudioDeviceId> {
        for d in &self.descs {
            if d.is_default && d.device_type.is_output() && !d.has_failed {
                return vec![d.device_id];
            }
        }
        for d in &self.descs {
            if d.device_type.is_output() && !d.has_failed {
                return vec![d.device_id];
            }
        }
        for d in &self.descs {
            if d.is_default && d.device_type.is_output() {
                return vec![d.device_id];
            }
        }
        Vec::new()
    }

    pub fn match_outputs(&self, outputs: &[&str]) -> Vec<AudioDeviceId> {
        let mut results = Vec::new();
        for d in &self.descs {
            if d.device_type.is_output() {
                for output in outputs {
                    if d.name.find(output).is_some() {
                        results.push(d.device_id);
                        break;
                    }
                }
            }
        }
        if results.len() == 0 {
            return self.default_output();
        }
        results
    }

    pub fn match_inputs(&self, inputs: &[&str]) -> Vec<AudioDeviceId> {
        let mut results = Vec::new();
        for d in &self.descs {
            if d.device_type.is_input() {
                for input in inputs {
                    if d.name.find(input).is_some() {
                        results.push(d.device_id);
                    }
                }
            }
        }
        return results;
    }
}

impl std::fmt::Display for AudioDevicesEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = write!(f, "Audio Devices:\n");
        for d in &self.descs {
            let _ = write!(f, "{}\n", d);
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum AudioDeviceType {
    Input,
    Output,
    Loopback, // Output device opened as input for capturing system audio
}

impl AudioDeviceType {
    pub fn is_input(&self) -> bool {
        match self {
            AudioDeviceType::Input => true,
            AudioDeviceType::Loopback => true, // Loopback acts as input
            _ => false,
        }
    }
    pub fn is_output(&self) -> bool {
        match self {
            AudioDeviceType::Output => true,
            _ => false,
        }
    }
    pub fn is_loopback(&self) -> bool {
        match self {
            AudioDeviceType::Loopback => true,
            _ => false,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct AudioTime {
    pub sample_time: f64,
    pub host_time: u64,
    pub rate_scalar: f64,
}

#[derive(Clone, Debug, Default)]
pub struct AudioBuffer {
    pub data: Vec<f32>,
    pub final_size: bool,
    pub frame_count: usize,
    pub channel_count: usize,
}

impl AudioBuffer {
    pub fn from_data(data: Vec<f32>, channel_count: usize) -> Self {
        let frame_count = data.len() / channel_count;
        Self {
            data,
            final_size: false,
            frame_count,
            channel_count,
        }
    }

    pub fn from_i16(inp: &[i16], channel_count: usize) -> Self {
        let mut data = Vec::new();
        data.resize(inp.len(), 0.0);
        let frame_count = data.len() / channel_count;
        for i in 0..data.len() {
            data[i] = (inp[i] as f32) / 32767.0;
        }
        Self {
            data,
            final_size: false,
            frame_count,
            channel_count,
        }
    }

    pub fn make_single_channel(&mut self) {
        self.data.resize(self.frame_count, 0.0);
        self.channel_count = 1;
    }

    pub fn into_data(self) -> Vec<f32> {
        self.data
    }

    pub fn to_i16(&self) -> Vec<i16> {
        let mut out = Vec::new();
        out.resize(self.data.len(), 0);
        for i in 0..self.data.len() {
            let f = (self.data[i] * 32767.0)
                .max(std::i16::MIN as f32)
                .min(std::i16::MAX as f32);
            out[i] = f as i16;
        }
        out
    }

    pub fn new_with_size(frame_count: usize, channel_count: usize) -> Self {
        let mut ret = Self::default();
        ret.resize(frame_count, channel_count);
        ret
    }

    pub fn new_like(like: &AudioBuffer) -> Self {
        let mut ret = Self::default();
        ret.resize_like(like);
        ret
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count
    }
    pub fn channel_count(&self) -> usize {
        self.channel_count
    }

    pub fn copy_from(&mut self, like: &AudioBuffer) -> &mut Self {
        self.resize(like.frame_count(), like.channel_count());
        self.data.copy_from_slice(&like.data);
        self
    }

    pub fn resize_like(&mut self, like: &AudioBuffer) -> &mut Self {
        self.resize(like.frame_count(), like.channel_count());
        self
    }

    pub fn resize(&mut self, frame_count: usize, channel_count: usize) {
        if self.frame_count != frame_count || self.channel_count != channel_count {
            if self.final_size {
                panic!("Audiobuffer is set to 'final size' and resize is different");
            }
            self.frame_count = frame_count;
            self.channel_count = channel_count;
            self.data.resize(frame_count * channel_count as usize, 0.0);
        }
    }

    pub fn clear_final_size(&mut self) {
        self.final_size = false;
    }

    pub fn set_final_size(&mut self) {
        self.final_size = true;
    }

    pub fn stereo_mut(&mut self) -> (&mut [f32], &mut [f32]) {
        if self.channel_count != 2 {
            panic!()
        }
        self.data.split_at_mut(self.frame_count)
    }

    pub fn stereo(&self) -> (&[f32], &[f32]) {
        if self.channel_count != 2 {
            panic!()
        }
        self.data.split_at(self.frame_count)
    }

    pub fn channel_mut(&mut self, channel: usize) -> &mut [f32] {
        &mut self.data[channel * self.frame_count..(channel + 1) * self.frame_count]
    }

    pub fn channel(&self, channel: usize) -> &[f32] {
        &self.data[channel * self.frame_count..(channel + 1) * self.frame_count]
    }

    pub fn zero(&mut self) {
        for i in 0..self.data.len() {
            self.data[i] = 0.0;
        }
    }

    pub fn copy_from_interleaved(&mut self, channel_count: usize, interleaved: &[f32]) {
        let frame_count = interleaved.len() / channel_count;
        self.resize(frame_count, channel_count);
        for i in 0..frame_count {
            for j in 0..channel_count {
                self.data[i + j * frame_count] = interleaved[i * channel_count + j];
            }
        }
    }

    pub fn copy_to_interleaved(&self, interleaved: &mut [f32]) {
        if interleaved.len() != self.frame_count * self.channel_count {
            panic!()
        }
        for i in 0..self.frame_count {
            for j in 0..self.channel_count {
                interleaved[i * self.channel_count + j] = self.data[i + j * self.frame_count];
            }
        }
    }
}
