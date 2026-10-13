//! The Cantor DiT voice tool (feature `dit`).
//!
//!   cantor_dit convert --pt model.pt --phones phone_set.json --out M.mksing
//!   cantor_dit voice   --model M.mksing --name N --wav clip.wav --score clip.score.tsv
//!   cantor_dit render  --model M.mksing --scores DIR --out DIR [--voice N] [--steps 32]
//!                      [--cfg 3] [--seed 1] [--only 00,01] [--repeat K]
//!
//! `voice` adds a sung clip (its wav and its score) as a voice. `render`
//! sings each `NN.score.tsv` (sing_eval / voice_eval `--dump` files: notes
//! with the word each sings, the words) into `NN.wav` (24 kHz, time 0 at
//! the score start) and prints the real-time factor. The device path's
//! numeric check against the reference is `tests/dit_parity.rs`.

use makepad_ai_sing::dit::front::{self, ScoreNote, Tokens};
use makepad_ai_sing::dit::{self, CantorDit, DitOpts, Voice};
use makepad_ai_sing::dsp;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn arg(k: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned()
}

fn need(k: &str) -> String {
    arg(k).unwrap_or_else(|| panic!("{k} is required"))
}

/// A dumped score: (singer, end seconds, word texts, notes).
fn read_score(path: &Path) -> (Option<String>, f64, Vec<String>, Vec<ScoreNote>) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (mut singer, mut end) = (None, None);
    let mut words: Vec<(usize, String)> = Vec::new();
    let mut notes = Vec::new();
    for l in text.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        match c[0] {
            "item" => {
                singer = Some(c[2].to_string());
                end = Some(c[3].parse::<f64>().unwrap() / 100.0);
            }
            "word" => words.push((c[1].parse().unwrap(), c[2].to_string())),
            "note" => notes.push(ScoreNote { start: c[1].parse().unwrap(), dur: c[2].parse().unwrap(), midi: c[3].parse().unwrap(), word: c[4].parse().unwrap() }),
            _ => {}
        }
    }
    words.sort_by_key(|w| w.0);
    let end = end.unwrap_or_else(|| notes.last().map(|n| (n.start + n.dur) as f64 + 0.5).unwrap_or(0.0));
    (singer, end, words.into_iter().map(|w| w.1).collect(), notes)
}

fn arpa(word: &str) -> Vec<String> {
    let w: String = word.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect();
    front::arpa_word(&w, &makepad_ai_speech::g2p::pronounce(&w))
}

fn score_tokens(dit: &CantorDit, path: &Path, lead: f64) -> (Option<String>, Tokens) {
    let (singer, end, words, notes) = read_score(path);
    let phones: Vec<Vec<String>> = words.iter().map(|w| arpa(w)).collect();
    let wn = front::word_notes(&notes, &phones, end, lead);
    (singer, dit.tokens(&wn).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

fn read_wav24(path: &Path) -> Vec<f32> {
    let (x, rate) = dsp::read_wav(&std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).expect("wav");
    if rate == dit::SR {
        x
    } else {
        dsp::resample(&x, rate, dit::SR)
    }
}

fn load(path: &str) -> CantorDit {
    let t = Instant::now();
    let m = CantorDit::load(Path::new(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    eprintln!("loaded {path} in {:.1} s ({} voices)", t.elapsed().as_secs_f32(), m.voices.len());
    m
}

fn opts() -> DitOpts {
    let d = DitOpts::default();
    DitOpts {
        steps: arg("--steps").and_then(|v| v.parse().ok()).unwrap_or(d.steps),
        cfg: arg("--cfg").and_then(|v| v.parse().ok()).unwrap_or(d.cfg),
        seed: arg("--seed").and_then(|v| v.parse().ok()).unwrap_or(d.seed),
        lead: arg("--lead").and_then(|v| v.parse().ok()).unwrap_or(d.lead),
    }
}

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "convert" => {
            let phones_json = std::fs::read_to_string(need("--phones")).expect("phone set");
            // A JSON list of strings (no escapes in the phone names).
            let phones: Vec<String> = phones_json.split('"').skip(1).step_by(2).map(String::from).collect();
            let (config, params) = dit::convert(Path::new(&need("--pt")), &phones).expect("convert");
            dit::save(Path::new(&need("--out")), &config, &params).expect("write");
            println!("{} tensors, {:.1} M parameters, {} phones", params.names.len(), params.count() as f64 / 1e6, phones.len());
        }
        "voice" => {
            let path = need("--model");
            let wf = makepad_ai_sing::weights::read(Path::new(&path)).expect("model");
            let dit_model = load(&path);
            let (_, tokens) = score_tokens(&dit_model, Path::new(&need("--score")), opts().lead);
            let mut wav = read_wav24(Path::new(&need("--wav")));
            wav.truncate(tokens.frames() * dit::HOP);
            let mel = dit_model.mel_of(&wav).expect("mel");
            let mut tokens = tokens;
            let frames = mel.len() / dit::MELS;
            tokens.mel2note.truncate(frames);
            let voice = Voice { name: need("--name"), mel, tokens };
            let (mut config, mut params) = (wf.config, wf.params);
            dit::add_voice(&mut config, &mut params, &voice);
            dit::save(Path::new(&path), &config, &params).expect("write");
            println!("voice {}: {} frames", voice.name, frames);
        }
        "render" => {
            let m = load(&need("--model"));
            let o = opts();
            let out = PathBuf::from(need("--out"));
            std::fs::create_dir_all(&out).unwrap();
            let only: Option<Vec<String>> = arg("--only").map(|s| s.split(',').map(String::from).collect());
            let repeat: usize = arg("--repeat").and_then(|v| v.parse().ok()).unwrap_or(1);
            let mut files: Vec<PathBuf> = std::fs::read_dir(need("--scores")).unwrap().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".score.tsv")).collect();
            files.sort();
            let (mut t_model, mut t_audio, mut t_dit, mut t_voc) = (0f64, 0f64, 0f64, 0f64);
            for f in &files {
                let k = f.file_name().unwrap().to_string_lossy().split('.').next().unwrap().to_string();
                if only.as_ref().map(|o| !o.contains(&k)).unwrap_or(false) {
                    continue;
                }
                let (singer, tokens) = score_tokens(&m, f, o.lead);
                let name = singer.or_else(|| arg("--voice")).unwrap_or_else(|| "1600".into());
                let voice = m.voice(&name).unwrap_or_else(|| panic!("no voice {name}"));
                let seed_o = DitOpts { seed: o.seed.wrapping_mul(1000).wrapping_add(k.parse::<u64>().unwrap_or(0)), ..o.clone() };
                let mut audio = Vec::new();
                for r in 0..repeat {
                    let t0 = Instant::now();
                    let mel = m.sample(voice, &tokens, &seed_o, None).expect("sample");
                    let t1 = Instant::now();
                    audio = m.vocode(&mel, tokens.frames()).expect("vocode");
                    let t2 = Instant::now();
                    if r + 1 == repeat {
                        t_dit += (t1 - t0).as_secs_f64();
                        t_voc += (t2 - t1).as_secs_f64();
                        t_model += (t2 - t0).as_secs_f64();
                        t_audio += audio.len() as f64 / dit::SR as f64;
                        println!("{k} {:.2} s audio, {:.3} s (DiT {:.3}, vocoder {:.3})", audio.len() as f64 / dit::SR as f64, (t2 - t0).as_secs_f64(), (t1 - t0).as_secs_f64(), (t2 - t1).as_secs_f64());
                    }
                }
                std::fs::write(out.join(format!("{k}.wav")), dsp::wav_bytes(dit::SR, &audio)).unwrap();
            }
            println!("steps {} cfg {}: RTF {:.4} ({:.1} s for {:.1} s of audio; DiT {:.4}, vocoder {:.4})", o.steps, o.cfg, t_model / t_audio.max(1e-9), t_model, t_audio, t_dit / t_audio.max(1e-9), t_voc / t_audio.max(1e-9));
        }
        _ => eprintln!("usage: cantor_dit convert|voice|render (see the source header)"),
    }
}
