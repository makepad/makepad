use makepad_drumkit::{DrumKit, DrumVoice, SampleBank};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn main() -> Result<(), String> {
    const RATE: u32 = 48_000;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus = root.join("local/score-corpus/drums/OH");
    if !corpus.is_dir() {
        println!("Salamander corpus not found at {}; demo skipped", corpus.display());
        return Ok(());
    }
    let bank = Arc::new(SampleBank::load(&corpus)?);
    println!("{}", bank.summary());
    let mut kit = DrumKit::new(RATE as f32);
    kit.set_bank(bank);

    let beat = RATE as usize / 2; // 120 bpm
    let total = beat * 16;
    let mut hits = Vec::new();
    for bar in 0..4usize {
        let start = bar * beat * 4;
        for eighth in 0..8 {
            hits.push((start + eighth * beat / 2, if eighth == 7 { DrumVoice::HiHatOpen } else { DrumVoice::HiHatClosed }, if eighth % 2 == 0 { 0.65 } else { 0.42 }));
        }
        for offset in [0, beat * 2 + beat / 2] {
            hits.push((start + offset, DrumVoice::Kick, 0.9));
        }
        for offset in [beat, beat * 3] {
            hits.push((start + offset, DrumVoice::Snare, 0.85));
        }
        if bar == 0 {
            hits.push((start, DrumVoice::Crash, 0.9));
        }
    }
    for (step, voice) in [DrumVoice::TomHigh, DrumVoice::TomMid, DrumVoice::TomLow, DrumVoice::TomFloor].into_iter().enumerate() {
        hits.push((total - beat + step * beat / 4, voice, 0.8 + step as f32 * 0.05));
    }
    hits.sort_by_key(|hit| hit.0);

    let mut output = vec![[0.0f32; 2]; total];
    let mut next = 0;
    for (frame, slot) in output.iter_mut().enumerate() {
        while next < hits.len() && hits[next].0 == frame {
            kit.trigger(hits[next].1, hits[next].2);
            next += 1;
        }
        kit.process(std::slice::from_mut(slot));
    }

    let path = root.join("target/drumkit/pattern_samples.wav");
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    fs::write(&path, encode_stereo_pcm16(&output, RATE)).map_err(|error| error.to_string())?;
    println!(
        "{}",
        path.canonicalize().unwrap_or(path).display()
    );
    Ok(())
}

fn encode_stereo_pcm16(frames: &[[f32; 2]], rate: u32) -> Vec<u8> {
    let data_len = frames.len() as u32 * 4;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in frames {
        for sample in frame {
            let pcm = (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
            bytes.extend_from_slice(&pcm.to_le_bytes());
        }
    }
    bytes
}
