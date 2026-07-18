//! Load the converted Kokoro weights and check they are what the graph expects.
//!
//!     cargo run --release --bin kokoro_probe -- kokoro-v1_0.mktts af_heart.mkvoice

use makepad_tts::kokoro::weights::Weights;

fn main() {
    let mut args = std::env::args().skip(1);
    let model_path = args.next().unwrap_or_else(|| "kokoro-v1_0.mktts".into());
    let voice_path = args.next().unwrap_or_else(|| "af_heart.mkvoice".into());

    let started = std::time::Instant::now();
    let weights = match Weights::load(&model_path) {
        Ok(weights) => weights,
        Err(err) => {
            println!("load failed: {err:?}");
            return;
        }
    };
    let params: usize = weights.names().filter_map(|n| weights.info(n)).map(|i| i.numel()).sum();
    println!(
        "loaded {} tensors, {:.2}M params in {:.2}s",
        weights.len(),
        params as f32 / 1e6,
        started.elapsed().as_secs_f32()
    );

    println!("\n--- shapes the graph depends on ---");
    for name in [
        "text_encoder.module.embedding.weight",
        "bert.module.embeddings.word_embeddings.weight",
        "bert_encoder.module.weight",
        "text_encoder.module.lstm.weight_ih_l0",
        "predictor.module.lstm.weight_ih_l0",
        "decoder.module.decode.0.norm1.fc.weight",
    ] {
        match weights.shape(name) {
            Some(shape) => println!("  {name:<52} {shape:?}"),
            None => println!("  {name:<52} MISSING"),
        }
    }

    println!("\n--- weight-norm reconstruction (W = g * v / ||v||) ---");
    for prefix in [
        "decoder.module.decode.0.conv1",
        "decoder.module.decode.0.conv1x1",
        "text_encoder.module.cnn.0.0",
    ] {
        match weights.weight_norm(prefix) {
            Ok(weight) => {
                let shape = weights.shape(&format!("{prefix}.weight_v")).unwrap();
                let peak = weight.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                let finite = weight.iter().all(|x| x.is_finite());
                // ||W[o]|| should equal g[o]: that is what weight norm means.
                let out = shape[0];
                let per = weight.len() / out;
                let row_norm = weight[..per].iter().map(|x| x * x).sum::<f32>().sqrt();
                let g = weights.get(&format!("{prefix}.weight_g")).unwrap()[0];
                println!(
                    "  {prefix:<38} {shape:?} peak={peak:.4} finite={finite} \
                     ||W[0]||={row_norm:.5} g[0]={g:.5}"
                );
            }
            Err(err) => println!("  {prefix:<38} {err:?}"),
        }
    }

    println!("\n--- voice pack ---");
    match Weights::load(&voice_path) {
        Ok(voice) => {
            let name = voice.names().next().unwrap_or("").to_string();
            let shape = voice.shape(&name).unwrap_or(&[]);
            println!("  {name}: {shape:?}");
            if let Some(data) = voice.get(&name) {
                // One 256-wide style vector per phoneme count; 128 for the
                // predictor, 128 for the decoder.
                let row = 10usize;
                let slice = &data[row * 256..row * 256 + 6];
                println!("  style[{row}][..6] = {slice:?}");
            }
        }
        Err(err) => println!("  {err:?}"),
    }
}
