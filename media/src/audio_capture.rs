use makepad_platform::{AudioBuffer, AudioInfo};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCaptureConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_samples: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrackConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_samples: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PcmAudioFrame {
    pub pts_samples: u64,
    pub duration_samples: u32,
    pub sample_rate: u32,
    pub channels: u8,
    /// Interleaved PCM samples in channel order.
    pub samples: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedOpusPacket {
    pub pts_samples: u64,
    pub duration_samples: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum AudioCaptureError {
    UnsupportedChannels(u8),
    InvalidSourceChannels(usize),
}

impl std::fmt::Display for AudioCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedChannels(channels) => {
                write!(f, "unsupported capture channel count: {channels}")
            }
            Self::InvalidSourceChannels(channels) => {
                write!(f, "invalid source channel count: {channels}")
            }
        }
    }
}

impl std::error::Error for AudioCaptureError {}

pub type Result<T> = std::result::Result<T, AudioCaptureError>;

pub struct AudioCaptureFrameizer {
    config: AudioCaptureConfig,
    pending_samples: VecDeque<f32>,
    next_pts_samples: u64,
    resampler: MonoResampler,
}

#[derive(Clone)]
pub struct MakepadAudioInputAdapter {
    input_tx: Sender<InputChunk>,
}

pub struct AudioCaptureReceiver {
    frame_rx: Receiver<PcmAudioFrame>,
}

struct InputChunk {
    sample_rate: u32,
    buffer: AudioBuffer,
}

struct MonoResampler {
    source_rate: Option<u32>,
    source_samples: Vec<f32>,
    source_pos: f64,
}

impl AudioCaptureConfig {
    pub fn validate(self) -> Result<Self> {
        if self.channels != 1 {
            return Err(AudioCaptureError::UnsupportedChannels(self.channels));
        }
        Ok(self)
    }
}

impl AudioCaptureFrameizer {
    pub fn new(config: AudioCaptureConfig) -> Result<Self> {
        Ok(Self {
            config: config.validate()?,
            pending_samples: VecDeque::new(),
            next_pts_samples: 0,
            resampler: MonoResampler::default(),
        })
    }

    pub fn push_buffer(&mut self, sample_rate: u32, buffer: &AudioBuffer) -> Result<Vec<PcmAudioFrame>> {
        let mono = downmix_to_mono(buffer)?;
        let normalized = self
            .resampler
            .push(sample_rate, self.config.sample_rate, &mono);
        self.pending_samples.extend(normalized);

        let mut out = Vec::new();
        let frame_samples = self.config.frame_samples as usize;
        while self.pending_samples.len() >= frame_samples {
            let mut samples = Vec::with_capacity(frame_samples);
            for _ in 0..frame_samples {
                samples.push(self.pending_samples.pop_front().unwrap());
            }
            out.push(PcmAudioFrame {
                pts_samples: self.next_pts_samples,
                duration_samples: self.config.frame_samples,
                sample_rate: self.config.sample_rate,
                channels: self.config.channels,
                samples,
            });
            self.next_pts_samples += self.config.frame_samples as u64;
        }
        Ok(out)
    }
}

impl MakepadAudioInputAdapter {
    pub fn new(config: AudioCaptureConfig) -> Result<(Self, AudioCaptureReceiver)> {
        let config = config.validate()?;
        let (input_tx, input_rx) = mpsc::channel::<InputChunk>();
        let (frame_tx, frame_rx) = mpsc::channel::<PcmAudioFrame>();

        std::thread::spawn(move || {
            let mut frameizer = AudioCaptureFrameizer::new(config).expect("validated config");
            while let Ok(chunk) = input_rx.recv() {
                let frames = match frameizer.push_buffer(chunk.sample_rate, &chunk.buffer) {
                    Ok(frames) => frames,
                    Err(_) => continue,
                };
                for frame in frames {
                    if frame_tx.send(frame).is_err() {
                        return;
                    }
                }
            }
        });

        Ok((Self { input_tx }, AudioCaptureReceiver { frame_rx }))
    }

    pub fn push(&self, info: AudioInfo, input: &AudioBuffer) {
        let mut buffer = AudioBuffer::new_like(input);
        buffer.copy_from(input);
        let _ = self.input_tx.send(InputChunk {
            sample_rate: info.sample_rate.round() as u32,
            buffer,
        });
    }
}

impl AudioCaptureReceiver {
    pub fn recv(&self) -> std::result::Result<PcmAudioFrame, mpsc::RecvError> {
        self.frame_rx.recv()
    }

    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> std::result::Result<PcmAudioFrame, mpsc::RecvTimeoutError> {
        self.frame_rx.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> std::result::Result<PcmAudioFrame, mpsc::TryRecvError> {
        self.frame_rx.try_recv()
    }
}

impl Default for MonoResampler {
    fn default() -> Self {
        Self {
            source_rate: None,
            source_samples: Vec::new(),
            source_pos: 0.0,
        }
    }
}

impl MonoResampler {
    fn push(&mut self, source_rate: u32, target_rate: u32, input: &[f32]) -> Vec<f32> {
        if source_rate == target_rate {
            return input.to_vec();
        }

        if self.source_rate != Some(source_rate) {
            self.source_rate = Some(source_rate);
            self.source_samples.clear();
            self.source_pos = 0.0;
        }

        self.source_samples.extend_from_slice(input);

        let mut out = Vec::new();
        let step = source_rate as f64 / target_rate as f64;
        while self.source_pos < self.source_samples.len() as f64 {
            let base = self.source_pos.floor() as usize;
            let next = (base + 1).min(self.source_samples.len() - 1);
            let frac = (self.source_pos - base as f64) as f32;
            let a = self.source_samples[base];
            let b = self.source_samples[next];
            out.push(a + (b - a) * frac);
            self.source_pos += step;
        }

        let consumed = self.source_pos.floor() as usize;
        if consumed > 0 {
            self.source_samples.drain(0..consumed);
            self.source_pos -= consumed as f64;
        }

        out
    }
}

fn downmix_to_mono(buffer: &AudioBuffer) -> Result<Vec<f32>> {
    if buffer.channel_count() == 0 {
        return Err(AudioCaptureError::InvalidSourceChannels(0));
    }
    let frame_count = buffer.frame_count();
    let channel_count = buffer.channel_count();
    let mut mono = vec![0.0; frame_count];
    let scale = 1.0 / channel_count as f32;
    for channel in 0..channel_count {
        let samples = buffer.channel(channel);
        for i in 0..frame_count {
            mono[i] += samples[i] * scale;
        }
    }
    Ok(mono)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;
    use std::time::Duration;

    fn config() -> AudioCaptureConfig {
        AudioCaptureConfig {
            sample_rate: 48_000,
            channels: 1,
            frame_samples: 960,
        }
    }

    fn mono_buffer(samples: &[f32]) -> AudioBuffer {
        AudioBuffer::from_data(samples.to_vec(), 1)
    }

    fn stereo_buffer(left: &[f32], right: &[f32]) -> AudioBuffer {
        let mut data = Vec::with_capacity(left.len() + right.len());
        data.extend_from_slice(left);
        data.extend_from_slice(right);
        AudioBuffer::from_data(data, 2)
    }

    fn sine_samples(count: usize, sample_rate: u32) -> Vec<f32> {
        (0..count)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (TAU * 440.0 * t).sin() * 0.25
            })
            .collect()
    }

    #[test]
    fn uneven_chunks_produce_exact_frames() {
        let mut frameizer = AudioCaptureFrameizer::new(config()).unwrap();
        let chunks = [137usize, 211, 89, 523, 960];
        let samples = sine_samples(chunks.iter().sum(), 48_000);
        let mut offset = 0usize;
        let mut frames = Vec::new();

        for chunk in chunks {
            let end = offset + chunk;
            frames.extend(
                frameizer
                    .push_buffer(48_000, &mono_buffer(&samples[offset..end]))
                    .unwrap(),
            );
            offset = end;
        }

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].pts_samples, 0);
        assert_eq!(frames[1].pts_samples, 960);
        assert_eq!(frames[0].samples.len(), 960);
        assert_eq!(frames[1].samples.len(), 960);
    }

    #[test]
    fn stereo_downmixes_to_mono() {
        let mut frameizer = AudioCaptureFrameizer::new(config()).unwrap();
        let left = vec![1.0; 960];
        let right = vec![-1.0; 960];
        let frames = frameizer.push_buffer(48_000, &stereo_buffer(&left, &right)).unwrap();

        assert_eq!(frames.len(), 1);
        assert!(frames[0].samples.iter().all(|sample| sample.abs() < 0.0001));
    }

    #[test]
    fn resampler_outputs_960_samples_for_20ms_at_44k1() {
        let mut frameizer = AudioCaptureFrameizer::new(config()).unwrap();
        let samples = sine_samples(882, 44_100);
        let first = frameizer.push_buffer(44_100, &mono_buffer(&samples[..441])).unwrap();
        let second = frameizer.push_buffer(44_100, &mono_buffer(&samples[441..])).unwrap();

        assert!(first.is_empty());
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].samples.len(), 960);
        assert_eq!(second[0].duration_samples, 960);
    }

    #[test]
    fn worker_adapter_emits_frames() {
        let (adapter, receiver) = MakepadAudioInputAdapter::new(config()).unwrap();
        let samples = sine_samples(960, 48_000);
        adapter.push(
            AudioInfo {
                device_id: Default::default(),
                time: None,
                sample_rate: 48_000.0,
            },
            &mono_buffer(&samples),
        );

        let frame = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(frame.pts_samples, 0);
        assert_eq!(frame.samples.len(), 960);
    }
}
