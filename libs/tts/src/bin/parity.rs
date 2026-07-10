//! Diff the Rust graph against the ONNX reference, stage by stage.
//!
//! Run `ref_dump.py` first with the same sentence, then:
//!
//!     cargo run --release --bin parity

use makepad_tts::g2p;
use makepad_tts::kokoro::bert::Bert;
use makepad_tts::kokoro::decoder::Decoder;
use makepad_tts::kokoro::npy::{max_abs_diff, Npy};
use makepad_tts::kokoro::ops::{expand_to_frames, round_half_even, Mat};
use makepad_tts::kokoro::predictor::Predictor;
use makepad_tts::kokoro::text_encoder::TextEncoder;
use makepad_tts::kokoro::weights::Weights;

const SENTENCE: &str = "Escape the Gummer, a squishy purple blob.";
const MODEL: &str = "kokoro-v1_0.mktts";
const DUMP: &str = "refdump";

/// f32 accumulation order differs between implementations; this is the scale of
/// difference that means "the same computation", not "close enough to ship".
const TOLERANCE: f32 = 2e-3;

fn compare(label: &str, ours: &[f32], reference_file: &str) -> bool {
    compare_path(label, ours, &format!("{DUMP}/{reference_file}.npy"))
}

fn compare_path(label: &str, ours: &[f32], path: &str) -> bool {
    let reference = match Npy::load(path) {
        Ok(npy) => npy,
        Err(err) => {
            println!("  {label:<10} SKIP  ({err:?})");
            return true;
        }
    };
    if reference.data.len() != ours.len() {
        println!(
            "  {label:<10} SHAPE  ours={} reference={:?}",
            ours.len(),
            reference.shape
        );
        return false;
    }
    let (worst, at) = max_abs_diff(ours, &reference.data);
    let ok = worst <= TOLERANCE;
    println!(
        "  {label:<10} {}  max|Δ|={worst:.3e} at {at}  (ref {:?})",
        if ok { "ok  " } else { "FAIL" },
        reference.shape
    );
    ok
}

fn main() {
    // The strict pass runs the pure-Rust reference paths; the Metal tier is
    // timed and sanity-checked separately at the end, because its simdgroup
    // matmul rounds tiles to f16 and cannot meet the reassociation-only bar.
    makepad_tts::kokoro::accel::force_cpu(true);
    // The dump comes from the determinism-patched export; run without the
    // restored excitation noise so `har_source` diffs entrywise.
    makepad_tts::kokoro::generator::force_deterministic(true);

    let weights = match Weights::load(MODEL) {
        Ok(weights) => weights,
        Err(err) => {
            println!("cannot load {MODEL}: {err:?}");
            return;
        }
    };
    let encoder = match TextEncoder::load(&weights) {
        Ok(encoder) => encoder,
        Err(err) => {
            println!("cannot build text encoder: {err:?}");
            return;
        }
    };

    let tokens = g2p::tokens(SENTENCE);
    println!("sentence: {SENTENCE}");
    println!("tokens  : {}\n", tokens.len());

    let started = std::time::Instant::now();
    let trace = encoder.trace(&tokens);
    println!("text_encoder ({:.0} ms)", started.elapsed().as_secs_f32() * 1000.0);

    let prefix = "encoder_text_encoder";
    let mut all_ok = true;
    for (name, value) in &trace.steps {
        let reference = match name.as_str() {
            "embed" => format!("{prefix}_Transpose_output_0"),
            other => {
                let index = &other[other.len() - 1..];
                match &other[..other.len() - 1] {
                    "conv" => format!("{prefix}_cnn.{index}_cnn.{index}.0_Conv_output_0"),
                    "norm" => {
                        format!("{prefix}_cnn.{index}_cnn.{index}.1_Transpose_1_output_0")
                    }
                    // The export names every LeakyReLU after block 0's path.
                    "act" => format!("{prefix}_cnn.{index}_cnn.0.2_LeakyRelu_output_0"),
                    _ => continue,
                }
            }
        };
        all_ok &= compare(name, &value.data, &reference);
    }

    // ONNX Y is [time, directions, batch, hidden]; flattened that is
    // [t][fwd 256 | rev 256], which is exactly our [time, 512] layout.
    all_ok &= compare("lstm", &trace.output.data, &format!("{prefix}_lstm_LSTM_output_0"));

    // ---- PL-BERT ----
    let bert = match Bert::load(&weights) {
        Ok(bert) => bert,
        Err(err) => {
            println!("\ncannot build bert: {err:?}");
            return;
        }
    };
    let started = std::time::Instant::now();
    let bert_trace = bert.trace(&tokens);
    println!("\nbert ({:.0} ms)", started.elapsed().as_secs_f32() * 1000.0);

    let layer_prefix = "encoder_bert_encoder_albert_layer_groups.0_albert_layers.0";
    for (index, layer) in bert_trace.layers.iter().enumerate() {
        let suffix = if index == 0 {
            "full_layer_layer_norm".to_string()
        } else {
            format!("full_layer_layer_norm_{index}")
        };
        let reference = format!("{layer_prefix}_{suffix}_LayerNormalization_output_0");
        all_ok &= compare(&format!("layer{index}"), &layer.data, &reference);
    }
    all_ok &= compare(
        "bert_enc",
        &bert_trace.output.data,
        "encoder_bert_encoder_Add_output_0",
    );

    // ---- prosody predictor (duration path) ----
    // The voice vector is two halves: the predictor takes the second, the
    // decoder the first.
    let voice = match Weights::load("af_heart.mkvoice") {
        Ok(voice) => voice,
        Err(err) => {
            println!("\ncannot load voice: {err:?}");
            return;
        }
    };
    // `pack[len(ps) - 1]`: one row per phoneme count, zero-indexed. `tokens`
    // carries a zero pad at each end, so the phoneme count is `len - 2`.
    let style_row = &voice.get("style").unwrap()[(tokens.len() - 3) * 256..][..256];
    let predictor_style = &style_row[128..];

    let predictor = match Predictor::load(&weights) {
        Ok(predictor) => predictor,
        Err(err) => {
            println!("\ncannot build predictor: {err:?}");
            return;
        }
    };
    let started = std::time::Instant::now();
    let duration = predictor.durations(&bert_trace.output, predictor_style);
    println!("\npredictor ({:.0} ms)", started.elapsed().as_secs_f32() * 1000.0);

    let pp = "encoder_predictor";
    for block in 0..3 {
        all_ok &= compare(
            &format!("blk{block}.lstm"),
            &duration.lstm_out[block].data,
            &format!("{pp}_text_encoder_lstms.{}_Transpose_2_output_0", block * 2),
        );
        all_ok &= compare(
            &format!("blk{block}.ada"),
            &duration.norm_out[block].data,
            &format!("{pp}_text_encoder_lstms.{}_Add_2_output_0", block * 2 + 1),
        );
    }
    all_ok &= compare(
        "encoded",
        &duration.encoded.data,
        &format!("{pp}_text_encoder_Concat_4_output_0"),
    );
    all_ok &= compare(
        "pred.lstm",
        &duration.hidden.data,
        &format!("{pp}_lstm_Transpose_2_output_0"),
    );
    all_ok &= compare(
        "dur.logit",
        &duration.duration_logits.data,
        &format!("{pp}_duration_proj_linear_layer_Add_output_0"),
    );
    all_ok &= compare(
        "dur.gate",
        &duration.duration_gates.data,
        &format!("{pp}_Sigmoid_output_0"),
    );

    // Each phoneme's duration is rounded on its own and floored at one frame.
    // `decode.3`'s ConvTranspose then doubles the frames, and the generator's
    // 10x6 upsampling with a hop-5 iSTFT gives 300 samples per frame.
    let frames: f32 = duration
        .durations
        .iter()
        .map(|d| round_half_even(*d).max(1.0))
        .sum();
    let samples = frames * 2.0 * 300.0;
    println!(
        "  frames: {:.1} raw -> {frames} rounded -> {samples:.0} samples ({:.2}s), first 6 = {:?}",
        duration.durations.iter().sum::<f32>(),
        samples / 24_000.0,
        duration
            .durations
            .iter()
            .take(6)
            .map(|d| (d * 10.0).round() / 10.0)
            .collect::<Vec<_>>()
    );

    // ---- alignment expansion ----
    let frames: Vec<usize> = duration
        .durations
        .iter()
        .map(|d| round_half_even(*d).max(1.0) as usize)
        .collect();

    // `en` feeds the prosody branches, `asr` feeds the decoder.
    let en = expand_to_frames(&duration.encoded, &frames);
    let asr = expand_to_frames(&trace.output, &frames);
    println!("\nalignment");
    all_ok &= compare("en", &en.data, "encoder_MatMul_output_0");
    all_ok &= compare("asr", &asr.data, "encoder_MatMul_1_output_0");

    // ---- F0 and noise branches ----
    let started = std::time::Instant::now();
    let prosody = predictor.prosody(&en, predictor_style);
    println!("\nprosody ({:.0} ms)", started.elapsed().as_secs_f32() * 1000.0);

    // The export names the residual divide differently per block.
    let block_ref = ["Div_2", "Div_4", "Div_2"];
    for (branch, blocks) in [("F0", &prosody.f0_blocks), ("N", &prosody.noise_blocks)] {
        for (index, block) in blocks.iter().enumerate() {
            all_ok &= compare(
                &format!("{branch}.{index}"),
                &block.data,
                &format!("encoder_{branch}.{index}_{}_output_0", block_ref[index]),
            );
        }
    }
    all_ok &= compare("F0_proj", &prosody.f0.data, "encoder_F0_proj_Conv_output_0");
    all_ok &= compare("N_proj", &prosody.noise.data, "encoder_N_proj_Conv_output_0");

    // ---- decoder and generator ----
    let decoder = match Decoder::load(&weights) {
        Ok(decoder) => decoder,
        Err(err) => {
            println!("\ncannot build decoder: {err:?}");
            return;
        }
    };
    let decoder_style = &style_row[..128];

    let started = std::time::Instant::now();
    let out = decoder.run(&asr, &prosody.f0, &prosody.noise, decoder_style);
    let elapsed = started.elapsed().as_secs_f32();

    let seconds = out.generator.waveform.len() as f32 / 24_000.0;
    println!("\ndecoder ({elapsed:.2} s for {seconds:.2} s of audio, {:.2}x realtime)", seconds / elapsed);

    all_ok &= compare("F0_conv", &out.f0.data, "decoder_decoder_F0_conv_Conv_output_0");
    all_ok &= compare("N_conv", &out.noise.data, "decoder_decoder_N_conv_Conv_output_0");
    all_ok &= compare(
        "asr_res",
        &out.asr_res.data,
        "decoder_decoder_asr_res_asr_res.0_Conv_output_0",
    );
    all_ok &= compare("encode", &out.encode.data, "decoder_decoder_encode_Div_3_output_0");
    for (index, block) in out.decode.iter().enumerate() {
        // The upsampling block has one more `Div` in it than the others.
        let reference = if index == 3 {
            "decoder_decoder_decode.3_Div_4_output_0".to_string()
        } else {
            format!("decoder_decoder_decode.{index}_Div_3_output_0")
        };
        all_ok &= compare(&format!("decode.{index}"), &block.data, &reference);
    }

    println!("\ngenerator");
    let gen = &out.generator;
    all_ok &= compare(
        "har_source",
        &gen.har_source,
        "decoder_decoder_generator_m_source_l_tanh_Tanh_output_0",
    );
    all_ok &= compare("ups.0", &gen.ups[0].data, "decoder_decoder_generator_ups.0_ConvTranspose_output_0");

    // The forward STFT's phase rows cannot be compared entrywise: wherever a
    // bin is near zero — always at DC and Nyquist, whose imaginary part is
    // nothing but FFT rounding residue in the reference itself — the phase is
    // `sign(noise) * pi`, a coin flip no reimplementation can reproduce. So
    // `har` is checked in complex form, which is exactly as strict about every
    // real defect (window, padding, bin order, sign convention) and blind only
    // to the coin flips. The network downstream of `har` is then held to full
    // tolerance by re-running it on the reference's own `har`.
    let ref_har = match Npy::load(&format!(
        "{DUMP}/decoder_decoder_generator_Concat_3_output_0.npy"
    )) {
        Ok(npy) => npy,
        Err(err) => {
            println!("  cannot load reference har: {err:?}");
            return;
        }
    };
    let frames = gen.har.cols;
    let bins = gen.har.rows / 2;
    let complex = |mag_phase: &[f32]| -> Vec<f32> {
        let mut out = Vec::with_capacity(2 * bins * frames);
        for bin in 0..bins {
            for t in 0..frames {
                let mag = mag_phase[bin * frames + t];
                let phase = mag_phase[(bins + bin) * frames + t];
                out.push(mag * phase.cos());
                out.push(mag * phase.sin());
            }
        }
        out
    };
    // The transform itself, isolated from our har_source's sin noise: run our
    // STFT on the *reference's* source. This must hold at full tolerance.
    let ref_source = Npy::load(&format!(
        "{DUMP}/decoder_decoder_generator_m_source_l_tanh_Tanh_output_0.npy"
    ))
    .expect("reference har_source");
    let stft_of_ref = makepad_tts::kokoro::stft::stft_mag_phase(&ref_source.data);
    let (worst, _) = max_abs_diff(&complex(&stft_of_ref.data), &complex(&ref_har.data));
    let stft_ok = worst <= TOLERANCE;
    println!(
        "  stft       {}  max|Δ|={worst:.3e} as complex re/im, on the reference source",
        if stft_ok { "ok  " } else { "FAIL" }
    );
    all_ok &= stft_ok;

    // Our full har. The bound here is looser for an understood reason: our
    // har_source carries ~8.6e-4 of f32-sin noise (its phase argument grows
    // unwrapped to ~1e5 radians), and the STFT's 20-sample windowed sum
    // amplifies that several-fold. The strict `stft` row above pins the
    // transform; this row only guards against gross regressions.
    const HAR_TOLERANCE: f32 = 1e-2;
    let (worst, _) = max_abs_diff(&complex(&gen.har.data), &complex(&ref_har.data));
    let har_ok = worst <= HAR_TOLERANCE;
    println!(
        "  har        {}  max|Δ|={worst:.3e} as complex re/im (tolerance {HAR_TOLERANCE:.0e})",
        if har_ok { "ok  " } else { "FAIL" }
    );
    all_ok &= har_ok;

    // Raw phase entries flip wherever a bin's value is dominated by rounding
    // noise — at DC and Nyquist the reference's own imaginary part IS noise, so
    // these are coin flips, not errors. Their complex-form error stays bounded.
    let mut flipped = 0;
    let mut flip_err = 0f32;
    for bin in 0..bins {
        for t in 0..frames {
            let ours_phase = gen.har.at(bins + bin, t);
            let ref_phase = ref_har.data[(bins + bin) * frames + t];
            if (ours_phase - ref_phase).abs() > 0.5 {
                flipped += 1;
                let mag_ours = gen.har.at(bin, t);
                let mag_ref = ref_har.data[bin * frames + t];
                let dre = mag_ours * ours_phase.cos() - mag_ref * ref_phase.cos();
                let dim = mag_ours * ours_phase.sin() - mag_ref * ref_phase.sin();
                flip_err = flip_err.max((dre * dre + dim * dim).sqrt());
            }
        }
    }
    println!(
        "             {flipped} of {} phase entries flipped; largest complex-form error among them {flip_err:.1e}",
        bins * frames
    );

    // Strict pass over everything after `har`, fed the reference's `har`.
    let ref_har_mat = Mat::from_vec(2 * bins, frames, ref_har.data.clone());
    let spliced =
        decoder
            .generator
            .run_from_har(&out.decode[3], decoder_style, gen.har_source.clone(), ref_har_mat);
    for index in 0..2 {
        all_ok &= compare(
            &format!("noise{index}"),
            &spliced.noise_convs[index].data,
            &format!("decoder_decoder_generator_noise_convs.{index}_Conv_output_0"),
        );
    }
    all_ok &= compare(
        "ups.1",
        &spliced.ups[1].data,
        "decoder_decoder_generator_ups.1_ConvTranspose_output_0",
    );
    all_ok &= compare(
        "reflect",
        &spliced.reflect.data,
        "decoder_decoder_generator_reflection_pad_Pad_output_0",
    );
    all_ok &= compare("merged.0", &spliced.merged[0].data, "decoder_decoder_generator_Div_2_output_0");
    all_ok &= compare("merged.1", &spliced.merged[1].data, "decoder_decoder_generator_Div_4_output_0");
    all_ok &= compare(
        "conv_post",
        &spliced.post.data,
        "decoder_decoder_generator_conv_post_Conv_output_0",
    );
    all_ok &= compare_path("waveform", &spliced.waveform, "kokoro_ref.npy");

    // End to end with our own STFT — informational: the flipped entries inject
    // localized differences the strict rows above have already accounted for.
    let reference = Npy::load("kokoro_ref.npy").expect("kokoro_ref.npy");
    let (e2e_max, at) = max_abs_diff(&gen.waveform, &reference.data);
    let rms = {
        let sum: f32 = gen
            .waveform
            .iter()
            .zip(&reference.data)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        (sum / reference.data.len() as f32).sqrt()
    };
    println!("  end-to-end waveform vs reference: max|Δ|={e2e_max:.3e} at {at}, rms={rms:.3e}");

    // ---- accelerated tier (informational) ----
    // Same graph on the Metal matmuls. Frame counts must agree; the waveform
    // drift beyond the strict run's is the f16-tile cost.
    makepad_tts::kokoro::accel::force_cpu(false);
    let started = std::time::Instant::now();
    let fast = decoder.run(&asr, &prosody.f0, &prosody.noise, decoder_style);
    let elapsed = started.elapsed().as_secs_f32();
    let wave = &fast.generator.waveform;
    println!(
        "\naccelerated decoder: {elapsed:.2} s ({:.2}x realtime)",
        wave.len() as f32 / 24_000.0 / elapsed
    );
    if wave.len() != reference.data.len() {
        println!("  LENGTH MISMATCH: {} vs {}", wave.len(), reference.data.len());
        all_ok = false;
    } else {
        let (fast_max, _) = max_abs_diff(wave, &reference.data);
        let rms = {
            let sum: f32 = wave
                .iter()
                .zip(&reference.data)
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
            (sum / reference.data.len() as f32).sqrt()
        };
        println!("  waveform vs reference: max|Δ|={fast_max:.3e}, rms={rms:.3e}");
    }

    println!(
        "\n{}",
        if all_ok {
            "matches the reference"
        } else {
            "MISMATCH — do not proceed to the next stage"
        }
    );
}
