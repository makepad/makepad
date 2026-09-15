// Renders the kit to WAV files (48 kHz, stereo, 24-bit) for listening and
// for the measurement harness:
//   cargo run -p makepad-drumkit-phys --release --example demo_pattern -- <out-dir> [sample-rate]
// Writes model_<voice>_<vel>.wav for every voice at velocities 0.3/0.6/1.0
// (single hits, trimmed when the voice ends) and pattern_model.wav, a
// four-bar groove at 120 bpm that uses the whole kit.

use makepad_drumkit_phys::{DrumKit, DrumVoice};
use std::io::Write;

fn write_wav24(path: &std::path::Path, fs: u32, frames: &[[f32; 2]]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(44 + frames.len() * 6);
    let data_len = (frames.len() * 6) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&fs.to_le_bytes());
    bytes.extend_from_slice(&(fs * 6).to_le_bytes());
    bytes.extend_from_slice(&6u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for f in frames {
        for &s in f {
            let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            let b = v.to_le_bytes();
            bytes.extend_from_slice(&b[..3]);
        }
    }
    std::fs::File::create(path)?.write_all(&bytes)
}

fn voice_name(v: DrumVoice) -> &'static str {
    match v {
        DrumVoice::Kick => "kick",
        DrumVoice::Snare => "snare",
        DrumVoice::SideStick => "sidestick",
        DrumVoice::HiHatClosed => "hihatclosed",
        DrumVoice::HiHatOpen => "hihatopen",
        DrumVoice::HiHatPedal => "hihatpedal",
        DrumVoice::TomHigh => "tomhigh",
        DrumVoice::TomMid => "tommid",
        DrumVoice::TomLow => "tomlow",
        DrumVoice::TomFloor => "tomfloor",
        DrumVoice::Ride => "ride",
        DrumVoice::RideBell => "ridebell",
        DrumVoice::Crash => "crash",
        DrumVoice::Clap => "clap",
    }
}

/// Renders one hit until the voice goes quiet (or `max_s`), in 256-frame
/// blocks, and pads 50 ms of silence.
fn render_hit(fs: f32, voice: DrumVoice, vel: f32, max_s: f32) -> Vec<[f32; 2]> {
    let mut kit = DrumKit::new(fs);
    kit.trigger(voice, vel);
    let mut out = Vec::new();
    let mut block = [[0.0f32; 2]; 256];
    let max = (max_s * fs) as usize;
    while kit.active() && out.len() < max {
        block.iter_mut().for_each(|f| *f = [0.0; 2]);
        kit.process(&mut block);
        out.extend_from_slice(&block);
    }
    out.extend(std::iter::repeat([0.0; 2]).take((0.05 * fs) as usize));
    out
}

struct Hit {
    beat: f32,
    voice: DrumVoice,
    vel: f32,
}

fn groove() -> Vec<Hit> {
    use DrumVoice::*;
    let mut h = Vec::new();
    let mut add = |bar: usize, beat: f32, voice: DrumVoice, vel: f32| h.push(Hit { beat: bar as f32 * 4.0 + beat, voice, vel });
    for bar in 0..4 {
        // hats: 8ths, accents on the beat, open on the "and" of 4 in bars 2/4
        for i in 0..8 {
            let b = i as f32 * 0.5;
            let vel = if i % 2 == 0 { 0.75 } else { 0.45 };
            if bar % 2 == 1 && i == 7 {
                add(bar, b, HiHatOpen, 0.7);
            } else if !(bar == 3 && i >= 4) {
                add(bar, b, HiHatClosed, vel);
            }
        }
        if bar % 2 == 1 {
            add(bar, 4.0 - 0.02, HiHatPedal, 0.6); // close the open hat on the downbeat
        }
        // kick
        add(bar, 0.0, Kick, 1.0);
        add(bar, 1.75, Kick, 0.8);
        add(bar, 2.5, Kick, 0.9);
        // snare 2 and 4 (+ ghosts)
        add(bar, 1.0, Snare, 0.95);
        add(bar, 3.0, Snare, 1.0);
        add(bar, 2.25, Snare, 0.25);
        add(bar, 3.75, Snare, 0.3);
        if bar == 1 {
            add(bar, 2.0, SideStick, 0.7);
            add(bar, 3.5, Clap, 0.9);
        }
        if bar == 2 {
            add(bar, 1.0, Clap, 1.0);
            add(bar, 3.0, Clap, 0.8);
        }
    }
    // bar 4: ride + fill
    for i in 0..4 {
        add(3, i as f32 * 0.5, Ride, if i % 2 == 0 { 0.8 } else { 0.55 });
    }
    add(3, 1.5, RideBell, 0.9);
    add(3, 2.0, TomHigh, 0.9);
    add(3, 2.25, TomHigh, 0.7);
    add(3, 2.5, TomMid, 0.9);
    add(3, 2.75, TomMid, 0.7);
    add(3, 3.0, TomLow, 0.95);
    add(3, 3.25, TomLow, 0.7);
    add(3, 3.5, TomFloor, 1.0);
    add(3, 3.75, TomFloor, 0.8);
    add(0, 0.0, Crash, 1.0);
    add(2, 0.0, Crash, 0.8);
    h.push(Hit { beat: 16.0, voice: Crash, vel: 1.0 });
    h.push(Hit { beat: 16.0, voice: Kick, vel: 1.0 });
    h
}

pub fn render_pattern(fs: f32, bpm: f32, tail_s: f32) -> Vec<[f32; 2]> {
    let mut hits = groove();
    hits.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap());
    let spb = 60.0 / bpm;
    let total = ((16.0 * spb + tail_s) * fs) as usize;
    let mut kit = DrumKit::new(fs);
    let mut out = vec![[0.0f32; 2]; total];
    let block = 128usize;
    let mut next = 0usize;
    let mut pos = 0usize;
    while pos < total {
        let n = block.min(total - pos);
        while next < hits.len() && ((hits[next].beat * spb * fs) as usize) < pos + n {
            kit.trigger(hits[next].voice, hits[next].vel);
            next += 1;
        }
        kit.process(&mut out[pos..pos + n]);
        pos += n;
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or("."));
    let fs: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(48000.0);
    std::fs::create_dir_all(&dir).expect("output dir");
    for voice in DrumVoice::ALL {
        for vel in [0.3f32, 0.6, 1.0] {
            let frames = render_hit(fs, voice, vel, 12.0);
            let name = format!("model_{}_{:.1}.wav", voice_name(voice), vel);
            write_wav24(&dir.join(&name), fs as u32, &frames).expect("write");
            let peak = frames.iter().flat_map(|f| f.iter()).fold(0.0f32, |a, v| a.max(v.abs()));
            println!("{name}: {:.2} s, peak {:.1} dBFS", frames.len() as f32 / fs, 20.0 * peak.max(1e-9).log10());
        }
    }
    let pattern = render_pattern(fs, 120.0, 3.0);
    let peak = pattern.iter().flat_map(|f| f.iter()).fold(0.0f32, |a, v| a.max(v.abs()));
    write_wav24(&dir.join("pattern_model.wav"), fs as u32, &pattern).expect("write");
    println!("pattern_model.wav: {:.2} s, peak {:.2} dBFS", pattern.len() as f32 / fs, 20.0 * peak.max(1e-9).log10());
}
