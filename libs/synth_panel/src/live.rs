//! Where the live displays read from: the audio engine's meters and
//! scopes while a score plays (an instrument through the loops that play
//! it). Displays that show fields (wave, filter, envelope) need none.

use makepad_score_live::key;
use makepad_score_player::Meters;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-note values a loop last played with, in `Meters::last_opts`
/// order.
pub const OPT_KEYS: [&str; 7] = ["cutoff", "res", "attack", "decay", "sustain", "release", "drive"];

pub trait LiveSource {
    fn playing(&self) -> bool;
    /// The musical position, in beats.
    fn beat(&self) -> f64;
    fn rate(&self) -> f32;
    /// The newest samples of a node (an instrument through the loops that
    /// play it, an effect, a loop), oldest first; false when there are none.
    fn scope(&self, node: &str, out: &mut [f32]) -> bool;
    /// `(peak, rms)` of a node.
    fn level(&self, node: &str) -> Option<(f32, f32)>;
    /// What the node's notes last overrode (NaN: the patch value).
    fn note_opts(&self, node: &str) -> Option<[f32; 7]>;
    /// A compressor's gain reduction now, in dB (0 or below).
    fn gr(&self, node: &str) -> Option<f32>;
}

/// The engine's meters, with the score's instrument-to-loop map (a loop
/// owns the instrument instance that plays its notes).
pub struct TelemetrySource {
    pub telemetry: Arc<dyn Meters>,
    pub loops_of: HashMap<String, Vec<String>>,
}

impl TelemetrySource {
    /// The nodes whose meters stand for `node`: its loops for an
    /// instrument, else itself.
    fn keys(&self, node: &str) -> Vec<u64> {
        match self.loops_of.get(node) {
            Some(loops) if !loops.is_empty() => loops.iter().map(|l| key(l)).collect(),
            _ => vec![key(node)],
        }
    }
}

impl LiveSource for TelemetrySource {
    fn playing(&self) -> bool {
        self.telemetry.playing()
    }

    fn beat(&self) -> f64 {
        self.telemetry.beat()
    }

    fn rate(&self) -> f32 {
        self.telemetry.rate()
    }

    fn scope(&self, node: &str, out: &mut [f32]) -> bool {
        let keys = self.keys(node);
        let mut any = false;
        let mut tmp = vec![0.0; out.len()];
        out.fill(0.0);
        for k in keys {
            if self.telemetry.node_scope(k, &mut tmp) {
                any = true;
                for (o, t) in out.iter_mut().zip(&tmp) {
                    *o += *t;
                }
            }
        }
        any
    }

    fn level(&self, node: &str) -> Option<(f32, f32)> {
        self.keys(node).into_iter().filter_map(|k| self.telemetry.level(k)).reduce(|a, b| (a.0.max(b.0), (a.1 * a.1 + b.1 * b.1).sqrt()))
    }

    fn gr(&self, node: &str) -> Option<f32> {
        self.telemetry.gr(key(node))
    }

    fn note_opts(&self, node: &str) -> Option<[f32; 7]> {
        self.keys(node).into_iter().find_map(|k| self.telemetry.last_opts(k))
    }
}
