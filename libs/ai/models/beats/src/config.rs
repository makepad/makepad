//! Fixed Beat This! signal and network geometry.

pub const SAMPLE_RATE: u32 = 22_050;
pub const N_FFT: usize = 1024;
pub const HOP_LENGTH: usize = 441;
pub const MEL_BINS: usize = 128;
pub const FFT_BINS: usize = N_FFT / 2 + 1;
pub const F_MIN: f64 = 30.0;
pub const F_MAX: f64 = 11_000.0;
pub const LOG_MULTIPLIER: f32 = 1000.0;
pub const FRAME_RATE: f64 = SAMPLE_RATE as f64 / HOP_LENGTH as f64;

pub const CHUNK_FRAMES: usize = 1500;
pub const BORDER_FRAMES: usize = 6;
pub const CHUNK_STRIDE: usize = CHUNK_FRAMES - 2 * BORDER_FRAMES;

pub const HEAD_DIM: usize = 32;
pub const STEM_DIM: usize = 32;
pub const STEM_BLOCKS: usize = 3;
pub const MAIN_LAYERS: usize = 6;
pub const FF_MULT: usize = 4;
pub const ROPE_THETA: f32 = 10_000.0;
pub const NORM_EPS: f32 = 1e-12;
pub const BATCH_NORM_EPS: f32 = 1e-5;

pub const STEM_FREQS: [usize; 4] = [32, 16, 8, 4];
pub const STEM_CHANNELS: [usize; 4] = [32, 64, 128, 256];
pub const FRONTEND_FEATURES: usize = 256 * 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSize {
    Final,
    Small,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeatsConfig {
    pub size: ModelSize,
    pub transformer_dim: usize,
}

impl BeatsConfig {
    pub const FINAL: Self = Self {
        size: ModelSize::Final,
        transformer_dim: 512,
    };
    pub const SMALL: Self = Self {
        size: ModelSize::Small,
        transformer_dim: 128,
    };

    pub fn heads(self) -> usize {
        self.transformer_dim / HEAD_DIM
    }

    pub fn ff_inner(self) -> usize {
        self.transformer_dim * FF_MULT
    }
}
