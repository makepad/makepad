//! Cantor end to end: a sung line in, 48 kHz audio out.

use crate::acoustic::{self, AcousticConfig};
use crate::dsp::{self, midi_to_hz, Rng, SR};
use crate::nn::{Graph, Params, RowIndex};
use crate::phonemes as ph;
use crate::score::{self, Frames, Note, PitchStyle, SingScore};
use crate::vocoder::{self, VocoderConfig};
use crate::weights::{self, Dtype};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;

pub struct Cantor {
    pub ac: AcousticConfig,
    pub voc: VocoderConfig,
    pub params: Params,
    /// Whether the checkpoint's duration head was trained (else the rule table).
    pub durations_trained: bool,
    pub f0_trained: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum F0Mode {
    /// The rule curve (spring, scoop, vibrato).
    Rule,
    /// The model's f0 head over the note.
    Model,
}

#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub style: PitchStyle,
    pub f0: F0Mode,
    pub refine_steps: usize,
    pub refine_t0: f32,
    pub seed: u64,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts { style: PitchStyle::default(), f0: F0Mode::Rule, refine_steps: 4, refine_t0: 0.6, seed: 1 }
    }
}

/// What one phrase rendered to, for inspection.
pub struct Phrase {
    pub start: f32,
    pub frames: Frames,
    pub mel: Vec<f32>,
    pub audio: Vec<f32>,
}

impl Cantor {
    pub fn random(ac: AcousticConfig, voc: VocoderConfig, seed: u64) -> Cantor {
        let mut rng = Rng::new(seed);
        let mut params = Params::new();
        ac.declare(&mut params, &mut rng);
        voc.declare(&mut params, &mut rng);
        Cantor { ac, voc, params, durations_trained: false, f0_trained: false }
    }

    pub fn config(&self) -> Vec<(String, String)> {
        let mut c: Vec<(String, String)> = Vec::new();
        c.extend(self.ac.to_kv().into_iter().map(|(k, v)| (format!("ac.{k}"), v)));
        c.extend(self.voc.to_kv().into_iter().map(|(k, v)| (format!("voc.{k}"), v)));
        c.push(("durations_trained".into(), (self.durations_trained as u8).to_string()));
        c.push(("f0_trained".into(), (self.f0_trained as u8).to_string()));
        c
    }

    pub fn load(path: &Path) -> std::io::Result<Cantor> {
        let wf = weights::read(path)?;
        let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
        let ac = AcousticConfig::from_kv(&weights::section(&wf.config, "ac.")).ok_or_else(|| bad("acoustic config"))?;
        let voc = VocoderConfig::from_kv(&weights::section(&wf.config, "voc.")).ok_or_else(|| bad("vocoder config"))?;
        let flag = |k: &str| wf.config.iter().any(|(a, v)| a == k && v == "1");
        // Every parameter the configs declare must be present with its shape.
        let mut expect = Params::new();
        let mut rng = Rng::new(0);
        ac.declare(&mut expect, &mut rng);
        voc.declare(&mut expect, &mut rng);
        for (n, t) in expect.names.iter().zip(&expect.vals) {
            if !wf.params.has(n) {
                return Err(bad(&format!("missing tensor {n}")));
            }
            let w = wf.params.get(n);
            if (w.rows, w.cols) != (t.rows, t.cols) {
                return Err(bad(&format!("tensor {n} is {}x{}, expected {}x{}", w.rows, w.cols, t.rows, t.cols)));
            }
        }
        Ok(Cantor { ac, voc, params: wf.params, durations_trained: flag("durations_trained"), f0_trained: flag("f0_trained") })
    }

    pub fn save(&self, path: &Path, dtype: Dtype) -> std::io::Result<()> {
        weights::write(path, &self.config(), &self.params, dtype)
    }

    /// Align one phrase: rule lengths first, then (if trained) the duration
    /// head's lengths for the same consonants in the same order.
    pub fn frames(&self, phrase: &SingScore, opts: &RenderOpts) -> Frames {
        let rule = score::align(phrase, &score::rule_consonants, &opts.style, 0.3);
        if !self.durations_trained {
            return rule;
        }
        let mut g = Graph::new(&self.params, false);
        let enc = acoustic::encode(&mut g, &self.ac, &rule.tokens, &[phrase.singer], rule.tokens.len(), None);
        let pred = g.host(enc.log_dur);
        let queue: RefCell<VecDeque<f32>> = RefCell::new(
            rule.tokens.iter().zip(&pred).filter(|(t, _)| !ph::is_vowel(**t) && **t != ph::SP && **t != ph::AP).map(|(_, d)| (d.exp() - 1.0).max(1.0) / 100.0).collect(),
        );
        let clen = |p: ph::Ph, coda: bool| queue.borrow_mut().pop_front().unwrap_or_else(|| ph::rule_len(p, coda));
        score::align(phrase, &clen, &opts.style, 0.3)
    }

    /// Render one aligned phrase to (normalised mel, audio).
    pub fn render_frames(&self, f: &Frames, opts: &RenderOpts) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let t = f.len();
        let mut g = Graph::new(&self.params, false);
        let n = f.tokens.len();
        let enc = acoustic::encode(&mut g, &self.ac, &f.tokens, &[f.singer], n, None);
        let note = f.note_feats();
        let f0 = match opts.f0 {
            F0Mode::Rule => f.f0.clone(),
            F0Mode::Model => {
                // Run the f0 head with no f0 input, then build the curve.
                let mut g2 = Graph::new(&self.params, false);
                let e2 = acoustic::encode(&mut g2, &self.ac, &f.tokens, &[f.singer], n, None);
                let d = acoustic::decode(&mut g2, &self.ac, e2.enc, &RowIndex::Host(f.token_of_frame.clone()), t, None, &note, &vec![0.0; t * score::FEATS]);
                let head = g2.val(d.f0_head).clone();
                (0..t)
                    .map(|r| {
                        let voiced = ph::is_voiced(f.tokens[f.token_of_frame[r]]) && head.data[r * 2 + 1] > 0.0 && f.note[r] > 0.0;
                        if voiced { midi_to_hz(f.note[r] + 2.0 * head.data[r * 2]) } else { 0.0 }
                    })
                    .collect()
            }
        };
        let dec = acoustic::decode(&mut g, &self.ac, enc.enc, &RowIndex::Host(f.token_of_frame.clone()), t, None, &note, &f.f0_feats(&f0));
        let coarse = g.val(dec.mel).clone();
        let cond = g.val(dec.cond).clone();
        let mel = acoustic::refine(&self.params, &self.ac, &coarse, &cond, opts.refine_t0, opts.refine_steps, opts.seed);
        let src = vocoder::SourceCtl::new(&f0, t, opts.seed ^ 0x5eed);
        let mut gv = Graph::new(&self.params, false);
        let mi = gv.input(mel.clone());
        let w = vocoder::forward(&mut gv, &self.voc, mi, &f0, t, &src);
        (mel.data, gv.host(w), f0)
    }

    /// Render a whole line, phrase by phrase (split at rests of 0.6 s or more).
    pub fn render(&self, s: &SingScore, opts: &RenderOpts) -> (Vec<f32>, Vec<Phrase>) {
        let end = s.notes.iter().map(|n| n.start + n.dur).fold(0.0f32, f32::max);
        let mut out = vec![0.0; ((end + 0.6) * SR as f32) as usize];
        let mut phrases = Vec::new();
        for group in split_phrases(&s.notes, 0.6) {
            let p = SingScore { notes: group, singer: s.singer };
            let frames = self.frames(&p, opts);
            let (mel, audio, _) = self.render_frames(&frames, opts);
            let start = p.notes[0].start - 0.3;
            let at = (start * SR as f32).round() as isize;
            for (i, v) in audio.iter().enumerate() {
                let o = at + i as isize;
                if o >= 0 && (o as usize) < out.len() {
                    out[o as usize] += v;
                }
            }
            phrases.push(Phrase { start, frames, mel, audio });
        }
        (out, phrases)
    }
}

pub fn split_phrases(notes: &[Note], gap: f32) -> Vec<Vec<Note>> {
    let mut out: Vec<Vec<Note>> = Vec::new();
    for n in notes {
        let new = match out.last().and_then(|g| g.last()) {
            Some(prev) => n.start - (prev.start + prev.dur) >= gap,
            None => true,
        };
        if new {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(n.clone());
    }
    out
}

/// Pitch accuracy of rendered audio against its f0 curve: mean |cents| over
/// frames both call voiced (steady parts), and how many frames were compared.
pub fn f0_error_cents(audio: &[f32], f0: &[f32]) -> (f32, usize) {
    let est = dsp::f0_yin(audio, 55.0, 1400.0);
    let mut sum = 0.0;
    let mut n = 0;
    for t in 2..f0.len().min(est.len()).saturating_sub(2) {
        if f0[t] > 0.0 && est[t] > 0.0 && f0[t - 2] > 0.0 && f0[t + 2] > 0.0 {
            sum += (1200.0 * (est[t] / f0[t]).log2()).abs();
            n += 1;
        }
    }
    (if n > 0 { sum / n as f32 } else { f32::NAN }, n)
}
