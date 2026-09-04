//! Fixed program buses and the post-master dynamics pass.
//!
//! Every strip is audio-thread owned. Mute and solo move a smoothed listen
//! gate, so the UI never rewrites other strips and no state transition clicks.

pub const STRIP_COUNT: usize = 7;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum StripId {
    #[default]
    Video = 0,
    DjA = 1,
    DjB = 2,
    Sfx = 3,
    Piano = 4,
    Ironfish = 5,
    Drums = 6,
}

impl StripId {
    pub const ALL: [Self; STRIP_COUNT] = [
        Self::Video,
        Self::DjA,
        Self::DjB,
        Self::Sfx,
        Self::Piano,
        Self::Ironfish,
        Self::Drums,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Video => "VIDEO",
            Self::DjA => "DJ A",
            Self::DjB => "DJ B",
            Self::Sfx => "SFX",
            Self::Piano => "PIANO",
            Self::Ironfish => "IRONFISH",
            Self::Drums => "DRUMS",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StripSnapshot {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
    pub gain: f32,
    pub muted: bool,
    pub soloed: bool,
    pub audible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MasterParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_secs: f32,
    pub release_secs: f32,
    pub makeup_db: f32,
    pub ceiling_db: f32,
    pub bypass: bool,
}

impl Default for MasterParams {
    fn default() -> Self {
        Self {
            threshold_db: -10.0,
            ratio: 3.0,
            attack_secs: 0.012,
            release_secs: 0.18,
            makeup_db: 0.0,
            ceiling_db: -0.5,
            bypass: false,
        }
    }
}

impl MasterParams {
    fn sanitise(mut self) -> Self {
        self.threshold_db = finite(self.threshold_db, -10.0).clamp(-48.0, 0.0);
        self.ratio = finite(self.ratio, 3.0).clamp(1.0, 40.0);
        self.attack_secs = finite(self.attack_secs, 0.012).clamp(0.0002, 1.0);
        self.release_secs = finite(self.release_secs, 0.18).clamp(0.005, 4.0);
        self.makeup_db = finite(self.makeup_db, 0.0).clamp(-12.0, 18.0);
        self.ceiling_db = finite(self.ceiling_db, -0.5).clamp(-18.0, 0.0);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasterParam {
    Threshold,
    Ratio,
    Attack,
    Release,
    Makeup,
    Ceiling,
}

impl MasterParams {
    pub fn set_normalised(&mut self, param: MasterParam, value: f32) {
        let v = finite(value, 0.0).clamp(0.0, 1.0);
        match param {
            MasterParam::Threshold => self.threshold_db = -42.0 + v * 42.0,
            MasterParam::Ratio => self.ratio = 1.0 + v * v * 19.0,
            MasterParam::Attack => self.attack_secs = 0.001 + v * v * 0.199,
            MasterParam::Release => self.release_secs = 0.02 + v * v * 0.98,
            MasterParam::Makeup => self.makeup_db = v * 12.0,
            MasterParam::Ceiling => self.ceiling_db = -6.0 + v * 6.0,
        }
        *self = self.sanitise();
    }

    pub fn normalised(self, param: MasterParam) -> f32 {
        match param {
            MasterParam::Threshold => ((self.threshold_db + 42.0) / 42.0).clamp(0.0, 1.0),
            MasterParam::Ratio => ((self.ratio - 1.0) / 19.0).clamp(0.0, 1.0).sqrt(),
            MasterParam::Attack => ((self.attack_secs - 0.001) / 0.199).clamp(0.0, 1.0).sqrt(),
            MasterParam::Release => ((self.release_secs - 0.02) / 0.98).clamp(0.0, 1.0).sqrt(),
            MasterParam::Makeup => (self.makeup_db / 12.0).clamp(0.0, 1.0),
            MasterParam::Ceiling => ((self.ceiling_db + 6.0) / 6.0).clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MasterSnapshot {
    pub compressor_reduction_db: f32,
    pub limiter_reduction_db: f32,
    pub peak: f32,
}

fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

#[derive(Clone, Copy)]
struct Smooth {
    current: f32,
    target: f32,
}

impl Smooth {
    fn new(value: f32) -> Self {
        Self { current: value, target: value }
    }

    fn set(&mut self, value: f32) {
        self.target = finite(value, 0.0);
    }

    fn next(&mut self, rate: f32) -> f32 {
        let pole = (1.0 / (0.008 * rate.max(1.0))).min(1.0);
        self.current += (self.target - self.current) * pole;
        self.current
    }
}

struct Strip {
    gain: Smooth,
    listen: Smooth,
    muted: bool,
    soloed: bool,
    peak: [f32; 2],
    square_sum: [f64; 2],
    frames: usize,
}

impl Strip {
    fn new() -> Self {
        Self {
            gain: Smooth::new(1.0),
            listen: Smooth::new(1.0),
            muted: false,
            soloed: false,
            peak: [0.0; 2],
            square_sum: [0.0; 2],
            frames: 0,
        }
    }

    fn begin_block(&mut self) {
        self.peak = [0.0; 2];
        self.square_sum = [0.0; 2];
        self.frames = 0;
    }

    fn process(&mut self, input: [f32; 2], rate: f32) -> [f32; 2] {
        let gain = self.gain.next(rate) * self.listen.next(rate);
        let out = [input[0] * gain, input[1] * gain];
        self.peak[0] = self.peak[0].max(out[0].abs());
        self.peak[1] = self.peak[1].max(out[1].abs());
        self.square_sum[0] += (out[0] as f64) * (out[0] as f64);
        self.square_sum[1] += (out[1] as f64) * (out[1] as f64);
        self.frames += 1;
        out
    }

    fn snapshot(&self, any_solo: bool) -> StripSnapshot {
        let denom = self.frames.max(1) as f64;
        StripSnapshot {
            peak_l: self.peak[0],
            peak_r: self.peak[1],
            rms_l: (self.square_sum[0] / denom).sqrt() as f32,
            rms_r: (self.square_sum[1] / denom).sqrt() as f32,
            gain: self.gain.target,
            muted: self.muted,
            soloed: self.soloed,
            audible: !self.muted && (!any_solo || self.soloed),
        }
    }
}

struct MasterFx {
    params: MasterParams,
    envelope: f32,
    compressor_reduction_db: f32,
    limiter_reduction_db: f32,
    peak: f32,
    makeup: Smooth,
}

impl MasterFx {
    fn new() -> Self {
        let params = MasterParams::default();
        Self {
            params,
            envelope: 0.0,
            compressor_reduction_db: 0.0,
            limiter_reduction_db: 0.0,
            peak: 0.0,
            makeup: Smooth::new(db_to_gain(params.makeup_db)),
        }
    }

    fn set_params(&mut self, params: MasterParams) {
        self.params = params.sanitise();
        self.makeup.set(db_to_gain(self.params.makeup_db));
    }

    fn set_param(&mut self, param: MasterParam, value: f32) {
        self.params.set_normalised(param, value);
        self.makeup.set(db_to_gain(self.params.makeup_db));
    }

    fn begin_block(&mut self) {
        self.compressor_reduction_db = 0.0;
        self.limiter_reduction_db = 0.0;
        self.peak = 0.0;
    }

    fn process(&mut self, input: [f32; 2], rate: f32) -> [f32; 2] {
        if self.params.bypass {
            self.peak = self.peak.max(input[0].abs()).max(input[1].abs());
            return input;
        }
        let detector = input[0].abs().max(input[1].abs());
        let attack = coeff(self.params.attack_secs, rate);
        let release = coeff(self.params.release_secs, rate);
        let coefficient = if detector > self.envelope { attack } else { release };
        self.envelope += (detector - self.envelope) * coefficient;
        let level_db = gain_to_db(self.envelope.max(1e-9));
        let over = level_db - self.params.threshold_db;
        let knee = 6.0;
        let compressed_over = if over <= -knee * 0.5 {
            0.0
        } else if over >= knee * 0.5 {
            over * (1.0 - 1.0 / self.params.ratio)
        } else {
            let x = over + knee * 0.5;
            x * x / (2.0 * knee) * (1.0 - 1.0 / self.params.ratio)
        };
        let comp_gain = db_to_gain(-compressed_over) * self.makeup.next(rate);
        self.compressor_reduction_db = self.compressor_reduction_db.max(compressed_over);
        let mut out = [input[0] * comp_gain, input[1] * comp_gain];
        let ceiling = db_to_gain(self.params.ceiling_db);
        let peak = out[0].abs().max(out[1].abs());
        if peak > ceiling {
            let limiter_gain = ceiling / peak.max(1e-12);
            out[0] *= limiter_gain;
            out[1] *= limiter_gain;
            self.limiter_reduction_db = self
                .limiter_reduction_db
                .max(-gain_to_db(limiter_gain).min(0.0));
        }
        // The clamp is the limiter's numerical seat belt, not its transfer
        // curve. It also contains a non-finite third-party source.
        for sample in &mut out {
            *sample = if sample.is_finite() { sample.clamp(-ceiling, ceiling) } else { 0.0 };
        }
        self.peak = self.peak.max(out[0].abs()).max(out[1].abs());
        out
    }

    fn snapshot(&self) -> MasterSnapshot {
        MasterSnapshot {
            compressor_reduction_db: self.compressor_reduction_db,
            limiter_reduction_db: self.limiter_reduction_db,
            peak: self.peak,
        }
    }
}

fn coeff(seconds: f32, rate: f32) -> f32 {
    1.0 - (-1.0 / (seconds.max(1e-5) * rate.max(1.0))).exp()
}

fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-12).log10()
}

pub struct ProgramMix {
    strips: [Strip; STRIP_COUNT],
    master: MasterFx,
}

impl ProgramMix {
    pub fn new() -> Self {
        Self {
            strips: std::array::from_fn(|_| Strip::new()),
            master: MasterFx::new(),
        }
    }

    fn any_solo(&self) -> bool {
        self.strips.iter().any(|strip| strip.soloed)
    }

    fn update_listen_targets(&mut self) {
        let any_solo = self.any_solo();
        for strip in &mut self.strips {
            strip.listen.set(if !strip.muted && (!any_solo || strip.soloed) { 1.0 } else { 0.0 });
        }
    }

    pub fn set_gain(&mut self, id: StripId, gain: f32) {
        self.strips[id.index()].gain.set(finite(gain, 0.0).clamp(0.0, 1.5));
    }

    pub fn set_muted(&mut self, id: StripId, muted: bool) {
        self.strips[id.index()].muted = muted;
        self.update_listen_targets();
    }

    pub fn set_soloed(&mut self, id: StripId, soloed: bool) {
        self.strips[id.index()].soloed = soloed;
        self.update_listen_targets();
    }

    pub fn set_master_params(&mut self, params: MasterParams) {
        self.master.set_params(params);
    }

    pub fn set_master_param(&mut self, param: MasterParam, value: f32) {
        self.master.set_param(param, value);
    }

    pub fn set_master_bypass(&mut self, bypass: bool) {
        self.master.params.bypass = bypass;
    }

    pub fn master_params(&self) -> MasterParams {
        self.master.params
    }

    pub fn begin_block(&mut self) {
        for strip in &mut self.strips {
            strip.begin_block();
        }
        self.master.begin_block();
    }

    pub fn process_frame(
        &mut self,
        sources: [[f32; 2]; STRIP_COUNT],
        master_gain: f32,
        rate: f32,
    ) -> [f32; 2] {
        let mut sum = [0.0f32; 2];
        for (strip, input) in self.strips.iter_mut().zip(sources) {
            let out = strip.process(input, rate);
            sum[0] += out[0];
            sum[1] += out[1];
        }
        let master_gain = finite(master_gain, 0.0).clamp(0.0, 1.5);
        self.master
            .process([sum[0] * master_gain, sum[1] * master_gain], rate)
    }

    pub fn strip_snapshots(&self) -> [StripSnapshot; STRIP_COUNT] {
        let any_solo = self.any_solo();
        std::array::from_fn(|index| self.strips[index].snapshot(any_solo))
    }

    pub fn master_snapshot(&self) -> MasterSnapshot {
        self.master.snapshot()
    }
}

impl Default for ProgramMix {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_is_a_listen_mask_and_never_rewrites_mute() {
        let mut mix = ProgramMix::new();
        mix.set_muted(StripId::Video, true);
        mix.set_soloed(StripId::Piano, true);
        let solo = mix.strip_snapshots();
        assert!(!solo[StripId::Video.index()].audible);
        assert!(solo[StripId::Piano.index()].audible);
        assert!(!solo[StripId::Drums.index()].audible);
        mix.set_soloed(StripId::Piano, false);
        let clear = mix.strip_snapshots();
        assert!(clear[StripId::Video.index()].muted);
        assert!(!clear[StripId::Video.index()].audible);
        assert!(clear[StripId::Drums.index()].audible);
    }

    #[test]
    fn limiter_contains_hot_and_non_finite_sources() {
        let mut mix = ProgramMix::new();
        mix.begin_block();
        for _ in 0..4096 {
            let mut sources = [[0.0; 2]; STRIP_COUNT];
            sources[0] = [4.0, -4.0];
            let out = mix.process_frame(sources, 1.0, 48_000.0);
            assert!(out[0].abs() <= 1.0 && out[1].abs() <= 1.0);
            assert!(out[0].is_finite() && out[1].is_finite());
        }
        let mut sources = [[0.0; 2]; STRIP_COUNT];
        sources[0] = [f32::NAN, f32::INFINITY];
        let out = mix.process_frame(sources, 1.0, 48_000.0);
        assert_eq!(out, [0.0, 0.0]);
    }
}
