use opus2::{
    Application, Channels, Decoder as InnerDecoder, Encoder as InnerEncoder, Error as InnerError,
};
use std::fmt;

pub const OPUS_SAMPLE_RATE: u32 = 48_000;
pub const OPUS_CHANNELS: u8 = 1;
pub const OPUS_FRAME_SAMPLES: usize = 960;
pub const OPUS_FRAME_DURATION_MS: u32 = 20;
const MAX_PACKET_BYTES: usize = 1500;

#[derive(Debug)]
pub enum OpusError {
    InvalidPcmFrameLength { expected: usize, got: usize },
    InvalidDecodedFrameLength { expected: usize, got: usize },
    Opus(InnerError),
}

impl fmt::Display for OpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPcmFrameLength { expected, got } => {
                write!(f, "invalid pcm frame length: expected {expected}, got {got}")
            }
            Self::InvalidDecodedFrameLength { expected, got } => {
                write!(f, "invalid decoded frame length: expected {expected}, got {got}")
            }
            Self::Opus(err) => write!(f, "opus: {err}"),
        }
    }
}

impl std::error::Error for OpusError {}

impl From<InnerError> for OpusError {
    fn from(value: InnerError) -> Self {
        Self::Opus(value)
    }
}

pub type Result<T> = std::result::Result<T, OpusError>;

pub struct OpusEncoder {
    inner: InnerEncoder,
}

impl OpusEncoder {
    pub fn new() -> Result<Self> {
        let inner = InnerEncoder::new(OPUS_SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        Ok(Self { inner })
    }

    pub fn encode_frame(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        if pcm.len() != OPUS_FRAME_SAMPLES {
            return Err(OpusError::InvalidPcmFrameLength {
                expected: OPUS_FRAME_SAMPLES,
                got: pcm.len(),
            });
        }
        let mut packet = vec![0; MAX_PACKET_BYTES];
        let size = self.inner.encode_float(pcm, &mut packet)?;
        packet.truncate(size);
        Ok(packet)
    }
}

pub struct OpusDecoder {
    inner: InnerDecoder,
}

impl OpusDecoder {
    pub fn new() -> Result<Self> {
        let inner = InnerDecoder::new(OPUS_SAMPLE_RATE, Channels::Mono)?;
        Ok(Self { inner })
    }

    pub fn packet_duration_samples(&self, packet: &[u8]) -> Result<u32> {
        let samples = self.inner.get_nb_samples(packet)?;
        Ok(samples as u32)
    }

    pub fn decode_packet(&mut self, packet: &[u8]) -> Result<Vec<f32>> {
        let mut pcm = vec![0.0; OPUS_FRAME_SAMPLES];
        let decoded = self.inner.decode_float(packet, &mut pcm, false)?;
        if decoded != OPUS_FRAME_SAMPLES {
            return Err(OpusError::InvalidDecodedFrameLength {
                expected: OPUS_FRAME_SAMPLES,
                got: decoded,
            });
        }
        Ok(pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn sine_frame() -> Vec<f32> {
        (0..OPUS_FRAME_SAMPLES)
            .map(|i| {
                let t = i as f32 / OPUS_SAMPLE_RATE as f32;
                (TAU * 440.0 * t).sin() * 0.25
            })
            .collect()
    }

    #[test]
    fn encoder_init() {
        OpusEncoder::new().unwrap();
    }

    #[test]
    fn decoder_init() {
        OpusDecoder::new().unwrap();
    }

    #[test]
    fn encode_decode_roundtrip_smoke() {
        let mut encoder = OpusEncoder::new().unwrap();
        let mut decoder = OpusDecoder::new().unwrap();
        let input = sine_frame();

        let packet = encoder.encode_frame(&input).unwrap();
        assert!(!packet.is_empty());

        let output = decoder.decode_packet(&packet).unwrap();
        assert_eq!(output.len(), OPUS_FRAME_SAMPLES);
        assert!(output.iter().all(|sample| sample.is_finite()));
        let energy: f32 = output.iter().map(|sample| sample.abs()).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn packet_duration_is_960_samples() {
        let mut encoder = OpusEncoder::new().unwrap();
        let decoder = OpusDecoder::new().unwrap();
        let packet = encoder.encode_frame(&sine_frame()).unwrap();

        assert_eq!(decoder.packet_duration_samples(&packet).unwrap(), 960);
    }

    #[test]
    fn malformed_packet_returns_error() {
        let mut decoder = OpusDecoder::new().unwrap();
        let malformed = [0xffu8];

        assert!(decoder.packet_duration_samples(&malformed).is_err());
        assert!(decoder.decode_packet(&malformed).is_err());
    }
}
