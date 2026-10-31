//! Per-app cold/warm timing metadata. The boot document owns the weighted target
//! and easing; this module only reports measured work and previous phase durations.
use relax_compiler::run_state::{Phase, State};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone)]
pub struct History {
    pub path: PathBuf,
    cold: [f64; 6],
    warm: [f64; 6],
}

impl History {
    pub fn load(path: PathBuf) -> Self {
        let mut history = Self {
            path,
            cold: [1.0; 6],
            warm: [1.0; 6],
        };
        if let Ok(text) = std::fs::read_to_string(&history.path) {
            let mut lines = text.lines();
            if lines.next() == Some("makepad-run-boot-timings 1") {
                for dst in [&mut history.cold, &mut history.warm] {
                    if let Some(values) = lines.next().and_then(|line| {
                        let v: Vec<f64> = line
                            .split_whitespace()
                            .map(str::parse)
                            .collect::<Result<_, _>>()
                            .ok()?;
                        let v: [f64; 6] = v.try_into().ok()?;
                        (v.iter().all(|v| v.is_finite() && *v >= 0.0)
                            && v.iter().sum::<f64>() > 0.0)
                            .then_some(v)
                    }) {
                        *dst = values;
                    }
                }
            }
        }
        history
    }
    pub fn record(&mut self, warm: bool, durations: [f64; 6]) -> String {
        *if warm { &mut self.warm } else { &mut self.cold } = durations;
        format!(
            "makepad-run-boot-timings 1\n{}\n{}\n",
            self.cold.map(|v| v.to_string()).join(" "),
            self.warm.map(|v| v.to_string()).join(" ")
        )
    }
}

pub fn save(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pending = path.with_extension("pending");
    std::fs::write(&pending, contents)?;
    std::fs::rename(pending, path)
}

pub struct Progress {
    pub history: History,
    began: Instant,
    entered: Instant,
    phase: Phase,
    measured: [f64; 6],
    complete: [bool; 6],
}

impl Progress {
    pub fn new(history: History) -> Self {
        let now = Instant::now();
        Self {
            history,
            began: now,
            entered: now,
            phase: Phase::Resolve,
            measured: [0.0; 6],
            complete: [false; 6],
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new(self.history.clone());
    }
    pub fn phase(&mut self, phase: Phase) {
        if self.phase != phase {
            let now = Instant::now();
            self.measured[self.phase as usize] += now.duration_since(self.entered).as_secs_f64();
            self.complete[self.phase as usize] = true;
            self.complete[phase as usize] = false;
            self.entered = now;
            self.phase = phase;
        }
    }
    pub fn tick(&mut self, state: &mut State) {
        state.elapsed = self.began.elapsed().as_secs_f64();
        let weights = if state.warm {
            self.history.warm
        } else {
            self.history.cold
        };
        state.total_weight = weights.iter().sum();
        state.phase_weights = weights;
        state.phase_weight = weights[self.phase as usize];
        state.phase_elapsed = self.entered.elapsed().as_secs_f64();
        state.completed_weight = weights
            .iter()
            .zip(self.complete)
            .filter_map(|(weight, done)| done.then_some(*weight))
            .sum();
    }
    pub fn finish(&mut self, warm: bool) -> (PathBuf, String) {
        self.measured[self.phase as usize] += self.entered.elapsed().as_secs_f64();
        (
            self.history.path.clone(),
            self.history.record(warm, self.measured),
        )
    }
}
