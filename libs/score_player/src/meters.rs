//! What a score's live displays read: the clock and each node's level,
//! recent output and last note settings, from whatever plays the graph
//! (Stage's audio engine through its probes, a game's). Node keys are
//! `makepad_score_live::key(name)`.

pub trait Meters: Send + Sync {
    fn playing(&self) -> bool;
    /// The beat at the output.
    fn beat(&self) -> f64;
    fn bpm(&self) -> f64;
    fn rate(&self) -> f32;
    /// `(peak, rms)` of the node with `key`.
    fn level(&self, key: u64) -> Option<(f32, f32)>;
    /// The node's most recent output (mono) into `out`; false when unknown.
    fn node_scope(&self, key: u64, out: &mut [f32]) -> bool;
    /// A loop's last note settings: cutoff, res, attack, decay, sustain,
    /// release, drive (NaN where the patch played).
    fn last_opts(&self, key: u64) -> Option<[f32; 7]>;
    /// An FX node's gain reduction now, dB (≤ 0; a Comp's), when known.
    fn gr(&self, _key: u64) -> Option<f32> {
        None
    }
}
