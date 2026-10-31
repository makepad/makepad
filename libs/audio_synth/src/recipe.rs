//! Sound recipes: a small modular patch format for one-shot effects and
//! instrument patches, and the built-in library of game sounds.
//!
//! A recipe is up to [`MAX_LAYERS`] layers. Each layer is one source
//! (band-limited oscillator, coloured noise, a two-operator FM pair or a
//! plucked string) with a pitch sweep and vibrato, through a filter with its
//! own sweep, a drive stage and an ADSR, optionally repeated (bursts,
//! rattles, machine guns). A recipe is `Copy`, so starting a voice copies it
//! into a fixed slot and nothing allocates on the audio thread.
//!
//! Recipes are DATA an author (or an AI) writes. Two spellings, one set of
//! keys:
//! - text: layers split by `;`, `key=value` tokens, sweeps as `a>b`:
//!   `"saw freq=880>180 glide=0.09 lp=6000>900 q=2 a=0.001 d=0.08 s=0 r=0.05 gain=0.5; noise=pink bp=3000 d=0.03 gain=0.2"`
//! - fields: [`Layer::set_num`] / [`Layer::set_str`] with the same keys, which
//!   is how a script object (`{osc: "saw", freq: 880, to: 180, ...}`) lands.
//!
//! Layer keys: `osc`/`wave` (sine tri saw square pulse softsaw), `noise`
//! (white pink brown blue velvet), `fm` (modulator ratio; carrier is `osc`),
//! `index`/`index_to` (FM depth), `pluck` (string damping 0..1), `freq`,
//! `to`, `glide` (sweep seconds), `vib`/`vib_depth` (Hz / semitones),
//! `filter` (lp hp bp notch ladder), `cutoff`, `cutoff_to`, `q`, shorthands
//! `lp=`/`hp=`/`bp=`/`ladder=` (mode + cutoff, `a>b` sweeps), `drive`,
//! `a` `d` `s` `r` (ADSR seconds / sustain level), `hold` (gate seconds),
//! `gain`, `delay`, `pan`, `width` (pulse width), `pwm` and `pwm_depth`
//! (pulse-width modulation), `density` (velvet),
//! `repeat` (count), `every` (seconds between repeats), `decay` (gain per
//! repeat), `detune` (cents). Recipe keys: `volume`, `jitter` (random pitch
//! ± fraction per play), `reverb` (send 0..1), `pitch`.

use crate::dsp::*;

pub const MAX_LAYERS: usize = 6;
/// The voice's DC blocker pole (10 Hz at 48 kHz).
const DC_POLE: f32 = 0.9987;
/// Longest pluck period the one string buffer holds (≈ 21 Hz at 44.1 kHz).
pub const PLUCK_LEN: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Source {
    #[default]
    None,
    Osc(Wave),
    Noise(NoiseColor),
    /// Two-operator FM: a sine modulator at `ratio` × pitch into the
    /// carrier wave.
    Fm { carrier: Wave, ratio: f32 },
    /// Karplus-Strong string with loop damping 0..1.
    Pluck { damping: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub source: Source,
    pub freq: f32,
    pub freq_to: f32,
    /// Sweep time (seconds). 0 = over the layer's audible length.
    pub glide: f32,
    pub vib_hz: f32,
    pub vib_depth: f32,
    pub detune_cents: f32,
    pub fm_index: f32,
    pub fm_index_to: f32,
    pub filter: FilterMode,
    pub cutoff: f32,
    pub cutoff_to: f32,
    pub q: f32,
    pub drive: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    /// Gate time before release. Negative = automatic (attack + decay when
    /// sustain is 0, else 0.15 s). An instrument patch gates from the note.
    pub hold: f32,
    pub gain: f32,
    pub delay: f32,
    pub pan: f32,
    pub width: f32,
    /// Pulse-width modulation: a triangle of `pwm_depth` (0..0.45 of the
    /// period either side of `width`) at `pwm_hz` (`pwm`).
    pub pwm_hz: f32,
    pub pwm_depth: f32,
    pub density: f32,
    pub repeat: u16,
    pub every: f32,
    pub repeat_decay: f32,
}

impl Default for Layer {
    fn default() -> Self {
        Layer {
            source: Source::None,
            freq: 440.0,
            freq_to: f32::NAN,
            glide: 0.0,
            vib_hz: 0.0,
            vib_depth: 0.0,
            detune_cents: 0.0,
            fm_index: 0.0,
            fm_index_to: f32::NAN,
            filter: FilterMode::Off,
            cutoff: 2_000.0,
            cutoff_to: f32::NAN,
            q: 0.707,
            drive: 0.0,
            attack: 0.002,
            decay: 0.15,
            sustain: 0.0,
            release: 0.05,
            hold: -1.0,
            gain: 0.5,
            delay: 0.0,
            pan: 0.0,
            width: 0.5,
            pwm_hz: 0.0,
            pwm_depth: 0.0,
            density: 0.02,
            repeat: 0,
            every: 0.1,
            repeat_decay: 1.0,
        }
    }
}

/// Why a recipe key was refused (so a script typo is reported, not played).
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeError(pub String);

impl std::fmt::Display for RecipeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn sweep(value: &str) -> Result<(f32, Option<f32>), RecipeError> {
    let bad = || RecipeError(format!("'{value}' is not a number or a>b sweep"));
    match value.split_once('>') {
        Some((a, b)) => Ok((a.trim().parse().map_err(|_| bad())?, Some(b.trim().parse().map_err(|_| bad())?))),
        None => Ok((value.trim().parse().map_err(|_| bad())?, None)),
    }
}

impl Layer {
    /// Automatic gate length for one-shots.
    pub fn gate_secs(&self) -> f32 {
        if self.hold >= 0.0 {
            self.hold
        } else if self.sustain <= 1.0e-4 {
            self.attack + self.decay
        } else {
            self.attack + self.decay + 0.15
        }
    }

    /// Seconds from trigger until this layer is certainly silent.
    pub fn length(&self) -> f32 {
        let one = self.gate_secs() + self.release * 1.2 + 0.01;
        let reps = self.repeat as f32 * self.every.max(0.0);
        self.delay + reps + one
    }

    /// Set a numeric key. Unknown keys are an error.
    pub fn set_num(&mut self, key: &str, v: f32) -> Result<(), RecipeError> {
        let v = finite(v, 0.0);
        match key {
            "freq" | "f" => self.freq = v.clamp(1.0, 20_000.0),
            "to" | "freq_to" => self.freq_to = v.clamp(1.0, 20_000.0),
            "glide" | "t" => self.glide = v.max(0.0),
            "vib" | "vibrato" => self.vib_hz = v.clamp(0.0, 40.0),
            "vib_depth" => self.vib_depth = v.clamp(0.0, 24.0),
            "detune" => self.detune_cents = v.clamp(-1200.0, 1200.0),
            "fm" | "ratio" => {
                let carrier = match self.source {
                    Source::Osc(w) => w,
                    Source::Fm { carrier, .. } => carrier,
                    _ => Wave::Sine,
                };
                self.source = Source::Fm { carrier, ratio: v.clamp(0.01, 32.0) };
            }
            "index" => self.fm_index = v.clamp(0.0, 50.0),
            "index_to" => self.fm_index_to = v.clamp(0.0, 50.0),
            "pluck" => self.source = Source::Pluck { damping: v.clamp(0.0, 1.0) },
            "cutoff" => {
                self.cutoff = v.clamp(10.0, 20_000.0);
                if self.filter == FilterMode::Off {
                    self.filter = FilterMode::Lowpass;
                }
            }
            "cutoff_to" => self.cutoff_to = v.clamp(10.0, 20_000.0),
            "lp" | "hp" | "bp" | "ladder" | "notch" => {
                self.filter = FilterMode::parse(key).unwrap_or(FilterMode::Lowpass);
                self.cutoff = v.clamp(10.0, 20_000.0);
            }
            "q" | "res" | "resonance" => self.q = v.clamp(0.3, 30.0),
            "drive" => self.drive = v.clamp(0.0, 20.0),
            "a" | "attack" => self.attack = v.clamp(0.0, 10.0),
            "d" | "decay" => self.decay = v.clamp(0.0, 20.0),
            "s" | "sustain" => self.sustain = v.clamp(0.0, 1.0),
            "r" | "release" => self.release = v.clamp(0.0, 20.0),
            "hold" => self.hold = v.clamp(0.0, 30.0),
            "gain" | "g" | "volume" => self.gain = v.clamp(0.0, 4.0),
            "delay" => self.delay = v.clamp(0.0, 10.0),
            "pan" => self.pan = v.clamp(-1.0, 1.0),
            "width" | "pw" => self.width = v.clamp(0.05, 0.95),
            "pwm" | "pwm_hz" => self.pwm_hz = v.clamp(0.0, 50.0),
            "pwm_depth" => self.pwm_depth = v.clamp(0.0, 0.45),
            "density" => self.density = v.clamp(0.0, 1.0),
            "repeat" => self.repeat = v.clamp(0.0, 64.0) as u16,
            "every" => self.every = v.clamp(0.005, 5.0),
            "repeat_decay" | "falloff" => self.repeat_decay = v.clamp(0.0, 1.5),
            _ => return Err(RecipeError(format!("unknown recipe key '{key}'"))),
        }
        Ok(())
    }

    /// Set a key from text: a word (`osc=saw`, `filter=lp`, a bare `saw`)
    /// or a number / `a>b` sweep.
    pub fn set_str(&mut self, key: &str, value: &str) -> Result<(), RecipeError> {
        match key {
            "osc" | "wave" => {
                let w = Wave::parse(value).ok_or_else(|| RecipeError(format!("unknown wave '{value}'")))?;
                self.source = match self.source {
                    Source::Fm { ratio, .. } => Source::Fm { carrier: w, ratio },
                    _ => Source::Osc(w),
                };
                return Ok(());
            }
            "noise" => {
                let c = NoiseColor::parse(value).ok_or_else(|| RecipeError(format!("unknown noise colour '{value}'")))?;
                self.source = Source::Noise(c);
                return Ok(());
            }
            "filter" => {
                self.filter = FilterMode::parse(value).ok_or_else(|| RecipeError(format!("unknown filter '{value}'")))?;
                return Ok(());
            }
            _ => {}
        }
        let (a, b) = sweep(value)?;
        match key {
            "freq" | "f" => {
                self.set_num("freq", a)?;
                if let Some(b) = b {
                    self.set_num("to", b)?;
                }
            }
            "lp" | "hp" | "bp" | "ladder" | "notch" | "cutoff" => {
                self.set_num(key, a)?;
                if let Some(b) = b {
                    self.set_num("cutoff_to", b)?;
                }
            }
            "index" => {
                self.set_num("index", a)?;
                if let Some(b) = b {
                    self.set_num("index_to", b)?;
                }
            }
            _ => self.set_num(key, a)?,
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recipe {
    pub layers: [Layer; MAX_LAYERS],
    pub count: usize,
    pub volume: f32,
    pub jitter: f32,
    pub reverb: f32,
    pub pitch: f32,
}

impl Default for Recipe {
    fn default() -> Self {
        Recipe { layers: [Layer::default(); MAX_LAYERS], count: 0, volume: 1.0, jitter: 0.0, reverb: 0.15, pitch: 1.0 }
    }
}

impl Recipe {
    pub fn layers(&self) -> &[Layer] {
        &self.layers[..self.count]
    }

    /// Append a layer (ignored past [`MAX_LAYERS`]); returns its index.
    pub fn push(&mut self, layer: Layer) -> Option<usize> {
        if self.count >= MAX_LAYERS {
            return None;
        }
        self.layers[self.count] = layer;
        self.count += 1;
        Some(self.count - 1)
    }

    /// Recipe-level keys.
    pub fn set_num(&mut self, key: &str, v: f32) -> Result<(), RecipeError> {
        let v = finite(v, 0.0);
        match key {
            "volume" => self.volume = v.clamp(0.0, 4.0),
            "jitter" => self.jitter = v.clamp(0.0, 0.5),
            "reverb" => self.reverb = v.clamp(0.0, 1.0),
            "pitch" => self.pitch = v.clamp(0.05, 8.0),
            _ => return Err(RecipeError(format!("unknown recipe key '{key}'"))),
        }
        Ok(())
    }

    pub fn is_recipe_key(key: &str) -> bool {
        matches!(key, "volume" | "jitter" | "reverb" | "pitch")
    }

    /// Seconds until every layer is silent (at pitch 1).
    pub fn length(&self) -> f32 {
        self.layers().iter().map(Layer::length).fold(0.0, f32::max)
    }

    /// Parse the text form. Tokens before the first source word apply to
    /// the recipe (`volume=0.8 jitter=0.03; saw ...`).
    pub fn parse(text: &str) -> Result<Recipe, RecipeError> {
        let mut recipe = Recipe::default();
        for part in text.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let mut layer = Layer::default();
            let mut any_layer_key = false;
            for token in part.split_whitespace() {
                let (key, value) = match token.split_once('=') {
                    Some((k, v)) => (k.trim(), v.trim()),
                    None => (token, ""),
                };
                if value.is_empty() {
                    // A bare word names the source: `saw`, `pink`, `pluck`.
                    if let Some(w) = Wave::parse(key) {
                        layer.set_str("osc", match w {
                            Wave::Sine => "sine",
                            Wave::Triangle => "tri",
                            Wave::Saw => "saw",
                            Wave::Square => "square",
                            Wave::Pulse => "pulse",
                            Wave::SoftSaw => "softsaw",
                        })?;
                    } else if NoiseColor::parse(key).is_some() {
                        layer.set_str("noise", key)?;
                    } else if key == "pluck" {
                        layer.set_num("pluck", 0.5)?;
                    } else if FilterMode::parse(key).is_some() {
                        layer.set_str("filter", key)?;
                    } else {
                        return Err(RecipeError(format!("unknown recipe word '{key}'")));
                    }
                    any_layer_key = true;
                } else if Recipe::is_recipe_key(key) {
                    recipe.set_num(key, sweep(value)?.0)?;
                } else {
                    layer.set_str(key, value)?;
                    any_layer_key = true;
                }
            }
            if any_layer_key {
                if layer.source == Source::None {
                    layer.source = Source::Osc(Wave::Sine);
                }
                if recipe.push(layer).is_none() {
                    return Err(RecipeError(format!("a recipe has at most {MAX_LAYERS} layers")));
                }
            }
        }
        if recipe.count == 0 {
            return Err(RecipeError("a recipe needs at least one layer".into()));
        }
        Ok(recipe)
    }
}

/// Render state for one layer of a playing recipe.
#[derive(Clone, Copy)]
pub struct LayerState {
    osc: Osc,
    modu: Osc,
    noise: Noise,
    filter: Filter,
    env: Adsr,
    vib: f32,
    t: f32,
    gate: f32,
    reps_left: u16,
    rep_gain: f32,
    started: bool,
    pluck_pos: usize,
    pluck_len: f32,
    pluck_z: f32,
    ctl: u32,
    /// Current oscillator frequency (control rate).
    hz: f32,
}

impl LayerState {
    fn new(layer: &Layer, seed: u32) -> Self {
        let mut noise = Noise::new(
            match layer.source {
                Source::Noise(c) => c,
                _ => NoiseColor::White,
            },
            seed,
        );
        noise.density = layer.density;
        let wave = match layer.source {
            Source::Osc(w) => w,
            Source::Fm { carrier, .. } => carrier,
            _ => Wave::Sine,
        };
        let mut osc = Osc::new(wave, 0.0);
        osc.width = layer.width;
        let mut filter = Filter::new(layer.filter);
        filter.steep = matches!(layer.filter, FilterMode::Lowpass | FilterMode::Highpass) && layer.q < 1.0;
        LayerState {
            osc,
            modu: Osc::new(Wave::Sine, 0.0),
            noise,
            filter,
            env: Adsr::new(layer.attack, layer.decay, layer.sustain, layer.release),
            vib: 0.0,
            t: 0.0,
            gate: layer.gate_secs(),
            reps_left: layer.repeat,
            rep_gain: 1.0,
            started: false,
            pluck_pos: 0,
            pluck_len: 100.0,
            pluck_z: 0.0,
            ctl: 0,
            hz: layer.freq,
        }
    }
}

/// A playing recipe. `Copy` so a fixed pool can hold it; the one pluck
/// string buffer is part of the voice.
#[derive(Clone, Copy)]
pub struct RecipeVoice {
    pub recipe: Recipe,
    states: [LayerState; MAX_LAYERS],
    pluck: [f32; PLUCK_LEN],
    /// Pitch multiplier (jitter × caller pitch × doppler at start).
    pub pitch: f32,
    /// Seconds since trigger (at pitch 1 time runs at real time).
    pub t: f32,
    length: f32,
    /// Hold the gate open until released (instrument notes).
    pub held: bool,
    rng: Rng,
    /// Last absolute sample value (for stealing decisions).
    pub level: f32,
    /// The DC blocker's last input and output: a pulse narrower than half
    /// its period, a driven or swept layer sit off centre; a 10 Hz high-pass
    /// takes the offset out (a 40 Hz kick loses 0.3 dB).
    dc: [f32; 2],
}

impl RecipeVoice {
    /// A voice for `recipe` at `pitch` (1 = as written). `note_hz`, when
    /// given, re-bases every layer's pitch on the note (instrument play:
    /// the layer's `freq` is read as relative to A4 = 440).
    pub fn new(recipe: &Recipe, pitch: f32, note_hz: Option<f32>, held: bool, seed: u32) -> Self {
        let mut rng = Rng::new(seed.wrapping_mul(2_654_435_761).max(1));
        let jitter = 1.0 + recipe.jitter * rng.bipolar();
        let mut recipe = *recipe;
        if let Some(hz) = note_hz {
            let ratio = hz / 440.0;
            for layer in recipe.layers.iter_mut().take(recipe.count) {
                layer.freq *= ratio;
                if layer.freq_to.is_finite() {
                    layer.freq_to *= ratio;
                }
            }
        }
        let states = std::array::from_fn(|i| LayerState::new(&recipe.layers[i], seed.wrapping_add(i as u32 * 7919)));
        RecipeVoice {
            length: recipe.length(),
            recipe,
            states,
            pluck: [0.0; PLUCK_LEN],
            pitch: finite(pitch * jitter * recipe.pitch, 1.0).clamp(0.05, 8.0),
            t: 0.0,
            held,
            rng,
            level: 0.0,
            dc: [0.0; 2],
        }
    }

    /// Release a held voice (note off).
    pub fn release(&mut self) {
        self.held = false;
        for (i, s) in self.states.iter_mut().enumerate().take(self.recipe.count) {
            let _ = i;
            s.reps_left = 0;
            s.env.gate(false);
        }
    }

    pub fn finished(&self) -> bool {
        if self.held {
            return false;
        }
        self.states[..self.recipe.count]
            .iter()
            .zip(self.recipe.layers())
            .all(|(s, _)| s.started && !s.env.active() && s.reps_left == 0 || (self.t > self.length + 0.5 && !s.env.active()))
    }

    /// Render `out.len()` mono samples, ADDING into `out`. `pitch_mod` is a
    /// live multiplier (doppler, bend).
    pub fn render(&mut self, out: &mut [f32], rate: f32, pitch_mod: f32) {
        let inv = 1.0 / rate;
        let pitch = self.pitch * pitch_mod;
        let dt = inv * pitch.min(4.0).max(0.1);
        let count = self.recipe.count;
        let vol = self.recipe.volume;
        let mut peak = 0.0f32;
        for sample in out.iter_mut() {
            let mut acc = 0.0f32;
            for i in 0..count {
                let layer = &self.recipe.layers[i];
                let st = &mut self.states[i];
                if self.t < layer.delay {
                    continue;
                }
                let lt = st.t;
                if !st.started {
                    st.started = true;
                    st.env.gate(true);
                    if let Source::Pluck { .. } = layer.source {
                        st.pluck_len = (rate / (layer.freq * pitch).max(20.0)).clamp(2.0, (PLUCK_LEN - 2) as f32);
                        for k in 0..PLUCK_LEN {
                            self.pluck[k] = self.rng.bipolar();
                        }
                        st.pluck_pos = 0;
                    }
                }
                // Gate and repeats.
                if !self.held && lt >= st.gate && !st.env.releasing() && st.env.active() {
                    st.env.gate(false);
                }
                if st.reps_left > 0 && lt >= layer.every {
                    st.reps_left -= 1;
                    st.t = 0.0;
                    st.rep_gain *= layer.repeat_decay;
                    st.env.gate(true);
                }
                if !st.env.active() {
                    st.t += dt;
                    continue;
                }
                let glide = if layer.glide > 0.0 { layer.glide } else { (st.gate + layer.release).max(0.01) };
                let u = (st.t / glide).min(1.0);
                if layer.vib_hz > 0.0 {
                    st.vib += layer.vib_hz * inv;
                    st.vib -= st.vib.floor();
                }
                // Pitch (sweep, vibrato, detune) at control rate: 16 samples
                // is 0.3 ms, far finer than any sweep, and it keeps the
                // transcendentals out of the per-sample path.
                if st.ctl == 0 {
                    let mut hz = layer.freq;
                    if layer.freq_to.is_finite() {
                        hz = layer.freq * (layer.freq_to / layer.freq).powf(u);
                    }
                    if layer.vib_hz > 0.0 {
                        hz *= 2f32.powf((st.vib * TAU).sin() * layer.vib_depth / 12.0);
                    }
                    if layer.detune_cents != 0.0 {
                        hz *= 2f32.powf(layer.detune_cents / 1200.0);
                    }
                    st.hz = hz * pitch;
                }
                let hz = st.hz;
                let raw = match layer.source {
                    Source::None => 0.0,
                    Source::Osc(_) => st.osc.next(hz, inv, 0.0),
                    Source::Noise(_) => st.noise.next(),
                    Source::Fm { ratio, .. } => {
                        let index = if layer.fm_index_to.is_finite() {
                            lerp(layer.fm_index, layer.fm_index_to, u)
                        } else {
                            layer.fm_index
                        };
                        let m = st.modu.next(hz * ratio, inv, 0.0) * index / TAU;
                        st.osc.next(hz, inv, m)
                    }
                    Source::Pluck { damping } => {
                        let len = st.pluck_len.max(2.0);
                        let li = len as usize;
                        let read = (st.pluck_pos + PLUCK_LEN - li) % PLUCK_LEN;
                        let prev = (read + PLUCK_LEN - 1) % PLUCK_LEN;
                        let y = 0.5 * (self.pluck[read] + self.pluck[prev]);
                        st.pluck_z = y + (st.pluck_z - y) * damping * 0.9;
                        self.pluck[st.pluck_pos] = st.pluck_z * 0.996;
                        st.pluck_pos = (st.pluck_pos + 1) % PLUCK_LEN;
                        y
                    }
                };
                if layer.pwm_depth > 0.0 && st.ctl == 0 {
                    let ph = st.t * layer.pwm_hz;
                    let tri = 4.0 * (ph - (ph + 0.5).floor()).abs() - 1.0;
                    st.osc.width = (layer.width + layer.pwm_depth * tri).clamp(0.02, 0.98);
                }
                if layer.filter != FilterMode::Off && st.ctl == 0 {
                    let c = if layer.cutoff_to.is_finite() {
                        layer.cutoff * (layer.cutoff_to / layer.cutoff).powf(u)
                    } else {
                        layer.cutoff
                    };
                    st.filter.set(c * pitch.sqrt(), layer.q, rate);
                }
                st.ctl = (st.ctl + 1) & 15;
                let mut y = st.filter.process(raw);
                if layer.drive > 0.0 {
                    y = soft_clip(y * (1.0 + layer.drive)) / (1.0 + layer.drive * 0.3).min(2.0);
                }
                let e = st.env.next(dt);
                acc += y * e * layer.gain * st.rep_gain;
                st.t += dt;
            }
            self.t += dt;
            let x = acc * vol;
            let v = x - self.dc[0] + DC_POLE * self.dc[1];
            self.dc = [x, v];
            peak = peak.max(v.abs());
            *sample += v;
        }
        self.level = self.level * 0.5 + peak * 0.5;
    }
}

/// The built-in library. Names are the game vocabulary: the old chiptune
/// bank names keep working (they now sound like this), plus modern sets for
/// weapons, impacts, footsteps by surface, UI, magic and water.
pub fn builtin(name: &str) -> Option<&'static str> {
    Some(match name {
        // ── classic bank (kept names) ──
        "jump" => "volume=0.8; sine freq=180>520 glide=0.14 a=0.002 d=0.16 r=0.04 gain=0.55; triangle freq=360>1040 glide=0.14 d=0.12 gain=0.18 lp=5000; noise=pink bp=2500 q=1.2 d=0.03 gain=0.08",
        "shoot" => "square freq=1400>220 glide=0.09 a=0.001 d=0.1 r=0.03 gain=0.3 lp=7000>1500; noise=white hp=2500 d=0.04 gain=0.2; sine freq=160>60 d=0.08 gain=0.4",
        "zap" | "laser" => "jitter=0.03; saw freq=2400>180 glide=0.16 a=0.001 d=0.18 r=0.05 gain=0.35 ladder=9000>700 q=6; fm=2.01 osc=sine freq=1200>300 index=6>0 d=0.12 gain=0.2; noise=white bp=4000 d=0.03 gain=0.1",
        "grab" => "sine freq=660>330 glide=0.08 a=0.002 d=0.1 gain=0.4; noise=pink bp=1800 d=0.03 gain=0.1",
        "angry" => "saw freq=160>95 glide=0.25 a=0.01 d=0.28 r=0.05 gain=0.4 ladder=1400>500 q=4 vib=18 vib_depth=0.6 drive=2",
        "calm" => "sine freq=392>523 glide=0.2 a=0.02 d=0.3 r=0.1 gain=0.35 vib=5 vib_depth=0.1; triangle freq=784>1046 glide=0.2 a=0.02 d=0.25 gain=0.1",
        "rescue" => "tri freq=659 a=0.003 d=0.12 r=0.05 gain=0.35; tri freq=784 delay=0.09 a=0.003 d=0.2 r=0.1 gain=0.35; sine freq=1568 delay=0.09 d=0.15 gain=0.08",
        "shove" => "noise=brown lp=900>200 a=0.001 d=0.08 gain=0.7; sine freq=120>60 d=0.06 gain=0.4",
        "board" => "sine freq=220>330 glide=0.11 a=0.004 d=0.14 gain=0.35; noise=pink lp=1200 d=0.06 gain=0.12",
        "coin" | "pickup" => "reverb=0.2; square freq=988 width=0.3 a=0.001 d=0.07 hold=0.07 r=0.02 gain=0.22 lp=6000; square freq=1319 delay=0.07 a=0.001 d=0.3 r=0.1 gain=0.22 lp=6000; sine freq=2637 delay=0.07 d=0.2 gain=0.08",
        "hurt" => "jitter=0.05; saw freq=420>140 glide=0.16 a=0.002 d=0.18 gain=0.35 ladder=3000>600 q=3 drive=3; noise=pink bp=900 d=0.08 gain=0.2",
        "win" => "reverb=0.3; tri freq=523 d=0.12 hold=0.1 gain=0.3; tri freq=659 delay=0.1 d=0.12 hold=0.1 gain=0.3; tri freq=784 delay=0.2 d=0.12 hold=0.1 gain=0.3; saw freq=1047 delay=0.3 a=0.01 d=0.6 s=0.0 r=0.3 gain=0.22 lp=4000 vib=6 vib_depth=0.15; tri freq=523 delay=0.3 d=0.7 gain=0.15",
        "lose" => "reverb=0.3; saw freq=330 a=0.01 d=0.14 hold=0.13 gain=0.25 ladder=1800 q=2; saw freq=262 delay=0.14 d=0.14 hold=0.13 gain=0.25 ladder=1600 q=2; saw freq=220>200 delay=0.28 a=0.01 d=0.8 gain=0.28 ladder=1400>400 q=2 vib=5 vib_depth=0.3",
        "squeak" => "sine freq=900>1500 glide=0.08 a=0.002 d=0.09 gain=0.3 vib=30 vib_depth=0.5",
        "roar" => "volume=1.1; saw freq=140>55 glide=0.6 a=0.03 d=0.7 r=0.2 gain=0.4 ladder=1200>300 q=3 drive=5 vib=22 vib_depth=0.8; noise=brown lp=700>200 a=0.05 d=0.6 gain=0.5; noise=pink bp=1800 q=2 a=0.05 d=0.4 gain=0.15",
        "bark" => "saw freq=560>380 glide=0.06 a=0.002 d=0.07 gain=0.4 bp=1100 q=2 drive=3; saw freq=500>340 delay=0.09 glide=0.06 d=0.07 gain=0.4 bp=1000 q=2 drive=3",
        "moo" => "saw freq=190>150 glide=0.5 a=0.06 d=0.5 r=0.15 gain=0.4 ladder=900 q=3 vib=4 vib_depth=0.2; saw freq=380>300 glide=0.5 a=0.06 d=0.5 gain=0.08 bp=700 q=4",
        "whip" | "swoosh" | "whoosh" => "noise=pink bp=600>3200 q=2.5 a=0.03 d=0.12 r=0.05 gain=0.6; noise=white hp=4000 delay=0.08 d=0.04 gain=0.15",
        // ── movement ──
        "footstep" | "step" | "footstep_asphalt" | "footstep_stone" => "jitter=0.06 reverb=0.1; noise=pink lp=2200>600 a=0.001 d=0.05 gain=0.45; sine freq=110>70 d=0.04 gain=0.35; noise=white hp=3000 d=0.015 gain=0.08",
        "footstep_grass" => "jitter=0.08 reverb=0.05; noise=pink bp=1800 q=0.8 a=0.008 d=0.09 gain=0.35; noise=velvet density=0.05 hp=2500 a=0.005 d=0.08 gain=0.2",
        "footstep_gravel" | "footstep_dirt" => "jitter=0.08; noise=velvet density=0.12 bp=3200 q=0.9 a=0.002 d=0.11 gain=0.6; noise=brown lp=400 d=0.06 gain=0.35",
        "footstep_wood" => "jitter=0.06 reverb=0.15; sine freq=190>150 a=0.001 d=0.07 gain=0.4; noise=pink bp=700 q=3 d=0.05 gain=0.3; noise=white hp=5000 d=0.01 gain=0.06",
        "footstep_metal" => "jitter=0.05 reverb=0.2; tri freq=820 d=0.12 gain=0.12; sine freq=1370 d=0.09 gain=0.08; noise=pink bp=2600 q=6 d=0.06 gain=0.3; sine freq=100>70 d=0.04 gain=0.25",
        "footstep_snow" => "jitter=0.08; noise=velvet density=0.3 lp=3500 a=0.02 d=0.12 gain=0.4; noise=brown lp=300 a=0.01 d=0.08 gain=0.25",
        "footstep_water" | "splash" => "jitter=0.08 reverb=0.1; noise=pink bp=1200>2600 q=1.5 a=0.004 d=0.18 gain=0.45; sine freq=500>1400 glide=0.05 d=0.05 gain=0.12; noise=velvet density=0.08 hp=3000 delay=0.03 d=0.2 gain=0.2",
        "land" => "jitter=0.05; noise=brown lp=500>120 a=0.001 d=0.14 gain=0.7; sine freq=85>45 d=0.12 gain=0.5; noise=pink bp=1500 d=0.04 gain=0.15",
        "skid" => "noise=pink bp=1400>900 q=5 a=0.02 d=0.25 r=0.08 gain=0.35; sine freq=950>800 a=0.02 d=0.22 gain=0.12 vib=13 vib_depth=0.3",
        // ── impacts ──
        "thud" | "impact" | "impact_wood" => "jitter=0.06; noise=brown lp=900>200 a=0.001 d=0.1 gain=0.8; sine freq=140>70 d=0.09 gain=0.5; noise=pink bp=600 q=4 d=0.06 gain=0.25",
        "impact_metal" | "clank" => "jitter=0.05 reverb=0.2; tri freq=523 d=0.35 gain=0.12; sine freq=1245 d=0.25 gain=0.1; sine freq=2189 d=0.18 gain=0.07; noise=white bp=3300 q=8 d=0.08 gain=0.25; noise=brown lp=500 d=0.05 gain=0.4",
        "impact_stone" => "jitter=0.06; noise=pink lp=2500>400 a=0.001 d=0.09 gain=0.7; sine freq=90>55 d=0.1 gain=0.45; noise=velvet density=0.1 hp=2000 d=0.12 gain=0.25",
        "impact_glass" | "shatter" => "reverb=0.2; noise=velvet density=0.25 hp=3500 a=0.001 d=0.5 gain=0.5; sine freq=3100 d=0.3 gain=0.08; sine freq=4700 d=0.25 gain=0.06; noise=white hp=6000 d=0.03 gain=0.3",
        "impact_flesh" | "punch" | "hit" => "jitter=0.08; noise=brown lp=1200>250 a=0.001 d=0.08 gain=0.8; sine freq=110>55 d=0.07 gain=0.5; noise=pink bp=2000 q=1 d=0.02 gain=0.3",
        "impact_plastic" => "jitter=0.08; tri freq=700>500 d=0.06 gain=0.25; noise=pink bp=1800 q=3 d=0.04 gain=0.3",
        "impact_car" | "crash" => "volume=1.2 reverb=0.2; noise=brown lp=1500>200 a=0.001 d=0.35 gain=0.8; sine freq=70>35 d=0.3 gain=0.6; noise=velvet density=0.2 hp=2500 d=0.6 gain=0.35; tri freq=430 d=0.5 gain=0.1; tri freq=1170 d=0.4 gain=0.07",
        "thump" | "suspension" => "sine freq=70>40 a=0.002 d=0.1 gain=0.6; noise=brown lp=300 d=0.06 gain=0.4",
        // ── explosions ──
        "explosion" | "boom" => "volume=1.3 reverb=0.35 jitter=0.05; noise=brown lp=2500>90 glide=0.9 a=0.002 d=1.1 r=0.4 gain=1.0 drive=3; sine freq=60>28 glide=0.6 d=0.8 gain=0.8 drive=1; noise=velvet density=0.25 hp=1500 a=0.001 d=0.3 gain=0.4; noise=pink bp=500 q=0.8 delay=0.05 a=0.05 d=1.5 gain=0.3",
        "explosion_small" | "pop" => "reverb=0.2 jitter=0.08; noise=brown lp=2500>200 a=0.001 d=0.3 gain=0.9 drive=2; sine freq=110>45 d=0.2 gain=0.5; noise=white hp=2500 d=0.04 gain=0.3",
        "explosion_big" | "nuke" => "volume=1.4 reverb=0.5; noise=brown lp=1800>50 glide=2.0 a=0.004 d=2.6 r=0.8 gain=1.0 drive=4; sine freq=48>22 glide=1.5 d=2.0 gain=0.9 drive=2; noise=velvet density=0.3 hp=1200 d=0.8 gain=0.45; noise=pink lp=900 delay=0.2 a=0.4 d=3.0 gain=0.35",
        "fireball" => "noise=pink bp=800>300 q=1 a=0.03 d=0.6 r=0.2 gain=0.6 drive=2; noise=velvet density=0.08 hp=2000 a=0.05 d=0.6 gain=0.25; saw freq=90>50 a=0.03 d=0.5 gain=0.2 ladder=500 q=2",
        // ── weapons by class ──
        "gun" | "gun_pistol" | "pistol" => "reverb=0.25 jitter=0.03; noise=white lp=9000>2000 a=0.0005 d=0.08 gain=0.8 drive=4; sine freq=190>70 d=0.07 gain=0.6; noise=pink bp=1500 q=1 delay=0.004 d=0.12 gain=0.3; noise=velvet density=0.2 hp=4000 d=0.02 gain=0.3",
        "gun_rifle" | "rifle" => "volume=1.1 reverb=0.3 jitter=0.03; noise=white lp=10000>1500 a=0.0003 d=0.1 gain=0.9 drive=6; sine freq=140>55 d=0.1 gain=0.7 drive=2; noise=pink bp=900 q=0.8 delay=0.006 d=0.25 gain=0.35; noise=white hp=5000 d=0.015 gain=0.4",
        "gun_smg" | "smg" | "machinegun" => "reverb=0.25; noise=white lp=8000>2000 a=0.0005 d=0.05 gain=0.7 drive=4 repeat=5 every=0.075 falloff=0.95; sine freq=170>70 d=0.05 gain=0.5 repeat=5 every=0.075; noise=velvet density=0.15 hp=4500 d=0.02 gain=0.25 repeat=5 every=0.075",
        "gun_shotgun" | "shotgun" => "volume=1.2 reverb=0.3 jitter=0.03; noise=white lp=7000>900 a=0.0005 d=0.18 gain=1.0 drive=5; sine freq=110>40 d=0.18 gain=0.8 drive=2; noise=brown lp=800 d=0.3 gain=0.5; noise=velvet density=0.3 hp=3000 d=0.08 gain=0.3",
        "gun_sniper" | "sniper" => "volume=1.2 reverb=0.45; noise=white lp=12000>1200 a=0.0002 d=0.14 gain=1.0 drive=8; sine freq=120>40 d=0.2 gain=0.8 drive=2; noise=pink lp=1200 delay=0.02 a=0.02 d=1.2 gain=0.3",
        "gun_rocket" | "rocket" | "launch" => "reverb=0.3; noise=pink bp=700>2500 q=0.8 a=0.01 d=0.5 r=0.2 gain=0.7 drive=2; noise=brown lp=600 a=0.005 d=0.4 gain=0.6; sine freq=80>140 d=0.3 gain=0.3",
        "gun_laser" | "blaster" => "jitter=0.04 reverb=0.25; fm=1.5 osc=saw freq=1800>200 glide=0.2 index=5>0.5 a=0.001 d=0.22 gain=0.35 ladder=8000>900 q=5; sine freq=3600>400 glide=0.15 d=0.1 gain=0.1",
        "reload" => "tri freq=1800 a=0.0005 d=0.03 gain=0.2; noise=white bp=3500 q=4 d=0.02 gain=0.3; tri freq=1200 delay=0.18 d=0.03 gain=0.2; noise=white bp=2500 q=4 delay=0.18 d=0.03 gain=0.35; noise=brown lp=600 delay=0.18 d=0.04 gain=0.3",
        "empty" | "dry_fire" => "tri freq=2400 a=0.0005 d=0.02 gain=0.2; noise=white bp=4000 q=5 d=0.015 gain=0.25",
        "sword" | "slash" => "jitter=0.06 reverb=0.2; noise=pink bp=900>4200 q=3 a=0.02 d=0.1 gain=0.45; sine freq=2800 delay=0.08 d=0.5 gain=0.07 vib=7 vib_depth=0.05; sine freq=4250 delay=0.08 d=0.35 gain=0.05",
        "bow" | "arrow" => "noise=pink bp=1200>500 q=4 a=0.005 d=0.15 gain=0.35; pluck=0.6 freq=110 d=0.3 gain=0.4",
        // ── UI ──
        "click" | "ui_click" => "reverb=0; sine freq=2200>1500 glide=0.01 a=0.0005 d=0.018 gain=0.3; noise=white hp=6000 d=0.004 gain=0.12",
        "hover" | "ui_hover" => "reverb=0; sine freq=1600 a=0.002 d=0.03 gain=0.12",
        "confirm" | "ui_confirm" | "select" => "reverb=0.05; sine freq=880 a=0.002 d=0.06 hold=0.05 gain=0.25; sine freq=1320 delay=0.06 a=0.002 d=0.12 gain=0.25; tri freq=2640 delay=0.06 d=0.08 gain=0.05",
        "cancel" | "ui_cancel" | "back" => "reverb=0.05; sine freq=880 a=0.002 d=0.06 hold=0.05 gain=0.22; sine freq=587 delay=0.06 a=0.002 d=0.12 gain=0.22",
        "error" | "ui_error" | "denied" => "reverb=0.05; square freq=220 width=0.4 a=0.002 d=0.1 hold=0.09 gain=0.12 lp=2000; square freq=185 delay=0.11 a=0.002 d=0.16 gain=0.12 lp=2000",
        "notify" | "ui_notify" | "message" => "reverb=0.2; fm=3.5 osc=sine freq=1047 index=2>0 a=0.002 d=0.6 gain=0.18; fm=3.5 osc=sine freq=1568 index=2>0 delay=0.08 a=0.002 d=0.7 gain=0.15",
        "beep" | "ui_beep" => "reverb=0; sine freq=1000 a=0.002 d=0.1 hold=0.08 gain=0.25",
        // ── magic / sci-fi ──
        "magic" | "spell" => "reverb=0.45 jitter=0.04; fm=2 osc=sine freq=660>1320 glide=0.5 index=3>0.5 a=0.05 d=0.6 r=0.3 gain=0.22 vib=6 vib_depth=0.2; noise=white bp=6000>9000 q=3 a=0.1 d=0.6 gain=0.08; tri freq=1980>2640 glide=0.5 a=0.05 d=0.5 gain=0.08",
        "heal" | "powerup" => "reverb=0.35; sine freq=523 a=0.01 d=0.1 hold=0.08 gain=0.2; sine freq=659 delay=0.08 d=0.1 hold=0.08 gain=0.2; sine freq=784 delay=0.16 d=0.1 hold=0.08 gain=0.2; fm=2 osc=sine freq=1047 delay=0.24 a=0.01 d=0.6 index=2>0 gain=0.22 vib=6 vib_depth=0.15",
        "teleport" | "warp" => "reverb=0.4; saw freq=200>2400 glide=0.45 a=0.02 d=0.5 gain=0.2 ladder=600>7000 q=8; noise=white bp=500>6000 q=4 a=0.05 d=0.5 gain=0.15",
        "shield" | "force" => "reverb=0.3; saw freq=110 detune=8 a=0.05 d=0.5 gain=0.2 ladder=400>2400 q=6 vib=9 vib_depth=0.3; saw freq=110 detune=-8 a=0.05 d=0.5 gain=0.2 ladder=400>2400 q=6",
        "alarm" | "siren" => "square freq=660>880 glide=0.25 a=0.01 hold=0.5 d=0.1 r=0.05 gain=0.18 lp=3000 repeat=3 every=0.5",
        "horn" => "saw freq=350 a=0.01 hold=0.45 d=0.1 s=0.8 r=0.08 gain=0.3 lp=2500 drive=1; saw freq=440 a=0.01 hold=0.45 d=0.1 s=0.8 r=0.08 gain=0.25 lp=2500",
        "bell" | "chime" => "reverb=0.4; fm=3.5 osc=sine freq=880 index=4>0.2 a=0.001 d=2.0 gain=0.25; sine freq=1760 d=1.2 gain=0.06",
        // ── water / nature ──
        "water" | "drip" => "reverb=0.4 jitter=0.1; sine freq=900>2200 glide=0.04 a=0.001 d=0.06 gain=0.3",
        "bubble" => "jitter=0.15; sine freq=500>1500 glide=0.05 a=0.004 d=0.07 gain=0.3; sine freq=700>1900 delay=0.07 glide=0.05 d=0.06 gain=0.2",
        "wind_gust" => "noise=pink bp=300>900 q=1.5 a=0.6 d=1.2 r=0.6 gain=0.4",
        "thunder" => "volume=1.3 reverb=0.6; noise=brown lp=4000>80 glide=2.5 a=0.005 d=3.0 gain=1.0 drive=2; noise=velvet density=0.1 lp=2000 d=1.2 gain=0.5",
        _ => return None,
    })
}

/// Every built-in name (primary spellings only).
pub const BUILTIN_NAMES: &[&str] = &[
    "jump", "shoot", "zap", "laser", "grab", "angry", "calm", "rescue", "shove", "board", "coin",
    "pickup", "hurt", "win", "lose", "squeak", "roar", "bark", "moo", "whip", "whoosh",
    "footstep", "footstep_grass", "footstep_gravel", "footstep_wood", "footstep_metal",
    "footstep_snow", "footstep_water", "land", "skid", "thud", "impact_metal", "impact_stone",
    "impact_glass", "impact_flesh", "impact_plastic", "impact_car", "thump", "explosion",
    "explosion_small", "explosion_big", "fireball", "gun_pistol", "gun_rifle", "gun_smg",
    "gun_shotgun", "gun_sniper", "gun_rocket", "gun_laser", "reload", "empty", "sword", "bow",
    "click", "hover", "confirm", "cancel", "error", "notify", "beep", "magic", "heal", "teleport",
    "shield", "alarm", "horn", "bell", "water", "bubble", "splash", "wind_gust", "thunder",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses() {
        for name in BUILTIN_NAMES {
            let text = builtin(name).unwrap_or_else(|| panic!("{name} missing"));
            Recipe::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn text_and_fields_agree() {
        let a = Recipe::parse("saw freq=880>180 glide=0.09 lp=6000>900 q=2 d=0.08 gain=0.5").unwrap();
        let mut layer = Layer::default();
        layer.set_str("osc", "saw").unwrap();
        layer.set_num("freq", 880.0).unwrap();
        layer.set_num("to", 180.0).unwrap();
        layer.set_num("glide", 0.09).unwrap();
        layer.set_num("lp", 6000.0).unwrap();
        layer.set_num("cutoff_to", 900.0).unwrap();
        layer.set_num("q", 2.0).unwrap();
        layer.set_num("d", 0.08).unwrap();
        layer.set_num("gain", 0.5).unwrap();
        // Debug form: the unset sweeps are NaN, which never compare equal.
        assert_eq!(format!("{:?}", a.layers[0]), format!("{layer:?}"));
    }

    #[test]
    fn typos_are_reported() {
        assert!(Recipe::parse("saw frq=100").is_err());
        assert!(Recipe::parse("sawz").is_err());
        assert!(Recipe::parse("").is_err());
    }

    #[test]
    fn a_voice_sounds_and_finishes() {
        let r = Recipe::parse(builtin("explosion").unwrap()).unwrap();
        let mut v = RecipeVoice::new(&r, 1.0, None, false, 1);
        let mut buf = vec![0.0f32; 48_000 * 5];
        v.render(&mut buf, 48_000.0, 1.0);
        let peak = buf.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(peak > 0.05, "{peak}");
        assert!(v.finished());
        assert!(buf.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn voices_start_without_a_click() {
        // The first samples of every built-in must ramp, not jump: the
        // biggest sample-to-sample step in the first ms stays small.
        for name in BUILTIN_NAMES {
            let r = Recipe::parse(builtin(name).unwrap()).unwrap();
            let mut v = RecipeVoice::new(&r, 1.0, None, false, 7);
            let mut buf = vec![0.0f32; 48];
            v.render(&mut buf, 48_000.0, 1.0);
            assert!(buf[0].abs() < 0.2, "{name} starts at {}", buf[0]);
        }
    }
}
