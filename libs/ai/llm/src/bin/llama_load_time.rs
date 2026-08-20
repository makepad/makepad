//! Times a full gguf weight load and fingerprints the resulting arena.
//!
//! The point is an A/B that cannot lie: the same binary runs the old
//! per-tensor reader (`MAKEPAD_LOADER_BULK=0`) and the bulk reader, and
//! prints both the wall time and a checksum of every loaded byte. A faster
//! load with a different checksum is a bug, not a win.
//!
//! usage: llama-load-time <model.gguf> [--repeat:<n>] [--no-checksum]

use std::time::Instant;

use makepad_ai_llm::LlamaModel;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(path) = args.first() else {
        eprintln!("usage: llama-load-time <model.gguf> [--repeat:<n>] [--no-checksum]");
        std::process::exit(2);
    };
    let mut repeat = 1usize;
    let mut checksum = true;
    for arg in &args[1..] {
        if let Some(rest) = arg.strip_prefix("--repeat:") {
            repeat = rest.parse().unwrap_or(1);
        } else if arg == "--no-checksum" {
            checksum = false;
        }
    }

    println!(
        "bulk reader: {}",
        match std::env::var("MAKEPAD_LOADER_BULK").as_deref() {
            Ok("0") => "off (per-tensor seek+read)",
            _ => "on",
        }
    );

    let model = match LlamaModel::load(path) {
        Ok(model) => model,
        Err(err) => {
            eprintln!("llama-load-time: {}", err);
            std::process::exit(1);
        }
    };
    let plan = match model.execution_plan() {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!("llama-load-time: execution_plan: {}", err);
            std::process::exit(1);
        }
    };
    let bytes = plan.full_weights.total_bytes;
    println!(
        "model: {} ({} tensors, {:.2} GB of weights)",
        path,
        plan.full_weights.tensors.len(),
        bytes as f64 / (1u64 << 30) as f64
    );

    for iteration in 0..repeat {
        let started = Instant::now();
        let loaded = match plan.full_weights.allocate_and_load_with_extra(&model.gguf, 0) {
            Ok(loaded) => loaded,
            Err(err) => {
                eprintln!("llama-load-time: load: {}", err);
                std::process::exit(1);
            }
        };
        let elapsed = started.elapsed().as_secs_f64();
        let gb = bytes as f64 / (1u64 << 30) as f64;
        print!(
            "load[{}]: {:.3} s  {:.2} GB/s",
            iteration,
            elapsed,
            gb / elapsed
        );
        if checksum {
            let started = Instant::now();
            let sum = fnv1a(loaded.ctx.mem_buffer());
            print!("  checksum {:016x} ({:.2} s)", sum, started.elapsed().as_secs_f64());
        }
        println!();
        let started = Instant::now();
        drop(loaded);
        println!("  arena drop: {:.3} s", started.elapsed().as_secs_f64());
    }
}

/// FNV-1a over 8-byte words — fast enough to run on a 16 GB arena, and
/// sensitive to any byte landing in the wrong place.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let word = u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8]));
        hash ^= word;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    for byte in chunks.remainder() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}
