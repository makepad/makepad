//! A sung line through the neural voice (feature `neural`): the same lowered
//! events the DSP singer plays (syllables, melismas, the written beats)
//! become a Cantor score, so switching engines changes the timbre, not the
//! timing.
//!
//! [`NeuralSinger`] renders whole phrases ahead of time on its own thread and
//! keeps them by a key of what they sing, so a live instrument only plays
//! audio that is already there: the audio thread never runs the model.
//! [`shared`] is the one singer a process uses, loaded from the installed
//! voice ([`model_path`]).

use crate::lyrics::Lowered;
use crate::phoneme;
use crate::singer::PhonemeSeq;
use makepad_ai_sing::score::{Note, SingScore, Syllable};
use makepad_ai_sing::{phonemes, Cantor, RenderOpts};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

pub use makepad_ai_sing;

/// The voice the score's singer sings with (the trained main voice).
pub const SINGER: usize = 1600;
/// A phrase's audio starts this long before its first note (the onset
/// consonants lead the beat), at [`RATE`].
pub const LEAD: f32 = 0.3;
/// The rate of a phrase's audio.
pub const RATE: u32 = 48_000;
/// Where an installed voice lives under the Makepad home's `weights/`.
pub const MODEL_FILE: &str = "cantor/cantor.mksing";
/// The voice's output level: its sung lines sit where the realtime singer's
/// do, so switching a voice to it keeps the mix. Measured on the 20 eval
/// lines (96 bpm, one note per word): the voice's voiced RMS -12.7 dBFS
/// (peaks to -0.8), the realtime alto, soprano and tenor -26.6 dBFS on
/// average; -13.9 dB.
pub const LEVEL: f32 = 0.202;
/// Phrases kept (about 0.6 MB per 3 s phrase).
const CACHE_PHRASES: usize = 256;

/// The installed neural voice: `MAKEPAD_CANTOR_MODEL`, else
/// `<makepad home>/weights/cantor/cantor.mksing` (the AI Hub's weights
/// folder, `MAKEPAD_ASSET_AI_CACHE` when set).
pub fn model_path() -> PathBuf {
    if let Some(path) = std::env::var_os("MAKEPAD_CANTOR_MODEL").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let weights = std::env::var_os("MAKEPAD_ASSET_AI_CACHE").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("MAKEPAD_HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(".makepad"));
        home.join("weights")
    });
    weights.join(MODEL_FILE)
}

/// A sung line as a phrase: its score moved so the first note starts at
/// [`LEAD`] seconds, the key of what it sings, and the beat its first note
/// is on (the phrase's audio starts [`LEAD`] seconds before that beat).
pub fn phrase_of(line: &Lowered, bpm: f64, singer: usize) -> Option<(u64, SingScore, f64)> {
    let first = line.events.iter().filter(|e| e.part == 0).map(|e| e.vowel_beat).fold(f64::INFINITY, f64::min);
    let mut score = to_sing_score(line, bpm, singer);
    if score.notes.is_empty() || !first.is_finite() {
        return None;
    }
    let shift = score.notes[0].start - LEAD;
    for n in &mut score.notes {
        n.start -= shift;
    }
    Some((phrase_key(&score), score, first))
}

/// The key of what a phrase sings (notes, timing to a tenth of a
/// millisecond, syllables, singer).
pub fn phrase_key(score: &SingScore) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let q = |v: f32, step: f32| (v / step).round() as i64;
    score.singer.hash(&mut h);
    for n in &score.notes {
        q(n.start, 1e-4).hash(&mut h);
        q(n.dur, 1e-4).hash(&mut h);
        q(n.midi, 1e-3).hash(&mut h);
        q(n.vel, 1e-3).hash(&mut h);
        match &n.syllable {
            Some(s) => (1u8, &s.onset, &s.nucleus, &s.coda).hash(&mut h),
            None => 0u8.hash(&mut h),
        }
    }
    h.finish()
}

struct Jobs {
    queue: VecDeque<(u64, SingScore)>,
    queued: HashSet<u64>,
}

struct Inner {
    cantor: Cantor,
    opts: RenderOpts,
    /// Rendered phrases, and their order of arrival (oldest dropped first).
    cache: Mutex<(HashMap<u64, Arc<[f32]>>, VecDeque<u64>)>,
    jobs: Mutex<Jobs>,
    wake: Condvar,
}

impl Inner {
    fn render(&self, score: &SingScore) -> Arc<[f32]> {
        self.cantor.render(score, &self.opts).0.into_iter().map(|v| v * LEVEL).collect()
    }

    fn keep(&self, key: u64, audio: Arc<[f32]>) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let (map, order) = &mut *cache;
        if map.insert(key, audio).is_none() {
            order.push_back(key);
        }
        while order.len() > CACHE_PHRASES {
            if let Some(old) = order.pop_front() {
                map.remove(&old);
            }
        }
    }
}

/// The neural voice as a phrase renderer: phrases asked for with
/// [`NeuralSinger::want`] render on the singer's thread, in order; a player
/// takes them with [`NeuralSinger::get`] (never waits). Clones share one
/// model, thread and cache.
#[derive(Clone)]
pub struct NeuralSinger {
    inner: Arc<Inner>,
}

impl NeuralSinger {
    /// Load a voice and start its thread.
    pub fn load(path: &std::path::Path) -> Result<NeuralSinger, String> {
        let cantor = Cantor::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(NeuralSinger::new(cantor))
    }

    /// A singer for a voice already in memory.
    pub fn new(cantor: Cantor) -> NeuralSinger {
        let inner = Arc::new(Inner {
            cantor,
            opts: RenderOpts::default(),
            cache: Mutex::new((HashMap::new(), VecDeque::new())),
            jobs: Mutex::new(Jobs { queue: VecDeque::new(), queued: HashSet::new() }),
            wake: Condvar::new(),
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("neural-singer".into())
            .spawn(move || loop {
                let Some(inner) = weak.upgrade() else { return };
                let job = {
                    let mut jobs = inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(job) = jobs.queue.pop_front() {
                            break Some(job);
                        }
                        // Wake now and then so a dropped singer ends its thread.
                        let (j, _) = inner.wake.wait_timeout(jobs, std::time::Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner());
                        jobs = j;
                        if jobs.queue.is_empty() {
                            break None;
                        }
                    }
                };
                if let Some((key, score)) = job {
                    let audio = inner.render(&score);
                    inner.keep(key, audio);
                    inner.jobs.lock().unwrap_or_else(|e| e.into_inner()).queued.remove(&key);
                }
            })
            .expect("neural singer thread");
        NeuralSinger { inner }
    }

    /// Ask for a phrase (from a composer, not the audio thread): it renders
    /// unless it is kept or already asked for.
    pub fn want(&self, key: u64, score: SingScore) {
        if self.inner.cache.lock().unwrap_or_else(|e| e.into_inner()).0.contains_key(&key) {
            return;
        }
        let mut jobs = self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.queued.insert(key) {
            jobs.queue.push_back((key, score));
            self.inner.wake.notify_one();
        }
    }

    /// A rendered phrase, if it is there (an audio thread may ask: a short
    /// lock, never a wait on a render).
    pub fn get(&self, key: u64) -> Option<Arc<[f32]>> {
        self.inner.cache.lock().unwrap_or_else(|e| e.into_inner()).0.get(&key).cloned()
    }

    /// A phrase now, rendering it on this thread if it is not kept (offline
    /// renders: bakes, auditions).
    pub fn render_now(&self, key: u64, score: &SingScore) -> Arc<[f32]> {
        if let Some(audio) = self.get(key) {
            return audio;
        }
        let audio = self.inner.render(score);
        self.inner.keep(key, audio.clone());
        audio
    }

    /// Phrases waiting to render.
    pub fn backlog(&self) -> usize {
        self.inner.jobs.lock().unwrap_or_else(|e| e.into_inner()).queue.len()
    }
}

/// The phrase book: the scores of the phrases a score's `sing()` lines made,
/// by key, so the composer can ask the singer for a phrase it only knows by
/// its key (a score event carries the key, not the score). Bounded: the
/// oldest go first.
fn book() -> &'static Mutex<(HashMap<u64, SingScore>, VecDeque<u64>)> {
    static BOOK: OnceLock<Mutex<(HashMap<u64, SingScore>, VecDeque<u64>)>> = OnceLock::new();
    BOOK.get_or_init(|| Mutex::new((HashMap::new(), VecDeque::new())))
}

/// Keep a phrase's score in the book (a VM's `sing()`).
pub fn remember(key: u64, score: SingScore) {
    let mut book = book().lock().unwrap_or_else(|e| e.into_inner());
    let (map, order) = &mut *book;
    if map.insert(key, score).is_none() {
        order.push_back(key);
    }
    while order.len() > 4 * CACHE_PHRASES {
        if let Some(old) = order.pop_front() {
            map.remove(&old);
        }
    }
}

/// A phrase's score from the book.
pub fn recall(key: u64) -> Option<SingScore> {
    book().lock().unwrap_or_else(|e| e.into_inner()).0.get(&key).cloned()
}

/// The process's singer, loaded from [`model_path`] when first asked for;
/// `None` while no voice is installed (asked again on the next call).
pub fn shared() -> Option<NeuralSinger> {
    static SINGER_SLOT: OnceLock<Mutex<Option<NeuralSinger>>> = OnceLock::new();
    let slot = SINGER_SLOT.get_or_init(|| Mutex::new(None));
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let path = model_path();
        if path.is_file() {
            *slot = NeuralSinger::load(&path).ok();
        }
    }
    slot.clone()
}

/// The melody part of a lowered line as a Cantor score (seconds; notes start
/// on their written beats, Cantor leads the onsets itself).
pub fn to_sing_score(line: &Lowered, bpm: f64, singer: usize) -> SingScore {
    let spb = 60.0 / bpm;
    let mut notes = Vec::new();
    for e in line.events.iter().filter(|e| e.part == 0) {
        let seq = &line.table[e.syllable as usize];
        let written_end = e.beat + e.dur;
        let syllable = if seq.flags & PhonemeSeq::CONT != 0 {
            None
        } else {
            // The voice's phonemes from each phone's IPA (a diphthong or an
            // r-coloured vowel the realtime singer keeps as one phone is two
            // or more here).
            let ph: Vec<u8> = seq.phones().iter().flat_map(|p| phonemes::parse(phoneme::info(*p).ipa)).collect();
            Some(Syllable::from_phones(&ph))
        };
        notes.push(Note {
            start: (e.vowel_beat * spb) as f32,
            dur: ((written_end - e.vowel_beat) * spb).max(0.02) as f32,
            midi: e.note,
            vel: e.vel,
            syllable,
        });
    }
    SingScore { notes, singer }
}

/// Render the line at 48 kHz mono; sample 0 is beat 0.
pub fn render(cantor: &Cantor, line: &Lowered, bpm: f64, singer: usize, opts: &RenderOpts) -> Vec<f32> {
    cantor.render(&to_sing_score(line, bpm, singer), opts).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::{builtin_pronounce, lower_sing, NoteIn, SingOpts};

    #[test]
    fn a_lowered_line_renders_through_the_neural_voice() {
        let notes: Vec<NoteIn> = (0..4).map(|i| NoteIn { beat: i as f64, dur: 1.0, pitches: vec![60.0 + i as f32], vel: 0.8 }).collect();
        let line = lower_sing(&notes, "hel-lo my love", &SingOpts::default(), &builtin_pronounce);
        let s = to_sing_score(&line, 120.0, 0);
        assert_eq!(s.notes.len(), 4);
        assert!((s.notes[1].start - 0.5).abs() < 1e-4, "note starts on its written beat");
        assert!(s.notes.iter().all(|n| n.syllable.as_ref().map(|s| !s.nucleus.is_empty()).unwrap_or(true)));
        // Every phone the realtime singer uses has the voice's phonemes.
        let line = lower_sing(&notes, "go home now oh", &SingOpts::default(), &builtin_pronounce);
        let s = to_sing_score(&line, 120.0, 0);
        let sung: Vec<u8> = s.notes.iter().filter_map(|n| n.syllable.as_ref()).flat_map(|s| s.onset.iter().chain(&s.nucleus).chain(&s.coda).copied()).collect();
        assert_eq!(phonemes::parse("ɡoʊhoʊmnaʊoʊ"), sung);
        let two: Vec<NoteIn> = (0..2).map(|i| NoteIn { beat: i as f64, dur: 1.0, pitches: vec![64.0], vel: 0.8 }).collect();
        let la = phrase_of(&lower_sing(&two, "la la", &SingOpts::default(), &builtin_pronounce), 120.0, SINGER).unwrap();
        let lo = phrase_of(&lower_sing(&two, "me me", &SingOpts::default(), &builtin_pronounce), 120.0, SINGER).unwrap();
        assert_ne!(la.1, lo.1, "{:?} {:?}", la.1, lo.1);
        assert_ne!(la.0, lo.0);
        let cantor = Cantor::random(makepad_ai_sing::acoustic::AcousticConfig::tiny(), makepad_ai_sing::vocoder::VocoderConfig::tiny(), 1);
        let audio = render(&cantor, &line, 120.0, 0, &RenderOpts::default());
        assert!(audio.len() >= 48_000 * 2 && audio.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn a_phrase_renders_ahead_and_is_kept_by_what_it_sings() {
        let notes: Vec<NoteIn> = (0..3).map(|i| NoteIn { beat: 4.0 + i as f64, dur: 1.0, pitches: vec![62.0 + i as f32], vel: 0.8 }).collect();
        let line = lower_sing(&notes, "sing a-way", &SingOpts::default(), &builtin_pronounce);
        let (key, score, first) = phrase_of(&line, 120.0, SINGER).unwrap();
        assert!((score.notes[0].start - LEAD).abs() < 1e-5, "the phrase starts its lead before its first note");
        assert!((first - 4.0).abs() < 1e-9, "first note on beat {first}");
        // The same words and notes later in the song are the same phrase.
        let later: Vec<NoteIn> = notes.iter().map(|n| NoteIn { beat: n.beat + 16.0, ..n.clone() }).collect();
        let line2 = lower_sing(&later, "sing a-way", &SingOpts::default(), &builtin_pronounce);
        assert_eq!(phrase_of(&line2, 120.0, SINGER).unwrap().0, key);
        let other = lower_sing(&notes, "sing a-long", &SingOpts::default(), &builtin_pronounce);
        assert_ne!(phrase_of(&other, 120.0, SINGER).unwrap().0, key);

        let cantor = Cantor::random(makepad_ai_sing::acoustic::AcousticConfig::tiny(), makepad_ai_sing::vocoder::VocoderConfig::tiny(), 1);
        let singer = NeuralSinger::new(cantor);
        assert!(singer.get(key).is_none());
        singer.want(key, score.clone());
        let t = std::time::Instant::now();
        let audio = loop {
            if let Some(a) = singer.get(key) {
                break a;
            }
            assert!(t.elapsed().as_secs() < 30, "the phrase never rendered");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert!(audio.len() as f32 >= (LEAD + 1.5) * RATE as f32 && audio.iter().all(|v| v.is_finite()));
        assert!(Arc::ptr_eq(&audio, &singer.render_now(key, &score)), "kept, not rendered again");
        assert_eq!(singer.backlog(), 0);
    }
}
