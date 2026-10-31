use makepad_ai_speech::whisper::{quantize_ggml, WeightQuantization};
use std::io::{BufReader, BufWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: whisper-quantize <f16.bin> <output.bin> <q8_0|q5_k|q4_k|q5_0>");
        std::process::exit(2);
    }
    let format = match args[2].as_str() {
        "q8_0" => WeightQuantization::Q8_0,
        "q5_0" => WeightQuantization::Q5_0,
        "q5_k" => WeightQuantization::Q5K,
        "q4_k" => WeightQuantization::Q4K,
        _ => return Err("expected q8_0, q5_k, q4_k or q5_0 (CPU only)".into()),
    };
    let mut input = BufReader::new(std::fs::File::open(&args[0])?);
    // Never truncate an existing model, including an alias of the input.
    let output = std::fs::OpenOptions::new().write(true).create_new(true).open(&args[1])?;
    let mut output = BufWriter::new(output);
    match quantize_ggml(&mut input, &mut output, format) {
        Ok(stats) => {
            output.into_inner()?.sync_all()?;
            println!("{format:?}: {} matrices / {} tensors; weight bytes {} -> {}",
                stats.matrices, stats.tensors, stats.input_weight_bytes, stats.output_weight_bytes);
            Ok(())
        }
        Err(error) => {
            eprintln!("conversion failed; output is incomplete: {}", args[1]);
            Err(error.into())
        }
    }
}
