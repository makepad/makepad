use crate::PcmAudioFrame;
use makepad_platform::{AudioBuffer, AudioInfo};
#[cfg(feature = "opus")]
use std::sync::mpsc::{self, Receiver, Sender};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioPlayoutConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_samples: u32,
    pub target_delay_ms: u32,
    pub max_buffer_ms: u32,
}

impl Default for AudioPlayoutConfig {
    fn default() -> Self {
        #[cfg(feature = "opus")]
        {
            return Self {
                sample_rate: crate::OPUS_SAMPLE_RATE,
                channels: 1,
                frame_samples: crate::OPUS_FRAME_SAMPLES as u32,
                target_delay_ms: 80,
                max_buffer_ms: 160,
            };
        }
        #[cfg(not(feature = "opus"))]
        {
            Self {
                sample_rate: 48_000,
                channels: 2,
                frame_samples: 1024,
                target_delay_ms: 80,
                max_buffer_ms: 160,
            }
        }
    }
}

#[derive(Debug)]
pub enum AudioPlayoutError {
    UnsupportedChannels(u8),
    UnsupportedSampleRate(u32),
    InvalidFrameSamples(u32),
    InvalidBufferMs,
    #[cfg(feature = "opus")]
    Opus(crate::OpusError),
}

impl std::fmt::Display for AudioPlayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedChannels(channels) => {
                write!(f, "unsupported playout channels: {channels}")
            }
            Self::UnsupportedSampleRate(sample_rate) => {
                write!(f, "unsupported playout sample rate: {sample_rate}")
            }
            Self::InvalidFrameSamples(frame_samples) => {
                write!(f, "invalid playout frame samples: {frame_samples}")
            }
            Self::InvalidBufferMs => write!(f, "invalid playout buffer timing"),
            #[cfg(feature = "opus")]
            Self::Opus(err) => write!(f, "opus: {err}"),
        }
    }
}

impl std::error::Error for AudioPlayoutError {}

#[cfg(feature = "opus")]
impl From<crate::OpusError> for AudioPlayoutError {
    fn from(value: crate::OpusError) -> Self {
        Self::Opus(value)
    }
}

pub type Result<T> = std::result::Result<T, AudioPlayoutError>;

#[derive(Clone)]
pub struct PcmAudioPlayoutBuffer {
    shared: Arc<Mutex<SharedPlayoutState>>,
}

#[derive(Clone)]
pub struct PcmAudioOutputAdapter {
    shared: Arc<Mutex<SharedPlayoutState>>,
    source_cache: VecDeque<f32>,
    source_pos_frames: f64,
}

pub type MakepadAudioOutputAdapter = PcmAudioOutputAdapter;

struct SharedPlayoutState {
    config: AudioPlayoutConfig,
    samples: VecDeque<f32>,
    buffering: bool,
    target_delay_frames: usize,
    max_buffer_frames: usize,
    front_pts_samples: Option<u64>,
    rendered_frames: u64,
    eos: bool,
    underflows: u64,
}

impl AudioPlayoutConfig {
    pub fn validate(self) -> Result<Self> {
        if self.sample_rate == 0 {
            return Err(AudioPlayoutError::UnsupportedSampleRate(self.sample_rate));
        }
        if !(1..=2).contains(&self.channels) {
            return Err(AudioPlayoutError::UnsupportedChannels(self.channels));
        }
        if self.frame_samples == 0 {
            return Err(AudioPlayoutError::InvalidFrameSamples(self.frame_samples));
        }
        if self.target_delay_ms == 0 || self.max_buffer_ms < self.target_delay_ms {
            return Err(AudioPlayoutError::InvalidBufferMs);
        }
        Ok(self)
    }
}

impl PcmAudioPlayoutBuffer {
    pub fn new(config: AudioPlayoutConfig) -> Result<(Self, PcmAudioOutputAdapter)> {
        let config = config.validate()?;
        let shared = Arc::new(Mutex::new(SharedPlayoutState::new(config)));
        Ok((
            Self {
                shared: shared.clone(),
            },
            PcmAudioOutputAdapter {
                shared,
                source_cache: VecDeque::new(),
                source_pos_frames: 0.0,
            },
        ))
    }

    pub fn push_frame(&self, frame: PcmAudioFrame) -> Result<()> {
        let mut shared = self.shared.lock().map_err(|_| AudioPlayoutError::InvalidBufferMs)?;
        shared.push_frame(frame)
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.lock().map(|state| state.config.sample_rate).unwrap_or(0)
    }

    pub fn buffered_frames(&self) -> usize {
        self.shared.lock().map(|state| state.buffered_frames()).unwrap_or(0)
    }

    pub fn buffered_samples(&self) -> usize {
        self.shared.lock().map(|state| state.samples.len()).unwrap_or(0)
    }

    pub fn buffered_ms(&self) -> u64 {
        self.shared.lock().map(|state| state.buffered_ms()).unwrap_or(0)
    }

    pub fn has_adequate_buffer(&self) -> bool {
        self.shared.lock().map(|state| state.has_adequate_buffer()).unwrap_or(false)
    }

    pub fn front_pts_samples(&self) -> Option<u64> {
        self.shared.lock().ok().and_then(|state| state.front_pts_samples())
    }

    pub fn playout_head_pts_samples(&self) -> Option<u64> {
        self.shared.lock().ok().and_then(|state| state.playout_head_pts_samples())
    }

    pub fn underflow_count(&self) -> u64 {
        self.shared.lock().map(|state| state.underflows).unwrap_or(0)
    }

    pub fn set_end_of_stream(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.eos = true;
        }
    }

    pub fn is_drained(&self) -> bool {
        self.shared.lock().map(|state| state.eos && state.samples.is_empty()).unwrap_or(false)
    }

    pub fn flush(&self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.flush();
        }
    }
}

impl PcmAudioOutputAdapter {
    pub fn fill_output(&mut self, info: AudioInfo, output: &mut AudioBuffer) {
        output.zero();
        let frame_count = output.frame_count();
        if frame_count == 0 {
            return;
        }

        let target_rate = info.sample_rate.round().max(1.0) as u32;
        let (source_rate, source_channels) = match self.shared.lock() {
            Ok(state) => (state.config.sample_rate, state.config.channels as usize),
            Err(_) => return,
        };
        let step = source_rate as f64 / target_rate as f64;
        let needed_source_frames =
            (self.source_pos_frames + step * (frame_count.saturating_sub(1) as f64)).floor()
                as usize
                + 1;

        let Ok(mut shared) = self.shared.try_lock() else {
            return;
        };
        if !shared.fill_cache(&mut self.source_cache, needed_source_frames) {
            return;
        }
        drop(shared);

        let output_channels = output.channel_count();
        for out_frame in 0..frame_count {
            let source_frame = self.source_pos_frames;
            let base_frame = source_frame.floor() as usize;
            let next_frame = (base_frame + 1).min(self.cached_frames(source_channels).saturating_sub(1));
            let frac = (source_frame - base_frame as f64) as f32;

            let sample_for = |channel: usize, cache: &VecDeque<f32>| {
                let a = sample_at(cache, base_frame, channel, source_channels);
                let b = sample_at(cache, next_frame, channel, source_channels);
                a + (b - a) * frac
            };

            match (source_channels, output_channels) {
                (1, 1) => output.channel_mut(0)[out_frame] = sample_for(0, &self.source_cache),
                (1, 2) => {
                    let sample = sample_for(0, &self.source_cache);
                    output.channel_mut(0)[out_frame] = sample;
                    output.channel_mut(1)[out_frame] = sample;
                }
                (2, 1) => {
                    let left = sample_for(0, &self.source_cache);
                    let right = sample_for(1, &self.source_cache);
                    output.channel_mut(0)[out_frame] = (left + right) * 0.5;
                }
                (2, 2) => {
                    output.channel_mut(0)[out_frame] = sample_for(0, &self.source_cache);
                    output.channel_mut(1)[out_frame] = sample_for(1, &self.source_cache);
                }
                _ => {}
            }
            self.source_pos_frames += step;
        }

        let consumed_frames = self.source_pos_frames.floor() as usize;
        let consumed_samples = consumed_frames * source_channels;
        for _ in 0..consumed_samples.min(self.source_cache.len()) {
            self.source_cache.pop_front();
        }
        self.source_pos_frames -= consumed_frames as f64;

        if let Ok(mut shared) = self.shared.lock() {
            shared.on_rendered_frames(consumed_frames as u64);
        }
    }

    fn cached_frames(&self, channels: usize) -> usize {
        if channels == 0 {
            0
        } else {
            self.source_cache.len() / channels
        }
    }
}

impl SharedPlayoutState {
    fn new(config: AudioPlayoutConfig) -> Self {
        Self {
            config,
            samples: VecDeque::new(),
            buffering: true,
            target_delay_frames: ms_to_frames(config.sample_rate, config.target_delay_ms),
            max_buffer_frames: ms_to_frames(config.sample_rate, config.max_buffer_ms),
            front_pts_samples: None,
            rendered_frames: 0,
            eos: false,
            underflows: 0,
        }
    }

    fn push_frame(&mut self, frame: PcmAudioFrame) -> Result<()> {
        if frame.sample_rate != self.config.sample_rate {
            return Err(AudioPlayoutError::UnsupportedSampleRate(frame.sample_rate));
        }
        if frame.channels != self.config.channels {
            return Err(AudioPlayoutError::UnsupportedChannels(frame.channels));
        }
        if self.front_pts_samples.is_none() && self.samples.is_empty() {
            self.front_pts_samples = Some(frame.pts_samples);
        }
        self.samples.extend(frame.samples);
        let overflow_frames = self.buffered_frames().saturating_sub(self.max_buffer_frames);
        let overflow_samples = overflow_frames * self.config.channels as usize;
        for _ in 0..overflow_samples.min(self.samples.len()) {
            self.samples.pop_front();
        }
        self.eos = false;
        Ok(())
    }

    fn buffered_frames(&self) -> usize {
        self.samples.len() / self.config.channels as usize
    }

    fn buffered_ms(&self) -> u64 {
        self.buffered_frames() as u64 * 1000 / self.config.sample_rate as u64
    }

    fn has_adequate_buffer(&self) -> bool {
        self.buffered_frames() >= self.target_delay_frames
    }

    fn front_pts_samples(&self) -> Option<u64> {
        self.front_pts_samples
    }

    fn playout_head_pts_samples(&self) -> Option<u64> {
        self.front_pts_samples.map(|front| front + self.rendered_frames)
    }

    fn fill_cache(&mut self, cache: &mut VecDeque<f32>, needed_frames: usize) -> bool {
        let available_frames = cache.len() / self.config.channels as usize + self.buffered_frames();
        if self.buffering {
            if available_frames < self.target_delay_frames.max(needed_frames) {
                self.underflows += 1;
                return false;
            }
            self.buffering = false;
        }

        let cached_frames = cache.len() / self.config.channels as usize;
        let needed_from_queue_frames = needed_frames.saturating_sub(cached_frames);
        let needed_from_queue_samples = needed_from_queue_frames * self.config.channels as usize;
        if self.samples.len() < needed_from_queue_samples {
            self.buffering = true;
            self.underflows += 1;
            return false;
        }
        for _ in 0..needed_from_queue_samples {
            if let Some(sample) = self.samples.pop_front() {
                cache.push_back(sample);
            }
        }
        true
    }

    fn on_rendered_frames(&mut self, frames: u64) {
        self.rendered_frames = self.rendered_frames.saturating_add(frames);
        if self.samples.is_empty() {
            self.front_pts_samples = self.playout_head_pts_samples();
        }
    }

    fn flush(&mut self) {
        self.samples.clear();
        self.buffering = true;
        self.front_pts_samples = None;
        self.rendered_frames = 0;
        self.eos = false;
        self.underflows = 0;
    }
}

fn sample_at(cache: &VecDeque<f32>, frame: usize, channel: usize, channels: usize) -> f32 {
    cache.get(frame * channels + channel).copied().unwrap_or(0.0)
}

fn ms_to_frames(sample_rate: u32, ms: u32) -> usize {
    ((sample_rate as u64 * ms as u64) / 1000) as usize
}

#[cfg(feature = "opus")]
pub struct OpusAudioPlayoutBuffer {
    packet_tx: Sender<crate::EncodedOpusPacket>,
    pcm: PcmAudioPlayoutBuffer,
}

#[cfg(feature = "opus")]
impl OpusAudioPlayoutBuffer {
    pub fn new(config: AudioPlayoutConfig) -> Result<(Self, MakepadAudioOutputAdapter)> {
        let (pcm, adapter) = PcmAudioPlayoutBuffer::new(config)?;
        let (packet_tx, packet_rx) = mpsc::channel::<crate::EncodedOpusPacket>();
        let worker_pcm = pcm.clone();
        let mut decoder = crate::OpusDecoder::new()?;
        std::thread::spawn(move || decode_worker(packet_rx, worker_pcm, &mut decoder));
        Ok((Self { packet_tx, pcm }, adapter))
    }

    pub fn push_packet(
        &self,
        packet: crate::EncodedOpusPacket,
    ) -> std::result::Result<(), mpsc::SendError<crate::EncodedOpusPacket>> {
        self.packet_tx.send(packet)
    }

    pub fn buffered_samples(&self) -> usize {
        self.pcm.buffered_samples()
    }

    pub fn buffered_frames(&self) -> usize {
        self.pcm.buffered_frames()
    }

    pub fn front_pts_samples(&self) -> Option<u64> {
        self.pcm.front_pts_samples()
    }
}

#[cfg(feature = "opus")]
fn decode_worker(
    packet_rx: Receiver<crate::EncodedOpusPacket>,
    playout: PcmAudioPlayoutBuffer,
    decoder: &mut crate::OpusDecoder,
) {
    while let Ok(packet) = packet_rx.recv() {
        let Ok(samples) = decoder.decode_packet(&packet.data) else {
            continue;
        };
        let _ = playout.push_frame(PcmAudioFrame {
            pts_samples: packet.pts_samples,
            duration_samples: packet.duration_samples,
            sample_rate: packet.sample_rate,
            channels: packet.channels,
            samples,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(samples: Vec<f32>, pts_samples: u64, channels: u8) -> PcmAudioFrame {
        PcmAudioFrame {
            pts_samples,
            duration_samples: (samples.len() / channels as usize) as u32,
            sample_rate: 48_000,
            channels,
            samples,
        }
    }

    fn output_buffer(frame_count: usize, channels: usize) -> AudioBuffer {
        AudioBuffer::new_with_size(frame_count, channels)
    }

    #[test]
    fn callback_underflow_returns_silence() {
        let (_playout, mut adapter) = PcmAudioPlayoutBuffer::new(AudioPlayoutConfig::default()).unwrap();
        let mut output = output_buffer(480, 2);
        adapter.fill_output(
            AudioInfo { device_id: Default::default(), time: None, sample_rate: 48_000.0 },
            &mut output,
        );
        assert!(output.data.iter().all(|sample| sample.abs() < 0.0001));
    }

    #[test]
    fn stereo_pcm_playout_and_head_tracking() {
        let config = AudioPlayoutConfig { channels: 2, frame_samples: 64, target_delay_ms: 1, max_buffer_ms: 100, ..AudioPlayoutConfig::default() };
        let (playout, mut adapter) = PcmAudioPlayoutBuffer::new(config).unwrap();
        let mut samples = Vec::new();
        for i in 0..64 {
            samples.push((i as f32 / 8.0).sin());
            samples.push((i as f32 / 8.0).cos());
        }
        playout.push_frame(frame(samples, 100, 2)).unwrap();
        assert_eq!(playout.front_pts_samples(), Some(100));

        let mut output = output_buffer(4, 2);
        adapter.fill_output(
            AudioInfo { device_id: Default::default(), time: None, sample_rate: 48_000.0 },
            &mut output,
        );
        assert!(output.data.iter().any(|sample| sample.abs() > 0.1));
        assert_eq!(playout.playout_head_pts_samples(), Some(104));
    }

    #[test]
    fn queue_adequacy_and_flush() {
        let config = AudioPlayoutConfig { channels: 1, frame_samples: 4, target_delay_ms: 20, max_buffer_ms: 100, sample_rate: 48_000 };
        let (playout, _adapter) = PcmAudioPlayoutBuffer::new(config).unwrap();
        playout.push_frame(frame(vec![0.0; 960], 0, 1)).unwrap();
        assert!(playout.has_adequate_buffer());
        playout.flush();
        assert_eq!(playout.buffered_frames(), 0);
        assert!(!playout.has_adequate_buffer());
    }

    #[cfg(feature = "opus")]
    #[test]
    fn opus_wrapper_decodes_into_generic_pcm_queue() {
        let mut encoder = crate::OpusEncoder::new().unwrap();
        let pcm = (0..crate::OPUS_FRAME_SAMPLES)
            .map(|i| ((i as f32 / 32.0) * std::f32::consts::TAU).sin() * 0.25)
            .collect::<Vec<_>>();
        let data = encoder.encode_frame(&pcm).unwrap();
        let (playout, mut adapter) = OpusAudioPlayoutBuffer::new(AudioPlayoutConfig {
            target_delay_ms: 20,
            max_buffer_ms: 120,
            ..AudioPlayoutConfig::default()
        }).unwrap();
        playout.push_packet(crate::EncodedOpusPacket {
            pts_samples: 0,
            duration_samples: crate::OPUS_FRAME_SAMPLES as u32,
            sample_rate: crate::OPUS_SAMPLE_RATE,
            channels: 1,
            data,
        }).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let mut output = output_buffer(crate::OPUS_FRAME_SAMPLES, 1);
        adapter.fill_output(
            AudioInfo { device_id: Default::default(), time: None, sample_rate: 48_000.0 },
            &mut output,
        );
        let energy: f32 = output.data.iter().map(|sample| sample.abs()).sum();
        assert!(energy > 1.0);
    }
}
