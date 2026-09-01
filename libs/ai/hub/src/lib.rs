//! makepad-asset-ai: a small HTTP service that wraps AI content generation
//! (image / mesh / video / audio) behind a port, one instance per GPU box.
//!
//! Architecture (see the END DELIVERABLE in the content-gen campaign notes):
//! - `registry`: data-driven model table (model id -> HF repo/files -> local
//!   cache layout). Adding a model = adding a registry entry + a backend impl.
//! - `download`: on-demand, resumable HuggingFace downloader; progress is
//!   surfaced through the `/models` endpoint while a model is fetching.
//! - `backend`: the `ContentBackend` trait; `testpattern` (procedural PNG, no
//!   GPU) always ships, `flux` is behind the `flux` cargo feature and calls
//!   libs/diffusion's `FluxPipeline`.
//! - `jobs`: async job model — generations take seconds to minutes, one GPU
//!   runs one job at a time (FIFO queue with a queue-or-reject policy).
//! - `server`: the HTTP endpoints, built on the in-repo `makepad-network`
//!   `HttpServer` (same pattern as studio/hub gateway.rs).
//! - `client`: the `ContentProvider` abstraction the sandbox integrates
//!   against, with `LocalService` (this service over HTTP) as first impl.
//!   Cloud providers (fal / replicate / hosted image APIs) implement the same
//!   trait later so machines without local GPU boxes work identically.
//!
//! HTTP API summary (JSON via makepad-micro-serde):
//! - `GET  /health`          -> service name, version, GPU name, VRAM, loaded models
//! - `GET  /models`          -> registry list + per-model state
//!                              (absent / downloading{progress} / ready / loaded / error)
//! - `POST /generate`        -> `{model, prompt, width, height, seed, steps, ...}` -> `{job_id}`
//! - `GET  /job/<id>`        -> queued / running{stage, progress} / live{stage, frames_in,
//!                              frames_out, fps} / done{artifacts} / error
//! - `GET  /artifact/<id>`   -> artifact bytes with the right content-type
//! - `POST /realtime`        -> `{model, width, height, prompt, strength, steps, loop_mode,
//!                              ...}` -> `{job_id, ws_path}`; then `GET <ws_path>` (websocket
//!                              upgrade) streams input/output video frames + live control —
//!                              see `protocol.rs`'s wire doc block and `crate::realtime`.

pub mod backend;
pub mod body_backend;
pub mod body_native_backend;
pub mod chat_wire;
pub use makepad_base64;
pub mod client;
mod child_process;
pub mod control_image;
pub mod depth_backend;
pub mod download;
pub mod error;
pub mod fabric;
pub mod fleet;
pub mod gpu;
pub mod home;
pub mod hub;
#[cfg(feature = "llm")]
pub mod hub_chat;
pub mod machine;
pub mod fast_backend;
pub mod h3_backend;
pub mod http_client;
pub mod jobs;
pub mod lane_advert;
pub mod lease;
pub mod indextts_backend;
pub mod kokoro_backend;
#[cfg(feature = "stt")]
pub mod whisper_backend;
#[cfg(feature = "speech")]
pub mod speech;
pub mod llm_backend;
#[cfg(feature = "llm")]
pub mod local_llm;
pub mod matte_backend;
pub mod segment_backend;
pub mod upscale_backend;
pub mod vision_backend;
pub mod ocr_backend;
pub mod enhance_backend;
pub mod motion_backend;
#[cfg(feature = "motion-native")]
pub mod motion_native_backend;
#[cfg(feature = "motion-native")]
pub mod motion_retarget;
#[cfg(feature = "paint")]
pub mod paint_backend;
pub mod peer;
pub mod pipe;
pub mod peer_fetch;
pub mod peer_serve;
pub mod protocol;
pub mod providers;
pub mod realtime;
pub mod realtime_wire;
pub mod ram;
pub mod registry;
pub mod resample;
pub mod residency;
pub mod rig_backend;
#[cfg(feature = "rig-native")]
pub mod rig_native_backend;
pub mod ace_backend;
pub mod moss_backend;
pub mod sa3_backend;
pub mod discovery;
pub mod server;
pub mod sha256;
pub mod splat_backend;
pub mod subproc_img;
pub mod testpattern;
pub mod trellis_backend;
pub mod wav;
pub mod woosh_backend;
pub mod music3_backend;
pub mod world_backend;

#[cfg(feature = "flux")]
pub mod flux_backend;
#[cfg(feature = "flux")]
pub mod flux2_backend;
#[cfg(feature = "flux")]
pub mod control_backend;
#[cfg(feature = "flux")]
pub mod inpaint_backend;

pub use error::AssetAiError;
pub use server::{start_service, ServiceConfig, ServiceHandle};

pub const SERVICE_NAME: &str = "makepad-asset-ai";
pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const DEFAULT_PORT: u16 = 8765;
