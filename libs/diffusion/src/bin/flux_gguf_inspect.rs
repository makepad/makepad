//! Print architecture, tensor inventory, and inferred Flux DiT config
//! from a city96 / ComfyUI-GGUF file. Does not run the transformer.

use makepad_diffusion::flux_gguf::inspect;
use std::env;

fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: flux-gguf-inspect <model.gguf>");
        std::process::exit(2);
    });
    match inspect(&path) {
        Ok(info) => {
            println!("path: {}", info.path);
            println!("architecture: {}", info.architecture);
            println!("file_type: {}", info.file_type);
            println!(
                "tensors: {} (quantized {} raw {})",
                info.tensor_count, info.quantized_tensors, info.raw_tensors
            );
            println!("tensor_bytes: {}", info.tensor_bytes);
            println!("name_style: {:?}", info.transformer.tensor_name_style);
            println!(
                "canonical_tensors: {}",
                info.transformer.canonical_tensor_count
            );
            let c = info.transformer.config;
            println!(
                "config: hidden={} heads={} head_dim={} depth={} single={} in={} out={} ctx={} vec={} guidance={}",
                c.hidden_size,
                c.num_heads,
                c.head_dim(),
                c.depth,
                c.depth_single_blocks,
                c.in_channels,
                c.out_channels,
                c.context_in_dim,
                c.vec_in_dim,
                c.guidance_embed
            );
        }
        Err(err) => {
            eprintln!("flux-gguf-inspect: {err}");
            std::process::exit(1);
        }
    }
}
