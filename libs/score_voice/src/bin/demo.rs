//! Render the sung-voice demo phrases to WAV.
//!
//!     cargo run --release -p makepad-score-voice --bin score_voice_demo -- <out dir> [profiles dir]
//!
//! The profiles dir holds `kokoro_<voice>.mkvox` (from `voice_profile`) and
//! optionally `kokoro_<voice>_speech.wav` for the speech-to-song demo.

use makepad_score_voice::analysis::{TrackPlayer, VocalTrack};
use makepad_score_voice::phoneme;
use makepad_score_voice::render::{pitch_curve, read_wav16, render_line, wav_bytes};
use makepad_score_voice::*;
use std::path::Path;
use std::sync::Arc;

const RATE: f32 = 48_000.0;

fn notes(list: &[(f32, f64)], start: f64) -> Vec<NoteIn> {
    let mut beat = start;
    list.iter()
        .map(|(p, d)| {
            let n = NoteIn { beat, dur: *d, pitches: vec![*p], vel: 0.8 };
            beat += d;
            n
        })
        .collect()
}

fn write(out: &Path, name: &str, l: &[f32], r: &[f32]) {
    let peak = l.iter().chain(r).fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let g = 0.8 / peak;
    let (l, r): (Vec<f32>, Vec<f32>) = (l.iter().map(|v| v * g).collect(), r.iter().map(|v| v * g).collect());
    std::fs::write(out.join(name), wav_bytes(RATE as u32, &[&l, &r])).expect("write wav");
    println!("wrote {name}: {:.1} s, peak normalised by {:+.1} dB", l.len() as f32 / RATE, 20.0 * g.log10());
}

fn solo(profile: Arc<VoiceProfile>, style: Style, line: &Lowered, bpm: f64) -> (Vec<f32>, Vec<f32>) {
    let mut singer = Singer::new(profile, RATE, style, 5);
    render_line(&mut singer, line, bpm, RATE, 256, 0.6)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = Path::new(args.first().map(String::as_str).unwrap_or("."));
    std::fs::create_dir_all(out).unwrap();
    let dir = args.get(1).map(Path::new);
    let load = |name: &str| -> Option<Arc<VoiceProfile>> {
        let bytes = std::fs::read(dir?.join(format!("kokoro_{name}.mkvox"))).ok()?;
        VoiceProfile::decode(&bytes).ok().map(Arc::new)
    };
    let builtin = |name: &str| Arc::new(VoiceProfile::named(name).unwrap());

    // Measured vowel formants of each analysed voice.
    for name in ["af_heart", "af_bella", "bf_emma", "am_michael", "am_adam", "bm_george"] {
        if let Some(p) = load(name) {
            let f = |v: &str| p.shape(phoneme::by_ipa(v).unwrap()).formants;
            println!("{:18} f0 {:3.0} Hz  /i/ {:?}  /ɑ/ {:?}  /u/ {:?}", p.name, p.f0_ref, f("i").map(|x| x as i32), f("ɑ").map(|x| x as i32), f("u").map(|x| x as i32));
        }
    }

    // 1. A hymn line, solo, built-in alto.
    let bpm = 84.0;
    let grace = notes(
        &[(62.0, 1.0), (67.0, 2.0), (71.0, 0.5), (67.0, 0.5), (71.0, 2.0), (69.0, 1.0), (67.0, 2.0), (64.0, 1.0), (62.0, 2.0),
          (62.0, 1.0), (67.0, 2.0), (71.0, 0.5), (67.0, 0.5), (71.0, 2.0), (69.0, 1.0), (74.0, 3.0)],
        0.0,
    );
    let lyrics = "a-ma-zing _ grace how sweet the sound that saved a _ wretch like me";
    let line = lower_sing(&grace, lyrics, &SingOpts { bpm, ..SingOpts::default() }, &builtin_pronounce);
    if !line.warnings.is_empty() {
        println!("warnings: {:?}", line.warnings);
    }
    println!("syllables: {}", line.syllables.iter().map(|s| format!("{}[{}]", s.text, s.ipa)).collect::<Vec<_>>().join(" "));
    let (l, r) = solo(builtin("alto"), Style::default(), &line, bpm);
    write(out, "vx1_01_hymn_alto.wav", &l, &r);

    // The pitch curve over the lyrics (what the Score shows inline).
    let mut probe = Singer::new(builtin("alto"), RATE, Style::default(), 5);
    let curve = pitch_curve(&mut probe, &line, bpm, 0.25);
    let text: Vec<String> = curve.iter().step_by(4).map(|(b, m)| format!("{b:.1}:{m:.2}")).collect();
    println!("pitch curve (beat:midi, every beat): {}", text.join(" "));

    // 2. The same line sung by analysed Kokoro voices (an octave down for
    // the men).
    for (name, shift) in [("af_heart", 0.0), ("am_michael", -12.0), ("bf_emma", 0.0)] {
        let Some(p) = load(name) else { continue };
        let shifted: Vec<NoteIn> = grace.iter().map(|n| NoteIn { pitches: vec![n.pitches[0] + shift], ..n.clone() }).collect();
        let line = lower_sing(&shifted, lyrics, &SingOpts { bpm, ..SingOpts::default() }, &builtin_pronounce);
        let (l, r) = solo(p, Style::default(), &line, bpm);
        write(out, &format!("vx1_02_hymn_kokoro_{name}.wav"), &l, &r);
    }

    // 3. A choir in three-part harmony over a progression.
    let bpm = 72.0;
    let chords = [(0.0, [60.0, 64.0, 67.0]), (4.0, [57.0, 60.0, 64.0]), (8.0, [53.0, 57.0, 60.0]), (12.0, [55.0, 59.0, 62.0]), (16.0, [60.0, 64.0, 67.0])];
    let mel = notes(
        &[(76.0, 1.0), (76.0, 1.0), (74.0, 1.0), (72.0, 1.0), (72.0, 1.0), (72.0, 1.0), (71.0, 1.0), (69.0, 1.0),
          (69.0, 1.0), (72.0, 1.0), (72.0, 1.0), (69.0, 1.0), (71.0, 1.5), (72.0, 0.5), (74.0, 2.0), (72.0, 4.0)],
        0.0,
    );
    let opts = SingOpts {
        bpm, parts: 3, chords: chords.iter().map(|(b, p)| ChordAt { beat: *b, pitches: p.to_vec() }).collect(), ..SingOpts::default()
    };
    let line = lower_sing(&mel, "hal-le-lu-jah hal-le-lu-jah hal-le-lu-jah hal-le-lu-jah", &opts, &builtin_pronounce);
    let mut profiles: Vec<Arc<VoiceProfile>> = ["af_heart", "af_bella", "bf_emma", "am_michael", "am_adam", "bm_george"].iter().filter_map(|n| load(n)).collect();
    if profiles.len() < 3 {
        profiles = vec![builtin("soprano"), builtin("alto"), builtin("tenor")];
    }
    let started = std::time::Instant::now();
    let mut choir = Choir::new(&profiles, 6, RATE, ChoirStyle { parts: 3, ..ChoirStyle::default() }, 9);
    let (l, r) = render_line(&mut choir, &line, bpm, RATE, 256, 1.0);
    let secs = l.len() as f32 / RATE;
    println!("6-singer choir: {:.1} s rendered in {:.3} s ({:.0}x realtime)", secs, started.elapsed().as_secs_f32(), secs / started.elapsed().as_secs_f32());
    write(out, "vx1_03_choir_harmony.wav", &l, &r);

    // 4. An "ooh" pad: a unison choir of built-in voices on long chord tones.
    let bpm = 60.0;
    let mut choir = Choir::new(&[builtin("soprano"), builtin("alto")], 8, RATE, ChoirStyle { parts: 3, width: 1.0, ..ChoirStyle::default() }, 21);
    let pad: Vec<NoteIn> = chords[..4].iter().map(|(b, p)| NoteIn { beat: *b, dur: 3.8, pitches: p.iter().map(|x| x + 12.0).collect(), vel: 0.55 }).collect();
    let line = lower_sing(&pad, "ooh ah mm ooh", &SingOpts { bpm, ..SingOpts::default() }, &builtin_pronounce);
    let (l, r) = render_line(&mut choir, &line, bpm, RATE, 512, 1.5);
    write(out, "vx1_04_ooh_pad.wav", &l, &r);

    // 5. Analysis-resynthesis: Kokoro speech turned into a vocal track, its
    // pitch snapped to C major and raised, plus a harmony part under it.
    if let Some((pcm, rate)) = dir.and_then(|d| std::fs::read(d.join("kokoro_af_heart_speech.wav")).ok()).and_then(|b| read_wav16(&b)) {
        let mut track = VocalTrack::analyze(&pcm, rate as f32);
        println!("speech track: {:.2} s, {} bytes coded", track.duration(), track.encode().len());
        track.transpose(3.0);
        track.snap_to_scale(&[0, 2, 4, 5, 7, 9, 11], 1.0);
        let slow = track.retime(1.3);
        let harmony = slow.harmony(&[(0.0, vec![60.0, 64.0, 67.0]), (2.0, vec![53.0, 57.0, 60.0]), (4.0, vec![55.0, 59.0, 62.0])], 1);
        let dur = slow.duration();
        let lead = {
            let mut p = TrackPlayer::new(Arc::new(slow.clone()), RATE, 3);
            let mut out = vec![0.0; ((dur + 0.3) * RATE) as usize];
            p.render(&mut out);
            out
        };
        let mut p = TrackPlayer::new(Arc::new(harmony), RATE, 4);
        let mut low = vec![0.0; lead.len()];
        p.render(&mut low);
        let l: Vec<f32> = lead.iter().zip(&low).map(|(a, b)| a * 0.8 + b * 0.55).collect();
        let r: Vec<f32> = lead.iter().zip(&low).map(|(a, b)| a * 0.8 + b * 0.35).collect();
        write(out, "vx1_05_speech_to_song.wav", &l, &r);
        let mut p = TrackPlayer::new(Arc::new(VocalTrack::analyze(&pcm, rate as f32)), RATE, 3);
        let mut plain = vec![0.0; ((pcm.len() as f32 / rate as f32 + 0.3) * RATE) as usize];
        p.render(&mut plain);
        write(out, "vx1_06_speech_resynthesis.wav", &plain, &plain);
    }
}
