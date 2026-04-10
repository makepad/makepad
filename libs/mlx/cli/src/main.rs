use makepad_mlx::chat::{GemmaChatRole, GemmaChatSession};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

fn default_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
}

fn usage() -> &'static str {
    "Usage: mlx-cli [model.safetensors] [--max-new-tokens N]"
}

fn print_block(prefix: &str, text: &str) {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        println!("{prefix}");
        return;
    }
    let mut lines = trimmed.lines();
    if let Some(first) = lines.next() {
        println!("{prefix}{first}");
    }
    for line in lines {
        println!("  {line}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut model_path = default_model_path();
    let mut max_new_tokens = 128usize;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--max-new-tokens" => {
                let value = args.next().ok_or("--max-new-tokens requires a value")?;
                max_new_tokens = value.parse::<usize>()?;
            }
            value if value.starts_with("--") => {
                return Err(format!("unknown option: {value}\n{}", usage()).into());
            }
            value => {
                model_path = PathBuf::from(value);
            }
        }
    }

    eprintln!("loading model={}...", model_path.display());
    let mut session = GemmaChatSession::load(&model_path, max_new_tokens)?;
    println!("model={}", model_path.display());
    println!("max_new_tokens={}", session.max_new_tokens());
    println!("commands: /reset /history /exit");
    println!("ready");

    let stdin = io::stdin();
    loop {
        print!("you> ");
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        match input {
            "/exit" | "/quit" => break,
            "/reset" => {
                session.reset();
                println!("history cleared");
                continue;
            }
            "/history" => {
                for message in session.messages() {
                    let prefix = match message.role {
                        GemmaChatRole::User => "user> ",
                        GemmaChatRole::Assistant => "assistant> ",
                    };
                    print_block(prefix, message.content.as_ref());
                }
                continue;
            }
            _ => {}
        }

        print!("assistant> ");
        io::stdout().flush()?;
        let output = session.send_user_message_streaming(input, |delta| {
            print!("{delta}");
            io::stdout().flush()?;
            Ok(())
        })?;
        if output.generated_text.is_empty() {
            println!();
        } else {
            println!();
        }
    }

    Ok(())
}
