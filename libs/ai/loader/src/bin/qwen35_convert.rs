use makepad_ai_loader::quant::TensorType;
use makepad_ai_loader::qwen35_convert::{self, Options, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().unwrap_or_default();
    if command == "--help" || command == "-h" || command.is_empty() {
        println!(
            "qwen35-convert <convert|validate> --input CHECKPOINT_DIR --output MODEL.gguf
  [--reference MAIN.gguf] [--matrix-type q4_k|q5_k|q6_k|q8_0] [--mix a|b|c|d]
  [--rule GROUP[@START..END]=TYPE] (repeatable; last match wins)

Groups: default, ffn, ffn_up, ffn_gate, ffn_down, attention, attn_q, attn_k,
attn_v, attn_o, ssm, ssm_in, ssm_gate, ssm_alpha, ssm_beta, ssm_out, embedding,
head, mtp. Layer ranges are zero-based, end-exclusive. Main groups exclude MTP;
head selects the embedding too when tied. Vectors and convolution stay F32.
Precedence: reference/default, matrix-type, mix, explicit rules.
Mix a: Q4 up/gate, Q5 rest/MTP, Q6 embedding/head, Q8 SSM alpha/beta.
Mix b: a + Q4 down in the middle half of main layers.
Mix c: a + Q4 down in all main layers. Mix d: c + Q6 SSM in/out.

qwen35-convert <convert-vision|validate-vision> --input CHECKPOINT_DIR
  --output MMPROJ.gguf --reference MMPROJ-F16.gguf [--matrix-type f16|q8_0]
Dense vision matrices use the selected type (default f16). Patch kernels stay
F16; positions, normalization and biases stay F32 for runtime compatibility.

Both modes accept [--threads N] [--preserve-reference-provenance].
The text reference supplies tensor types and tokenizer compatibility metadata;
the vision reference supplies checked architecture and preprocessing metadata.
All tensor values come from the checkpoint. Text files always include MTP.
Validation reads every weight with the runtime dequantizers and writes a TSV
report to stdout. Conversion refuses to overwrite output or its .partial file.
No downloads."
        );
        return Ok(());
    }
    let mut options = Options {
        input: PathBuf::new(),
        output: PathBuf::new(),
        reference: None,
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        preserve_reference_provenance: false,
        matrix_type: None,
        mix: None,
        rules: Vec::new(),
        vision: command == "convert-vision" || command == "validate-vision",
    };
    while let Some(arg) = args.next() {
        if arg == "--preserve-reference-provenance" {
            options.preserve_reference_provenance = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {}", arg.to_string_lossy()))?;
        match arg.to_str() {
            Some("--input") => options.input = value.into(),
            Some("--output") => options.output = value.into(),
            Some("--reference") => options.reference = Some(value.into()),
            Some("--threads") => {
                options.threads = value.to_str().ok_or("invalid thread count")?.parse()?
            }
            Some("--matrix-type") => {
                options.matrix_type = Some(if value == "f16" {
                    TensorType::F16
                } else {
                    qwen35_convert::matrix_type(value.to_str().ok_or("invalid matrix type")?)?
                });
            }
            Some("--mix") => options.mix = Some(value.to_str().ok_or("invalid mix")?.into()),
            Some("--rule") => options
                .rules
                .push(value.to_str().ok_or("invalid rule")?.parse()?),
            _ => return Err(format!("unknown option {}", arg.to_string_lossy()).into()),
        }
    }
    if options.input.as_os_str().is_empty() || options.output.as_os_str().is_empty() {
        return Err("--input and --output are required".into());
    }
    match command.to_str() {
        Some("convert" | "convert-vision") => qwen35_convert::convert(&options),
        Some("validate" | "validate-vision") => {
            qwen35_convert::validate(&options, &mut std::io::stdout().lock())
        }
        _ => Err("expected convert, validate, convert-vision or validate-vision".into()),
    }
}
