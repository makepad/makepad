//! The sung-voice eval: 20 fixed lyric lines on fixed melodies, sung by the
//! DSP singer and (with --cantor) the neural voice through the same lowering,
//! loudness-normalised, written side by side, transcribed with our speech
//! recogniser, and scored.
//!
//!   voice_eval --whisper ggml-large-v3-turbo.bin --out DIR [--cantor M.mksing]
//!              [--singer N] [--profile alto] [--lines N]
//!   voice_eval --whisper W --cantor M.mksing --diag DATA_DIR --out DIR [--items N] [--holdout P] [--held]
//!
//! `--diag`: the pipeline check on sung-lyric TRAINING segments (songs outside
//! the --holdout P% the trainer held out), each rendered five ways and scored
//! against the recogniser's transcript of the original: the original, vocoder
//! copy synthesis of its mel, the decoder teacher-forced (the model's own
//! alignment of the audio, the original f0) coarse and refined, and the
//! decoder on its predicted durations (with the position-in-token feature as
//! training saw it and as rendering fills it); --held takes the held-out songs
//! instead; plus the eval lines' phonemes through
//! the score path against the training front end's.
//!
//! `--dump DIR` writes each line's score (`NN.score.tsv`: per note its start,
//! length, MIDI pitch and the index of the word it sings; the words) for
//! another engine to render; `--ext DIR` adds that engine's `DIR/NN.wav` (any
//! rate, time 0 at beat 0) as a column, normalised and scored the same way.
//!
//! Columns: word error rate (the recogniser's transcript against the lyric),
//! note pitch error (median |cents| of the sung f0 against the written note
//! over each note's middle half, YIN on the output), vibrato extent (cents,
//! std over notes of 0.8 s or longer).

use makepad_ai_sing::dsp::{self, f0_yin, hz_to_midi};
use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
use makepad_score_voice::render::render_line;
use makepad_score_voice::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const RATE: f32 = 48_000.0;
const BPM: f64 = 96.0;

/// The test lines: common words, every vowel, many consonant clusters.
const LINES: [&str; 20] = [
    "the sun is shining on the river",
    "i will remember you tonight",
    "we walk along the empty street",
    "she sings a song of love and rain",
    "hold my hand and never let me go",
    "the morning light is breaking through",
    "my heart is beating like a drum",
    "follow the wind across the sea",
    "every star is burning bright",
    "dance with me until the end",
    "the city sleeps beneath the moon",
    "bring me home before the night",
    "a golden bird is flying high",
    "we are young and we are free",
    "lonely roads and broken dreams",
    "tell me why you had to leave",
    "summer days will never last",
    "the clock is ticking on the wall",
    "open the door and let me in",
    "singing softly in the dark",
];

fn syllable_count(text: &str) -> usize {
    text.split_whitespace()
        .map(|w| lyrics::syllabify(&phoneme::parse(&kokoro::pronounce(w))).len().max(1))
        .sum()
}

/// A deterministic singable melody (C major, a random walk) with one note
/// per syllable; the last note held.
fn melody(n: usize, seed: u32, low: f32) -> Vec<NoteIn> {
    let scale = [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0, 12.0, 14.0, 16.0];
    let mut rng = pitch::Rng::new(seed);
    let mut deg: i32 = 2;
    let mut beat = 0.0;
    let mut out = Vec::new();
    for i in 0..n {
        let step = (rng.next_u32() % 5) as i32 - 2;
        deg = (deg + step).clamp(0, scale.len() as i32 - 1);
        let dur = if i + 1 == n { 2.0 } else { [1.0, 1.0, 0.5, 0.5, 1.5][(rng.next_u32() % 5) as usize] };
        out.push(NoteIn { beat, dur, pitches: vec![low + scale[deg as usize]], vel: 0.8 });
        beat += dur;
    }
    out
}

/// RMS to -20 dBFS over the voiced part, peaks kept under -1 dBFS.
fn normalise(x: &mut [f32]) {
    let loud: Vec<f32> = x.chunks(480).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let peak_rms = loud.iter().fold(0.0f32, |m, v| m.max(*v));
    let active: Vec<f32> = loud.iter().copied().filter(|v| *v > peak_rms * 0.1).collect();
    let rms = (active.iter().map(|v| v * v).sum::<f32>() / active.len().max(1) as f32).sqrt().max(1e-9);
    let mut g = 0.1 / rms;
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())) * g;
    if peak > 0.891 {
        g *= 0.891 / peak;
    }
    for v in x.iter_mut() {
        *v *= g;
    }
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_matches('\'').to_string())
        .collect()
}

/// Word edit distance and the reference length.
fn wer(reference: &str, hyp: &str) -> (usize, usize) {
    let (r, h) = (words(reference), words(hyp));
    let mut d: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut prev = d[0];
        d[0] = i;
        for j in 1..=h.len() {
            let cur = d[j];
            d[j] = (prev + (r[i - 1] != h[j - 1]) as usize).min(d[j] + 1).min(d[j - 1] + 1);
            prev = cur;
        }
    }
    (d[h.len()], r.len())
}

/// Mean note pitch error (cents; the median over the middle half of each
/// note), its vibrato extent, and how many notes the tracker heard an octave
/// (or more) away: a weak fundamental reads as the octave above.
fn pitch_stats(audio: &[f32], notes: &[NoteIn]) -> (f32, f32, usize, Vec<f32>) {
    let f0 = f0_yin(audio, 55.0, 1400.0);
    let spb = 60.0 / BPM;
    let (mut errs, mut vib) = (Vec::new(), Vec::new());
    let mut octaves = 0;
    for n in notes {
        let (s, e) = (n.beat * spb, (n.beat + n.dur) * spb);
        let (a, b) = (((s + 0.25 * (e - s)) * 100.0) as usize, ((s + 0.75 * (e - s)) * 100.0) as usize);
        let mut cents: Vec<f32> = (a..b.min(f0.len())).filter(|t| f0[*t] > 0.0).map(|t| 100.0 * (hz_to_midi(f0[t]) - n.pitches[0])).collect();
        if cents.len() < 3 {
            continue;
        }
        cents.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let c = cents[cents.len() / 2];
        if c.abs() > 600.0 {
            octaves += 1;
        }
        errs.push(c.abs());
        if e - s >= 0.8 {
            let m = cents.iter().sum::<f32>() / cents.len() as f32;
            vib.push((cents.iter().map(|c| (c - m).powi(2)).sum::<f32>() / cents.len() as f32).sqrt());
        }
    }
    let mean = |v: &[f32]| if v.is_empty() { f32::NAN } else { v.iter().sum::<f32>() / v.len() as f32 };
    (mean(&errs), mean(&vib), octaves, errs)
}

struct Engine {
    name: &'static str,
    /// Per-line WER capped at 100% (a recogniser stuck in a loop must not swamp the mean).
    wer_lines: Vec<f32>,
    pitch: Vec<f32>,
    vib: Vec<f32>,
    octaves: usize,
    pitch_notes: Vec<f32>,
}

fn transcribe(whisper: &WhisperModel, audio: &[f32]) -> String {
    let mut a = audio.to_vec();
    normalise(&mut a);
    let mut st = WhisperState::new(whisper);
    st.transcribe(whisper, &dsp::resample(&a, 48_000, 16_000), &WhisperParams::default()).iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" ")
}

fn rms_db(x: &[f32]) -> f32 {
    10.0 * ((x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).max(1e-12)).log10()
}

fn diag(whisper: &WhisperModel, cantor: &makepad_ai_sing::Cantor, data: &str, out: &Path, items: usize, holdout: u64, held: bool) {
    use makepad_ai_sing::nn::{Graph, RowIndex};
    use makepad_ai_sing::score::FEATS;
    use makepad_ai_sing::{acoustic, data::Store, layers::linear, phonemes as ph, train, vocoder};
    let store = Store::open(&[data.to_string()]).expect("data");
    let picked: Vec<_> = store.items.iter().filter(|r| store.held_out(r, holdout) == held && r.frames <= 600).step_by(7).take(items).copied().collect();
    let names = ["gt", "copy", "tf", "tf_refined", "free", "free_pos"];
    let mut wer_sum = [0.0f32; 6];
    let mut lvl = [0.0f32; 6];
    let mut n_lines = 0;
    for (k, r) in picked.iter().enumerate() {
        let it = store.item(r);
        let Some(al) = train::Aligned::from_sungtext_item(&it, 100_000) else { continue };
        let b = train::ac_batch(&[al.clone()]);
        let (n, t) = (b.n, b.t);
        let audio = al.audio[..(t * makepad_ai_sing::dsp::HOP).min(al.audio.len())].to_vec();
        let voc = |mel: &makepad_ai_sing::nn::Tensor| -> Vec<f32> {
            let mut g = Graph::new(&cantor.params, false);
            let m = g.input(mel.clone());
            let src = vocoder::SourceCtl::new(&al.f0, t, 3);
            let w = vocoder::forward(&mut g, &cantor.voc, m, &al.f0, t, &src);
            g.host(w)
        };
        let mut f0f = vec![0.0; t * FEATS];
        for q in 0..t {
            if b.f0[q] > 0.0 {
                f0f[q * FEATS + 5] = (b.f0[q] / dsp::F0_REF).ln();
                f0f[q * FEATS + 6] = 1.0;
                if b.notes[q] > 0.0 {
                    f0f[q * FEATS + 7] = ((hz_to_midi(b.f0[q]) - b.notes[q]) / 2.0).clamp(-3.0, 3.0);
                }
            }
        }
        let mut g = Graph::new(&cantor.params, false);
        let mel_t = train::target_mel(&mut g, &b.audio, 1, t);
        let gt_mel = g.val(mel_t).clone();
        let enc = acoustic::encode(&mut g, &cantor.ac, &b.tokens, &b.singers, n, Some(b.tok_lens.clone()));
        let mu = linear(&mut g, "ac.align", enc.enc);
        let (idx, _) = g.mas_align(mu, mel_t, &b.tok_lens, &b.frame_lens);
        let dec = acoustic::decode(&mut g, &cantor.ac, enc.enc, &idx, t, Some(b.frame_lens.clone()), &b.note_feats, &f0f);
        let coarse = g.val(dec.mel).clone();
        let cond = g.val(dec.cond).clone();
        let refined = acoustic::refine(&cantor.params, &cantor.ac, &coarse, &cond, 0.0, 8, 1);
        let mas_dur: Vec<usize> = match &idx {
            RowIndex::Host(v) => (0..b.tok_lens[0] as usize).map(|tk| v.iter().filter(|x| **x == tk).count()).collect(),
            _ => vec![],
        };
        // Free: the duration head's lengths, scaled to the item's frames.
        let ld = g.host(enc.log_dur);
        let pd: Vec<f32> = (0..b.tok_lens[0] as usize).map(|i| (ld[i].exp() - 1.0).max(1.0)).collect();
        let scale = t as f32 / pd.iter().sum::<f32>();
        let mut tof = Vec::new();
        let mut acc = 0.0f32;
        for (i, d) in pd.iter().enumerate() {
            acc += d * scale;
            while (tof.len() as f32) < acc.round() && tof.len() < t {
                tof.push(i);
            }
        }
        while tof.len() < t {
            tof.push(pd.len() - 1);
        }
        let mut g2 = Graph::new(&cantor.params, false);
        let enc2 = acoustic::encode(&mut g2, &cantor.ac, &b.tokens, &b.singers, n, Some(b.tok_lens.clone()));
        let tof_c = tof.clone();
        let dec2 = acoustic::decode(&mut g2, &cantor.ac, enc2.enc, &RowIndex::Host(tof), t, Some(b.frame_lens.clone()), &b.note_feats, &f0f);
        let free = g2.val(dec2.mel).clone();
        // The same with the position-in-token feature filled in as rendering
        // fills it (training leaves it 0 on unaligned segments).
        let mut nf_pos = b.note_feats.clone();
        let mut q = 0;
        while q < t {
            let tk = tof_c[q];
            let mut e = q;
            while e < t && tof_c[e] == tk {
                e += 1;
            }
            for (j, r) in (q..e).enumerate() {
                nf_pos[r * FEATS + 4] = if e - q > 1 { j as f32 / (e - q - 1) as f32 } else { 0.5 };
            }
            q = e;
        }
        let mut g3 = Graph::new(&cantor.params, false);
        let enc3 = acoustic::encode(&mut g3, &cantor.ac, &b.tokens, &b.singers, n, Some(b.tok_lens.clone()));
        let dec3 = acoustic::decode(&mut g3, &cantor.ac, enc3.enc, &RowIndex::Host(tof_c.clone()), t, Some(b.frame_lens.clone()), &nf_pos, &f0f);
        let free_pos = g3.val(dec3.mel).clone();
        let renders = [audio.clone(), voc(&gt_mel), voc(&coarse), voc(&refined), voc(&free), voc(&free_pos)];
        let reference = transcribe(whisper, &audio);
        let toks: String = al.tokens.iter().map(|p| ph::symbol(*p)).collect::<Vec<_>>().join(" ");
        println!("#{k:02} singer {} {} frames, {} tokens [{toks}]", al.singer, t, al.tokens.len());
        println!("    MAS frames per token {:?}", mas_dur);
        println!("    predicted frames per token {:?}", pd.iter().map(|d| d.round() as usize).collect::<Vec<_>>());
        for (e, a) in renders.iter().enumerate() {
            std::fs::write(out.join(format!("{k:02}-{}.wav", names[e])), dsp::wav_bytes(48_000, a)).unwrap();
            let text = if e == 0 { reference.clone() } else { transcribe(whisper, a) };
            let (err, nw) = wer(&reference, &text);
            let w = (err as f32 / nw.max(1) as f32).min(1.0);
            wer_sum[e] += w;
            lvl[e] += rms_db(a);
            println!("    {:<10} {:5.0}%  {:6.1} dB  [{}]", names[e], w * 100.0, rms_db(a), text.trim());
        }
        n_lines += 1;
    }
    println!();
    println!("{:<10} {:>8} {:>9}   (WER against the recogniser's transcript of the original; {} training segments)", "path", "WER*", "level", n_lines);
    for e in 0..names.len() {
        println!("{:<10} {:>7.1}% {:>6.1} dB", names[e], 100.0 * wer_sum[e] / n_lines.max(1) as f32, lvl[e] / n_lines.max(1) as f32);
    }
    // The eval's phonemes (score path) against the training front end's.
    let pron: &dyn Fn(&str) -> String = &kokoro::pronounce;
    let mut lost = 0;
    for line in LINES.iter() {
        let notes = melody(syllable_count(line), 17, 60.0);
        let lowered = lower_sing(&notes, line, &SingOpts { bpm: BPM, ..SingOpts::default() }, pron);
        let score = makepad_score_voice::neural::to_sing_score(&lowered, BPM, 1600);
        let eval_ph: Vec<u8> = score.notes.iter().filter_map(|n| n.syllable.as_ref()).flat_map(|s| s.onset.iter().chain(&s.nucleus).chain(&s.coda).copied().collect::<Vec<_>>()).collect();
        let train_ph: Vec<u8> = line.split_whitespace().flat_map(|w| ph::parse(&makepad_ai_speech::g2p::pronounce(w))).collect();
        if eval_ph != train_ph {
            lost += 1;
            println!("phonemes differ: {line}\n  eval  {}\n  train {}", eval_ph.iter().map(|p| ph::symbol(*p)).collect::<String>(), train_ph.iter().map(|p| ph::symbol(*p)).collect::<String>());
        }
    }
    println!("eval lines whose phonemes differ from the training front end: {lost} of {}", LINES.len());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out = PathBuf::from(arg("--out").unwrap_or_else(|| "voice-eval".into()));
    std::fs::create_dir_all(&out).unwrap();
    let whisper = WhisperModel::load_file(&arg("--whisper").expect("--whisper <ggml model>")).expect("whisper model");
    let cantor = arg("--cantor").map(|p| makepad_ai_sing::Cantor::load(Path::new(&p)).expect("cantor weights"));
    if let Some(d) = arg("--diag") {
        let items = arg("--items").and_then(|s| s.parse().ok()).unwrap_or(16);
        let holdout = arg("--holdout").and_then(|s| s.parse().ok()).unwrap_or(10);
        diag(&whisper, cantor.as_ref().expect("--cantor"), &d, &out, items, holdout, args.iter().any(|a| a == "--held"));
        return;
    }
    let singer: usize = arg("--singer").and_then(|s| s.parse().ok()).unwrap_or(1536);
    let profile = Arc::new(VoiceProfile::named(&arg("--profile").unwrap_or_else(|| "alto".into())).expect("profile"));
    let count: usize = arg("--lines").and_then(|s| s.parse().ok()).unwrap_or(LINES.len());
    let mut engines = vec![Engine { name: "dsp", wer_lines: vec![], pitch: vec![], vib: vec![], octaves: 0, pitch_notes: vec![] }];
    if cantor.is_some() {
        engines.push(Engine { name: "cantor", wer_lines: vec![], pitch: vec![], vib: vec![], octaves: 0, pitch_notes: vec![] });
    }
    let dump = arg("--dump").map(PathBuf::from);
    let ext = arg("--ext").map(PathBuf::from);
    if let Some(d) = &dump {
        std::fs::create_dir_all(d).unwrap();
    }
    if ext.is_some() {
        engines.push(Engine { name: "ext", wer_lines: vec![], pitch: vec![], vib: vec![], octaves: 0, pitch_notes: vec![] });
    }
    let pron: &dyn Fn(&str) -> String = &kokoro::pronounce;
    let params = WhisperParams::default();
    // --cantor-f0 rule|model: the neural voice's f0 (the rule curve, or its f0 head);
    // --cantor-vibrato S: the rule curve's vibrato depth in semitones;
    // --cantor-refine N: flow refiner steps (0 = the decoder's mel as is).
    let mut cantor_opts = makepad_ai_sing::RenderOpts::default();
    // --melodies K: another set of melodies for the same lines (K = 0 is the standard set).
    let melody_seed: u32 = arg("--melodies").and_then(|v| v.parse::<u32>().ok()).map(|k| k * 1000).unwrap_or(0);
    if arg("--cantor-f0").as_deref() == Some("model") {
        cantor_opts.f0 = makepad_ai_sing::cantor::F0Mode::Model;
    }
    if let Some(v) = arg("--cantor-refine").and_then(|v| v.parse().ok()) {
        cantor_opts.refine_steps = v;
    }
    if let Some(v) = arg("--cantor-vibrato").and_then(|v| v.parse().ok()) {
        cantor_opts.style.vibrato_depth = v;
    }
    println!("{:>3}  {:<36} {:>6} {:>6} {:>6}  transcripts", "#", "lyric", "dsp", "cantor", "ext");
    for (i, line) in LINES.iter().take(count).enumerate() {
        let notes = melody(syllable_count(line), 17 + i as u32 + melody_seed, 60.0);
        let lowered = lower_sing(&notes, line, &SingOpts { bpm: BPM, ..SingOpts::default() }, pron);
        if let Some(d) = &dump {
            // One note per syllable, the words' syllables in order.
            let spb = 60.0 / BPM;
            let mut o = format!("line\t{}\t{line}\n", i + 1);
            let mut word_of = Vec::new();
            for (wi, w) in line.split_whitespace().enumerate() {
                o.push_str(&format!("word\t{wi}\t{w}\n"));
                word_of.extend(std::iter::repeat(wi).take(syllable_count(w)));
            }
            for (n, wi) in notes.iter().zip(&word_of) {
                o.push_str(&format!("note\t{:.4}\t{:.4}\t{}\t{wi}\n", n.beat * spb, n.dur * spb, n.pitches[0]));
            }
            std::fs::write(d.join(format!("{:02}.score.tsv", i + 1)), o).unwrap();
        }
        let mut row = Vec::new();
        for e in engines.iter_mut() {
            let mut audio = match e.name {
                "dsp" => {
                    let mut s = Singer::new(profile.clone(), RATE, Style::default(), 5);
                    let (l, _) = render_line(&mut s, &lowered, BPM, RATE, 256, 0.6);
                    // render_line starts at the first event (an onset may lead beat 0).
                    let origin = lowered.events.iter().map(|e| e.beat).fold(0.0f64, f64::min);
                    let lead = ((-origin) * 60.0 / BPM * RATE as f64) as usize;
                    l[lead.min(l.len())..].to_vec()
                }
                "ext" => {
                    let path = ext.as_ref().unwrap().join(format!("{:02}.wav", i + 1));
                    let (x, rate) = std::fs::read(&path).ok().and_then(|b| dsp::read_wav(&b)).unwrap_or_else(|| panic!("{}", path.display()));
                    if rate == 48_000 {
                        x
                    } else {
                        dsp::resample(&x, rate, 48_000)
                    }
                }
                _ => makepad_score_voice::neural::render(cantor.as_ref().unwrap(), &lowered, BPM, singer, &cantor_opts),
            };
            normalise(&mut audio);
            std::fs::write(out.join(format!("{:02}-{}.wav", i + 1, e.name)), dsp::wav_bytes(48_000, &audio)).unwrap();
            let mut st = WhisperState::new(&whisper);
            let text: String = st.transcribe(&whisper, &dsp::resample(&audio, 48_000, 16_000), &params).iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" ");
            let (err, n) = wer(line, &text);
            e.wer_lines.push((err as f32 / n.max(1) as f32).min(1.0));
            let (p, v, o, per_note) = pitch_stats(&audio, &notes);
            e.octaves += o;
            e.pitch_notes.extend(per_note);
            if p.is_finite() {
                e.pitch.push(p);
            }
            if v.is_finite() {
                e.vib.push(v);
            }
            let mut t = text.trim().to_string();
            if t.len() > 80 {
                t = format!("{}...", &t[..t.char_indices().nth(77).map(|c| c.0).unwrap_or(t.len())]);
            }
            row.push(((err as f32 / n as f32).min(1.0), t));
        }
        let w = |k: usize| row.get(k).map(|r| format!("{:5.0}%", r.0 * 100.0)).unwrap_or_else(|| "     -".into());
        println!("{:>3}  {:<36} {} {} {}  {}", i + 1, line, w(0), w(1), w(2), row.iter().map(|r| format!("[{}]", r.1)).collect::<Vec<_>>().join(" "));
    }
    println!();
    println!("{:<8} {:>8} {:>16} {:>16} {:>14} {:>16}", "engine", "WER*", "note pitch err", "vibrato extent", "octave notes", "err w/o octave");
    for e in &engines {
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        let in_octave: Vec<f32> = e.pitch_notes.iter().copied().filter(|c| *c <= 600.0).collect();
        println!(
            "{:<8} {:>7.1}% {:>13.1} c {:>13.1} c {:>8} of {:<3} {:>13.1} c",
            e.name, 100.0 * mean(&e.wer_lines), mean(&e.pitch), mean(&e.vib), e.octaves, e.pitch_notes.len(), mean(&in_octave)
        );
    }
    println!("WER* = mean over lines of each line's word error rate, capped at 100%.");
    println!("WAVs: {}", out.display());
}
