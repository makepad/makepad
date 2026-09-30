//! Training data: prepared items in shard files, written by `sing_prep`
//! and streamed by the trainer.
//!
//! A shard is `MKSDAT01` then items, each: a fixed header (kind, speaker,
//! sample count, frame count, token count, note count, band limit in Hz),
//! the tokens (u8), the notes per frame (f32 MIDI, 0 = none; only for sung
//! items), the f0 per frame (f32 Hz, 0 = unvoiced) and the 48 kHz audio (i16).

use crate::dsp::{self, HOP, SR};
use crate::phonemes::{self as ph, Ph};
use std::io::{self, Read, Write};

pub const MAGIC: &[u8; 8] = b"MKSDAT01";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    /// Read speech with a phoneme transcript (durations learned by alignment).
    Speech = 0,
    /// Singing with a phoneme sequence and notes.
    Sung = 1,
    /// Audio only (the vocoder).
    Audio = 2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub speaker: u32,
    /// Content above this is missing (the source's Nyquist).
    pub band_hz: f32,
    pub tokens: Vec<Ph>,
    pub notes: Vec<f32>,
    pub f0: Vec<f32>,
    pub audio: Vec<i16>,
}

impl Item {
    pub fn frames(&self) -> usize {
        self.f0.len()
    }

    pub fn audio_f32(&self) -> Vec<f32> {
        self.audio.iter().map(|v| *v as f32 / 32768.0).collect()
    }

    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        for v in [self.kind as u32, self.speaker, self.audio.len() as u32, self.f0.len() as u32, self.tokens.len() as u32, self.notes.len() as u32] {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&self.band_hz.to_le_bytes())?;
        w.write_all(&self.tokens)?;
        for v in &self.notes {
            w.write_all(&v.to_le_bytes())?;
        }
        for v in &self.f0 {
            w.write_all(&v.to_le_bytes())?;
        }
        let mut b = Vec::with_capacity(self.audio.len() * 2);
        for v in &self.audio {
            b.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&b)
    }

    pub fn read(r: &mut impl Read) -> io::Result<Option<Item>> {
        let mut h = [0u8; 28];
        match r.read_exact(&mut h) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let u = |i: usize| u32::from_le_bytes(h[i * 4..i * 4 + 4].try_into().unwrap());
        let kind = match u(0) {
            0 => Kind::Speech,
            1 => Kind::Sung,
            _ => Kind::Audio,
        };
        let (speaker, ns, nf, nt, nn) = (u(1), u(2) as usize, u(3) as usize, u(4) as usize, u(5) as usize);
        let band_hz = f32::from_le_bytes(h[24..28].try_into().unwrap());
        let mut tokens = vec![0u8; nt];
        r.read_exact(&mut tokens)?;
        let mut f32s = |n: usize| -> io::Result<Vec<f32>> {
            let mut b = vec![0u8; n * 4];
            r.read_exact(&mut b)?;
            Ok(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
        };
        let notes = f32s(nn)?;
        let f0 = f32s(nf)?;
        let mut b = vec![0u8; ns * 2];
        r.read_exact(&mut b)?;
        let audio = b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        Ok(Some(Item { kind, speaker, band_hz, tokens, notes, f0, audio }))
    }
}

pub fn write_shard(path: &std::path::Path, items: &[Item]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut w = io::BufWriter::new(std::fs::File::create(&tmp)?);
        w.write_all(MAGIC)?;
        for it in items {
            it.write(&mut w)?;
        }
        w.flush()?;
    }
    std::fs::rename(tmp, path)
}

pub fn read_shard(path: &std::path::Path) -> io::Result<Vec<Item>> {
    let mut r = io::BufReader::new(std::fs::File::open(path)?);
    let mut m = [0u8; 8];
    r.read_exact(&mut m)?;
    if &m != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not an MKSDAT01 shard"));
    }
    let mut out = Vec::new();
    while let Some(it) = Item::read(&mut r)? {
        out.push(it);
    }
    Ok(out)
}

/// Audio at any rate to a prepared item's audio + f0 (48 kHz, 10 ms frames).
pub fn prepare_audio(x: &[f32], rate: u32) -> (Vec<i16>, Vec<f32>, f32) {
    let y = dsp::resample(x, rate, SR);
    let frames = y.len() / HOP;
    let y = &y[..frames * HOP];
    let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-4);
    let gain = if peak > 0.95 { 0.95 / peak } else { 1.0 };
    let audio = y.iter().map(|v| ((v * gain).clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let mut f0 = dsp::f0_yin(y, 55.0, 1400.0);
    f0.truncate(frames);
    (audio, f0, (rate as f32 / 2.0).min(SR as f32 / 2.0))
}

/// Notes from a sung f0 curve: the nearest semitone of a 150 ms median,
/// held while the voice stays within 0.6 semitones of it.
pub fn notes_from_f0(f0: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0; f0.len()];
    let mut cur = 0.0f32;
    for t in 0..f0.len() {
        if f0[t] <= 0.0 {
            cur = 0.0;
            continue;
        }
        let mut w: Vec<f32> = f0[t.saturating_sub(7)..(t + 8).min(f0.len())].iter().filter(|v| **v > 0.0).map(|v| dsp::hz_to_midi(*v)).collect();
        w.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let m = w[w.len() / 2];
        if cur == 0.0 || (m - cur).abs() > 0.6 {
            cur = m.round();
        }
        out[t] = cur;
    }
    out
}

/// A sustained-vowel item's tokens: SP, the vowel, SP (the aligner finds
/// where the vowel is).
pub fn vowel_tokens(vowel: Ph) -> Vec<Ph> {
    vec![ph::SP, vowel, ph::SP]
}

/// A transcript to tokens: each word through `pronounce` (IPA in the TTS
/// notation), SP at the ends and at sentence punctuation.
pub fn text_tokens(text: &str, pronounce: &dyn Fn(&str) -> String) -> Vec<Ph> {
    let mut out = vec![ph::SP];
    for raw in text.split_whitespace() {
        let word: String = raw.chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect();
        if !word.is_empty() {
            out.extend(ph::parse(&pronounce(&word.to_lowercase())));
        }
        if raw.ends_with([',', '.', ';', ':', '!', '?']) && out.last() != Some(&ph::SP) {
            out.push(ph::SP);
        }
    }
    if out.last() != Some(&ph::SP) {
        out.push(ph::SP);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_round_trip_and_notes() {
        let it = Item { kind: Kind::Sung, speaker: 3, band_hz: 22050.0, tokens: vec![1, 5, 1], notes: vec![0.0, 60.0], f0: vec![0.0, 261.6], audio: vec![1, -2, 3] };
        let mut b = Vec::new();
        it.write(&mut b).unwrap();
        let back = Item::read(&mut &b[..]).unwrap().unwrap();
        assert_eq!(back, it);
        let f0: Vec<f32> = (0..50).map(|i| if i < 25 { 261.63 } else { 293.66 }).collect();
        let n = notes_from_f0(&f0);
        assert_eq!(n[5], 60.0);
        assert_eq!(n[45], 62.0);
    }
}

/// Where one item sits in its shard, and its sizes.
#[derive(Clone, Copy, Debug)]
pub struct ItemRef {
    pub shard: u32,
    pub offset: u64,
    pub kind: Kind,
    pub speaker: u32,
    pub band_hz: f32,
    pub samples: u32,
    pub frames: u32,
    pub tokens: u32,
    pub notes: u32,
}

impl ItemRef {
    fn tokens_at(&self) -> u64 {
        self.offset + 28
    }
    fn notes_at(&self) -> u64 {
        self.tokens_at() + self.tokens as u64
    }
    fn f0_at(&self) -> u64 {
        self.notes_at() + 4 * self.notes as u64
    }
    fn audio_at(&self) -> u64 {
        self.f0_at() + 4 * self.frames as u64
    }
}

/// Shards indexed, items read on demand (positioned reads; the page cache
/// keeps what is hot), so a corpus larger than memory streams.
pub struct Store {
    files: Vec<std::fs::File>,
    pub items: Vec<ItemRef>,
}

#[cfg(unix)]
fn read_at(f: &std::fs::File, buf: &mut [u8], at: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.read_exact_at(buf, at)
}

#[cfg(windows)]
fn read_at(f: &std::fs::File, buf: &mut [u8], mut at: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        let n = f.seek_read(&mut buf[done..], at)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short read"));
        }
        done += n;
        at += n as u64;
    }
    Ok(())
}

impl Store {
    /// Index every `.mksdat` shard in `dirs` (reads only the item headers).
    pub fn open(dirs: &[String]) -> io::Result<Store> {
        let mut paths = Vec::new();
        for d in dirs {
            for e in std::fs::read_dir(d)?.flatten() {
                if e.path().extension().map(|x| x == "mksdat").unwrap_or(false) {
                    paths.push(e.path());
                }
            }
        }
        paths.sort();
        let mut files = Vec::new();
        let mut items = Vec::new();
        for (si, p) in paths.iter().enumerate() {
            let f = std::fs::File::open(p)?;
            let len = f.metadata()?.len();
            let mut m = [0u8; 8];
            read_at(&f, &mut m, 0)?;
            if &m != MAGIC {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: not an MKSDAT01 shard", p.display())));
            }
            let mut at = 8u64;
            let mut h = [0u8; 28];
            while at + 28 <= len {
                read_at(&f, &mut h, at)?;
                let u = |i: usize| u32::from_le_bytes(h[i * 4..i * 4 + 4].try_into().unwrap());
                let kind = match u(0) {
                    0 => Kind::Speech,
                    1 => Kind::Sung,
                    _ => Kind::Audio,
                };
                let r = ItemRef {
                    shard: si as u32,
                    offset: at,
                    kind,
                    speaker: u(1),
                    samples: u(2),
                    frames: u(3),
                    tokens: u(4),
                    notes: u(5),
                    band_hz: f32::from_le_bytes(h[24..28].try_into().unwrap()),
                };
                at = r.audio_at() + 2 * r.samples as u64;
                items.push(r);
            }
            files.push(f);
        }
        Ok(Store { files, items })
    }

    fn bytes(&self, r: &ItemRef, at: u64, n: usize) -> Vec<u8> {
        let mut b = vec![0u8; n];
        read_at(&self.files[r.shard as usize], &mut b, at).expect("shard read");
        b
    }

    fn f32s(&self, r: &ItemRef, at: u64, n: usize) -> Vec<f32> {
        self.bytes(r, at, n * 4).chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
    }

    pub fn tokens(&self, r: &ItemRef) -> Vec<Ph> {
        self.bytes(r, r.tokens_at(), r.tokens as usize)
    }

    pub fn notes(&self, r: &ItemRef) -> Vec<f32> {
        self.f32s(r, r.notes_at(), r.notes as usize)
    }

    /// f0 of frames `from..from + n`.
    pub fn f0(&self, r: &ItemRef, from: usize, n: usize) -> Vec<f32> {
        self.f32s(r, r.f0_at() + 4 * from as u64, n)
    }

    /// Audio of frames `from..from + n` (n * HOP samples, as f32).
    pub fn audio(&self, r: &ItemRef, from: usize, n: usize) -> Vec<f32> {
        let s0 = from * HOP;
        let cnt = (n * HOP).min(r.samples as usize - s0);
        self.bytes(r, r.audio_at() + 2 * s0 as u64, cnt * 2).chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
    }

    /// The whole item.
    pub fn item(&self, r: &ItemRef) -> Item {
        let audio = self.bytes(r, r.audio_at(), 2 * r.samples as usize).chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        Item { kind: r.kind, speaker: r.speaker, band_hz: r.band_hz, tokens: self.tokens(r), notes: self.notes(r), f0: self.f0(r, 0, r.frames as usize), audio }
    }

    pub fn hours(&self) -> f64 {
        self.items.iter().map(|r| r.samples as f64).sum::<f64>() / SR as f64 / 3600.0
    }
}
