//! Karaoke lyrics: transcribe the separated VOCALS stem once per track, cache
//! the timed lines, and hand the program view something a person can sing from.
//!
//! Three facts drive the whole design.
//!
//! 1. **The vocals stem is already paid for.** `stems.rs` separates a track
//!    span by span into a digest-keyed cache; the moment that cache is
//!    COMPLETE the clean vocal is sitting on disk with no music under it.
//!    That is the best possible whisper input — no drums to hallucinate
//!    consonants out of, no bass smearing the mel bins — and it costs nothing
//!    extra to read it back.
//! 2. **Whisper's segment stamps are LATE.** The decoder emits a timestamp
//!    token where decoding settles, not where the voice starts, so a line
//!    stamped at 41.30 s is often sung from 41.05 s. Karaoke cannot tolerate
//!    that, so every segment start is snapped to the vocal's own onset — an
//!    RMS rise in the stem we already have in memory — inside a ±0.5 s window.
//! 3. **Karaoke is about the LEAD, not the stamp.** A line that appears
//!    exactly when its first word is sung is useless: the singer is still
//!    reading it. The display therefore runs AHEAD of the voice — a line
//!    appears a bar or two early and holds until its last word is out — and
//!    both transitions are quantized to the track's beat grid so the
//!    subtitles breathe with the music instead of at arbitrary millisecond
//!    stamps. The classic two-row layout does the rest: the current line
//!    bright with a colour sweep across it, the next line dim below, so there
//!    is ALWAYS one line of lookahead on screen.
//!
//! The bake runs on its own worker (`vj-lyrics`), never on the audio or UI
//! thread, and only after separation is finished so the two never fight for
//! the GPU. Results land in `local/vj/lyrics-cache/<digest>.json` — same
//! digest the stem cache is keyed by — so the second load of a track is a
//! file read.

use crate::decks::DeckId;
use crate::wave_analysis::TrackGrid;
use makepad_ai_stems::{CacheHeader, StemCache, Stem};
use makepad_widgets::makepad_platform::thread::{ThreadOptions, ThreadSpawner};

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;

/// How long the worker waits on the deck inbox before looking at the
/// background one. Mirrors the separator's own split.
const DECK_INBOX_WAIT_MS: u64 = 100;
use std::sync::Arc;

/// Sample rate whisper's mel front end expects.
pub use makepad_audio_lyrics::align::WHISPER_RATE;

/// Cache format tag + version, canonical in `makepad-audio-lyrics` now that
/// the same document is also the server-side `Lyrics` side-channel payload.
const CACHE_FORMAT: &str = makepad_audio_lyrics::LYRICS_FORMAT;
pub const CACHE_VERSION: u32 = makepad_audio_lyrics::LYRICS_VERSION;

/// The whisper checkpoint this bake wants. MIT weights, so no license gate.
const WHISPER_MODEL_FILE: &str = "ggml-large-v3-turbo.bin";

// ---------------------------------------------------------------------------
// the data
// ---------------------------------------------------------------------------

/// One timed lyric line in TRACK time (canonical in `makepad-audio-lyrics`;
/// deck tempo/keylock changes need no conversion because a deck's playhead
/// is already reported in track seconds).
pub use makepad_audio_lyrics::{LyricLine, OnsetStats, TrackLyrics};

/// Seconds added to the deck playhead before the karaoke display reads it.
///
/// The chain from a deck's playhead to a person's ear is short but not zero:
/// the mixer reports the frame it has RENDERED to, and the output buffer
/// between there and the speakers is a few milliseconds deep — plus whatever
/// a particular interface, a Bluetooth speaker or a projector adds. The
/// measured error on this machine is inside the noise, so the default is
/// zero and `VJ_KARAOKE_OFFSET_MS` is the one knob for a rig that needs it:
/// positive runs the words EARLIER, negative later.
pub fn display_offset_secs() -> f64 {
    static OFFSET: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::env::var("VJ_KARAOKE_OFFSET_MS")
            .ok()
            .and_then(|text| text.trim().parse::<f64>().ok())
            .filter(|ms| ms.is_finite() && ms.abs() <= 2_000.0)
            .map(|ms| ms / 1000.0)
            .unwrap_or(0.0)
    })
}


/// The karaoke fill contract lives in the shared widget family now (the
/// asset UI preview and the VJ surfaces paint the same fill); the VJ keeps
/// its `LyricLine`-shaped entry point.

/// How far across a line's characters the green has reached at `secs`.
pub fn sung_fraction(line: &LyricLine, secs: f64) -> f32 {
    makepad_asset_widgets::lyric_reader::sung_fraction(
        line.start_secs,
        line.end_secs,
        &line.text,
        &line.words,
        line.confident,
        secs,
    )
}


// ---------------------------------------------------------------------------
// where the cache lives
// ---------------------------------------------------------------------------

pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VJ_LYRICS_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/vj/lyrics-cache")
}

pub fn cache_path(dir: &std::path::Path, digest: &str) -> PathBuf {
    dir.join(format!("{digest}.json"))
}

pub fn load_cached(dir: &std::path::Path, digest: &str) -> Option<TrackLyrics> {
    let bytes = std::fs::read(cache_path(dir, digest)).ok()?;
    TrackLyrics::from_json(&bytes, digest)
}

pub fn store_cached(dir: &std::path::Path, digest: &str, lyrics: &TrackLyrics) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = cache_path(dir, digest);
    // Write beside and rename, so a crash mid-write cannot leave a half file
    // that parses as a shorter track.
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, lyrics.to_json(digest).as_bytes()).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// The whisper checkpoint, if this machine has one. `MAKEPAD_VOICE_MODEL`
/// wins; otherwise the working directory and the checkout root, which is
/// where the converse stack keeps it.
pub fn whisper_model_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MAKEPAD_VOICE_MODEL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let canonical = root.join("local/models").join(WHISPER_MODEL_FILE);
    [
        PathBuf::from(WHISPER_MODEL_FILE),
        root.join(WHISPER_MODEL_FILE),
        root.join("local").join(WHISPER_MODEL_FILE),
        canonical.clone(),
    ]
    .into_iter()
    .find(|path| {
        if !path.is_file() {
            return false;
        }
        // Every PROBED path must be the pinned artifact exactly. A download
        // that stopped half way leaves a short file, and trusting it by
        // presence alone hides INSTALL MODELS behind a model that cannot
        // load. Only an explicit `MAKEPAD_VOICE_MODEL` override (handled
        // above) is taken on trust.
        let _ = &canonical;
        crate::models::file_is_pinned_size(path, crate::models::WHISPER.bytes)
    })
}

// ---------------------------------------------------------------------------
// vocal onsets
// ---------------------------------------------------------------------------

/// Hop and window of the RMS envelope every onset is measured against.
///
/// 5 ms is four times finer than whisper's own 20 ms timestamp grid, which is
/// the point: the stamps are the coarse input and the stem is the ruler. It
/// also puts the quantization error of a snapped onset at 2.5 ms, well inside
/// the tens-of-milliseconds budget a karaoke fill has before the eye notices
/// it trailing the voice.
pub const ENV_HOP_SECS: f64 = 0.005;
const ENV_WINDOW_SECS: f64 = 0.020;
/// How far either side of whisper's stamp an onset is looked for.
const ONSET_SEARCH_SECS: f64 = 0.5;
/// A rise has to STAY up this long to count as a sung onset rather than a
/// breath or a separation artefact.
const ONSET_HOLD_SECS: f64 = 0.05;

/// RMS envelope of the vocals stem, in mono at the stem's own rate.
#[derive(Clone, Debug)]
pub struct VocalEnvelope {
    pub hop_secs: f64,
    pub values: Vec<f32>,
    /// A robust "this is what loud looks like on this track" reference: the
    /// 90th percentile of the envelope. Used to throw away lines whisper
    /// invented over silence.
    pub loud: f32,
}

impl VocalEnvelope {
    pub fn build(mono: &[f32], rate: f64) -> VocalEnvelope {
        let rate = rate.max(1.0);
        let hop = ((rate * ENV_HOP_SECS).round() as usize).max(1);
        let window = ((rate * ENV_WINDOW_SECS).round() as usize).max(hop);
        let frames = mono.len() / hop;
        let mut values = Vec::with_capacity(frames);
        for frame in 0..frames {
            let from = frame * hop;
            let to = (from + window).min(mono.len());
            let mut sum = 0.0f64;
            for sample in &mono[from..to] {
                sum += (*sample as f64) * (*sample as f64);
            }
            let count = (to - from).max(1) as f64;
            values.push((sum / count).sqrt() as f32);
        }
        let mut sorted: Vec<f32> = values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let loud = if sorted.is_empty() {
            0.0
        } else {
            sorted[(sorted.len() as f64 * 0.9) as usize % sorted.len()]
        };
        VocalEnvelope { hop_secs: hop as f64 / rate, values, loud }
    }

    fn frame_at(&self, secs: f64) -> usize {
        ((secs.max(0.0) / self.hop_secs).round() as usize).min(self.values.len().saturating_sub(1))
    }

    fn peak_between(&self, from: f64, to: f64) -> f32 {
        if self.values.is_empty() {
            return 0.0;
        }
        let a = self.frame_at(from);
        let b = self.frame_at(to).max(a);
        self.values[a..=b].iter().fold(0.0f32, |m, v| m.max(*v))
    }

    /// The sung start of a line whisper stamped at `stamp`, or `None` when the
    /// stem shows no rise worth snapping to. Never crosses `floor_secs` (the
    /// previous line's end) and never moves further than ±`ONSET_SEARCH_SECS`.
    pub fn onset_near(&self, stamp: f64, floor_secs: f64, ceiling_secs: f64) -> Option<f64> {
        if self.values.len() < 4 {
            return None;
        }
        let hold = ((ONSET_HOLD_SECS / self.hop_secs).round() as usize).max(1);
        let lo = self.frame_at((stamp - ONSET_SEARCH_SECS).max(floor_secs));
        let hi = self.frame_at((stamp + ONSET_SEARCH_SECS).min(ceiling_secs));
        if hi <= lo + hold {
            return None;
        }
        let stamp_frame = self.frame_at(stamp).clamp(lo, hi);
        let quiet = self.values[lo..=hi]
            .iter()
            .fold(f32::INFINITY, |m, v| m.min(*v));
        let peak = self.values[stamp_frame..=hi]
            .iter()
            .fold(0.0f32, |m, v| m.max(*v));
        // Nothing sung here, or no contrast to find an edge in.
        if peak <= 0.0 || peak < self.loud * 0.05 || peak <= quiet * 1.4 {
            return None;
        }
        let rise = peak - quiet;
        let up = quiet + rise * 0.20;
        let foot = quiet + rise * 0.05;
        let mut cross = None;
        for frame in lo..=hi.saturating_sub(hold) {
            if self.values[frame..frame + hold].iter().all(|v| *v >= up) {
                cross = Some(frame);
                break;
            }
        }
        let mut frame = cross?;
        // Walk back down the leading edge to the foot of the rise: the ear
        // hears the attack start there, not where it clears the threshold.
        while frame > lo && self.values[frame - 1] >= foot {
            frame -= 1;
        }
        Some(frame as f64 * self.hop_secs)
    }
}

// ---------------------------------------------------------------------------
// phrases
// ---------------------------------------------------------------------------

/// A stretch of the vocals stem that is actually being sung, bounded by the
/// silences either side of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocalPhrase {
    pub start_secs: f64,
    pub end_secs: f64,
}

impl VocalPhrase {
    fn secs(&self) -> f64 {
        (self.end_secs - self.start_secs).max(0.0)
    }
}

/// Silence that separates two sung phrases. Longer than the gaps consonants
/// leave inside a line, shorter than the pause between one line and the next.
const PHRASE_GAP_SECS: f64 = 0.38;
/// Anything shorter than this is a consonant or a separation artefact.
const PHRASE_MIN_SECS: f64 = 0.30;
/// A phrase this long is a line nobody can read at once; it gets cut again at
/// its quietest interior moment.
const PHRASE_MAX_SECS: f64 = 5.5;
/// Silence long enough that the phrases either side of it belong to different
/// passages of the song rather than the same sung stretch.
const CLUSTER_GAP_SECS: f64 = 4.0;
/// A cluster carrying less than this share of the strongest one's energy is
/// leakage, not singing.
const CLUSTER_KEEP: f64 = 0.35;
/// Words a singer gets through in a second. Deliberately on the slow side:
/// the number sets how much of a window is taken to be voice, and taking a
/// little too much only widens a phrase, while taking too little clips one.
const SUNG_WORDS_PER_SEC: f64 = 1.6;

impl VocalEnvelope {
    /// The sung phrases between `from` and `to`.
    ///
    /// This is the timing information the separation actually bought us:
    /// whisper hands back paragraphs — thirty seconds of five sentences under
    /// one pair of timestamps — but the isolated vocal shows exactly where the
    /// singer breathed. Cutting on those silences is what turns a transcript
    /// into karaoke.
    pub fn phrases(&self, from: f64, to: f64) -> Vec<VocalPhrase> {
        self.phrases_for_words(from, to, None)
    }

    /// The same, told how many words the window has to hold.
    ///
    /// That number is what makes the gate honest. No separation is clean: over
    /// a loud instrumental introduction the leaked band sits at HALF the level
    /// of the singing that follows (measured on the test track: 0.07 against
    /// 0.145 RMS), which no fixed threshold can cut between without also
    /// cutting quiet verses on other tracks. But thirteen words are about
    /// eight seconds of singing however loud the leak is, so take the eight
    /// LOUDEST seconds of the window and gate there. The threshold calibrates
    /// itself per segment against the one thing whisper is reliable about —
    /// what was said.
    pub fn phrases_for_words(
        &self,
        from: f64,
        to: f64,
        words: Option<usize>,
    ) -> Vec<VocalPhrase> {
        if self.values.is_empty() {
            return Vec::new();
        }
        let lo = self.frame_at(from.max(0.0));
        let hi = self.frame_at(to).max(lo);
        if hi <= lo {
            return Vec::new();
        }
        let peak = self.values[lo..=hi].iter().fold(0.0f32, |m, v| m.max(*v));
        if peak <= 0.0 || peak < self.loud * 0.03 {
            return Vec::new();
        }
        // A floor under the calibrated gate, so a window with no voice in it
        // cannot select its own silence, and one with nothing but voice keeps
        // all of it.
        let mut gate = (peak * 0.14).max(self.loud * 0.08);
        if let Some(words) = words {
            let span = hi - lo + 1;
            let wanted = (words as f64 / SUNG_WORDS_PER_SEC / self.hop_secs) as usize;
            let wanted = wanted
                .clamp((span as f64 * 0.15) as usize, (span as f64 * 0.85) as usize)
                .max(1);
            let mut sorted: Vec<f32> = self.values[lo..=hi].to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let at = span.saturating_sub(wanted).min(span - 1);
            gate = gate.max(sorted[at]);
        }
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut open: Option<usize> = None;
        for frame in lo..=hi {
            if self.values[frame] >= gate {
                open.get_or_insert(frame);
            } else if let Some(start) = open.take() {
                runs.push((start, frame));
            }
        }
        if let Some(start) = open {
            runs.push((start, hi));
        }
        let gap = (PHRASE_GAP_SECS / self.hop_secs).round() as usize;
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in runs {
            match merged.last_mut() {
                Some(last) if start.saturating_sub(last.1) <= gap => last.1 = end,
                _ => merged.push((start, end)),
            }
        }
        let min = ((PHRASE_MIN_SECS / self.hop_secs).round() as usize).max(1);
        merged.retain(|(start, end)| end - start >= min);
        let merged = self.keep_sung_clusters(merged);
        // How long a phrase may be. The cap is normally readability, but a
        // legato vocal — one that never quite stops between words — offers
        // few silences to cut on, and a whole verse arriving as one twelve
        // word line is not karaoke. So the number of LINES the words need
        // tightens the cap: split until there are enough of them, at the
        // quietest interior moment each time, which is still a breath.
        let mut long = (PHRASE_MAX_SECS / self.hop_secs).round() as usize;
        if let Some(words) = words {
            let lines = words.div_ceil(WORDS_PER_LINE).max(1);
            let voiced: usize = merged.iter().map(|(start, end)| end - start).sum();
            let share = voiced / lines.max(1);
            let floor = (1.2 / self.hop_secs).round() as usize;
            long = long.min(share.max(floor));
        }
        let split = self.split_long_runs(merged, long, min);
        split
            .into_iter()
            .map(|(start, end)| VocalPhrase {
                start_secs: start as f64 * self.hop_secs,
                end_secs: end as f64 * self.hop_secs,
            })
            .collect()
    }

    /// Keep the stretch of the window the words were actually sung in.
    ///
    /// Whisper stamps a segment far wider than the singing inside it — the
    /// opening line of a track sung at 0:16 comes back stamped from 0:00 —
    /// and no separation is perfect, so an instrumental introduction leaves
    /// low but continuous energy in the "vocals" stem. On a dense production
    /// that leakage reaches most of the way to the singing's own level, so a
    /// loudness threshold alone cannot tell them apart.
    ///
    /// What DOES tell them apart is shape: a sung passage is a run of phrases
    /// close together, while leakage shows up as isolated blips seconds from
    /// anything else. So group the phrases into clusters separated by long
    /// silences, total the energy in each, and keep the clusters that carry
    /// the weight. One sung stretch outweighs a scattering of blips by an
    /// order of magnitude, whatever their peak level.
    fn keep_sung_clusters(&self, runs: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
        if runs.len() < 2 {
            return runs;
        }
        let split_gap = ((CLUSTER_GAP_SECS / self.hop_secs).round() as usize).max(1);
        let mut clusters: Vec<Vec<(usize, usize)>> = Vec::new();
        for run in runs {
            match clusters.last_mut() {
                Some(last)
                    if run.0.saturating_sub(last.last().map(|r| r.1).unwrap_or(0)) <= split_gap =>
                {
                    last.push(run)
                }
                _ => clusters.push(vec![run]),
            }
        }
        let energy = |cluster: &Vec<(usize, usize)>| -> f64 {
            cluster
                .iter()
                .map(|(start, end)| {
                    self.values[*start..*end].iter().map(|v| *v as f64).sum::<f64>()
                })
                .sum()
        };
        let strongest = clusters.iter().map(energy).fold(0.0f64, f64::max);
        if strongest <= 0.0 {
            return clusters.into_iter().flatten().collect();
        }
        clusters
            .into_iter()
            .filter(|cluster| energy(cluster) >= strongest * CLUSTER_KEEP)
            .flatten()
            .collect()
    }

    /// Cut runs longer than `long` at their quietest interior frame, which on
    /// a sung line is the breath between its halves.
    fn split_long_runs(
        &self,
        runs: Vec<(usize, usize)>,
        long: usize,
        min: usize,
    ) -> Vec<(usize, usize)> {
        let mut queue = runs;
        let mut out: Vec<(usize, usize)> = Vec::new();
        // Bounded: every pass either splits a run or emits it, and a split
        // always shortens both halves by at least `min`.
        for _ in 0..8 {
            let mut again = Vec::new();
            for (start, end) in queue.drain(..) {
                if end - start <= long {
                    out.push((start, end));
                    continue;
                }
                // Look only in the middle, so the cut is a breath and not the
                // quiet approach to the phrase's own first or last word.
                let inset = ((end - start) as f64 * 0.25) as usize;
                let (a, b) = (start + inset.max(min), end - inset.max(min));
                if b <= a {
                    out.push((start, end));
                    continue;
                }
                let mut at = a;
                let mut quietest = f32::INFINITY;
                for frame in a..b {
                    if self.values[frame] < quietest {
                        quietest = self.values[frame];
                        at = frame;
                    }
                }
                if at - start < min || end - at < min {
                    out.push((start, end));
                    continue;
                }
                again.push((start, at));
                again.push((at, end));
            }
            if again.is_empty() {
                break;
            }
            queue = again;
        }
        out.append(&mut queue);
        out.sort_by_key(|(start, _)| *start);
        out
    }
}

fn ends_a_clause(word: &str) -> bool {
    word.ends_with([',', '.', '!', '?', ';', ':', '—'])
}

/// Words a karaoke line should aim for. Four to eight is what a singer can
/// take in at a glance; six is the middle of that.
const WORDS_PER_LINE: usize = 6;

/// Reduce a segment's phrases to the number of LINES its words can fill,
/// merging across the smallest silences first.
///
/// Without this a segment of eight words over ten detected phrases becomes
/// ten lines of one word — perfectly timed and completely unreadable. The
/// silences the stem shows are all real; this picks the biggest ones, which
/// are the ones a singer would break on anyway. A merge that would make a
/// line too long to read is refused, so the phrase cap still holds.
pub fn merge_phrases_for_words(mut phrases: Vec<VocalPhrase>, words: usize) -> Vec<VocalPhrase> {
    let target = words.div_ceil(WORDS_PER_LINE).max(1);
    while phrases.len() > target {
        let mut best: Option<(usize, f64)> = None;
        for index in 1..phrases.len() {
            let joined = phrases[index].end_secs - phrases[index - 1].start_secs;
            if joined > PHRASE_MAX_SECS * 1.15 {
                continue;
            }
            let gap = phrases[index].start_secs - phrases[index - 1].end_secs;
            if best.is_none_or(|(_, smallest)| gap < smallest) {
                best = Some((index, gap));
            }
        }
        let Some((index, _)) = best else { break };
        phrases[index - 1].end_secs = phrases[index].end_secs;
        phrases.remove(index);
    }
    phrases
}

/// Spread one whisper segment's words across the phrases the stem shows,
/// proportionally to how long each phrase lasts, preferring to break where
/// the punctuation already breaks.
///
/// Proportional-by-character is crude next to word timestamps, but the
/// PHRASE boundaries are exact — they come from the audio — so every line
/// still starts and ends on a real sung edge. That is the trade karaoke
/// wants: honest phrase timing beats guessed word timing.
pub fn distribute_words(text: &str, phrases: &[VocalPhrase]) -> Vec<LyricLine> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() || phrases.is_empty() {
        return Vec::new();
    }
    if phrases.len() == 1 {
        return vec![LyricLine::new(
            phrases[0].start_secs,
            phrases[0].end_secs,
            text.trim(),
        )];
    }
    let total_secs: f64 = phrases.iter().map(VocalPhrase::secs).sum();
    let total_chars: usize = words.iter().map(|word| word.chars().count() + 1).sum();
    if total_secs <= 0.0 || total_chars == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(phrases.len());
    let mut index = 0usize;
    let mut consumed = 0usize;
    let mut elapsed = 0.0f64;
    for (slot, phrase) in phrases.iter().enumerate() {
        if index >= words.len() {
            break;
        }
        elapsed += phrase.secs();
        let last = slot + 1 == phrases.len();
        let target = if last {
            total_chars
        } else {
            (total_chars as f64 * elapsed / total_secs).round() as usize
        };
        let mut line = String::new();
        while index < words.len() {
            let word = words[index];
            if !line.is_empty() && !last && consumed >= target {
                break;
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            consumed += word.chars().count() + 1;
            index += 1;
            // Close on a comma or full stop once the share is nearly spent:
            // the singer's own punctuation is a better break than arithmetic.
            if !last && consumed * 100 >= target * 85 && ends_a_clause(word) {
                break;
            }
        }
        if !line.is_empty() {
            out.push(LyricLine::new(phrase.start_secs, phrase.end_secs, line));
        }
    }
    // Rounding can leave a word behind; it belongs to the last line sung.
    if index < words.len() {
        if let Some(line) = out.last_mut() {
            for word in &words[index..] {
                line.text.push(' ');
                line.text.push_str(word);
            }
        }
    }
    out
}

/// Turn whisper segments into lines: trim, drop blanks and drop anything the
/// decoder invented over a silent stretch of the stem.
pub fn segments_to_lines(
    segments: &[(i64, i64, String)],
    envelope: Option<&VocalEnvelope>,
    duration_secs: f64,
) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::with_capacity(segments.len());
    for (start_ms, end_ms, text) in segments {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        // Whisper's "[BLANK_AUDIO]"-style bracketed markers are not lyrics.
        if text.starts_with('[') && text.ends_with(']') {
            continue;
        }
        if text.starts_with('(') && text.ends_with(')') {
            continue;
        }
        let start = (*start_ms as f64) / 1000.0;
        let mut end = (*end_ms as f64) / 1000.0;
        if duration_secs > 0.0 {
            end = end.min(duration_secs);
        }
        if !(end > start) {
            continue;
        }
        if let Some(envelope) = envelope {
            // A line over silence in the VOCALS stem is a hallucination, and
            // the stem is exactly the signal that can prove it.
            if envelope.loud > 0.0 && envelope.peak_between(start, end) < envelope.loud * 0.05 {
                continue;
            }
        }
        // Whisper repeats itself when it loses the thread; a line identical to
        // the one before it and butted straight against it is that loop.
        if let Some(last) = out.last() {
            if last.text == text && start - last.end_secs < 0.05 {
                continue;
            }
        }
        // Cut the segment on the stem's own silences. Whisper hands back
        // paragraphs; the singer sang phrases, and the phrases are what the
        // screen can hold.
        let split = envelope
            .map(|envelope| {
                // Reach a little either side: whisper's boundaries are
                // approximate and a phrase must not be clipped by one.
                let words = text.split_whitespace().count();
                let phrases = envelope.phrases_for_words(start - 0.4, end + 0.25, Some(words));
                distribute_words(text, &merge_phrases_for_words(phrases, words))
            })
            .filter(|lines| !lines.is_empty())
            .unwrap_or_else(|| {
                vec![LyricLine::new(start, end, text)]
            });
        for mut line in split {
            // Consecutive segments reach into each other (the window is
            // widened either side, and whisper's boundaries are approximate),
            // so a phrase can be claimed twice. Trim the overlap rather than
            // dropping the line — dropping it takes its WORDS with it, and a
            // verse that silently loses its opening clause is the worst
            // failure this whole pipeline has.
            if let Some(last) = out.last() {
                if line.start_secs < last.end_secs {
                    line.start_secs = last.end_secs;
                }
            }
            if line.end_secs - line.start_secs < 0.15 {
                if let Some(last) = out.last_mut() {
                    last.text.push(' ');
                    last.text.push_str(&line.text);
                    last.end_secs = last.end_secs.max(line.end_secs);
                }
                continue;
            }
            out.push(line);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// word timing
// ---------------------------------------------------------------------------

/// How far a word start may be pulled to reach the nearest vocal onset.
const WORD_SNAP_SECS: f64 = 0.15;
/// No word gets less than this; it stops a snap from stacking two words on
/// one instant when the stem is ambiguous.
const WORD_MIN_SECS: f64 = 0.06;
/// How far before a line's stamp the search for its first word's attack
/// reaches. The stamp is whisper's; the attack is the stem's, and the stem
/// is the one that was there.
const ONSET_REACH_SECS: f64 = 0.05;

impl VocalEnvelope {
    /// Every syllable onset the stem shows between `from` and `to`.
    ///
    /// A sung word starts with an attack: the envelope climbs through a
    /// fraction of the phrase's own peak after having been below it. That is
    /// what this finds, walked back to the FOOT of each climb — the instant
    /// the ear calls the start of the note, not the instant the level clears
    /// a threshold, which is tens of milliseconds later and is exactly the
    /// bias that makes a karaoke fill feel like it is chasing the singer.
    pub fn onsets_between(&self, from: f64, to: f64) -> Vec<f64> {
        if self.values.is_empty() {
            return Vec::new();
        }
        let lo = self.frame_at(from.max(0.0));
        let hi = self.frame_at(to).max(lo);
        if hi <= lo + 2 {
            return Vec::new();
        }
        let peak = self.values[lo..=hi].iter().fold(0.0f32, |m, v| m.max(*v));
        if peak <= 0.0 {
            return Vec::new();
        }
        // Half-wave-rectified first difference: the classic onset detection
        // function. A LEVEL threshold cannot work inside a sung phrase — the
        // envelope hardly ever returns to silence between syllables, so a
        // "wait until it drops, then wait until it rises" rule fires once per
        // line and every word after the first falls back to a guess. What
        // separates syllables is the RISE, not the level.
        let width = hi - lo + 1;
        let mut flux = vec![0.0f32; width];
        for index in 1..width {
            flux[index] = (self.values[lo + index] - self.values[lo + index - 1]).max(0.0);
        }
        // Adaptive threshold: a rise counts when it stands out against the
        // quarter second around it, so a quiet passage is judged on its own
        // terms and a loud one is not carpeted with onsets.
        let neighbourhood = ((0.25 / self.hop_secs).round() as usize).max(2);
        let local = ((0.03 / self.hop_secs).round() as usize).max(1);
        let refractory = ((0.08 / self.hop_secs).round() as usize).max(1);
        let floor = peak * 0.02;
        let mut out: Vec<f64> = Vec::new();
        let mut last: Option<usize> = None;
        for index in 1..width {
            let value = flux[index];
            if value < floor {
                continue;
            }
            let from = index.saturating_sub(local);
            let to = (index + local + 1).min(width);
            if flux[from..to].iter().any(|other| *other > value) {
                continue;
            }
            let from = index.saturating_sub(neighbourhood);
            let to = (index + neighbourhood + 1).min(width);
            let mean = flux[from..to].iter().sum::<f32>() / (to - from) as f32;
            if value < mean * 1.45 + floor {
                continue;
            }
            // Back to the foot of the rise: the ear places the note where the
            // attack STARTS, not where it is steepest, and the difference is
            // tens of milliseconds — exactly the budget a karaoke fill has.
            let limit = ((0.06 / self.hop_secs).round() as usize).max(1);
            let mut at = index;
            while at > 1 && index - at < limit && flux[at - 1] > floor * 0.5 {
                at -= 1;
            }
            if last.is_some_and(|previous| at.saturating_sub(previous) < refractory) {
                continue;
            }
            out.push((lo + at) as f64 * self.hop_secs);
            last = Some(at);
        }
        out
    }

    /// Start times for the words of one line, read off the stem.
    ///
    /// Two things make this better than dividing the line's duration by its
    /// word count. First, time is measured in VOICED frames, not wall clock:
    /// the breath in the middle of a line, or the bar of instrumental between
    /// its halves, advances the clock not at all — which is exactly the
    /// "sweep ploughs through the gap" failure a constant rate has. Second,
    /// each resulting instant is pulled to the nearest energy RISE within
    /// 150 ms, which is where a sung word actually starts.
    ///
    /// It is not a forced aligner and does not pretend to be: it is the
    /// timing the isolated vocal can support, and it costs one pass over an
    /// envelope that is already in memory.
    pub fn word_times(&self, line: &LyricLine) -> (Vec<f64>, usize) {
        let words: Vec<&str> = line.text.split_whitespace().collect();
        if words.is_empty() || self.values.is_empty() {
            return (Vec::new(), 0);
        }
        if words.len() == 1 {
            return (vec![line.start_secs], 1);
        }
        let lo = self.frame_at(line.start_secs);
        let hi = self.frame_at(line.end_secs).max(lo);
        if hi <= lo + 2 {
            return (Vec::new(), 0);
        }
        let peak = self.values[lo..=hi].iter().fold(0.0f32, |m, v| m.max(*v));
        if peak <= 0.0 {
            return (Vec::new(), 0);
        }
        let gate = (peak * 0.18).max(self.loud * 0.08);
        // Voiced frames only. A line whose gate leaves nothing is not one the
        // stem can time, so fall back to wall clock for it.
        let mut voiced: Vec<usize> = Vec::with_capacity(hi - lo + 1);
        for frame in lo..=hi {
            if self.values[frame] >= gate {
                voiced.push(frame);
            }
        }
        let clock: &[usize] = if voiced.len() >= words.len() * 2 {
            &voiced
        } else {
            return (Vec::new(), 0);
        };
        // A first guess: each word gets a share of the line's VOICED time
        // proportional to its length. Voiced, not wall clock — the breath in
        // the middle of a line must cost a word nothing, which is precisely
        // what a constant-rate sweep gets wrong.
        let weights: Vec<usize> = words.iter().map(|w| w.chars().count() + 1).collect();
        let total: usize = weights.iter().sum();
        let mut estimate = Vec::with_capacity(words.len());
        let mut before = 0usize;
        for weight in &weights {
            let at = (clock.len() as f64 * before as f64 / total as f64) as usize;
            estimate.push(clock[at.min(clock.len() - 1)] as f64 * self.hop_secs);
            before += weight;
        }
        estimate[0] = line.start_secs;

        // Then put every word ON an attack the stem actually shows, keeping
        // the order and leaving enough onsets for the words still to come.
        // A guess landing between two syllables is what reads as "off"; an
        // attack is what the ear is listening for.
        let onsets = self.onsets_between(line.start_secs - ONSET_REACH_SECS, line.end_secs);
        let mut out = Vec::with_capacity(words.len());
        let mut cursor = 0usize;
        let mut previous = f64::NEG_INFINITY;
        // How many words the stem actually PLACED, as opposed to guessed.
        // The display uses this to decide whether it has earned the right to
        // hop word by word or should fall back to a smooth sweep: a fill that
        // hops confidently onto the wrong word is worse than one that never
        // claimed to know.
        let mut placed = 0usize;
        for index in 0..words.len() {
            let want = estimate[index];
            let remaining = words.len() - index - 1;
            let last = onsets.len().saturating_sub(remaining);
            let mut chosen = None;
            let mut best = f64::INFINITY;
            for slot in cursor..last {
                let at = onsets[slot];
                if at <= previous + WORD_MIN_SECS - 1e-9 || at > line.end_secs {
                    continue;
                }
                let error = (at - want).abs();
                if error > WORD_SNAP_SECS {
                    // Past the window and getting worse: nothing later helps.
                    if at > want {
                        break;
                    }
                    continue;
                }
                if error < best {
                    best = error;
                    chosen = Some(slot);
                }
            }
            let at = match chosen {
                Some(slot) => {
                    cursor = slot + 1;
                    placed += 1;
                    onsets[slot]
                }
                None => want.max(previous + WORD_MIN_SECS),
            };
            let at = at.min(line.end_secs - WORD_MIN_SECS).max(if index == 0 {
                line.start_secs.min(at)
            } else {
                previous + WORD_MIN_SECS
            });
            out.push(at);
            previous = at;
        }
        (out, placed)
    }
}

/// Share of a line's words that must land on an attack the stem really shows
/// before the display is allowed to hop through them.
const WORD_CONFIDENCE: f64 = 0.8;

/// Fill in every line's word times — but only where the stem earned them.
///
/// Graceful degradation beats confident wrongness. A fill that hops crisply
/// onto the WRONG word is worse than a smooth sweep, because the sweep never
/// claimed to know where the words were and the hop did. So a line keeps its
/// word times only if the stem placed most of them on real attacks, in order,
/// with no word swallowing the line; anything short of that stores no times
/// at all and renders as the honest linear sweep. Both look deliberate, and
/// the display never claims more precision than the audio supports.
pub fn time_words(lines: &mut [LyricLine], envelope: &VocalEnvelope) -> usize {
    time_words_with(lines, envelope, word_hops_enabled())
}

/// Whether the display hops word-by-word (the aligned best-effort word
/// mapper) or sweeps each line smoothly. A live UI toggle — ON by default
/// now that the alignment lane landed; per-line confidence still downgrades
/// doubtful lines to the sweep, so a hop never claims precision the data
/// lacks. `VJ_KARAOKE_WORD_HOPS=0` forces the old line-sweep-only start.
// 0 = uninitialized, 1 = off, 2 = on. Atomic initialization avoids the
// blocking OnceLock path when the UI toggle races the lyrics worker on wasm.
static WORD_HOPS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn word_hops_enabled() -> bool {
    let ordering = std::sync::atomic::Ordering::Acquire;
    let mut state = WORD_HOPS.load(ordering);
    if state == 0 {
        let on = std::env::var("VJ_KARAOKE_WORD_HOPS")
            .map(|value| matches!(value.trim(), "1" | "on" | "true" | "yes"))
            .unwrap_or(true);
        let desired = if on { 2 } else { 1 };
        let _ = WORD_HOPS.compare_exchange(
            0,
            desired,
            std::sync::atomic::Ordering::AcqRel,
            ordering,
        );
        state = WORD_HOPS.load(ordering);
    }
    state == 2
}

/// The UI checkbox's write half: flips hop mode live; the caller rebuilds
/// the karaoke schedules so already-loaded decks re-time immediately.
pub fn set_word_hops(on: bool) {
    WORD_HOPS.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Release);
}

/// The same, told explicitly whether to hop — the form tests and the audit
/// use, so neither depends on the process environment.
pub fn time_words_with(
    lines: &mut [LyricLine],
    envelope: &VocalEnvelope,
    hops: bool,
) -> usize {
    let mut timed = 0;
    for line in lines.iter_mut() {
        line.words = Vec::new();
        if !hops {
            continue;
        }
        let (times, placed) = envelope.word_times(line);
        if word_times_are_trustworthy(line, &times, placed) {
            // A line begins where its first word begins.
            line.start_secs = line.start_secs.min(times[0]).max(0.0);
            line.words = times;
            line.confident = true;
            timed += 1;
        }
    }
    timed
}

/// Whether a line's word times are good enough to hop on.
pub fn word_times_are_trustworthy(line: &LyricLine, times: &[f64], placed: usize) -> bool {
    let words = line.text.split_whitespace().count();
    if times.len() != words || times.len() < 2 {
        return false;
    }
    // Floored, so a short line is allowed the same one miss a long one is:
    // ceiling arithmetic on four words silently demands all four.
    if placed < (times.len() as f64 * WORD_CONFIDENCE).floor() as usize {
        return false;
    }
    let span = line.end_secs - line.start_secs;
    if span <= 0.0 {
        return false;
    }
    for index in 0..times.len() {
        // The first word may sit a hair before the line's own stamp: the
        // onset search reaches back past it, and the attack is the truth.
        if !times[index].is_finite()
            || times[index] < line.start_secs - ONSET_REACH_SECS
            || times[index] > line.end_secs
        {
            return false;
        }
        if index > 0 && times[index] - times[index - 1] < WORD_MIN_SECS - 1e-9 {
            return false;
        }
    }
    // One word eating most of a line is an assignment that went wrong, not a
    // singer holding a note — a real sustain is one word among several.
    if times.len() >= 4 {
        for index in 0..times.len() {
            let ends = times.get(index + 1).copied().unwrap_or(line.end_secs);
            if ends - times[index] > span * 0.6 {
                return false;
            }
        }
    }
    true
}

/// Snap every line's start to the vocal onset the stem shows near it.
/// Returns what it moved, so the bake can report it honestly.
pub fn refine_onsets(lines: &mut [LyricLine], envelope: &VocalEnvelope) -> OnsetStats {
    let mut stats = OnsetStats::default();
    let mut total = 0.0f64;
    let mut previous_end = 0.0f64;
    for index in 0..lines.len() {
        let (start, end) = (lines[index].start_secs, lines[index].end_secs);
        // Never snap past the middle of the line, and never back over the
        // previous line — a start that jumps a line boundary is worse than a
        // late one.
        let ceiling = (start + ONSET_SEARCH_SECS).min(start + (end - start) * 0.5);
        if let Some(onset) = envelope.onset_near(start, previous_end, ceiling) {
            let onset = onset.clamp(previous_end, ceiling);
            let moved = (onset - start).abs();
            if moved >= envelope.hop_secs {
                lines[index].start_secs = onset;
                stats.snapped += 1;
                total += moved;
                stats.max_ms = stats.max_ms.max(moved * 1000.0);
            }
        }
        previous_end = lines[index].end_secs;
    }
    if stats.snapped > 0 {
        stats.mean_ms = total * 1000.0 / stats.snapped as f64;
    }
    stats
}

// ---------------------------------------------------------------------------
// display timing
// ---------------------------------------------------------------------------

/// How far ahead of the voice the display runs, and how the beat grid shapes
/// it. Built from the track's own grid so the subtitles land on the music.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KaraokeTiming {
    /// Seconds the current line is on screen before its first word.
    pub lead_in_secs: f64,
    /// Seconds the current line stays up after its last word.
    pub hold_secs: f64,
    /// How early the dim NEXT row starts showing an upcoming line during a
    /// gap (an instrumental break).
    pub preview_secs: f64,
    /// Grid the appear/disappear moments are quantized onto; zero = none.
    pub beat_secs: f64,
    pub first_beat_secs: f64,
}

/// Lead-in target: two beats, but never so short that there is no time to
/// read and never so long that the line floats free of the phrase.
pub const LEAD_IN_MIN_SECS: f64 = 0.8;
pub const LEAD_IN_MAX_SECS: f64 = 2.0;
const HOLD_SECS: f64 = 0.5;

impl Default for KaraokeTiming {
    fn default() -> Self {
        KaraokeTiming {
            lead_in_secs: 1.2,
            hold_secs: HOLD_SECS,
            preview_secs: 6.0,
            beat_secs: 0.0,
            first_beat_secs: 0.0,
        }
    }
}

impl KaraokeTiming {
    pub fn from_grid(grid: Option<&TrackGrid>) -> KaraokeTiming {
        let mut out = KaraokeTiming::default();
        let Some(grid) = grid.filter(|grid| grid.has_grid()) else {
            return out;
        };
        out.beat_secs = grid.beat_secs;
        out.first_beat_secs = grid.first_beat_secs;
        // Two beats of lead at any tempo the clamp allows; a bar at slow
        // tempos would float, half a bar at fast ones would not be readable.
        out.lead_in_secs = (grid.beat_secs * 2.0).clamp(LEAD_IN_MIN_SECS, LEAD_IN_MAX_SECS);
        out.preview_secs = (grid.beat_secs * 8.0).clamp(4.0, 10.0);
        out
    }

    /// The beat at or before `secs`. Without a grid this is the identity, so
    /// every rule below reads the same with and without one.
    pub fn beat_floor(&self, secs: f64) -> f64 {
        if self.beat_secs <= 1e-4 {
            return secs;
        }
        let beats = ((secs - self.first_beat_secs) / self.beat_secs).floor();
        self.first_beat_secs + beats * self.beat_secs
    }

    /// The least time a line may hold the screen. A beat, bounded: enough
    /// that a line is always seen, short enough that a dense verse is not
    /// pushed further and further behind itself.
    pub fn floor_secs(&self) -> f64 {
        if self.beat_secs <= 1e-4 {
            return 0.35;
        }
        self.beat_secs.clamp(0.25, 1.0)
    }

    /// The beat at or after `secs`.
    pub fn beat_ceil(&self, secs: f64) -> f64 {
        if self.beat_secs <= 1e-4 {
            return secs;
        }
        let beats = ((secs - self.first_beat_secs) / self.beat_secs).ceil();
        self.first_beat_secs + beats * self.beat_secs
    }
}

/// What the overlay shows at one instant.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KaraokeFrame {
    /// The line being sung (bright row), if any.
    pub current: Option<usize>,
    /// The line to sing next (dim row), if it is worth showing yet.
    pub next: Option<usize>,
    /// 0..1 across the current line's sung duration — the colour sweep.
    /// Zero through the whole lead-in, so nothing moves before the voice does.
    pub progress: f32,
}

/// Lines plus the beat-quantized moments they appear and leave on. Built once
/// per (track, grid) and looked up by binary search every frame.
#[derive(Clone, Debug)]
pub struct KaraokeSchedule {
    pub lines: Vec<LyricLine>,
    /// `appear[i]` — when line `i` becomes the bright row.
    pub appear: Vec<f64>,
    /// `leave[i]` — when line `i` gives the bright row up, if no later line
    /// has claimed it first.
    pub leave: Vec<f64>,
    pub timing: KaraokeTiming,
}

impl KaraokeSchedule {
    pub fn build(lines: Vec<LyricLine>, timing: KaraokeTiming) -> KaraokeSchedule {
        let mut appear: Vec<f64> = Vec::with_capacity(lines.len());
        let mut leave = Vec::with_capacity(lines.len());
        let mut previous_end = f64::NEG_INFINITY;
        let floor = timing.floor_secs();
        for line in &lines {
            // The line goes up on the BEAT at or before its lead-in target,
            // so the displayed lead is never shorter than the target — and
            // never before the previous line has been sung out, because a
            // singer mid-phrase must not have the words taken away.
            let wanted = timing.beat_floor(line.start_secs - timing.lead_in_secs);
            let mut at = wanted.max(previous_end).min(line.start_secs).max(0.0);
            // …but never at or before the line before it. Three separate
            // clamps above can collapse two lines onto one instant — two
            // lines both pinned to zero at the top of a track, or a start
            // pulled back by the onset snap under the previous line's end —
            // and the lookup takes the LAST line whose moment has passed, so
            // a collapsed pair means the earlier one is never shown at all.
            // Every line gets a beat of the screen to itself; a line pushed
            // past its own first word is late, which is survivable, where a
            // line nobody ever sees is not.
            if let Some(previous) = appear.last() {
                at = at.max(previous + floor);
            }
            at = at.min((line.end_secs - 0.05).max(0.0));
            appear.push(at);
            // …and comes down on the beat at or after its last word plus a
            // hold, so the final syllable is still readable.
            leave.push(
                timing
                    .beat_ceil(line.end_secs + timing.hold_secs)
                    .max(at + floor),
            );
            previous_end = line.end_secs;
        }
        KaraokeSchedule { lines, appear, leave, timing }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The line whose text first reaches the screen for line `index` — which
    /// is when the line BEFORE it went up, because the dim row is always one
    /// line ahead. This is the number the "never surprised by a late line"
    /// gate is measured against.
    pub fn on_screen_at(&self, index: usize) -> f64 {
        match index.checked_sub(1) {
            Some(previous) => self.appear[previous].min(self.appear[index]),
            None => self.appear[index],
        }
    }

    pub fn at(&self, secs: f64) -> KaraokeFrame {
        if self.lines.is_empty() {
            return KaraokeFrame::default();
        }
        // Last line whose appear moment has passed.
        let after = self.appear.partition_point(|at| *at <= secs);
        let Some(index) = after.checked_sub(1) else {
            // Before the first line: preview it once it is close enough.
            let first = 0;
            let waiting = self.appear[first] - secs <= self.timing.preview_secs;
            return KaraokeFrame {
                current: None,
                next: waiting.then_some(first),
                progress: 0.0,
            };
        };
        if secs >= self.leave[index] {
            // A gap — an instrumental. Nothing is being sung; show what is
            // coming once it is near.
            let upcoming = index + 1;
            let waiting = upcoming < self.lines.len()
                && self.appear[upcoming] - secs <= self.timing.preview_secs;
            return KaraokeFrame {
                current: None,
                next: waiting.then_some(upcoming),
                progress: 0.0,
            };
        }
        let line = &self.lines[index];
        let progress = sung_fraction(line, secs);
        KaraokeFrame {
            current: Some(index),
            next: (index + 1 < self.lines.len()).then_some(index + 1),
            progress,
        }
    }

    pub fn text(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(|line| line.text.as_str())
    }
}

/// Which deck the program should be quoting. The crossfader decides between
/// two playing decks; a lone playing deck always wins, because a performer
/// cueing the other side is not what the room is hearing.
pub fn live_deck(crossfader: f32, playing_a: bool, playing_b: bool) -> DeckId {
    match (playing_a, playing_b) {
        (true, false) => DeckId::A,
        (false, true) => DeckId::B,
        _ if crossfader <= 0.5 => DeckId::A,
        _ => DeckId::B,
    }
}

// ---------------------------------------------------------------------------
// dispatch bookkeeping
// ---------------------------------------------------------------------------

/// One bake per track, ever. The stems worker reports a track's cache
/// coverage twice — once when it opens it and once when the run finishes —
/// and a deck can be reloaded any number of times; this turns all of that
/// into at most one read job and at most one bake job per digest.
#[derive(Default)]
pub struct LyricsDispatch {
    seen: HashSet<(String, bool)>,
}

impl LyricsDispatch {
    pub fn should_dispatch(&mut self, digest: &str, bake: bool) -> bool {
        // A bake subsumes a read, so once a bake has gone out no read job for
        // the same track is worth sending.
        if !bake && self.seen.contains(&(digest.to_string(), true)) {
            return false;
        }
        self.seen.insert((digest.to_string(), bake))
    }

    pub fn forget(&mut self, digest: &str) {
        self.seen.remove(&(digest.to_string(), false));
        self.seen.remove(&(digest.to_string(), true));
    }
}

// ---------------------------------------------------------------------------
// the worker
// ---------------------------------------------------------------------------

pub struct LyricsJob {
    pub deck: DeckId,
    pub gen: u64,
    /// The stem cache's key — the decoded track's content digest.
    pub digest: String,
    /// Frames the stems model works in for this track, so the stem cache can
    /// be opened with the header it was written under.
    pub model_frames: u64,
    pub duration_secs: f64,
    /// False for a cache probe on track load, true once separation is
    /// complete and the vocals stem is actually there to transcribe.
    pub bake: bool,
}

pub enum LyricsMsg {
    Status { deck: DeckId, gen: u64, text: String },
    Ready { deck: DeckId, gen: u64, digest: String, lyrics: Arc<TrackLyrics> },
    /// A background bake let go of the worker, however it went. `Ready`
    /// already files a transcript by digest whatever generation asked for
    /// it, so this says nothing about the words — it only tells the host
    /// the background lane is free again.
    PrefetchDone { digest: String },
}

/// One transcription thread. The whisper model is expensive to load and
/// thread-affine (Metal), so it is created here and never leaves.
pub struct LyricsPool {
    tx: Sender<LyricsJob>,
    jobs: Option<Receiver<LyricsJob>>,
    /// The BACKGROUND inbox, kept apart so a queued track's transcript can
    /// never sit in front of the deck the operator just cued.
    prefetch_tx: Sender<LyricsJob>,
    prefetch_jobs: Option<Receiver<LyricsJob>>,
    out: Sender<LyricsMsg>,
    rx: Receiver<LyricsMsg>,
}

impl Default for LyricsPool {
    fn default() -> Self {
        LyricsPool::new()
    }
}

impl LyricsPool {
    pub fn new() -> LyricsPool {
        let (tx, jobs) = channel::<LyricsJob>();
        let (prefetch_tx, prefetch_jobs) = channel::<LyricsJob>();
        let (out, rx) = channel::<LyricsMsg>();
        LyricsPool {
            tx,
            jobs: Some(jobs),
            prefetch_tx,
            prefetch_jobs: Some(prefetch_jobs),
            out,
            rx,
        }
    }

    pub fn start(&mut self, spawner: ThreadSpawner) {
        let Some(jobs) = self.jobs.take() else { return };
        let Some(prefetch_jobs) = self.prefetch_jobs.take() else { return };
        let out = self.out.clone();
        let options = ThreadOptions { name: Some("vj-lyrics".into()), ..Default::default() };
        match spawner.spawn_worker(options, move || {
                let mut backend: Option<Transcriber> = None;
                let mut backend_failed = false;
                loop {
                    // A deck's words come first; the background inbox is
                    // read only once this one has gone quiet.
                    let job = match jobs.recv_timeout(Duration::from_millis(DECK_INBOX_WAIT_MS)) {
                        Ok(job) => job,
                        Err(RecvTimeoutError::Timeout) => match prefetch_jobs.try_recv() {
                            Ok(job) => job,
                            Err(TryRecvError::Empty) => continue,
                            Err(TryRecvError::Disconnected) => break,
                        },
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    let prefetching = job.gen == crate::stems::PREFETCH_GEN;
                    let digest = job.digest.clone();
                    run_job(
                        job,
                        &cache_dir(),
                        &crate::stems::cache_dir(),
                        &mut backend,
                        &mut backend_failed,
                        &out,
                    );
                    if prefetching {
                        let _ = out.send(LyricsMsg::PrefetchDone { digest });
                    }
                }
            }) {
            Ok(handle) => handle.detach(),
            Err(error) => makepad_widgets::log!("vj lyrics worker unavailable: {error}"),
        }
    }

    /// Bake a queued track's transcript while nothing is asking. Carries
    /// [`crate::stems::PREFETCH_GEN`], so its status lines reach no deck —
    /// but `Ready` still files the words by digest, which is the point.
    pub fn submit_prefetch(&self, job: LyricsJob) {
        let _ = self.prefetch_tx.send(job);
    }

    pub fn submit(&self, job: LyricsJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<LyricsMsg> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(message) => out.push(message),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

fn status(out: &Sender<LyricsMsg>, job: &LyricsJob, text: impl Into<String>) {
    let _ = out.send(LyricsMsg::Status { deck: job.deck, gen: job.gen, text: text.into() });
}

/// `dir` is the transcript cache and `stem_root` the separated-span cache.
/// Both are passed rather than read from the environment here so the probe
/// below — the one branch with no model in it — is testable on its own.
fn run_job(
    job: LyricsJob,
    dir: &std::path::Path,
    stem_root: &std::path::Path,
    backend: &mut Option<Transcriber>,
    backend_failed: &mut bool,
    out: &Sender<LyricsMsg>,
) {
    if let Some(lyrics) = load_cached(dir, &job.digest) {
        let lines = lyrics.lines.len();
        let _ = out.send(LyricsMsg::Ready {
            deck: job.deck,
            gen: job.gen,
            digest: job.digest.clone(),
            lyrics: Arc::new(lyrics),
        });
        status(out, &job, format!("lyrics: {lines} lines (cached)"));
        return;
    }
    if !job.bake {
        // A probe that found nothing still has to SAY it found nothing.
        //
        // Returning in silence leaves this deck's lyrics status empty, and
        // empty is precisely what the reader paints as "waiting for
        // separation" — so the one path that knows the answer became the one
        // path that never spoke, and a track whose stems finished long ago
        // sat under a message about separation for the rest of the session.
        // Which of the two things is actually missing is a question the span
        // cache answers.
        status(
            out,
            &job,
            match crate::stems::cache_is_complete(stem_root, &job.digest, job.model_frames) {
                true => "lyrics: no transcript",
                false => "lyrics: waiting for separation",
            },
        );
        return;
    }
    if *backend_failed {
        // Not latched across a model install: the INSTALL MODELS flow can
        // put a whisper checkpoint on disk mid-session, and the next bake
        // must try again instead of sitting on an old failure.
        if whisper_model_path().is_none() {
            status(out, &job, "lyrics: no transcriber");
            return;
        }
        *backend_failed = false;
    }

    // The vocals stem, read straight out of the separation cache.
    let started = crate::clock::Instant::now();
    status(&out.clone(), &job, "lyrics: reading vocals…");
    let (mono, rate) = match read_vocals_mono(&job) {
        Ok(pair) => pair,
        Err(error) => {
            status(out, &job, format!("lyrics: {error}"));
            return;
        }
    };
    let envelope = VocalEnvelope::build(&mono, rate);
    let analysis =
        crate::lyrics_align::analyze_vocals(&mono, rate, crate::lyrics_align::OnsetPreset::Snapping);
    let samples = crate::stems::resample(&mono, rate, WHISPER_RATE);
    drop(mono);

    // The Apple fallback yields to whisper the moment a checkpoint appears
    // (the INSTALL MODELS flow drops one in mid-session): whisper's measured
    // word path is strictly better than the dictation-tuned fallback.
    if let Some(Transcriber::System) = backend.as_ref() {
        if whisper_model_path().is_some() {
            *backend = None;
        }
    }
    if backend.is_none() {
        match Transcriber::open() {
            Ok(loaded) => *backend = Some(loaded),
            Err(error) => {
                *backend_failed = true;
                status(out, &job, format!("lyrics: {error}"));
                return;
            }
        }
    }
    let Some(backend) = backend.as_mut() else { return };
    status(out, &job, "lyrics: transcribing…");
    let language = std::env::var("VJ_LYRICS_LANG").unwrap_or_else(|_| "en".to_string());
    // Whisper gets the measured path: cross-attention DTW word times,
    // teacher-forced re-alignment per segment, onset snap, sanity layer
    // (`lyrics_align`). The Apple backend has no attention to read and keeps
    // the envelope pipeline.
    let (onset, timed, lines) = match backend.bake_aligned(
        &samples,
        &analysis,
        job.duration_secs,
        &language,
    ) {
        Some((segments, timed_lines)) => {
            let hops = word_hops_enabled();
            let mut snapped = 0usize;
            let mut total_ms = 0.0f64;
            let mut max_ms = 0.0f64;
            for segment in &segments {
                for word in &segment.words {
                    if let Some(delta) = word.snap {
                        snapped += 1;
                        total_ms += delta.abs() * 1000.0;
                        max_ms = max_ms.max(delta.abs() * 1000.0);
                    }
                }
            }
            let onset = OnsetStats {
                snapped,
                mean_ms: if snapped > 0 { total_ms / snapped as f64 } else { 0.0 },
                max_ms,
            };
            let timed = timed_lines.iter().filter(|line| line.confident).count();
            let lines: Vec<LyricLine> = timed_lines
                .into_iter()
                .map(|line| LyricLine {
                    start_secs: line.start,
                    end_secs: line.end,
                    text: line.text,
                    words: line.words,
                    confident: line.confident && hops,
                })
                .collect();
            (onset, timed, lines)
        }
        None => {
            let segments = match backend.transcribe(&samples, &language) {
                Ok(segments) => segments,
                Err(error) => {
                    status(out, &job, format!("lyrics: {error}"));
                    return;
                }
            };
            let mut lines = segments_to_lines(&segments, Some(&envelope), job.duration_secs);
            let onset = refine_onsets(&mut lines, &envelope);
            let timed = time_words(&mut lines, &envelope);
            (onset, timed, lines)
        }
    };
    let lyrics = TrackLyrics {
        backend: backend.name().to_string(),
        model: backend.model().to_string(),
        language,
        duration_secs: job.duration_secs,
        onset,
        lines,
    };
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "lyrics: {} lines in {:.1} s ({} onset-snapped, mean {:.0} ms, max {:.0} ms; \
         {timed} word-timed) — {}",
        lyrics.lines.len(),
        elapsed,
        onset.snapped,
        onset.mean_ms,
        onset.max_ms,
        backend.name(),
    );
    store_cached(&dir, &job.digest, &lyrics);
    let count = lyrics.lines.len();
    let _ = out.send(LyricsMsg::Ready {
        deck: job.deck,
        gen: job.gen,
        digest: job.digest.clone(),
        lyrics: Arc::new(lyrics),
    });
    status(out, &job, format!("lyrics: {count} lines in {elapsed:.0}s"));
}

/// Read the separated vocals back span by span and mix to mono. Reading the
/// whole `StemSet` at once would hold four full-length stereo f32 stems —
/// a third of a gigabyte for a six-minute track — when only one mono lane is
/// wanted.
fn read_vocals_mono(job: &LyricsJob) -> Result<(Vec<f32>, f64), String> {
    let header = CacheHeader::for_track(job.model_frames);
    let rate = header.sample_rate as f64;
    let mut cache = StemCache::open(crate::stems::cache_dir(), &job.digest, header)
        .map_err(|error| error.to_string())?;
    if !cache.is_complete() {
        return Err("separation incomplete".to_string());
    }
    let mut mono = Vec::with_capacity(job.model_frames as usize);
    for span in 0..cache.span_count() {
        let set = cache
            .read_span(span)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("span {span} missing"))?;
        let vocals = &set[Stem::Vocals as usize];
        for frame in 0..vocals.frames() {
            mono.push((vocals.left[frame] + vocals.right[frame]) * 0.5);
        }
    }
    if mono.is_empty() {
        return Err("vocals stem is empty".to_string());
    }
    Ok((mono, rate))
}

// ---------------------------------------------------------------------------
// the transcriber
// ---------------------------------------------------------------------------

/// Which of `makepad-ai-speech`'s two backends is doing the work.
///
/// Whisper is preferred and is what ships: `ggml-large-v3-turbo` transcribes
/// SUNG speech, returns segment timestamps on a 20 ms grid, and takes a whole
/// track in one call. Apple's native recognizer is dictation-tuned — it is
/// the fallback only when the checkpoint is not on the machine, and its
/// timings are coarser.
enum Transcriber {
    Whisper {
        model: Box<makepad_ai_speech::whisper::WhisperModel>,
        state: makepad_ai_speech::whisper::WhisperState,
        path: String,
    },
    /// The OS recognizer (makepad-system-speech): PCM in, coarse timings.
    System,
}

impl Transcriber {
    fn open() -> Result<Transcriber, String> {
        if let Some(path) = whisper_model_path() {
            let text = path.to_string_lossy().to_string();
            let model = makepad_ai_speech::whisper::WhisperModel::load_file(&text)
                .map_err(|error| format!("whisper model: {error}"))?;
            let state = makepad_ai_speech::whisper::WhisperState::new(&model);
            return Ok(Transcriber::Whisper {
                model: Box::new(model),
                state,
                path: text,
            });
        }
        if !makepad_system_speech::stt::capabilities().pcm_input {
            return Err("no whisper checkpoint and no PCM-input system recognizer".to_string());
        }
        let _ = makepad_system_speech::stt::prepare("en");
        Ok(Transcriber::System)
    }

    fn name(&self) -> &'static str {
        match self {
            Transcriber::Whisper { .. } => "whisper",
            Transcriber::System => makepad_system_speech::stt::engine_name(),
        }
    }

    fn model(&self) -> &str {
        match self {
            Transcriber::Whisper { path, .. } => path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(WHISPER_MODEL_FILE),
            Transcriber::System => "system",
        }
    }

    /// The measured word path, whisper only: transcription with
    /// cross-attention capture, then the full `lyrics_align` refinement
    /// (teacher-forced windows, onset snap, sanity layer). `None` on the
    /// Apple backend, which has no attention to read — the caller falls
    /// back to the envelope pipeline.
    fn bake_aligned(
        &mut self,
        samples_16k: &[f32],
        analysis: &crate::lyrics_align::VocalAnalysis,
        duration_secs: f64,
        language: &str,
    ) -> Option<(
        Vec<crate::lyrics_align::SegmentWords>,
        Vec<crate::lyrics_align::TimedLine>,
    )> {
        match self {
            Transcriber::Whisper { model, state, .. } => {
                let mut params = makepad_ai_speech::whisper::WhisperParams::default();
                params.language = language.to_string();
                params.no_timestamps = false;
                params.single_segment = false;
                params.temperature = 0.0;
                params.suppress_blank = true;
                let aligned = state.transcribe_aligned(model, samples_16k, &params);
                let config = crate::lyrics_align::PipelineConfig {
                    language: language.to_string(),
                    force: true,
                    snap: true,
                };
                Some(crate::lyrics_align::refine(
                    state,
                    model,
                    samples_16k,
                    aligned,
                    analysis,
                    duration_secs,
                    &config,
                ))
            }
            Transcriber::System => None,
        }
    }

    fn transcribe(
        &mut self,
        samples: &[f32],
        language: &str,
    ) -> Result<Vec<(i64, i64, String)>, String> {
        let mut params = makepad_ai_speech::whisper::WhisperParams::default();
        params.language = language.to_string();
        params.no_timestamps = false;
        params.single_segment = false;
        params.temperature = 0.0;
        params.suppress_blank = true;
        let segments = match self {
            Transcriber::Whisper { model, state, .. } => state.transcribe(model, samples, &params),
            Transcriber::System => {
                let options = makepad_system_speech::SttOptions {
                    language: language.to_string(),
                    timestamps: true,
                    ..Default::default()
                };
                let transcript = makepad_system_speech::stt::transcribe(samples, &options)
                    .map_err(|error| error.to_string())?;
                return Ok(transcript
                    .segments
                    .into_iter()
                    .map(|segment| (segment.start_ms, segment.end_ms, segment.text))
                    .collect());
            }
        };
        Ok(segments
            .into_iter()
            .map(|segment| (segment.start_ms, segment.end_ms, segment.text))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_widgets::lyric_reader::WORD_HOP_SECS;

    fn line(start: f64, end: f64, text: &str) -> LyricLine {
        LyricLine::new(start, end, text)
    }

    fn grid(bpm: f64) -> TrackGrid {
        TrackGrid {
            bpm,
            beat_secs: 60.0 / bpm,
            first_beat_secs: 0.0,
            downbeat_phase: 0,
            confidence: 1.0,
        }
    }

    // ---- cache -----------------------------------------------------------

    #[test]
    fn the_cache_round_trips_every_field() {
        let lyrics = TrackLyrics {
            backend: "whisper".into(),
            model: "ggml-large-v3-turbo.bin".into(),
            language: "en".into(),
            duration_secs: 231.44,
            onset: OnsetStats { snapped: 12, mean_ms: 83.5, max_ms: 412.0 },
            lines: vec![
                line(12.34, 15.02, "You can dance"),
                line(15.10, 18.00, "you can jive"),
                // Quotes and backslashes have to survive the writer.
                line(20.0, 22.0, "having the \"time\" of your \\ life"),
            ],
        };
        let json = lyrics.to_json("abc123");
        let back = TrackLyrics::from_json(json.as_bytes(), "abc123").expect("parse");
        assert_eq!(back, lyrics);
        // Another track's digest is another track's cache.
        assert!(TrackLyrics::from_json(json.as_bytes(), "def456").is_none());
    }

    #[test]
    fn a_cache_from_another_version_is_not_read() {
        let json = format!(
            "{{\"format\":\"{CACHE_FORMAT}\",\"version\":{},\"digest\":\"aa\",\
             \"backend\":\"whisper\",\"model\":\"m\",\"language\":\"en\",\
             \"duration_secs\":1.0,\"lines\":[]}}",
            CACHE_VERSION + 1
        );
        assert!(TrackLyrics::from_json(json.as_bytes(), "aa").is_none());
        // …and a file that is not ours at all.
        assert!(TrackLyrics::from_json(b"{\"format\":\"other\"}", "aa").is_none());
        assert!(TrackLyrics::from_json(b"not json", "aa").is_none());
    }

    #[test]
    fn cache_files_are_written_once_and_reused() {
        let dir = std::env::temp_dir().join(format!(
            "makepad-vj-lyrics-{}-{}",
            std::process::id(),
            "roundtrip"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let lyrics = TrackLyrics {
            backend: "whisper".into(),
            model: "m".into(),
            language: "en".into(),
            duration_secs: 10.0,
            onset: OnsetStats::default(),
            lines: vec![line(1.0, 2.0, "hello")],
        };
        assert!(load_cached(&dir, "feed").is_none());
        store_cached(&dir, "feed", &lyrics);
        assert_eq!(load_cached(&dir, "feed"), Some(lyrics));
        // The temp file must not be left behind next to the real one.
        assert!(!cache_path(&dir, "feed").with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- dispatch --------------------------------------------------------

    #[test]
    fn a_digest_is_dispatched_once_per_reason() {
        let mut dispatch = LyricsDispatch::default();
        // The read probe on load, then the same deck reloaded twice.
        assert!(dispatch.should_dispatch("aa", false));
        assert!(!dispatch.should_dispatch("aa", false));
        assert!(!dispatch.should_dispatch("aa", false));
        // Separation finishing is a different reason, and fires exactly once.
        assert!(dispatch.should_dispatch("aa", true));
        assert!(!dispatch.should_dispatch("aa", true));
        // …and after a bake, a fresh load does not re-probe.
        assert!(!dispatch.should_dispatch("aa", false));
        // Another track is another key.
        assert!(dispatch.should_dispatch("bb", true));
        dispatch.forget("aa");
        assert!(dispatch.should_dispatch("aa", false));
    }

    // ---- the load-time probe ---------------------------------------------

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("makepad-vj-lyrics-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn probe_job(digest: &str, model_frames: u64) -> LyricsJob {
        LyricsJob {
            deck: DeckId::A,
            gen: 7,
            digest: digest.to_string(),
            model_frames,
            duration_secs: 120.0,
            bake: false,
        }
    }

    fn said(rx: &Receiver<LyricsMsg>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let LyricsMsg::Status { text, .. } = message {
                out.push(text);
            }
        }
        out
    }

    /// A probe that finds no transcript has to SAY it found no transcript.
    ///
    /// Returning in silence leaves the deck's lyrics status empty, and empty
    /// is exactly what the reader renders as "waiting for separation" — so
    /// the one path that knows the answer is the one path that never speaks.
    /// While the stems really are unseparated that fallback happens to be
    /// true, and this is the case that says so out loud instead of by
    /// omission.
    #[test]
    fn a_probe_that_finds_nothing_says_the_stems_are_not_ready() {
        let lyrics_dir = temp_dir("probe-cold");
        let stem_root = temp_dir("probe-cold-stems");
        let (out, rx) = channel::<LyricsMsg>();
        let mut backend = None;
        let mut failed = false;

        run_job(
            probe_job(&"c".repeat(64), 4 * 44_100),
            &lyrics_dir,
            &stem_root,
            &mut backend,
            &mut failed,
            &out,
        );

        assert_eq!(said(&rx), vec!["lyrics: waiting for separation".to_string()]);
        let _ = std::fs::remove_dir_all(&lyrics_dir);
        let _ = std::fs::remove_dir_all(&stem_root);
    }

    /// The lie this whole path existed to tell: stems separated end to end,
    /// no transcript, and a reader reporting that it is waiting for the
    /// separation that already finished. Once the stems are complete the
    /// answer is about the WORDS, not about the stems.
    #[test]
    fn a_probe_over_finished_stems_does_not_blame_the_separation() {
        let lyrics_dir = temp_dir("probe-warm");
        let stem_root = temp_dir("probe-warm-stems");
        let digest = "d".repeat(64);
        let frames = 2 * makepad_ai_stems::CHUNK_STEP;
        {
            let mut cache =
                StemCache::open(&stem_root, &digest, CacheHeader::for_track(frames as u64))
                    .unwrap();
            for span in 0..cache.span_count() {
                let set: makepad_ai_stems::StemSet = std::array::from_fn(|_| {
                    makepad_ai_stems::StereoBuf {
                        left: vec![0.0; makepad_ai_stems::CHUNK_STEP],
                        right: vec![0.0; makepad_ai_stems::CHUNK_STEP],
                    }
                });
                cache
                    .write_span(span * makepad_ai_stems::CHUNK_STEP, &set)
                    .unwrap();
            }
            assert!(cache.is_complete());
        }
        let (out, rx) = channel::<LyricsMsg>();
        let mut backend = None;
        let mut failed = false;

        run_job(
            probe_job(&digest, frames as u64),
            &lyrics_dir,
            &stem_root,
            &mut backend,
            &mut failed,
            &out,
        );

        assert_eq!(said(&rx), vec!["lyrics: no transcript".to_string()]);
        let _ = std::fs::remove_dir_all(&lyrics_dir);
        let _ = std::fs::remove_dir_all(&stem_root);
    }

    /// The probe's happy path, unchanged: a track transcribed in an earlier
    /// session hangs its words from the cache without a job.
    #[test]
    fn a_probe_that_finds_a_transcript_hands_it_over() {
        let lyrics_dir = temp_dir("probe-hit");
        let stem_root = temp_dir("probe-hit-stems");
        let digest = "e".repeat(64);
        let lyrics = TrackLyrics {
            backend: "whisper".into(),
            model: "ggml-large-v3-turbo.bin".into(),
            language: "en".into(),
            duration_secs: 120.0,
            onset: OnsetStats { snapped: 1, mean_ms: 20.0, max_ms: 40.0 },
            lines: vec![line(1.0, 2.0, "already known")],
        };
        store_cached(&lyrics_dir, &digest, &lyrics);
        let (out, rx) = channel::<LyricsMsg>();
        let mut backend = None;
        let mut failed = false;

        run_job(
            probe_job(&digest, 4 * 44_100),
            &lyrics_dir,
            &stem_root,
            &mut backend,
            &mut failed,
            &out,
        );

        let mut ready = 0;
        let mut texts = Vec::new();
        while let Ok(message) = rx.try_recv() {
            match message {
                LyricsMsg::Ready { lyrics, .. } => {
                    ready += 1;
                    assert_eq!(lyrics.lines.len(), 1);
                }
                LyricsMsg::Status { text, .. } => texts.push(text),
                // A deck job never reports one.
                LyricsMsg::PrefetchDone { .. } => panic!("a deck job prefetched"),
            }
        }
        assert_eq!(ready, 1, "the cached transcript has to reach the deck");
        assert_eq!(texts, vec!["lyrics: 1 lines (cached)".to_string()]);
        let _ = std::fs::remove_dir_all(&lyrics_dir);
        let _ = std::fs::remove_dir_all(&stem_root);
    }

    // ---- onsets ----------------------------------------------------------

    /// A burst of tone starting at `onset`, in an otherwise silent buffer.
    fn burst(rate: f64, length: f64, onset: f64, duration: f64) -> Vec<f32> {
        let total = (rate * length) as usize;
        let from = (rate * onset) as usize;
        let to = ((rate * (onset + duration)) as usize).min(total);
        (0..total)
            .map(|index| {
                if index >= from && index < to {
                    (index as f64 * 0.05).sin() as f32 * 0.6
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn a_late_whisper_stamp_is_pulled_back_to_the_vocal_onset() {
        let rate = 16_000.0;
        let audio = burst(rate, 6.0, 2.00, 1.5);
        let envelope = VocalEnvelope::build(&audio, rate);
        // Whisper stamps where decoding settles — a quarter second late.
        let mut lines = vec![line(2.25, 3.5, "here it comes")];
        let stats = refine_onsets(&mut lines, &envelope);
        assert_eq!(stats.snapped, 1);
        assert!(
            (lines[0].start_secs - 2.0).abs() < 0.06,
            "snapped to {:.3}s, wanted ~2.00s",
            lines[0].start_secs
        );
        assert!(stats.max_ms > 150.0 && stats.max_ms < 350.0, "{stats:?}");
    }

    #[test]
    fn an_onset_search_never_crosses_the_previous_line() {
        let rate = 16_000.0;
        // Two bursts: 1.0-1.8 s and 2.0-2.8 s.
        let mut audio = burst(rate, 5.0, 1.0, 0.8);
        for (index, value) in burst(rate, 5.0, 2.0, 0.8).into_iter().enumerate() {
            audio[index] += value;
        }
        let envelope = VocalEnvelope::build(&audio, rate);
        let mut lines = vec![line(1.0, 1.8, "first"), line(2.2, 2.8, "second")];
        refine_onsets(&mut lines, &envelope);
        assert!(
            lines[1].start_secs >= lines[0].end_secs - 1e-9,
            "second line snapped back over the first: {:?}",
            lines
        );
        assert!(lines[1].start_secs <= 2.2 + 1e-9);
    }

    #[test]
    fn silence_offers_no_onset_to_snap_to() {
        let rate = 16_000.0;
        let envelope = VocalEnvelope::build(&vec![0.0f32; 16_000 * 4], rate);
        let mut lines = vec![line(1.0, 2.0, "nothing here")];
        let stats = refine_onsets(&mut lines, &envelope);
        assert_eq!(stats.snapped, 0);
        assert_eq!(lines[0].start_secs, 1.0);
    }

    #[test]
    fn a_line_whisper_invented_over_silence_is_dropped() {
        let rate = 16_000.0;
        let audio = burst(rate, 8.0, 1.0, 1.0);
        let envelope = VocalEnvelope::build(&audio, rate);
        let segments = vec![
            (1_000i64, 2_000i64, " real words ".to_string()),
            (5_000, 6_000, "Thank you.".to_string()),
            (6_000, 6_500, "[BLANK_AUDIO]".to_string()),
            (6_600, 6_900, "   ".to_string()),
        ];
        let lines = segments_to_lines(&segments, Some(&envelope), 8.0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "real words");
    }

    // ---- phrases ---------------------------------------------------------

    /// Bursts of tone at the given (onset, duration) pairs, silence between.
    fn bursts(rate: f64, length: f64, spans: &[(f64, f64)]) -> Vec<f32> {
        let mut out = vec![0.0f32; (rate * length) as usize];
        for (onset, duration) in spans {
            let from = (rate * onset) as usize;
            let to = ((rate * (onset + duration)) as usize).min(out.len());
            for index in from..to {
                out[index] = (index as f64 * 0.05).sin() as f32 * 0.6;
            }
        }
        out
    }

    #[test]
    fn the_stem_silences_are_where_the_phrases_are() {
        let rate = 16_000.0;
        // Three sung phrases with real gaps, plus a breath too short to
        // separate one line from the next.
        let audio = bursts(
            rate,
            14.0,
            &[(1.0, 1.6), (2.75, 1.4), (6.0, 2.0), (10.0, 1.5)],
        );
        let envelope = VocalEnvelope::build(&audio, rate);
        let phrases = envelope.phrases(0.0, 14.0);
        // 1.0–2.6 and 2.75–4.15 are 0.15 s apart: one line, not two.
        assert_eq!(phrases.len(), 3, "{phrases:?}");
        assert!((phrases[0].start_secs - 1.0).abs() < 0.08, "{phrases:?}");
        assert!((phrases[0].end_secs - 4.15).abs() < 0.10, "{phrases:?}");
        assert!((phrases[1].start_secs - 6.0).abs() < 0.08, "{phrases:?}");
        assert!((phrases[2].start_secs - 10.0).abs() < 0.08, "{phrases:?}");
        // Silence offers no phrases at all.
        let quiet = VocalEnvelope::build(&vec![0.0f32; 16_000 * 4], rate);
        assert!(quiet.phrases(0.0, 4.0).is_empty());
    }

    #[test]
    fn a_phrase_too_long_to_read_is_cut_at_its_quietest_moment() {
        let rate = 16_000.0;
        // Nine seconds of continuous voice with one dip in the middle: a
        // singer who never fully stopped, but did take a breath.
        let mut audio = bursts(rate, 12.0, &[(1.0, 9.0)]);
        let dip = ((rate * 5.4) as usize)..((rate * 5.6) as usize);
        for sample in &mut audio[dip] {
            *sample *= 0.02;
        }
        let envelope = VocalEnvelope::build(&audio, rate);
        let phrases = envelope.phrases(0.0, 12.0);
        assert!(phrases.len() >= 2, "a 9 s phrase must be cut: {phrases:?}");
        for phrase in &phrases {
            assert!(
                phrase.secs() <= PHRASE_MAX_SECS + 0.2,
                "{phrase:?} is still unreadable"
            );
        }
        // The first cut lands on the breath, not at an arbitrary midpoint.
        assert!(
            (phrases[0].end_secs - 5.5).abs() < 0.4,
            "cut at {:.2}s, wanted ~5.5s",
            phrases[0].end_secs
        );
    }

    #[test]
    fn words_follow_the_phrases_and_break_on_punctuation() {
        let phrases = vec![
            VocalPhrase { start_secs: 1.0, end_secs: 3.0 },
            VocalPhrase { start_secs: 4.0, end_secs: 6.0 },
        ];
        let lines = distribute_words(
            "You can dance, you can jive, having the time of your life.",
            &phrases,
        );
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start_secs, 1.0);
        assert_eq!(lines[0].end_secs, 3.0);
        assert_eq!(lines[1].start_secs, 4.0);
        // The break is on a comma, not mid-clause.
        assert!(
            lines[0].text.ends_with(','),
            "broke at {:?}",
            lines[0].text
        );
        // Every word survives, in order, exactly once.
        let rejoined = format!("{} {}", lines[0].text, lines[1].text);
        assert_eq!(
            rejoined,
            "You can dance, you can jive, having the time of your life."
        );
    }

    #[test]
    fn fewer_words_than_phrases_still_leaves_every_word_on_screen() {
        let phrases: Vec<VocalPhrase> = (0..5)
            .map(|i| VocalPhrase {
                start_secs: i as f64 * 2.0,
                end_secs: i as f64 * 2.0 + 1.0,
            })
            .collect();
        let lines = distribute_words("two words", &phrases);
        assert!(lines.len() <= 2, "{lines:?}");
        let rejoined: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(rejoined.join(" "), "two words");
        // …and a phrase list with no words at all produces nothing.
        assert!(distribute_words("   ", &phrases).is_empty());
        assert!(distribute_words("anything", &[]).is_empty());
    }

    #[test]
    fn phrases_are_merged_down_to_what_the_words_can_fill() {
        // Twelve tiny phrases and eight words: two lines, broken on the two
        // biggest silences, not twelve lines of one word.
        let phrases: Vec<VocalPhrase> = (0..12)
            .map(|i| {
                let start = i as f64 * 0.6 + if i >= 6 { 3.0 } else { 0.0 };
                VocalPhrase { start_secs: start, end_secs: start + 0.4 }
            })
            .collect();
        let merged = merge_phrases_for_words(phrases.clone(), 8);
        assert_eq!(merged.len(), 2, "{merged:?}");
        // The break is the 3 s gap, and the ends of the span are preserved.
        assert_eq!(merged[0].start_secs, phrases[0].start_secs);
        assert_eq!(merged[0].end_secs, phrases[5].end_secs);
        assert_eq!(merged[1].start_secs, phrases[6].start_secs);
        assert_eq!(merged[1].end_secs, phrases[11].end_secs);
        // A long phrase list with plenty of words is left alone.
        let many = merge_phrases_for_words(phrases.clone(), 200);
        assert_eq!(many.len(), phrases.len());
        // …and merging never builds a line too long to read.
        let sparse: Vec<VocalPhrase> = (0..6)
            .map(|i| VocalPhrase {
                start_secs: i as f64 * 10.0,
                end_secs: i as f64 * 10.0 + 4.0,
            })
            .collect();
        for phrase in merge_phrases_for_words(sparse, 3) {
            assert!(phrase.secs() <= PHRASE_MAX_SECS * 1.15 + 1e-9, "{phrase:?}");
        }
    }

    #[test]
    fn a_paragraph_segment_becomes_singable_phrases() {
        let rate = 16_000.0;
        let audio = bursts(rate, 20.0, &[(2.0, 2.0), (5.0, 2.0), (8.0, 2.0)]);
        let envelope = VocalEnvelope::build(&audio, rate);
        // Exactly the shape whisper hands back: one stamp pair over three
        // sung lines.
        let segments = vec![(
            1_800i64,
            10_200i64,
            "Friday night and the lights are low, looking out for a place to go, \
             where they play the right music."
                .to_string(),
        )];
        let lines = segments_to_lines(&segments, Some(&envelope), 20.0);
        // At least one line per sung burst, and every burst is the start of
        // one — a long burst may be cut again to keep lines readable.
        assert!(lines.len() >= 3, "{lines:?}");
        for burst in [2.0, 5.0, 8.0] {
            assert!(
                lines.iter().any(|line| (line.start_secs - burst).abs() < 0.1),
                "no line starts on the burst at {burst}s: {lines:?}"
            );
        }
        // Starts are strictly increasing and no line outlives its phrase.
        for pair in lines.windows(2) {
            assert!(pair[0].end_secs <= pair[1].start_secs + 1e-9, "{lines:?}");
        }
        // …and the whole segment's words survive, in order.
        let rejoined: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            rejoined.join(" "),
            "Friday night and the lights are low, looking out for a place to go, \
             where they play the right music."
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    #[test]
    fn a_repeated_line_butted_against_itself_is_a_decoder_loop() {
        let segments = vec![
            (0i64, 1_000i64, "same".to_string()),
            (1_000, 2_000, "same".to_string()),
            (4_000, 5_000, "same".to_string()),
        ];
        let lines = segments_to_lines(&segments, None, 10.0);
        // The butted repeat goes; a genuine reprise three seconds later stays.
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].start_secs, 4.0);
    }

    // ---- word timing -----------------------------------------------------

    #[test]
    fn words_land_on_the_stem_and_the_gap_between_them_costs_nothing() {
        let rate = 16_000.0;
        // Four sung words: 1.0, 1.5, then a whole second of silence, then
        // 3.0 and 3.5. A constant sweep would put word three at 2.0 s, in
        // the middle of the gap, which is exactly the failure to avoid.
        let audio = bursts(
            rate,
            6.0,
            &[(1.0, 0.35), (1.5, 0.35), (3.0, 0.35), (3.5, 0.35)],
        );
        let envelope = VocalEnvelope::build(&audio, rate);
        let mut lines = vec![line(1.0, 3.9, "aaa bbb ccc ddd")];
        let timed = time_words_with(&mut lines, &envelope, true);
        assert_eq!(timed, 1);
        let words = &lines[0].words;
        assert_eq!(words.len(), 4, "{words:?}");
        for (index, wanted) in [1.0, 1.5, 3.0, 3.5].into_iter().enumerate() {
            assert!(
                (words[index] - wanted).abs() <= 0.15,
                "word {index} landed at {:.3}s, wanted {wanted}s ({words:?})",
                words[index]
            );
        }
        // Strictly increasing, inside the line.
        for pair in words.windows(2) {
            assert!(pair[1] > pair[0], "{words:?}");
        }
    }

    #[test]
    fn the_fill_hops_word_by_word_and_holds_between_them() {
        let mut line = line(1.0, 4.0, "aaa bbb ccc");
        line.words = vec![1.0, 2.0, 3.0];
        line.confident = true;
        // Before the line: nothing.
        assert_eq!(sung_fraction(&line, 0.5), 0.0);
        // The first word fills over the hop and then HOLDS: the fraction at
        // 1.5 s and at 1.9 s is the same, because no word has come due.
        // "aaa bbb ccc" is eleven characters; the first word plus its space
        // is four of them, and the fraction is in exactly the units the
        // renderers colour by.
        let after_first = sung_fraction(&line, 1.0 + WORD_HOP_SECS + 1e-3);
        assert!((after_first - 4.0 / 11.0).abs() < 0.02, "{after_first}");
        assert_eq!(sung_fraction(&line, 1.5), sung_fraction(&line, 1.9));
        // Mid-hop is partway into that word and nowhere else.
        let mid = sung_fraction(&line, 2.0 + WORD_HOP_SECS * 0.5);
        assert!(mid > 4.0 / 11.0 && mid < 8.0 / 11.0, "{mid}");
        // Past the last word: the whole line.
        assert!((sung_fraction(&line, 3.5) - 1.0).abs() < 1e-6);
        // Without word times it degrades to the linear sweep.
        let plain = self::line(1.0, 3.0, "aaa bbb ccc");
        assert!((sung_fraction(&plain, 2.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn syllables_inside_a_sung_phrase_are_found_by_their_rise() {
        let rate = 16_000.0;
        // Continuous singing: the level never returns to silence between
        // syllables, it only dips. A level-threshold detector fires once here
        // and every word after the first falls back to a guess — which is
        // exactly the "one word late" failure. The rise is what separates
        // them, so this is the case that has to work.
        let hop = 0.25;
        let mut audio = vec![0.0f32; (rate * 4.0) as usize];
        for syllable in 0..8 {
            let onset = 0.5 + syllable as f64 * hop;
            let from = (onset * rate) as usize;
            let to = ((onset + hop) * rate) as usize;
            for (offset, index) in (from..to.min(audio.len())).enumerate() {
                // An attack that decays: a note, not a square pulse.
                let decay = (-(offset as f64) / (rate * 0.08)).exp() as f32;
                audio[index] = (index as f64 * 0.05).sin() as f32 * 0.6 * (0.35 + 0.65 * decay);
            }
        }
        let envelope = VocalEnvelope::build(&audio, rate);
        let onsets = envelope.onsets_between(0.3, 3.0);
        assert!(
            onsets.len() >= 7,
            "only {} attacks found inside continuous singing: {onsets:?}",
            onsets.len()
        );
        // Each expected syllable has an attack within a hop or two of it.
        for syllable in 0..8 {
            let want = 0.5 + syllable as f64 * hop;
            let nearest = onsets
                .iter()
                .cloned()
                .min_by(|a, b| (a - want).abs().partial_cmp(&(b - want).abs()).unwrap())
                .unwrap();
            assert!(
                (nearest - want).abs() <= 0.05,
                "syllable at {want:.2}s has no attack nearer than {nearest:.3}s"
            );
        }
        // …and they come back in order, none repeated.
        for pair in onsets.windows(2) {
            assert!(pair[1] > pair[0], "{onsets:?}");
        }
    }

    #[test]
    fn every_word_of_a_sung_phrase_lands_on_an_attack() {
        let rate = 16_000.0;
        let hop = 0.30;
        let mut audio = vec![0.0f32; (rate * 4.0) as usize];
        for syllable in 0..6 {
            let onset = 0.5 + syllable as f64 * hop;
            let from = (onset * rate) as usize;
            let to = ((onset + hop * 0.8) * rate) as usize;
            for (offset, index) in (from..to.min(audio.len())).enumerate() {
                let decay = (-(offset as f64) / (rate * 0.08)).exp() as f32;
                audio[index] = (index as f64 * 0.05).sin() as f32 * 0.6 * (0.35 + 0.65 * decay);
            }
        }
        let envelope = VocalEnvelope::build(&audio, rate);
        let mut lines = vec![line(0.5, 2.4, "one two three four five six")];
        assert_eq!(time_words_with(&mut lines, &envelope, true), 1);
        let words = &lines[0].words;
        assert_eq!(words.len(), 6, "{words:?}");
        let attacks = envelope.onsets_between(0.35, 2.5);
        for (index, at) in words.iter().enumerate() {
            let nearest = attacks
                .iter()
                .cloned()
                .min_by(|a, b| (a - at).abs().partial_cmp(&(b - at).abs()).unwrap())
                .unwrap();
            assert!(
                (nearest - at).abs() <= 0.02,
                "word {index} at {at:.3}s is not on an attack (nearest {nearest:.3}s)"
            );
        }
    }

    #[test]
    fn a_line_the_stem_cannot_time_keeps_no_word_times() {
        let rate = 16_000.0;
        let envelope = VocalEnvelope::build(&vec![0.0f32; 16_000 * 4], rate);
        let mut lines = vec![line(1.0, 3.0, "nothing to hear here")];
        assert_eq!(time_words_with(&mut lines, &envelope, true), 0);
        assert!(lines[0].words.is_empty());
        // …and the display falls back rather than freezing.
        assert!((sung_fraction(&lines[0], 2.0) - 0.5).abs() < 1e-6);
    }

    // ---- display timing --------------------------------------------------

    #[test]
    fn the_lead_in_is_two_beats_clamped_to_something_readable() {
        // 120 BPM: two beats = 1.0 s, inside the band.
        assert!((KaraokeTiming::from_grid(Some(&grid(120.0))).lead_in_secs - 1.0).abs() < 1e-9);
        // 174 BPM drum & bass: two beats = 0.69 s, floored at 0.8.
        assert!(
            (KaraokeTiming::from_grid(Some(&grid(174.0))).lead_in_secs - LEAD_IN_MIN_SECS).abs()
                < 1e-9
        );
        // 55 BPM: two beats = 2.18 s, capped at 2.0.
        assert!(
            (KaraokeTiming::from_grid(Some(&grid(55.0))).lead_in_secs - LEAD_IN_MAX_SECS).abs()
                < 1e-9
        );
        // No grid at all still gives a usable lead.
        let none = KaraokeTiming::from_grid(None);
        assert!(none.lead_in_secs >= LEAD_IN_MIN_SECS && none.beat_secs == 0.0);
    }

    #[test]
    fn display_transitions_land_on_beats() {
        let timing = KaraokeTiming::from_grid(Some(&grid(120.0))); // 0.5 s beats
        let lines = vec![line(10.3, 12.7, "on the beat")];
        let schedule = KaraokeSchedule::build(lines, timing);
        // Lead-in target is 10.3 - 1.0 = 9.3 s; the beat AT OR BEFORE it is
        // 9.0, so the displayed lead is 1.3 s — never less than the target.
        assert!((schedule.appear[0] - 9.0).abs() < 1e-9, "{:?}", schedule.appear);
        assert!(schedule.lines[0].start_secs - schedule.appear[0] >= timing.lead_in_secs);
        // Hold target is 12.7 + 0.5 = 13.2; the beat at or after it is 13.5.
        assert!((schedule.leave[0] - 13.5).abs() < 1e-9, "{:?}", schedule.leave);
        // Every appear and leave is an exact multiple of the beat.
        for at in schedule.appear.iter().chain(schedule.leave.iter()) {
            assert!((at / 0.5 - (at / 0.5).round()).abs() < 1e-9, "{at} is off-grid");
        }
    }

    #[test]
    fn a_line_never_takes_the_screen_from_one_still_being_sung() {
        let timing = KaraokeTiming::from_grid(Some(&grid(120.0)));
        // Dense lines: the second one's lead-in target is inside the first.
        let lines = vec![line(10.0, 12.0, "first"), line(12.2, 14.0, "second")];
        let schedule = KaraokeSchedule::build(lines, timing);
        assert!(
            schedule.appear[1] >= 12.0,
            "second line appeared at {} — over the first line's last words",
            schedule.appear[1]
        );
        // …but it was already on screen, as the dim next row, long before.
        assert!(schedule.on_screen_at(1) <= 9.0);
    }

    #[test]
    fn the_lookup_walks_before_between_and_after() {
        let timing = KaraokeTiming::from_grid(Some(&grid(120.0)));
        let schedule = KaraokeSchedule::build(
            vec![
                line(10.0, 12.0, "one"),
                line(20.0, 22.0, "two"),
                line(23.0, 25.0, "three"),
            ],
            timing,
        );
        // Long before anything: nothing at all, not even a preview.
        let early = schedule.at(0.0);
        assert_eq!(early.current, None);
        assert_eq!(early.next, None);
        // Inside the first line's lead-in: it is up, and not sweeping yet.
        let lead = schedule.at(schedule.appear[0] + 0.1);
        assert_eq!(lead.current, Some(0));
        assert_eq!(lead.progress, 0.0);
        assert_eq!(lead.next, Some(1));
        // Mid-line: the sweep is halfway across.
        let mid = schedule.at(11.0);
        assert_eq!(mid.current, Some(0));
        assert!((mid.progress - 0.5).abs() < 1e-6);
        // The gap between line one and two: nothing current, line two
        // previewed once it is near.
        let gap = schedule.at(15.0);
        assert_eq!(gap.current, None);
        assert_eq!(gap.next, Some(1));
        let deep_gap = schedule.at(13.6);
        assert_eq!(deep_gap.current, None);
        // Past the last line: the screen clears.
        let after = schedule.at(60.0);
        assert_eq!(after.current, None);
        assert_eq!(after.next, None);
        // An empty track never reports anything.
        let empty = KaraokeSchedule::build(Vec::new(), timing);
        assert_eq!(empty.at(5.0), KaraokeFrame::default());
    }

    #[test]
    fn no_line_is_ever_shown_after_its_first_word() {
        // The gate, stated as an assertion: over a whole track's worth of
        // lines, every one is on screen before it is sung — with at least the
        // lead-in target as the bright row, and far more as the dim row.
        let timing = KaraokeTiming::from_grid(Some(&grid(100.0)));
        let mut lines = Vec::new();
        let mut at = 4.0;
        for index in 0..40 {
            let length = 1.5 + (index % 3) as f64 * 0.4;
            lines.push(line(at, at + length, &format!("line {index}")));
            at += length + 0.3 + (index % 4) as f64 * 0.5;
        }
        let schedule = KaraokeSchedule::build(lines, timing);
        for index in 0..schedule.lines.len() {
            let start = schedule.lines[index].start_secs;
            assert!(
                schedule.on_screen_at(index) <= start - timing.lead_in_secs + 1e-9,
                "line {index} reached the screen at {:.2}s for a {:.2}s start",
                schedule.on_screen_at(index),
                start
            );
            // And the moment it is sung, it IS the bright row.
            assert_eq!(schedule.at(start + 1e-3).current, Some(index));
        }
    }

    /// Walk a schedule at display rate and report, per line, the seconds it
    /// held the bright row and the order it did so in.
    fn walk(schedule: &KaraokeSchedule, until: f64) -> Vec<(usize, f64)> {
        let step = 1.0 / 60.0;
        let mut runs: Vec<(usize, f64)> = Vec::new();
        let mut at = 0.0;
        while at < until {
            if let Some(index) = schedule.at(at).current {
                match runs.last_mut() {
                    Some((last, held)) if *last == index => *held += step,
                    _ => runs.push((index, step)),
                }
            }
            at += step;
        }
        runs
    }

    #[test]
    fn every_line_gets_the_screen_exactly_once_and_long_enough_to_read() {
        // The failure this locks out is a line NOBODY EVER SEES. Three clamps
        // shape a line's appear moment — the lead-in, the previous line's
        // end, and zero — and any of them can put two lines on the same
        // instant: two lines pinned to zero at the top of a track, or a start
        // pulled back under the line before it. The lookup takes the last
        // line whose moment has passed, so a collapsed pair silently drops
        // the earlier one.
        let timing = KaraokeTiming::from_grid(Some(&grid(128.0)));
        let mut lines = vec![
            // Two lines inside the first lead-in: both would clamp to zero.
            line(0.30, 0.90, "one"),
            line(0.95, 1.60, "two"),
            // A line whose start was snapped back under its predecessor.
            line(1.55, 2.40, "three"),
            // Butted, then a gap, then dense again.
            line(2.40, 3.10, "four"),
            line(8.00, 9.50, "five"),
            line(9.55, 10.10, "six"),
            line(10.12, 10.60, "seven"),
        ];
        // …and a long tail of ordinary lines, so the guarantee is not a
        // property of a hand-picked seven.
        let mut at = 12.0;
        for index in 0..30 {
            let length = 0.8 + (index % 5) as f64 * 0.35;
            lines.push(line(at, at + length, &format!("line {index}")));
            at += length + (index % 3) as f64 * 0.2;
        }
        let last = lines.last().unwrap().end_secs;
        let schedule = KaraokeSchedule::build(lines, timing);
        let runs = walk(&schedule, last + 4.0);

        // Every line, exactly once, in order.
        let order: Vec<usize> = runs.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            order,
            (0..schedule.lines.len()).collect::<Vec<_>>(),
            "lines were skipped, repeated or shown out of order"
        );
        // …and each held the screen long enough to be read.
        let floor = timing.floor_secs();
        for (index, held) in &runs {
            assert!(
                *held >= floor - 0.02,
                "line {index} ({:?}) held the screen for only {held:.3}s, floor {floor:.3}s",
                schedule.lines[*index].text,
            );
        }
        // The appear moments are strictly increasing, which is the invariant
        // the lookup's binary search depends on.
        for pair in schedule.appear.windows(2) {
            assert!(pair[1] > pair[0], "{:?}", schedule.appear);
        }
    }

    #[test]
    fn a_held_note_keeps_the_fill_moving_instead_of_freezing() {
        // A singer holding one word for two seconds while the rest of the
        // line is quick: hopping and then freezing looks broken, so the held
        // word fills across its own interval instead — and still lands
        // exactly where the hop would have.
        let mut held = line(0.0, 5.0, "aaa bbb ccc ddd");
        held.words = vec![0.0, 0.3, 0.6, 2.6];
        held.confident = true;
        let a = sung_fraction(&held, 2.8);
        let b = sung_fraction(&held, 3.6);
        let c = sung_fraction(&held, 4.4);
        assert!(a < b && b < c, "the fill froze on the held word: {a} {b} {c}");
        assert!((sung_fraction(&held, 5.0) - 1.0).abs() < 1e-6);
        // A word at the line's own pace still hops: quick, then holds.
        let quick = sung_fraction(&held, 0.3 + WORD_HOP_SECS + 1e-3);
        assert_eq!(quick, sung_fraction(&held, 0.55));
    }

    #[test]
    fn word_times_that_cannot_be_trusted_are_not_offered_to_the_display() {
        let mut good = line(0.0, 4.0, "aaa bbb ccc ddd");
        let times = vec![0.0, 1.0, 2.0, 3.0];
        assert!(word_times_are_trustworthy(&good, &times, 4));
        // Most of them guessed rather than placed: sweep instead.
        assert!(!word_times_are_trustworthy(&good, &times, 1));
        // Out of order, or stacked on one instant.
        assert!(!word_times_are_trustworthy(&good, &[0.0, 1.0, 0.9, 3.0], 4));
        assert!(!word_times_are_trustworthy(&good, &[0.0, 1.0, 1.0, 3.0], 4));
        // Outside the line, or the wrong number of them.
        assert!(!word_times_are_trustworthy(&good, &[0.0, 1.0, 2.0, 9.0], 4));
        assert!(!word_times_are_trustworthy(&good, &[0.0, 1.0, 2.0], 3));
        // One word swallowing the line is a bad assignment, not a sustain.
        assert!(!word_times_are_trustworthy(&good, &[0.0, 0.2, 0.4, 0.6], 4));
        // …and an untrusted line renders as the sweep, whatever it carries.
        good.words = times;
        good.confident = false;
        assert!(!good.hops());
        assert!((sung_fraction(&good, 2.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn without_a_grid_the_timing_still_leads_the_voice() {
        let timing = KaraokeTiming::from_grid(None);
        let schedule = KaraokeSchedule::build(vec![line(10.0, 12.0, "ungridded")], timing);
        assert!((schedule.appear[0] - (10.0 - timing.lead_in_secs)).abs() < 1e-9);
        assert!((schedule.leave[0] - (12.0 + timing.hold_secs)).abs() < 1e-9);
    }

    // ---- which deck ------------------------------------------------------

    // ---- the real thing --------------------------------------------------

    /// Separate a real track end to end into the REAL stem cache, bake its
    /// lyrics, and report what karaoke would actually look like:
    ///
    /// ```text
    /// VJ_AUDIO_SAMPLE="local/music/ABBA/Arrival/98 ABBA - Dancing Queen.mp3" \
    ///     cargo test -p makepad-vj --release -- --nocapture --ignored karaoke_bakes
    /// ```
    ///
    /// It asserts the gate: every line reaches the screen before it is sung,
    /// with at least the lead-in target of warning.
    #[test]
    #[ignore = "loads a 527 MB separation checkpoint and a whisper model"]
    fn karaoke_bakes_a_real_track_end_to_end() {
        use crate::mixer::TrackPcm;
        use makepad_ai_stems::{Demixer, StemsModel};

        let Ok(sample) = std::env::var("VJ_AUDIO_SAMPLE") else {
            eprintln!("VJ_AUDIO_SAMPLE not set; skipping");
            return;
        };
        let path = PathBuf::from(&sample);
        let decoded = crate::wave_analysis::decode_audio_file(&path).expect("decode");
        let pcm = Arc::new(TrackPcm {
            frames: decoded.frames.clone(),
            sample_rate: decoded.sample_rate,
        });
        let duration = pcm.frames.len() as f64 / pcm.sample_rate.max(1) as f64;
        let digest = crate::stems::track_digest(&pcm);
        let frames = crate::stems::model_frames(&pcm) as u64;
        eprintln!(
            "track: {sample}\n  {duration:.1}s at {} Hz, digest {}",
            pcm.sample_rate, digest
        );

        // 1. Separation, into the cache the app itself uses.
        let header = CacheHeader::for_track(frames);
        let mut cache = StemCache::open(crate::stems::cache_dir(), &digest, header.clone())
            .expect("stem cache");
        if !cache.is_complete() {
            let started = crate::clock::Instant::now();
            let mut model =
                StemsModel::load(&crate::stems::checkpoint_path()).expect("checkpoint");
            let buffer = {
                let mut left = Vec::with_capacity(pcm.frames.len());
                let mut right = Vec::with_capacity(pcm.frames.len());
                for frame in &pcm.frames {
                    left.push(frame[0] as f32 / 32768.0);
                    right.push(frame[1] as f32 / 32768.0);
                }
                let rate = pcm.sample_rate.max(1) as f64;
                let target = makepad_ai_stems::SAMPLE_RATE as f64;
                makepad_ai_stems::StereoBuf {
                    left: crate::stems::resample(&left, rate, target),
                    right: crate::stems::resample(&right, rate, target),
                }
            };
            let mut demixer = Demixer::new(&mut model, &buffer).expect("demixer");
            while let Some(span) = demixer.next_span().expect("span") {
                cache.write_span(span.start, &span.stems).expect("write");
            }
            eprintln!(
                "  separation: {:.1}s wall for {duration:.1}s of audio",
                started.elapsed().as_secs_f64()
            );
        } else {
            eprintln!("  separation: already cached");
        }
        assert!(cache.is_complete(), "separation left gaps");
        drop(cache);

        // 2. The bake, exactly as the worker runs it.
        let job = LyricsJob {
            deck: DeckId::A,
            gen: 1,
            digest: digest.clone(),
            model_frames: frames,
            duration_secs: duration,
            bake: true,
        };
        let started = crate::clock::Instant::now();
        let (mono, rate) = read_vocals_mono(&job).expect("vocals");
        let read_secs = started.elapsed().as_secs_f64();
        let envelope = VocalEnvelope::build(&mono, rate);
        let samples = crate::stems::resample(&mono, rate, WHISPER_RATE);
        let prep_secs = started.elapsed().as_secs_f64();
        let mut backend = Transcriber::open().expect("transcriber");
        let load_secs = started.elapsed().as_secs_f64();
        let segments = backend.transcribe(&samples, "en").expect("transcribe");
        let transcribe_secs = started.elapsed().as_secs_f64();
        let mut lines = segments_to_lines(&segments, Some(&envelope), duration);
        let onset = refine_onsets(&mut lines, &envelope);
        // The audit always measures the algorithm, whatever this build ships
        // as its default.
        let timed = time_words_with(&mut lines, &envelope, true);
        eprintln!(
            "  word-timed lines: {timed} of {} (the rest render as the smooth sweep)",
            lines.len()
        );

        // ---- the timing audit ------------------------------------------
        //
        // Where does the error live? Four places can shift a karaoke fill,
        // and each one is measured here rather than argued about:
        //   1. stem vs track time — does the separated vocal sit where the
        //      mix's vocal sits, or has the model's STFT moved it?
        //   2. the resampler into whisper — a causal FIR would delay every
        //      stamp by half its length.
        //   3. the word times in the cache against the stem's own attacks.
        //   4. the playhead the UI reads against what leaves the speakers.
        {
            // (1) Cross-correlate the vocals stem against the mixed track
            // over a loud vocal stretch. A non-zero lag would offset every
            // word by a constant.
            let probe_from = lines
                .get(lines.len() / 3)
                .map(|line| line.start_secs)
                .unwrap_or(30.0);
            let mix_rate = pcm.sample_rate.max(1) as f64;
            let from = (probe_from * mix_rate) as usize;
            let span = (4.0 * mix_rate) as usize;
            let to = (from + span).min(pcm.frames.len());
            if to > from + 1000 {
                let mix: Vec<f32> = pcm.frames[from..to]
                    .iter()
                    .map(|frame| (frame[0] as f32 + frame[1] as f32) / 65536.0)
                    .collect();
                let stem_from = (probe_from * rate) as usize;
                let stem_to = (stem_from + (4.0 * rate) as usize).min(mono.len());
                let stem = &mono[stem_from..stem_to];
                let reach = (0.050 * rate) as isize;
                let mut best = (0isize, f64::NEG_INFINITY);
                for lag in -reach..=reach {
                    let mut dot = 0.0f64;
                    let mut count = 0usize;
                    let mut at = 0usize;
                    while at + 8 < stem.len() {
                        let other = at as isize + lag;
                        if other >= 0 && (other as usize) < mix.len() {
                            dot += stem[at] as f64 * mix[other as usize] as f64;
                            count += 1;
                        }
                        at += 8;
                    }
                    let score = if count > 0 { dot / count as f64 } else { 0.0 };
                    if score > best.1 {
                        best = (lag, score);
                    }
                }
                eprintln!(
                    "  audit: stem-vs-mix lag {:+.1} ms (0 = the separated vocal sits exactly \
                     where the mix's vocal sits)",
                    best.0 as f64 * 1000.0 / rate
                );
            }
            // (2) The 44.1k -> 16k resampler: a centred windowed sinc has no
            // group delay, and an impulse proves it.
            let mut impulse = vec![0.0f32; 4410];
            impulse[2205] = 1.0;
            let out = crate::stems::resample(&impulse, 44_100.0, WHISPER_RATE);
            let peak = out
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
                .map(|(at, _)| at)
                .unwrap_or(0);
            eprintln!(
                "  audit: whisper resampler group delay {:+.2} ms (impulse at 50.00 ms came \
                 back at {:.2} ms)",
                peak as f64 * 1000.0 / WHISPER_RATE - 50.0,
                peak as f64 * 1000.0 / WHISPER_RATE,
            );
            // (3) Every word against the nearest attack the stem shows.
            let mut errors: Vec<f64> = Vec::new();
            let mut shown = 0usize;
            let mut attack_count = 0usize;
            let mut word_count = 0usize;
            eprintln!("  audit: word | cached time | nearest stem attack | error");
            for line in lines.iter() {
                let attacks = envelope.onsets_between(line.start_secs - 0.15, line.end_secs + 0.1);
                attack_count += attacks.len();
                word_count += line.text.split_whitespace().count();
                if attacks.is_empty() {
                    continue;
                }
                for (index, word) in line.text.split_whitespace().enumerate() {
                    let Some(at) = line.words.get(index) else { break };
                    let nearest = attacks
                        .iter()
                        .cloned()
                        .min_by(|a, b| {
                            (a - at).abs().partial_cmp(&(b - at).abs()).unwrap()
                        })
                        .unwrap();
                    let error = (at - nearest) * 1000.0;
                    errors.push(error);
                    if shown < 20 {
                        eprintln!(
                            "    {:>14} | {:8.3}s | {:8.3}s | {:+7.1} ms",
                            word, at, nearest, error
                        );
                        shown += 1;
                    }
                }
            }
            eprintln!(
                "  audit: the stem shows {attack_count} attacks for {word_count} words \
                 ({:.2} per word)",
                attack_count as f64 / word_count.max(1) as f64
            );
            if !errors.is_empty() {
                let mean = errors.iter().sum::<f64>() / errors.len() as f64;
                let mut sorted: Vec<f64> = errors.iter().map(|e| e.abs()).collect();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let within = |limit: f64| {
                    sorted.iter().filter(|e| **e <= limit).count() as f64 * 100.0
                        / sorted.len() as f64
                };
                eprintln!(
                    "  audit: {} words — mean {:+.1} ms, |err| p50 {:.1} ms, p90 {:.1} ms, \
                     max {:.1} ms; within 50 ms {:.0}%, within 100 ms {:.0}%",
                    errors.len(),
                    mean,
                    sorted[sorted.len() / 2],
                    sorted[sorted.len() * 9 / 10],
                    sorted[sorted.len() - 1],
                    within(50.0),
                    within(100.0),
                );
            }
        }

        // Does a SECOND whisper pass over one line's window buy finer stamps
        // than the stem already gives? `VJ_LYRICS_PASS2=1` measures it: the
        // cost per line, and how many timestamped pieces come back. Our port
        // returns segment bounds only — there is no token-timestamp / DTW
        // path in `libs/voice` — so this is the honest way to find out
        // whether short windows make the decoder emit more of them.
        if std::env::var("VJ_LYRICS_PASS2").is_ok() {
            for line in lines.iter().take(4) {
                let from = ((line.start_secs - 0.25).max(0.0) * WHISPER_RATE) as usize;
                let to = (((line.end_secs + 0.25) * WHISPER_RATE) as usize).min(samples.len());
                if to <= from {
                    continue;
                }
                let window = samples[from..to].to_vec();
                let started = crate::clock::Instant::now();
                let pieces = backend.transcribe(&window, "en").expect("pass 2");
                eprintln!(
                    "    pass2 [{:.2}..{:.2}] {:.2}s wall -> {} piece(s): {}",
                    line.start_secs,
                    line.end_secs,
                    started.elapsed().as_secs_f64(),
                    pieces.len(),
                    pieces
                        .iter()
                        .map(|(a, b, t)| format!(
                            "{:.2}..{:.2} {:?}",
                            *a as f64 / 1000.0,
                            *b as f64 / 1000.0,
                            t.trim()
                        ))
                        .collect::<Vec<_>>()
                        .join(" | "),
                );
            }
        }
        eprintln!(
            "  lyrics: {} raw segments -> {} lines in {:.1}s \
             (read {:.1}s, resample {:.1}s, model load {:.1}s, whisper {:.1}s) via {}",
            segments.len(),
            lines.len(),
            started.elapsed().as_secs_f64(),
            read_secs,
            prep_secs - read_secs,
            load_secs - prep_secs,
            transcribe_secs - load_secs,
            backend.name(),
        );
        eprintln!(
            "  onset snap: {} of {} lines, mean {:.0} ms, max {:.0} ms",
            onset.snapped,
            lines.len(),
            onset.mean_ms,
            onset.max_ms
        );
        assert!(!lines.is_empty(), "no lyrics came back");

        let lyrics = TrackLyrics {
            backend: backend.name().to_string(),
            model: backend.model().to_string(),
            language: "en".to_string(),
            duration_secs: duration,
            onset,
            lines: lines.clone(),
        };
        store_cached(&cache_dir(), &digest, &lyrics);
        eprintln!("  cached at {}", cache_path(&cache_dir(), &digest).display());

        // 3. What the screen would do with it, on this track's own grid.
        let analysis = crate::wave_analysis::analyze(&decoded);
        let timing = KaraokeTiming::from_grid(Some(&analysis.grid));
        eprintln!(
            "  grid: {:.1} BPM -> lead-in {:.2}s, hold {:.2}s",
            analysis.grid.bpm, timing.lead_in_secs, timing.hold_secs
        );
        let schedule = KaraokeSchedule::build(lines, timing);
        eprintln!("  first 12 lines (sung / bright row up / first on screen):");
        for index in 0..schedule.lines.len().min(12) {
            let line = &schedule.lines[index];
            eprintln!(
                "    {:7.2}s  {:7.2}s  {:7.2}s (lead {:5.2}s)  {}",
                line.start_secs,
                schedule.appear[index],
                schedule.on_screen_at(index),
                line.start_secs - schedule.on_screen_at(index),
                line.text,
            );
        }
        let mut bright_lead = Vec::new();
        for index in 0..schedule.lines.len() {
            let start = schedule.lines[index].start_secs;
            bright_lead.push(start - schedule.appear[index]);
            // A line sung before the lead-in has even elapsed goes up as early
            // as the track allows, which is time zero — there is no earlier.
            let wanted = (start - timing.lead_in_secs).max(0.0);
            assert!(
                schedule.on_screen_at(index) <= wanted + 1e-6,
                "line {index} ({}) reached the screen at {:.2}s for a {:.2}s start",
                schedule.lines[index].text,
                schedule.on_screen_at(index),
                start,
            );
        }
        let worst = bright_lead.iter().cloned().fold(f64::INFINITY, f64::min);
        let mean = bright_lead.iter().sum::<f64>() / bright_lead.len() as f64;
        eprintln!(
            "  bright-row lead over {} lines: mean {:.2}s, worst {:.2}s (target {:.2}s)",
            bright_lead.len(),
            mean,
            worst,
            timing.lead_in_secs,
        );
    }

    #[test]
    fn the_lyrics_follow_the_deck_the_room_is_hearing() {
        // One deck playing wins whatever the fader says — the other is cued.
        assert_eq!(live_deck(1.0, true, false), DeckId::A);
        assert_eq!(live_deck(0.0, false, true), DeckId::B);
        // Both playing: the fader decides, and the centre favours A.
        assert_eq!(live_deck(0.2, true, true), DeckId::A);
        assert_eq!(live_deck(0.5, true, true), DeckId::A);
        assert_eq!(live_deck(0.8, true, true), DeckId::B);
        // Neither playing: still the fader, so a paused deck shows its words.
        assert_eq!(live_deck(0.9, false, false), DeckId::B);
    }
}
