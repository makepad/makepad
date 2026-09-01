// End-to-end VLM probe: image + question -> answer, fully on makepad-ggml.
//
// Pipeline: preprocess -> vision tower -> <|vision_start|> [embeddings]
// <|vision_end|> ChatML wrap -> M-RoPE prefill -> greedy generation.
// Compare against: llama-mtmd-cli -m <model> --mmproj <mmproj> --image <img> -p <prompt>
//
// usage: vlm-probe <model.gguf> <mmproj.gguf> <image.ppm> <question> [--max-new-tokens N] [--max-context N]

use makepad_ai_llm::{
    preprocess_rgb8, LlamaSession, LlamaSessionConfig, VisionConfig, VisionTower,
};
use std::io::Write;
use std::time::Instant;

fn read_ppm(path: &str) -> (Vec<u8>, usize, usize) {
    let data = std::fs::read(path).expect("cannot read image");
    let mut fields = Vec::new();
    let mut pos = 0usize;
    while fields.len() < 4 {
        while pos < data.len() && (data[pos] as char).is_ascii_whitespace() {
            pos += 1;
        }
        if data[pos] == b'#' {
            while pos < data.len() && data[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while pos < data.len() && !(data[pos] as char).is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    pos += 1;
    assert_eq!(fields[0], "P6");
    let w: usize = fields[1].parse().unwrap();
    let h: usize = fields[2].parse().unwrap();
    assert_eq!(fields[3], "255");
    (data[pos..pos + w * h * 3].to_vec(), w, h)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!(
            "usage: {} <model.gguf> <mmproj.gguf> <image.ppm> <question> [--max-new-tokens N] [--max-context N]",
            args[0]
        );
        std::process::exit(1);
    }
    let model_path = &args[1];
    let mmproj_path = &args[2];
    let image_path = &args[3];
    let question = &args[4];
    let mut max_new_tokens = 256usize;
    let mut max_context = 8192u32;
    let mut i = 5;
    while i < args.len() {
        match args[i].as_str() {
            "--max-new-tokens" => {
                i += 1;
                max_new_tokens = args[i].parse().unwrap();
            }
            "--max-context" => {
                i += 1;
                max_context = args[i].parse().unwrap();
            }
            other => panic!("unknown arg {other}"),
        }
        i += 1;
    }

    // 1. vision leg
    let (rgb, w, h) = read_ppm(image_path);
    let gguf = makepad_ai_llm::GgufFile::open(mmproj_path).expect("open mmproj");
    let vision_config = VisionConfig::from_gguf(&gguf).expect("vision config");
    let t0 = Instant::now();
    let prepared = preprocess_rgb8(&rgb, w, h, &vision_config).expect("preprocess");
    let mut tower = VisionTower::load(mmproj_path).expect("vision tower");
    let embeddings = tower.encode(&prepared).expect("encode image");
    eprintln!(
        "vision: {}x{} -> {}x{} tokens ({}) in {:.2}s",
        w,
        h,
        prepared.tokens_w(),
        prepared.tokens_h(),
        prepared.n_tokens(),
        t0.elapsed().as_secs_f64()
    );

    // 2. text leg
    let t1 = Instant::now();
    let mut session = LlamaSession::load(
        model_path,
        LlamaSessionConfig {
            max_context: Some(max_context),
            ..LlamaSessionConfig::default()
        },
    )
    .expect("load llama session");
    eprintln!("session loaded in {:.2}s", t1.elapsed().as_secs_f64());

    // 3. ChatML prefill with the image span; non-thinking prefill for
    // deterministic short answers
    let prefix = "<|im_start|>user\n<|vision_start|>";
    let suffix = format!(
        "<|vision_end|>{question}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    );

    let t2 = Instant::now();
    let prefix_ids = session.vocab().tokenize(prefix, false, true).expect("tokenize prefix");
    session.append_tokens(&prefix_ids).expect("prefill prefix");
    session
        .append_image_embeddings(&embeddings, prepared.tokens_w(), prepared.tokens_h())
        .expect("prefill image");
    let suffix_ids = session.vocab().tokenize(&suffix, false, true).expect("tokenize suffix");
    session.append_tokens(&suffix_ids).expect("prefill suffix");
    eprintln!(
        "prefill: {} tokens ({} image) in {:.2}s",
        session.token_count(),
        prepared.n_tokens(),
        t2.elapsed().as_secs_f64()
    );

    // 4. greedy generation
    let t3 = Instant::now();
    let mut generated = 0usize;
    let mut decoder = session.vocab().text_decoder();
    while generated < max_new_tokens {
        let Some(token) = session.next_greedy_token().expect("generate") else {
            break;
        };
        generated += 1;
        if let Some(text) = decoder.push_token(session.vocab(), token) {
            print!("{text}");
            std::io::stdout().flush().ok();
        }
    }
    println!();
    let dt = t3.elapsed().as_secs_f64();
    eprintln!(
        "generated {} tokens in {:.2}s ({:.1} tok/s)",
        generated,
        dt,
        generated as f64 / dt.max(1e-9)
    );
}
