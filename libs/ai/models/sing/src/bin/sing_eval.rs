//! Held-out check of a Cantor checkpoint on recorded sung lyric lines
//! (feature `eval`).
//!
//!   sing_eval --whisper ggml.bin --cantor M.mksing --words DIR --out DIR
//!             [--holdout 10] [--items 40] [--train] [--wavs 6]
//!
//! `--words` is `sing_prep lyrics`'s input (timed words per segment). The
//! songs the trainer held out (`--holdout P`, by name, as `sing_train ac`)
//! are scored, or with `--train` the others. Each segment is rendered:
//!   copy  the vocoder on the recording's mel;
//!   tf    the acoustic model on the segment's score-aligned frames and the
//!         recorded f0 (teacher-forced);
//!   free  the full render of the segment's score, as a host renders a score
//!         (durations from the model, f0 from its f0 head, refined).
//! Pitch: how far (median |cents|) each render's f0 is from the f0 it was
//! given (copy, tf: the recording's), and the free render's vowels from the
//! score's notes (`--f0 rule|model`: the free render's f0, default model).
//! Each render (and the recording, `gt`) is transcribed and scored against
//! the line's lyric (word error rate, capped at 100% per line); a segment can
//! hold sung words of the next line at its edges, so `gt` is the floor. Also: how well the
//! duration head's lengths follow the score-aligned ones (correlation of
//! ln(1 + frames)).

use makepad_ai_sing::acoustic;
use makepad_ai_sing::cantor::{F0Mode, RenderOpts};
use makepad_ai_sing::data::{self, Item};
use makepad_ai_sing::dsp::{self, hz_to_midi, HOP};
use makepad_ai_sing::nn::{Graph, RowIndex, Tensor};
use makepad_ai_sing::phonemes as ph;
use makepad_ai_sing::score::{self, SungWord, FEATS};
use makepad_ai_sing::{train, vocoder, Cantor};
use makepad_ai_speech::whisper::{WhisperModel, WhisperParams, WhisperState};
use std::path::{Path, PathBuf};

fn arg(k: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned()
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect::<String>().to_lowercase()).filter(|w| !w.is_empty()).collect()
}

/// Word errors of `hyp` against `reference`, and the reference's length.
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

fn transcribe(whisper: &WhisperModel, audio: &[f32]) -> String {
    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-4);
    let a: Vec<f32> = audio.iter().map(|v| v * 0.5 / peak).collect();
    let mut st = WhisperState::new(whisper);
    st.transcribe(whisper, &dsp::resample(&a, 48_000, 16_000), &WhisperParams::default()).iter().map(|s| s.text.clone()).collect::<Vec<_>>().join(" ")
}

/// Median |cents| between the f0 of `audio` and `want` (Hz per 10 ms frame) where both are voiced.
fn follow_cents(audio: &[f32], want: &[f32]) -> Vec<f32> {
    let got = dsp::f0_yin(audio, 55.0, 1400.0);
    got.iter().zip(want).filter(|(g, w)| **g > 0.0 && **w > 0.0).map(|(g, w)| (1200.0 * (g / w).log2()).abs()).collect()
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn corr(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().max(1) as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    sab / (saa * sbb).sqrt().max(1e-9)
}

/// Segments of one timed-words file: (shard, item index, words, lyric).
fn read_words(path: &Path) -> Vec<(String, usize, Vec<SungWord>, String)> {
    let mut out: Vec<(String, usize, Vec<SungWord>, String)> = Vec::new();
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let c: Vec<&str> = line.split('\t').collect();
        match c[0] {
            "seg" => out.push((c[1].to_string(), c[2].parse().unwrap(), Vec::new(), String::new())),
            "w" => {
                if let Some(s) = out.last_mut() {
                    s.2.push(SungWord { start: c[1].parse().unwrap(), end: c[2].parse().unwrap(), phones: ph::parse(c[4]) });
                    s.3.push_str(c.get(5).copied().unwrap_or(""));
                    s.3.push(' ');
                }
            }
            _ => {}
        }
    }
    out
}

fn main() {
    let whisper = WhisperModel::load_file(&arg("--whisper").expect("--whisper <ggml model>")).expect("whisper model");
    let cantor = Cantor::load(Path::new(&arg("--cantor").expect("--cantor M.mksing"))).expect("cantor");
    let wdir = PathBuf::from(arg("--words").expect("--words DIR"));
    let out = PathBuf::from(arg("--out").expect("--out DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let holdout: u64 = arg("--holdout").and_then(|v| v.parse().ok()).unwrap_or(10);
    let items: usize = arg("--items").and_then(|v| v.parse().ok()).unwrap_or(40);
    let wavs: usize = arg("--wavs").and_then(|v| v.parse().ok()).unwrap_or(6);
    let held = !std::env::args().any(|a| a == "--train");

    let mut groups: Vec<PathBuf> = std::fs::read_dir(&wdir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "tsv").unwrap_or(false)).collect();
    groups.sort();
    groups.retain(|g| data::held_out_name(&g.file_stem().unwrap().to_string_lossy(), holdout) == held);
    // One segment per song, songs spread over the set, up to `items`.
    let step = (groups.len() / items.max(1)).max(1);
    let picked: Vec<PathBuf> = groups.iter().step_by(step).take(items).cloned().collect();

    let names = ["gt", "copy", "tf", "free"];
    let mut sum = [0.0f32; 4];
    let mut pitch_follow: [Vec<f32>; 4] = Default::default();
    let mut pitch_free_note = Vec::new();
    let (mut d_lab, mut d_pred, mut c_lab, mut c_pred) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut n_lines = 0usize;
    for (k, g) in picked.iter().enumerate() {
        let segs = read_words(g);
        let Some((shard, idx, ws, lyric)) = segs.get(segs.len() / 2).cloned() else { continue };
        let Some(it): Option<Item> = data::read_shard(Path::new(&shard)).ok().and_then(|v| v.get(idx).cloned()) else { continue };
        let sc = score::score_from_words(&ws, &it.f0, it.speaker as usize);
        if sc.notes.is_empty() {
            continue;
        }
        let f = score::align_segment(&sc, it.frames());
        let al = train::Aligned {
            tokens: f.tokens.clone(),
            dur: f.token_frames.clone(),
            notes: f.note.clone(),
            f0: it.f0.clone(),
            vel: f.note.iter().map(|n| if *n > 0.0 { 0.8 } else { 0.0 }).collect(),
            audio: it.audio_f32(),
            singer: it.speaker as usize,
            band: it.band_hz,
        };
        let b = train::ac_batch(&[al.clone()]);
        let (n, t) = (b.n, b.t);
        let audio = al.audio[..(t * HOP).min(al.audio.len())].to_vec();
        let voc = |mel: &Tensor| -> Vec<f32> {
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
        let mut gr = Graph::new(&cantor.params, false);
        let mel_t = train::target_mel(&mut gr, &b.audio, 1, t);
        let gt_mel = gr.val(mel_t).clone();
        let enc = acoustic::encode(&mut gr, &cantor.ac, &b.tokens, &b.singers, n, Some(b.tok_lens.clone()));
        let dec = acoustic::decode(&mut gr, &cantor.ac, enc.enc, &RowIndex::Host(b.frame_idx.clone()), t, Some(b.frame_lens.clone()), &b.note_feats, &f0f);
        let coarse = gr.val(dec.mel).clone();
        let ld = gr.host(enc.log_dur);
        for (i, tk) in al.tokens.iter().enumerate() {
            let (l, p) = ((1.0 + al.dur[i] as f32).ln(), ld[i]);
            d_lab.push(l);
            d_pred.push(p);
            if !ph::is_vowel(*tk) && *tk != ph::SP && *tk != ph::AP {
                c_lab.push(l);
                c_pred.push(p);
            }
        }
        let opts = RenderOpts { f0: if arg("--f0").as_deref() == Some("rule") { F0Mode::Rule } else { F0Mode::Model }, ..RenderOpts::default() };
        let (free, phrases) = cantor.render(&sc, &opts);
        // Free: per vowel (a token's run of frames on one note), the median of
        // the render's signed cents from the note, as a note pitch error.
        for p in &phrases {
            let got = dsp::f0_yin(&p.audio, 55.0, 1400.0);
            let f = &p.frames;
            let mut q = 0;
            while q < f.len() {
                let e = (q..f.len()).find(|r| f.token_of_frame[*r] != f.token_of_frame[q] || f.note[*r] != f.note[q]).unwrap_or(f.len());
                if ph::is_vowel(f.tokens[f.token_of_frame[q]]) && f.note[q] > 0.0 && e - q >= 8 {
                    let (a, b) = (q + (e - q) / 4, q + 3 * (e - q) / 4);
                    let c: Vec<f32> = (a..b.min(got.len())).filter(|r| got[*r] > 0.0).map(|r| 100.0 * (hz_to_midi(got[r]) - f.note[q])).collect();
                    if c.len() >= 3 {
                        pitch_free_note.push(median(c).abs());
                    }
                }
                q = e;
            }
        }
        let renders = [audio.clone(), voc(&gt_mel), voc(&coarse), free];
        let reference = lyric.trim().to_string();
        let toks: String = al.tokens.iter().map(|p| ph::symbol(*p)).collect::<Vec<_>>().join(" ");
        println!("#{k:02} singer {} {} frames [{toks}]", al.singer, t);
        println!("    frames per token {:?}", al.dur);
        println!("    predicted        {:?}", ld.iter().take(al.tokens.len()).map(|d| (d.exp() - 1.0).max(0.0).round() as usize).collect::<Vec<_>>());
        println!("    {:<6} [{}]", "lyric", reference);
        for (e, a) in renders.iter().enumerate() {
            let text = transcribe(&whisper, a);
            let (err, nw) = wer(&reference, &text);
            let w = (err as f32 / nw.max(1) as f32).min(1.0);
            sum[e] += w;
            if e == 1 || e == 2 {
                pitch_follow[e].extend(follow_cents(a, &al.f0));
            }
            println!("    {:<6} {:4.0}%  [{}]", names[e], w * 100.0, text.trim());
            if k < wavs {
                std::fs::write(out.join(format!("{k:02}-{}.wav", names[e])), dsp::wav_bytes(48_000, a)).unwrap();
            }
        }
        n_lines += 1;
    }
    println!();
    println!("{} {} segments (one per song), WER* against the lyric", n_lines, if held { "held-out" } else { "training" });
    for e in 0..names.len() {
        println!("{:<6} {:5.1}%", names[e], 100.0 * sum[e] / n_lines.max(1) as f32);
    }
    println!(
        "pitch: copy follows the recording's f0 within {:.1} c, tf {:.1} c (median |cents|); free: note pitch error {:.1} c (mean over vowels of |median cents|)",
        median(pitch_follow[1].clone()),
        median(pitch_follow[2].clone()),
        pitch_free_note.iter().sum::<f32>() / pitch_free_note.len().max(1) as f32
    );
    println!("durations: corr(ln 1+frames) all tokens {:.3} ({}), consonants {:.3} ({})", corr(&d_lab, &d_pred), d_lab.len(), corr(&c_lab, &c_pred), c_lab.len());
}
