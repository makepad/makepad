//! What a person reads while a model works: one stage label, one bar, one
//! time left. The hub reports a raw stage string ("queued-for-memory: waiting
//! for 2560 MB free", "load unet 8.2/23.8GB", "denoise 12/50") and a 0..1
//! progress on every job; this turns that pair into the same words and the
//! same ETA in Stage's generate UI, the AI chat panel and the code app.
//!
//! The ETA is measured, not guessed: within one stage the fraction's growth
//! over the stage's elapsed time gives a rate, and the time left is what is
//! left of the stage at that rate. A stage that has not run a second or moved
//! 2 % gives no ETA (a number that jumps is worse than none). A new stage
//! starts its own clock, so "loading weights" never promises the length of
//! "generating".

use std::time::{Duration, Instant};

/// The words for a raw hub stage, by what the stage is for.
pub fn friendly_stage(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    let label = if lower.trim().is_empty() {
        "Working"
    } else if has(&["queued-for-memory", "making room", "vram-release", "waiting for"]) && has(&["mb free", "memory", "vram"]) {
        "Making room on the GPU"
    } else if has(&["evict", "unload"]) {
        "Unloading the resident model"
    } else if has(&["listening for the fleet", "looking for"]) {
        "Finding the AI node"
    } else if has(&["queued", "waiting in"]) {
        "Waiting in the queue"
    } else if has(&["download", "fetch"]) {
        "Fetching weights"
    } else if has(&["warm", "compil", "prefill", "reading new input", "ingest"]) {
        "Warming up"
    } else if has(&["text-encode", "tokeniz", "prompt"]) {
        "Reading the prompt"
    } else if has(&["decod", "vocoder", "vae", "encode", "mux", "save", "finaliz"]) {
        "Decoding the result"
    } else if has(&["load", "starting"]) {
        "Loading weights"
    } else if has(&["denois", "dit ", "sampl", "step", "synth", "generat", "diffus", "tokens", "think", "writ"]) {
        "Generating"
    } else {
        return capitalise(raw.trim());
    };
    label.to_string()
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// One reading of a job, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressView {
    /// The stage in plain words.
    pub stage: String,
    /// 0..=1, or `None` when the stage reports no progress (draw the sweep).
    pub fraction: Option<f64>,
    /// Seconds left in this stage, when measured.
    pub eta: Option<Duration>,
    /// Seconds since the job's first reading.
    pub elapsed: Duration,
}

impl ProgressView {
    /// `Loading weights · 42% · about 12 s left`.
    pub fn line(&self) -> String {
        let mut line = self.stage.clone();
        if let Some(f) = self.fraction {
            line.push_str(&format!(" · {:.0}%", (f * 100.0).clamp(0.0, 100.0)));
        }
        line.push_str(&self.time_note());
        line
    }

    /// The stage and the time, without the percentage, for a place that
    /// draws the bar itself: `Loading weights · about 12 s left`.
    pub fn detail(&self) -> String {
        format!("{}{}", self.stage, self.time_note())
    }

    fn time_note(&self) -> String {
        let mut line = String::new();
        if let Some(eta) = self.eta {
            line.push_str(&format!(" · about {} left", human(eta)));
        } else if self.elapsed >= Duration::from_secs(3) {
            line.push_str(&format!(" · {} so far", human(self.elapsed)));
        }
        line
    }
}

fn human(d: Duration) -> String {
    let s = d.as_secs_f64().round() as u64;
    if s < 90 {
        format!("{} s", s.max(1))
    } else if s < 5400 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{:.1} h", s as f64 / 3600.0)
    }
}

/// Feeds on `(raw stage, fraction)` readings, answers with a [`ProgressView`].
#[derive(Debug, Default)]
pub struct ProgressTracker {
    job_start: Option<Instant>,
    stage_key: String,
    stage_start: Option<Instant>,
    stage_first: f64,
    last: Option<ProgressView>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the job (it finished or a new one begins).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The reading shown now, if any job is being tracked.
    pub fn current(&self) -> Option<&ProgressView> {
        self.last.as_ref()
    }

    /// One reading. `fraction` is the stage's own progress (0..=1); a
    /// negative or NaN value means the stage reports none.
    pub fn observe(&mut self, raw_stage: &str, fraction: f64, now: Instant) -> ProgressView {
        let job_start = *self.job_start.get_or_insert(now);
        let stage = friendly_stage(raw_stage);
        let known = fraction.is_finite() && fraction >= 0.0;
        let f = if known { fraction.min(1.0) } else { 0.0 };
        if stage != self.stage_key || self.stage_start.is_none() || f + 1e-9 < self.stage_first {
            self.stage_key = stage.clone();
            self.stage_start = Some(now);
            self.stage_first = f;
        }
        let stage_elapsed = now.duration_since(self.stage_start.unwrap_or(now)).as_secs_f64();
        let moved = f - self.stage_first;
        let eta = (known && f < 1.0 && stage_elapsed >= 1.0 && moved >= 0.02).then(|| {
            let rate = moved / stage_elapsed;
            Duration::from_secs_f64(((1.0 - f) / rate).min(99.0 * 3600.0))
        });
        let view = ProgressView {
            stage,
            fraction: known.then_some(f),
            eta,
            elapsed: now.duration_since(job_start),
        };
        self.last = Some(view.clone());
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_stages_read_in_plain_words() {
        let cases = [
            ("queued-for-memory: waiting for 2560 MB free (have 1255 MB)", "Making room on the GPU"),
            ("listening for the fleet", "Finding the AI node"),
            ("load unet 8.2/23.8GB", "Loading weights"),
            ("denoise 12/50", "Generating"),
            ("synth 1/1", "Generating"),
            ("DiT 18/30 w2/3", "Generating"),
            ("text-encode", "Reading the prompt"),
            ("vocoder w3/3", "Decoding the result"),
            ("encode", "Decoding the result"),
            ("reading new input", "Warming up"),
            ("queued", "Waiting in the queue"),
            ("polishing", "Polishing"),
            ("", "Working"),
        ];
        for (raw, want) in cases {
            assert_eq!(friendly_stage(raw), want, "{raw}");
        }
    }

    #[test]
    fn eta_comes_from_the_stage_rate_and_restarts_with_each_stage() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut tracker = ProgressTracker::new();
        assert_eq!(tracker.observe("load weights", 0.0, at(0.0)).eta, None);
        // 0.5 s in: too early for a number.
        assert_eq!(tracker.observe("load weights", 0.1, at(0.5)).eta, None);
        // 10 s, 50 %: 10 s more.
        let v = tracker.observe("load weights", 0.5, at(10.0));
        assert_eq!(v.eta.map(|d| d.as_secs()), Some(10));
        assert_eq!(v.line(), "Loading weights · 50% · about 10 s left");
        // The next stage has its own clock.
        let v = tracker.observe("denoise 1/50", 0.02, at(11.0));
        assert_eq!((v.stage.as_str(), v.eta), ("Generating", None));
        let v = tracker.observe("denoise 25/50", 0.5, at(41.0));
        assert_eq!(v.eta.map(|d| d.as_secs()), Some(31));
        // A stage with no progress shows time so far, not a bar.
        let v = tracker.observe("making room", -1.0, at(50.0));
        assert_eq!(v.fraction, None);
        assert!(v.line().ends_with("so far"), "{}", v.line());
    }

    #[test]
    fn a_bar_never_promises_past_the_end() {
        let t0 = Instant::now();
        let mut tracker = ProgressTracker::new();
        tracker.observe("denoise", 0.1, t0);
        let v = tracker.observe("denoise", 1.0, t0 + Duration::from_secs(20));
        assert_eq!((v.fraction, v.eta), (Some(1.0), None));
    }
}
