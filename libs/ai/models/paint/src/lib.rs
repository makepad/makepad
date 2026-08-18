//! Generative PBR backend core.
//!
//! This crate is the model-independent core for the generative-PBR domain:
//! mesh + reference image -> explicit PBR channel maps. It carries
//!
//! * the channel **contract** ([`contract`]): which maps exist, their color
//!   spaces, glTF packing, and an explicit machine-readable statement of where
//!   each channel came from — including *honest absence* when a model cannot
//!   generate a channel;
//! * a **deterministic test backend** ([`test_backend`]): seeded, byte-stable
//!   output used by service plumbing and UI work without any GPU or weights;
//! * the model-independent pipeline stages of the **Hunyuan3D-Paint-2.1**
//!   native port: camera math ([`camera`]), view selection ([`view_select`]),
//!   CPU geometry rendering ([`raster`]), the zero-terminal-SNR v-prediction
//!   DDIM schedule ([`schedule`]), the 15-step / 3-branch CFG loop
//!   ([`denoise`]), and UV back-projection baking ([`bake`]);
//! * the pinned checkpoint specs ([`hunyuan`], [`trellis2`]): exact revisions,
//!   file sizes, sha256, architecture constants, runtime defaults, and license
//!   identity/URLs surfaced as provenance. Hunyuan checkpoint provisioning and
//!   real-model execution require an explicit acknowledgement of the pinned
//!   license digest and otherwise fail closed.
//!
//! A complete native Hunyuan executor does **not** exist in this crate yet.
//! `cuda-taps` builds frozen numerical taps plus VAE and the UNet down+mid
//! prefix; it does not register or advertise a usable model. The
//! deterministic backend remains available for service/UI integration.

pub mod bake;
pub mod camera;
pub mod cond_assembly;
pub mod contract;
pub mod denoise;
pub mod dual_stream;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod cuda_unet;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod sd_vae;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod unet_first;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod unet_attn;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod unet_extras;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod unet_forward;
#[cfg(all(feature = "cuda-taps", any(target_os = "linux", target_os = "windows")))]
pub mod native_exec;
pub mod dino_proj;
pub mod dino_vit;
pub mod digest;
pub mod hunyuan;
pub mod mesh;
pub mod numerical_fixtures;
pub mod pipeline;
pub mod png;
pub mod raster;
pub mod safetensors;
pub mod schedule;
pub mod test_backend;
pub mod torch_bin;
pub mod trellis2;
pub mod unet_keys;
pub mod weight_plan;
pub mod view_select;

pub use contract::{
    ChannelOrigin, ColorSpace, ContractError, PbrChannel, PbrMap, PbrMaterialSet, PixelFormat,
};
pub use pipeline::{
    admission_estimate, fits_24g_service, native_implementation_status, AdmissionEstimate,
    ExecStatus, HunyuanPaintPipeline, MemoryProfile, NativeImplementationStatus, PaintConfig,
    MockPaintExec, PaintExecutionKind, PaintInputs, PaintModelExec, UnavailableExec,
};
pub use test_backend::{DeterministicTestPbr, PbrError, PbrGenerator, PbrJobParams, PbrProgress, PbrStage};
