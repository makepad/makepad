//! THE Metal shader store for Makepad AI models (aiarch.md §1 + §8b ring 1).
//!
//! This crate is, for now, purely a shader/build store: `shaders/ggml/`
//! (ggml-metal.metal + ggml-common.h + ggml-metal-impl.h, absorbed from
//! `libs/ggml/src/backend/metal/ggml/`) and `shaders/mlx_qmm/` (the MLX
//! steel_qmm kernels, absorbed from `libs/ggml/src/backend/metal/mlx_qmm/`),
//! plus `build.rs` (absorbed from `libs/ggml/build.rs::build_metallib`),
//! which compiles them into a metallib on macOS.
//!
//! The Rust Metal launch surface (device/runtime/compiled-graph executor,
//! selector, gpu_tensor, affine, qmm) deliberately stays in `makepad-ggml`
//! this lane; it moves here in ring 2. Until then this crate has no Rust
//! API of its own — `makepad-ggml` reaches the build artifacts through the
//! links-metadata handshake documented in `build.rs`, exactly mirroring the
//! `makepad-ai-cuda` -> `makepad-ggml` handshake for CUDA kernels (lane T4).
