//! Weight-file format parsers. Each submodule is a standalone reader; see
//! /aiarch.md §1 for which upstream crate each was moved or staged from.

pub mod gguf;
pub mod gguf_write;
pub mod npy;
pub mod onnx;
pub mod paddle;
pub mod safetensors;
pub mod torch;
pub mod torch_pth;
