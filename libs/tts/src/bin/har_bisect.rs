//! Which rows of `har` carry the 4.8/9.6 kHz ring? Run the generator with
//! hybrid har tensors — ours/reference magnitudes crossed with ours/reference
//! phases — and write each waveform for spectral comparison.
//!
//! Needs the same `refdump/` as the parity harness.

use std::io::Write;

use makepad_tts::g2p;
use makepad_tts::kokoro::bert::Bert;
use makepad_tts::kokoro::decoder::Decoder;
use makepad_tts::kokoro::npy::Npy;
use makepad_tts::kokoro::ops::{expand_to_frames, round_half_even, Mat};
use makepad_tts::kokoro::predictor::Predictor;
use makepad_tts::kokoro::text_encoder::TextEncoder;
use makepad_tts::kokoro::weights::Weights;

const SENTENCE: &str = "Escape the Gummer, a squishy purple blob.";
const MODEL: &str = "kokoro-v1_0.mktts";
const REF_HAR: &str = "refdump/decoder_decoder_generator_Concat_3_output_0.npy";

fn write_wav(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    let data_len = (samples.len() * 2) as u32;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&24_000u32.to_le_bytes())?;
    file.write_all(&48_000u32.to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    for sample in samples {
        file.write_all(&((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    Ok(())
}

fn main() {
    makepad_tts::kokoro::accel::force_cpu(true);
    makepad_tts::kokoro::generator::force_deterministic(true);

    let weights = Weights::load(MODEL).expect("weights");
    let encoder = TextEncoder::load(&weights).expect("text encoder");
    let bert = Bert::load(&weights).expect("bert");
    let predictor = Predictor::load(&weights).expect("predictor");
    let decoder = Decoder::load(&weights).expect("decoder");

    let voice = Weights::load("af_heart.mkvoice").expect("voice");
    let tokens = g2p::tokens(SENTENCE);
    let style_row = &voice.get("style").unwrap()[(tokens.len() - 3) * 256..][..256];
    let (decoder_style, predictor_style) = style_row.split_at(128);

    let trace = encoder.trace(&tokens);
    let bert_trace = bert.trace(&tokens);
    let duration = predictor.durations(&bert_trace.output, predictor_style);
    let frames: Vec<usize> = duration
        .durations
        .iter()
        .map(|d| round_half_even(*d).max(1.0) as usize)
        .collect();
    let en = expand_to_frames(&duration.encoded, &frames);
    let asr = expand_to_frames(&trace.output, &frames);
    let prosody = predictor.prosody(&en, predictor_style);
    let out = decoder.run(&asr, &prosody.f0, &prosody.noise, decoder_style);

    let ours = &out.generator.har;
    let reference = Npy::load(REF_HAR).expect("reference har");
    assert_eq!(ours.data.len(), reference.data.len(), "har shape mismatch");
    let rows = ours.rows;
    let cols = ours.cols;
    let bins = rows / 2;
    let reference = Mat::from_vec(rows, cols, reference.data.clone());

    // rows 0..bins are magnitudes, bins..2*bins are phases.
    let hybrid = |mag: &Mat, phase: &Mat| -> Mat {
        let mut out = mag.clone();
        out.data[bins * cols..].copy_from_slice(&phase.data[bins * cols..]);
        out
    };

    let mut cases: Vec<(String, Mat)> = vec![
        ("ours".into(), ours.clone()),
        ("ref".into(), reference.clone()),
        ("refmag_ourphase".into(), hybrid(&reference, ours)),
        ("ourmag_refphase".into(), hybrid(ours, &reference)),
    ];

    // Reference har with OUR phase spliced in for a single bin: which phase
    // row carries the ring?
    for bin in 0..bins {
        let mut har = reference.clone();
        let row = (bins + bin) * cols;
        har.data[row..row + cols].copy_from_slice(&ours.data[row..row + cols]);
        cases.push((format!("phasebin{bin}"), har));
    }

    // Raw f32 dumps of both har tensors for offline stats.
    for (name, mat) in [("ours", ours), ("ref", &reference)] {
        let bytes: Vec<u8> = mat.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(format!("har_{name}.f32"), bytes).expect("dump");
    }
    println!("har dims: {rows} x {cols}");

    for (name, har) in cases {
        let run = decoder.generator.run_from_har(
            &out.decode[3],
            decoder_style,
            out.generator.har_source.clone(),
            har,
        );
        let path = format!("bisect_{name}.wav");
        write_wav(&path, &run.waveform).expect("wav");
        println!("wrote {path} ({} samples)", run.waveform.len());
    }
}
