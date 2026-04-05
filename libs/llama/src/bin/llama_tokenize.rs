use std::ffi::OsString;
use std::path::PathBuf;

use makepad_llama::{LlamaModel, LlamaVocab};

struct Args {
    model_path: PathBuf,
    prompt: String,
    ids_only: bool,
    no_bos: bool,
    parse_special: bool,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-tokenize failed: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(std::env::args_os())?;
    let model = LlamaModel::load(&args.model_path)?;
    let vocab = LlamaVocab::from_model(&model)?;
    let token_ids = vocab.tokenize(&args.prompt, !args.no_bos, args.parse_special)?;

    if args.ids_only {
        println!("{:?}", token_ids);
        return Ok(());
    }

    for token_id in token_ids {
        let piece = vocab
            .escaped_piece(token_id)
            .unwrap_or_else(|| "<invalid-token-id>".to_owned());
        println!("{token_id}\t{piece}");
    }
    Ok(())
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Args, Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let _exe = args.next();

    let mut model_path = None;
    let mut prompt = None;
    let mut prompt_parts = Vec::new();
    let mut ids_only = false;
    let mut no_bos = false;
    let mut parse_special = true;

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-p" | "--prompt" => {
                let value = args.next().ok_or("--prompt requires a value")?;
                prompt = Some(value.to_string_lossy().into_owned());
            }
            "--ids" => {
                ids_only = true;
            }
            "--no-bos" => {
                no_bos = true;
            }
            "--parse-special" => {
                parse_special = true;
            }
            "--no-parse-special" => {
                parse_special = false;
            }
            _ if model_path.is_none() => {
                model_path = Some(PathBuf::from(arg));
            }
            _ => {
                prompt_parts.push(arg.to_string_lossy().into_owned());
            }
        }
    }

    let model_path = model_path.ok_or_else(|| {
        print_usage();
        "usage: llama-tokenize <model.gguf> [--ids] [--no-bos] [--no-parse-special] [--prompt TEXT | prompt words ...]"
    })?;
    let prompt = prompt.unwrap_or_else(|| prompt_parts.join(" "));
    if prompt.is_empty() {
        return Err("missing prompt text".into());
    }

    Ok(Args {
        model_path,
        prompt,
        ids_only,
        no_bos,
        parse_special,
    })
}

fn print_usage() {
    eprintln!(
        "usage: llama-tokenize <model.gguf> [--ids] [--no-bos] [--no-parse-special] [--prompt TEXT | prompt words ...]"
    );
}
