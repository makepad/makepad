//! Vendored AAC-LC decoder wrapper.
//!
//! Uses vendored Symphonia AAC-LC decoder code with a small `makepad-media`
//! adapter for `AudioSpecificConfig` and `PcmAudioFrame` output.

use crate::PcmAudioFrame;
use std::fmt;
use symphonia_codec_aac::AacDecoder as SymphoniaAacDecoder;
use symphonia_core::{
    audio::{Channels, Signal},
    codecs::{CodecParameters, Decoder as _, DecoderOptions, CODEC_TYPE_AAC},
    formats::Packet,
};

const AAC_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
    8_000, 7_350,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    pub audio_object_type: u8,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_samples: u32,
    pub raw: Vec<u8>,
}

pub struct AacLcDecoder {
    decoder: SymphoniaAacDecoder,
    config: AudioSpecificConfig,
    track_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AacDecoderError {
    InvalidConfig(&'static str),
    UnsupportedConfig(&'static str),
    Decode(String),
}

impl fmt::Display for AacDecoderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "invalid AAC config: {msg}"),
            Self::UnsupportedConfig(msg) => write!(f, "unsupported AAC config: {msg}"),
            Self::Decode(msg) => write!(f, "AAC decode failed: {msg}"),
        }
    }
}

impl std::error::Error for AacDecoderError {}

impl AudioSpecificConfig {
    pub fn parse(raw: &[u8]) -> Result<Self, AacDecoderError> {
        let mut bits = BitReader::new(raw);
        let mut audio_object_type = bits.read_u8(5)?;
        let mut sample_rate = read_sample_rate(&mut bits)?;
        let channels = bits.read_u8(4)?;

        if audio_object_type == 5 || audio_object_type == 29 {
            sample_rate = read_sample_rate(&mut bits)?;
            audio_object_type = read_audio_object_type(&mut bits)?;
        }

        if audio_object_type != 2 {
            return Err(AacDecoderError::UnsupportedConfig(
                "only AAC-LC AudioSpecificConfig is supported",
            ));
        }
        if channels == 0 {
            return Err(AacDecoderError::UnsupportedConfig(
                "program config element channel layouts are not supported",
            ));
        }
        if channels > 2 {
            return Err(AacDecoderError::UnsupportedConfig(
                "only mono and stereo AAC-LC are supported",
            ));
        }

        let frame_length_flag = bits.read_u8(1)?;
        let frame_samples = if frame_length_flag == 0 { 1024 } else { 960 };

        Ok(Self {
            audio_object_type,
            sample_rate,
            channels,
            frame_samples,
            raw: raw.to_vec(),
        })
    }
}

impl AacLcDecoder {
    pub fn new(audio_specific_config: &[u8]) -> Result<Self, AacDecoderError> {
        let config = AudioSpecificConfig::parse(audio_specific_config)?;
        let mut codec_params = CodecParameters::new();
        codec_params
            .for_codec(CODEC_TYPE_AAC)
            .with_sample_rate(config.sample_rate)
            .with_channels(match config.channels {
                1 => Channels::FRONT_LEFT,
                2 => Channels::FRONT_LEFT | Channels::FRONT_RIGHT,
                _ => unreachable!(),
            })
            .with_extra_data(config.raw.clone().into_boxed_slice());

        let decoder = SymphoniaAacDecoder::try_new(&codec_params, &DecoderOptions::default())
            .map_err(|err| AacDecoderError::Decode(err.to_string()))?;

        Ok(Self {
            decoder,
            config,
            track_id: 0,
        })
    }

    pub fn config(&self) -> &AudioSpecificConfig {
        &self.config
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.config.channels
    }

    pub fn frame_samples(&self) -> u32 {
        self.config.frame_samples
    }

    pub fn reset(&mut self) {
        self.decoder.reset();
    }

    pub fn decode_access_unit(
        &mut self,
        data: &[u8],
        pts_samples: u64,
    ) -> Result<PcmAudioFrame, AacDecoderError> {
        let packet = Packet::new_from_slice(
            self.track_id,
            pts_samples,
            self.config.frame_samples as u64,
            data,
        );
        let decoded = self
            .decoder
            .decode(&packet)
            .map_err(|err| AacDecoderError::Decode(err.to_string()))?;

        let mut pcm = decoded.make_equivalent::<f32>();
        decoded.convert(&mut pcm);

        let frames = pcm.frames();
        let channels = pcm.spec().channels.count();
        let mut samples = Vec::with_capacity(frames * channels);
        for frame in 0..frames {
            for channel in 0..channels {
                samples.push(pcm.chan(channel)[frame]);
            }
        }

        Ok(PcmAudioFrame {
            pts_samples,
            duration_samples: frames as u32,
            sample_rate: pcm.spec().rate,
            channels: channels as u8,
            samples,
        })
    }
}

fn read_audio_object_type(bits: &mut BitReader<'_>) -> Result<u8, AacDecoderError> {
    let audio_object_type = bits.read_u8(5)?;
    if audio_object_type == 31 {
        Ok(32 + bits.read_u8(6)?)
    } else {
        Ok(audio_object_type)
    }
}

fn read_sample_rate(bits: &mut BitReader<'_>) -> Result<u32, AacDecoderError> {
    let sample_rate_index = bits.read_u8(4)?;
    if sample_rate_index == 0x0f {
        let sample_rate = bits.read_u32(24)?;
        if sample_rate == 0 {
            return Err(AacDecoderError::InvalidConfig("explicit sample rate is zero"));
        }
        Ok(sample_rate)
    } else {
        AAC_SAMPLE_RATES
            .get(sample_rate_index as usize)
            .copied()
            .ok_or(AacDecoderError::InvalidConfig("unknown sample rate index"))
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_u8(&mut self, bits: usize) -> Result<u8, AacDecoderError> {
        let value = self.read_u32(bits)?;
        Ok(value as u8)
    }

    fn read_u32(&mut self, bits: usize) -> Result<u32, AacDecoderError> {
        if bits > 32 {
            return Err(AacDecoderError::InvalidConfig("bit field too wide"));
        }
        let mut value = 0u32;
        for _ in 0..bits {
            let byte = self.bit_pos / 8;
            if byte >= self.data.len() {
                return Err(AacDecoderError::InvalidConfig(
                    "truncated AudioSpecificConfig",
                ));
            }
            let shift = 7 - (self.bit_pos % 8);
            value = (value << 1) | (((self.data[byte] >> shift) & 1) as u32);
            self.bit_pos += 1;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASC_AAC_LC_STEREO_48K: [u8; 2] = [0x11, 0x90];

    #[test]
    fn parse_aac_lc_audio_specific_config() {
        let config = AudioSpecificConfig::parse(&ASC_AAC_LC_STEREO_48K).unwrap();
        assert_eq!(config.audio_object_type, 2);
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.frame_samples, 1024);
    }

    #[test]
    fn decoder_rejects_non_lc_config() {
        let err = AudioSpecificConfig::parse(&[0x29, 0x88, 0x00]).unwrap_err();
        assert!(matches!(
            err,
            AacDecoderError::UnsupportedConfig(
                "only AAC-LC AudioSpecificConfig is supported"
            )
        ));
    }

    #[test]
    fn malformed_access_unit_returns_decode_error() {
        let mut decoder = AacLcDecoder::new(&ASC_AAC_LC_STEREO_48K).unwrap();
        let err = decoder.decode_access_unit(&[0; 16], 0).unwrap_err();
        assert!(matches!(err, AacDecoderError::Decode(_)));
    }
}
