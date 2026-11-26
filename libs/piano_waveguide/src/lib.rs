//! A grand piano from first principles.
//!
//! Every string is a digital waveguide: a delay loop that carries all of the
//! string's partials, with an allpass that stretches them the way stiffness
//! does. The felt hammer is a nonlinear mass inside that loop. The strings of
//! a key end on one bridge junction with a real impedance, which is where a
//! note's decay, its aftersound and its loudness come from. The bridge drives
//! a modal soundboard that radiates in stereo.
//!
//! [`WaveguidePiano::process`] is the audio callback: it allocates nothing,
//! takes no lock and makes no system call. All state is built by
//! [`WaveguidePiano::new`], which is slow enough to keep off the audio
//! thread.

mod board;
pub mod design;
mod duplex;
mod room;
pub mod scale;
mod voice;

pub use voice::{loop_filters, loop_filters_with, Voicing};

use board::{Board, Smoothed, BLOCK, BUSES};
use room::Room;
use scale::{build_scale, KeyScale, FIRST_KEY, KEYS, LAST_KEY};
use voice::Voice;

pub use board::BoardShape;

/// The name this instrument goes by in preset lists.
pub const PRESET_NAME: &str = "Waveguide Grand";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PianoEvent {
    NoteOn { key: u8, velocity: u8 },
    NoteOff { key: u8 },
    /// Damper pedal, 0 (up) to 1 (down); values between half-pedal.
    Sustain { value: f32 },
    SoftPedal { on: bool },
    Sostenuto { on: bool },
    AllSoundOff,
}

/// An event `offset` samples into the block it is passed with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedEvent {
    pub offset: u32,
    pub event: PianoEvent,
}

#[derive(Clone)]
struct Key {
    voice: Voice,
    down: bool,
    held_by_sostenuto: bool,
    active: bool,
    /// Bridge region and how the key divides between it and the next: what
    /// the other strings feel.
    bus: usize,
    bus_mix: f32,
    /// The board's place for this key and how it divides between it and
    /// the next: what the board radiates.
    point: usize,
    point_mix: f32,
    gain: f32,
    /// No damper on this key (the top of the compass).
    undamped: bool,
    /// Bridge resistance: velocity to force on the bridge.
    resistance: f32,
    /// Bridge force of its region to bridge velocity at this key.
    receive: f32,
    /// The force this key put on the bridge in the previous block.
    own: [f32; BLOCK],
    /// dB by which the board's accident at this key stood out from its
    /// register (exact against smoothed), and the part of it evened.
    regulation: [f32; 2],
    /// Share of the touch law's span its hammer speeds take (see
    /// [`touch_span`]).
    touch: f32,
}

#[derive(Clone)]
pub struct WaveguidePiano {
    fs: f32,
    scale: Vec<KeyScale>,
    keys: Vec<Key>,
    active: [u8; KEYS],
    active_count: usize,
    board: Board,
    duplex: duplex::Duplex,
    room: Room,
    sustain: f32,
    sostenuto: bool,
    master: f32,
    /// Output level above which the limiter bends the sound (infinite: off).
    knee: f32,
    /// Bridge velocity at each of the board's places.
    bus: Vec<[f32; BLOCK]>,
    /// The board's place for each bridge region (where the duplex segments
    /// of the region sound).
    region_point: [usize; BUSES],
    scratch: [f32; BLOCK],
    /// Force on each bridge region in the previous block and in this one,
    /// and the previous block's peak: what the other strings feel.
    felt: [[f32; BLOCK]; BUSES],
    force: [[f32; BLOCK]; BUSES],
    felt_peak: [f32; BUSES],
    drive: [f32; BLOCK],
    /// Summed hammer force of the block, N.
    blow: [f32; BLOCK],
    /// Blocks in a row with no key moving and nothing left to hear.
    silent_blocks: u32,
    /// Treble shelf: gain above the corner, the corner's one-pole
    /// coefficient, and the low-passed signal per ear.
    shelf_gain: f32,
    shelf_coef: f32,
    shelf_low: [f32; 2],
    /// Samples left of the fade an AllSoundOff starts (0: none), and its
    /// length: the sound stops on a short slope, not on a cliff.
    fade: u32,
    fade_len: u32,
    /// Events that came during the fade, played when it ends.
    deferred: [PianoEvent; DEFERRED],
    deferred_count: usize,
}

/// Output level of the regulated instrument.
const OUTPUT_GAIN: f32 = 69.0;
/// The whole output, knock and room included, 10.7 dB under that: headroom
/// for loud chords, which peak near the limiter's knee instead of in it.
/// The touch spans some 18 dB from v40 to v110: a v110 chord peaks about
/// 9 dB over a mezzo-forte phrase.
const HEADROOM: f32 = 0.292;
/// A key sleeps once its strings move less than this, m/s.
const SLEEP_LEVEL: f32 = 2e-8;
/// How much of the bridge force of far regions a key feels, against its own
/// region's.
const FAR_SHARE: f32 = 0.3;
/// An undamped key at rest wakes when the bridge moves it by this, m/s.
const WAKE_LEVEL: f32 = 4e-7;
/// The board and the room are put to rest after this many blocks below
/// `SILENCE`, so their states never decay into denormals.
const SILENT_BLOCKS: u32 = 32;
const SILENCE: f32 = 1e-9;
/// How long an AllSoundOff takes to silence the instrument, s.
const FADE_SECS: f64 = 0.008;
/// Events held during that fade; more end the fade at once.
const DEFERRED: usize = 64;

/// Level of a key at mezzo forte in dB re middle C. The lowest octave's low
/// partials ring on below the board's modes, and the tenor's sing on, so
/// over their first second those keys sound louder than their first 0.3 s,
/// which the gain is set from: they are set that much softer. From middle C
/// up it follows the reference AU's first 0.3 s, the trend of neighbouring
/// keys at v40, v60 and v87: as loud as middle C up to C6, then falling to
/// 15 dB under it at C8. From G5 up the bridge drains a note faster in its
/// first half second (see `scale::prompt_decay_db`), and that moves the
/// level the gain is set from apart from the first 0.3 s at the ears, by
/// 0.3 dB at F#5, 0.7 at G5 and 2-2.4 from D6 to F7: the curve there is
/// that much lower.
fn register_level(midi: u8) -> f32 {
    scale::curve(&[(21.0, -0.2), (33.0, -0.7), (40.0, -0.57), (48.0, -1.1), (54.0, -1.15), (58.0, -1.0), (60.0, -0.5), (64.0, 0.0), (67.0, -0.56), (72.0, -0.9), (76.0, -0.63), (77.0, -0.25), (78.0, -0.21), (79.0, -0.17), (84.0, -2.89), (86.0, -4.47), (88.0, -5.16), (91.0, -6.81), (96.0, -9.57), (100.0, -12.22), (104.0, -15.07), (106.0, -15.25), (108.0, -16.39)], midi as f64) as f32
}

/// Each key's felt at most `ceiling` times as hard as the median felt of the
/// keys within an octave of it (see [`Voicing::felt_ceiling`]).
fn limit_felt(voices: &mut [Voice], ceiling: f64) {
    if !(ceiling.is_finite() && ceiling > 0.0) {
        return;
    }
    // Softening one key lowers its neighbours' trend: settle in a few passes.
    for _ in 0..16 {
        let felt: Vec<f64> = voices.iter().map(|v| v.felt_at_mm()).collect();
        let mut changed = false;
        for (i, voice) in voices.iter_mut().enumerate() {
            let limit = ceiling * felt_trend(&felt, i);
            if felt[i] > limit {
                voice.limit_felt(limit);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// Median of `felt` over the keys within an octave either side of key `i`,
/// as far as the keyboard reaches on both sides alike (so that a steady rise
/// to the top or the bottom is its own trend).
pub(crate) fn felt_trend(felt: &[f64], i: usize) -> f64 {
    let reach = 12.min(i).min(felt.len() - 1 - i);
    let mut near: Vec<f64> = felt[i - reach..=i + reach].to_vec();
    near.sort_by(f64::total_cmp);
    near[near.len() / 2]
}

/// What a key's mezzo-forte blow amounts to at the ears, before any trim:
/// the level of each partial at the bridge over the first 0.3 s, times what
/// the board makes of it from this key's place on the bridge, the gain at
/// that frequency (`exact`) and over the third of an octave around it
/// (`smoothed`); in that order. A key is regulated to the smoothed level,
/// within `Voicing::regulation_tolerance` of the exact one: evened on its
/// exact fundamental, a key whose fundamental falls in a dip of the board
/// was lifted whole and its treble came out up to 5 dB brighter than its
/// neighbours'.
fn regulated_level(voice: &mut Voice, key: &KeyScale, board: &Board, smoothed: &Smoothed, point: usize, point_mix: f32, fs: f64) -> (f32, f32) {
    let bridge = blow(voice, voice::VOICING_SPEED, fs, 0.3, true);
    let len = bridge.len();
    let mut mix = vec![0.0; board.points()];
    mix[point] = 1.0 - point_mix as f64;
    mix[point + 1] = point_mix as f64;
    let (mut exact, mut smooth) = (0.0, 0.0);
    for n in 1..=32 {
        let f = design::string_partial(key.f1, key.b, n);
        if f > 0.4 * fs {
            break;
        }
        // Fourier coefficient at the partial, by a rotating phasor
        let (step_im, step_re) = (std::f64::consts::TAU * f / fs).sin_cos();
        let (mut c, mut s) = (1.0, 0.0);
        let (mut re, mut im) = (0.0, 0.0);
        for &v in &bridge {
            re += v as f64 * c;
            im += v as f64 * s;
            (c, s) = (c * step_re - s * step_im, s * step_re + c * step_im);
        }
        let (l, r) = board.response(&mix, f, fs);
        exact += (re * re + im * im) * 0.5 * (l + r);
        smooth += (re * re + im * im) * smoothed.power(point, point_mix as f64, f);
    }
    let level = |power: f64| (power.sqrt() / len as f64).max(1e-12) as f32 * 3.0e3;
    (level(exact), level(smooth))
}

/// The bridge velocity of a blow at `speed` m/s on the key alone over its
/// first `secs`, through a Hann window if `hann`.
fn blow(voice: &mut Voice, speed: f64, fs: f64, secs: f64, hann: bool) -> Vec<f32> {
    let len = (secs * fs) as usize;
    let mut bridge = vec![0.0f32; len];
    voice.clear();
    voice.set_damper(0.0);
    voice.strike(speed);
    for chunk in bridge.chunks_mut(BLOCK) {
        voice.process(None, chunk, &mut [0.0; BLOCK]);
    }
    voice.clear();
    if hann {
        for (i, v) in bridge.iter_mut().enumerate() {
            *v *= (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos()) as f32;
        }
    }
    bridge
}

/// The power of a `bridge` motion as the ears get it from the key's place
/// on the board: its whole spectrum up to 0.4 of the rate,
/// each frequency weighted by the board's gain over the third of an octave
/// around it.
fn heard_power(bridge: &[f32], smoothed: &Smoothed, point: usize, point_mix: f32, fs: f64) -> f64 {
    let n = bridge.len().next_power_of_two();
    let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
    for (r, &v) in re.iter_mut().zip(bridge) {
        *r = v as f64;
    }
    design::fft(&mut re, &mut im);
    let bin = fs / n as f64;
    (1..n / 2).take_while(|&k| k as f64 * bin < 0.4 * fs).map(|k| (re[k] * re[k] + im[k] * im[k]) * smoothed.power(point, point_mix as f64, k as f64 * bin)).sum()
}

/// Where the limiter starts to bend the output, as a share of full scale:
/// -1 dBFS, over the loudest chords of ordinary playing, so it only
/// catches the peaks of the loudest.
const KNEE: f32 = 0.891;

/// Untouched below `knee`; above it the output bends towards full scale
/// instead of clipping when many loud notes pile up.
#[inline]
fn soft_limit(x: f32, knee: f32) -> f32 {
    let a = x.abs();
    if a <= knee {
        return x;
    }
    let over = (a - knee) / (1.0 - knee);
    (knee + (1.0 - knee) * over / (1.0 + over)).copysign(x)
}

/// The touch law: dB of level against mezzo forte (v67) for a MIDI
/// velocity. Each step of velocity is a step in level, as a touch is
/// heard: some 0.25 dB a step through the middle of the range, a little
/// more below it and less above, so that a note grows 18 dB from v40 to
/// v110 in every register, as the reference AU's do.
fn touch_db(velocity: u8) -> f64 {
    scale::curve(&[(1.0, -19.0), (40.0, -7.6), (60.0, -1.76), (67.0, 0.0), (87.0, 5.04), (110.0, 10.4), (127.0, 13.3)], velocity.clamp(1, 127) as f64)
}

/// Hammer speed in m/s for a MIDI velocity, as many dB from the speed the
/// felt is voiced at (mezzo forte, v67) as the touch law gives. Each key
/// takes its own share of that span (see [`touch_span`]).
pub fn hammer_speed(velocity: u8) -> f64 {
    key_speed(1.0, velocity)
}

/// Hammer speed for a key whose speeds take `span` of the touch law's.
fn key_speed(span: f64, velocity: u8) -> f64 {
    voice::VOICING_SPEED * 10f64.powf(span * touch_db(velocity) / 20.0)
}

/// Touch regulation, the twin of the level regulation: the share of the
/// touch law's span (in dB about mezzo forte) a key's hammer speeds take
/// so that its level at the ears, as [`heard_power`] has it, grows from
/// v40 to v110 as the law says: the law's growth over the key's own at the
/// nominal speeds (a second measurement at that span moved no register's
/// growth by more than 0.2 dB). Measured as the level regulation measures
/// (partials 1-32 at their nominal frequencies) a bass note's growth read
/// 3-4 dB low; its whole spectrum reads within a dB of what is heard. This
/// regulates the instrument, as a
/// technician evens the touch by register; it is not hammer physics. A
/// felt that stiffens as it is driven harder shortens its blow and lifts
/// the low partials with it: from C4 to B7 a note grew 2-5 dB more than
/// the law from v40 to v110 (up to 7 dB at D#4) and the fourth and fifth
/// octaves take about 0.8 of the span; the bass, where the law was
/// fitted, keeps about the whole of it.
/// Mezzo forte does not move, so the voicing and the level regulation
/// stand as they are.
fn touch_span(voice: &mut Voice, smoothed: &Smoothed, point: usize, point_mix: f32, fs: f64) -> f64 {
    let mut level = |velocity: u8| 10.0 * heard_power(&blow(voice, hammer_speed(velocity), fs, TOUCH_SECS, false), smoothed, point, point_mix, fs).log10();
    (touch_db(110) - touch_db(40)) / (level(110) - level(40))
}

/// [`touch_span`] weighs a blow's first half second, as a note's level is
/// heard and measured against the reference AU's.
const TOUCH_SECS: f64 = 0.5;

/// Keys either side over which the touch spans are smoothed: the touch is
/// evened by register, not key by key.
const TOUCH_REACH: usize = 3;

/// Each key's touch span, the mean (of the logarithms) of the spans of the
/// keys within [`TOUCH_REACH`] of it, as far as the keyboard goes: at the
/// ends of the keyboard the last keys share their neighbours' span, so the
/// span steps by well under half a dB from key to key everywhere.
fn smooth_touch(spans: &[f64]) -> Vec<f64> {
    (0..spans.len())
        .map(|i| {
            let near = &spans[i.saturating_sub(TOUCH_REACH)..=(i + TOUCH_REACH).min(spans.len() - 1)];
            (near.iter().map(|s| s.ln()).sum::<f64>() / near.len() as f64).exp()
        })
        .collect()
}

impl WaveguidePiano {
    pub fn new(sample_rate: f32) -> Self {
        Self::with_voicing(sample_rate, &Voicing::default(), &BoardShape::default())
    }

    /// The instrument with its hammers and unisons set by `voicing` and its
    /// soundboard by `board`.
    pub fn with_voicing(sample_rate: f32, voicing: &Voicing, board: &BoardShape) -> Self {
        let fs = sample_rate.max(8000.0) as f64;
        let scale = build_scale();
        let board_model = Board::new(fs, board);
        let smoothed = board_model.smoothed(fs);
        let points = board_model.points();
        let mut voices: Vec<Voice> = scale.iter().map(|k| Voice::new(k, fs, voicing)).collect();
        limit_felt(&mut voices, voicing.felt_ceiling);
        let mut keys = scale
            .iter()
            .zip(voices)
            .map(|(k, mut voice)| {
                let place = (k.midi - FIRST_KEY) as f32 / (KEYS - 1) as f32 * (BUSES - 1) as f32;
                let bus = (place.floor() as usize).min(BUSES - 2);
                let bus_mix = place - bus as f32;
                let at = (k.midi - FIRST_KEY) as f32 / (KEYS - 1) as f32 * (points - 1) as f32;
                let point = (at.floor() as usize).min(points - 2);
                let point_mix = at - point as f32;
                let (exact, smooth) = regulated_level(&mut voice, k, &board_model, &smoothed, point, point_mix, fs);
                let touch = touch_span(&mut voice, &smoothed, point, point_mix, fs) as f32;
                // what of the board's accident at this key stands out beyond
                // the tolerance is evened
                let stands_out = 20.0 * (exact / smooth).log10();
                let evened = stands_out.signum() * (stands_out.abs() - voicing.regulation_tolerance as f32).max(0.0);
                let level = smooth * 10f32.powf(evened / 20.0);
                let gain = OUTPUT_GAIN * 10f32.powf(register_level(k.midi) / 20.0) / level;
                Key {
                    voice,
                    down: false,
                    held_by_sostenuto: false,
                    active: false,
                    bus,
                    bus_mix,
                    point,
                    point_mix,
                    gain,
                    undamped: k.damper_t60 == 0.0,
                    resistance: k.bridge_r as f32,
                    receive: (board.coupling / k.bridge_r) as f32,
                    own: [0.0; BLOCK],
                    regulation: [stands_out, evened],
                    touch,
                }
            })
            .collect::<Vec<Key>>();
        let spans: Vec<f64> = keys.iter().map(|k| k.touch as f64).collect();
        for (key, span) in keys.iter_mut().zip(smooth_touch(&spans)) {
            key.touch = span as f32;
        }
        let gains: Vec<f32> = keys.iter().map(|k| k.gain).collect();
        let duplex = duplex::Duplex::new(&scale, &gains, fs, board.duplex);
        WaveguidePiano {
            fs: fs as f32,
            scale,
            keys,
            active: [0; KEYS],
            active_count: 0,
            board: board_model,
            duplex,
            room: Room::new(fs),
            sustain: 0.0,
            sostenuto: false,
            master: 1.0,
            knee: KNEE,
            bus: vec![[0.0; BLOCK]; points],
            region_point: std::array::from_fn(|b| (b * (points - 1) + (BUSES - 1) / 2) / (BUSES - 1)),
            scratch: [0.0; BLOCK],
            felt: [[0.0; BLOCK]; BUSES],
            force: [[0.0; BLOCK]; BUSES],
            felt_peak: [0.0; BUSES],
            drive: [0.0; BLOCK],
            blow: [0.0; BLOCK],
            silent_blocks: SILENT_BLOCKS,
            fade: 0,
            fade_len: ((FADE_SECS * fs) as u32).max(1),
            deferred: [PianoEvent::AllSoundOff; DEFERRED],
            deferred_count: 0,
            shelf_gain: 1.0,
            shelf_coef: 0.0,
            shelf_low: [0.0; 2],
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    /// The physical description of every key, A0 first.
    pub fn scale(&self) -> &[KeyScale] {
        &self.scale
    }

    /// How hard each key's hammer felt is: its force in N at 1 mm of
    /// compression, A0 first.
    pub fn felt_at_mm(&self) -> Vec<f64> {
        self.keys.iter().map(|k| k.voice.felt_at_mm()).collect()
    }

    /// (seconds the felt stayed on the string, peak force in N) of the last
    /// blow on `key`; zeros before the hammer has left.
    pub fn last_blow(&self, key: u8) -> (f64, f64) {
        if !(FIRST_KEY..=LAST_KEY).contains(&key) {
            return (0.0, 0.0);
        }
        self.keys[(key - FIRST_KEY) as usize].voice.last_blow()
    }

    /// Measurement aid (allocates): the sound a unit impulse of bridge
    /// velocity at the board's place `bus` makes, without the room.
    pub fn board_impulse(&mut self, bus: usize, len: usize) -> (Vec<f32>, Vec<f32>) {
        self.reset();
        let (mut l, mut r) = (vec![0.0; len], vec![0.0; len]);
        let mut input = vec![[0.0f32; BLOCK]; self.board.points()];
        input[bus.min(self.board.points() - 1)][0] = 1.0;
        for (l, r) in l.chunks_mut(BLOCK).zip(r.chunks_mut(BLOCK)) {
            self.board.process(&input, &[0.0; BLOCK], false, l, r);
            input.iter_mut().for_each(|b| *b = [0.0; BLOCK]);
        }
        self.reset();
        (l, r)
    }

    /// The board's mix of places for motion at `key`.
    fn key_mix(&self, key: u8) -> (Vec<f64>, f32) {
        let k = &self.keys[(key.clamp(FIRST_KEY, LAST_KEY) - FIRST_KEY) as usize];
        let mut mix = vec![0.0; self.board.points()];
        mix[k.point] = 1.0 - k.point_mix as f64;
        mix[k.point + 1] = k.point_mix as f64;
        (mix, k.gain)
    }

    /// Measurement aid: the soundboard's complex gain (re, im) to the left
    /// and the right ear at `hz` for motion at `key`'s place on the bridge;
    /// with `regulated`, the key's regulation gain included.
    pub fn board_transfer(&self, key: u8, hz: f64, regulated: bool) -> [(f64, f64); 2] {
        let (mix, gain) = self.key_mix(key);
        let g = if regulated { gain as f64 } else { 1.0 };
        self.board.transfer(&mix, hz, self.fs as f64).map(|(re, im)| (re * g, im * g))
    }

    /// Measurement aid: the duplex segments' frequencies, Hz, region by
    /// region from the bass (regions without segments are empty).
    pub fn duplex_frequencies(&self) -> Vec<Vec<f64>> {
        self.duplex.frequencies(self.fs as f64)
    }

    /// Measurement aid: dB by which the board's accident at `key` stands
    /// out from its register, and the part of it the regulation evens (see
    /// [`Voicing::regulation_tolerance`]).
    pub fn regulation(&self, key: u8) -> [f32; 2] {
        self.keys[(key.clamp(FIRST_KEY, LAST_KEY) - FIRST_KEY) as usize].regulation
    }

    /// Measurement aid: the share of the touch law's span the key's hammer
    /// speeds take (see [`touch_span`]).
    pub fn touch(&self, key: u8) -> f32 {
        self.keys[(key.clamp(FIRST_KEY, LAST_KEY) - FIRST_KEY) as usize].touch
    }

    /// Measurement aid: power gain of the soundboard to the left and right
    /// ear at `hz` for motion at `key`'s place on the bridge, the key's
    /// regulation gain included.
    pub fn board_gain(&self, key: u8, hz: f64) -> (f64, f64) {
        let (mix, gain) = self.key_mix(key);
        let (l, r) = self.board.response(&mix, hz, self.fs as f64);
        let g = (gain as f64).powi(2);
        (l * g, r * g)
    }

    /// Measurement aid (allocates): the bridge velocity in m/s of `key`
    /// struck alone, before the soundboard.
    pub fn bridge_velocity(&mut self, key: u8, velocity: u8, len: usize) -> Vec<f32> {
        self.reset();
        let k = &mut self.keys[(key.clamp(FIRST_KEY, LAST_KEY) - FIRST_KEY) as usize];
        let voice = &mut k.voice;
        voice.set_damper(0.0);
        voice.strike(key_speed(k.touch as f64, velocity));
        let mut out = vec![0.0; len];
        for chunk in out.chunks_mut(BLOCK) {
            voice.process(None, chunk, &mut [0.0; BLOCK]);
        }
        self.reset();
        out
    }

    /// Measurement aid (allocates): the bridge velocity in m/s and the
    /// hammer's force in N of `key` struck alone, before the soundboard.
    pub fn blow_trace(&mut self, key: u8, velocity: u8, len: usize) -> (Vec<f32>, Vec<f32>) {
        self.reset();
        let k = &mut self.keys[(key.clamp(FIRST_KEY, LAST_KEY) - FIRST_KEY) as usize];
        let voice = &mut k.voice;
        voice.set_damper(0.0);
        voice.strike(key_speed(k.touch as f64, velocity));
        let (mut out, mut force) = (vec![0.0; len], vec![0.0; len]);
        for (chunk, f) in out.chunks_mut(BLOCK).zip(force.chunks_mut(BLOCK)) {
            voice.process(None, chunk, f);
        }
        self.reset();
        (out, force)
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        self.master = if gain.is_finite() { gain.clamp(0.0, 8.0) } else { 1.0 };
    }

    /// Where the output limiter starts to bend the sound, as a share of
    /// full scale (0.891, -1 dBFS, as built); 1 or more turns it off.
    pub fn set_limit_knee(&mut self, knee: f32) {
        self.knee = if !knee.is_finite() || knee >= 1.0 { f32::INFINITY } else { knee.max(0.05) };
    }

    /// A treble shelf over the whole instrument: `gain_db` above about
    /// `corner_hz` (0 dB is the instrument as built).
    pub fn set_brightness(&mut self, gain_db: f32, corner_hz: f32) {
        let gain_db = if gain_db.is_finite() { gain_db.clamp(-24.0, 24.0) } else { 0.0 };
        let corner = if corner_hz.is_finite() { corner_hz.clamp(100.0, 0.45 * self.fs) } else { 3000.0 };
        self.shelf_gain = 10f32.powf(gain_db / 20.0);
        self.shelf_coef = (-std::f32::consts::TAU * corner / self.fs).exp();
    }

    /// Strength of the strings' longitudinal components (see
    /// [`Voicing::phantoms`]); takes effect at once.
    pub fn set_phantoms(&mut self, strength: f64) {
        for key in &mut self.keys {
            key.voice.set_phantoms(strength);
        }
    }

    /// Measurement aid: every damper takes `times` as long to settle as
    /// built.
    pub fn set_damper_slew(&mut self, times: f64) {
        for key in &mut self.keys {
            key.voice.set_damper_slew(times);
        }
    }

    /// How loudly the case answers a hammer blow (see [`BoardShape::knock`]).
    pub fn set_knock(&mut self, knock: f64) {
        self.board.set_knock(knock);
    }

    /// Strength of the duplex segments (see [`BoardShape::duplex`]).
    pub fn set_duplex(&mut self, strength: f64) {
        self.duplex.set_strength(strength);
    }

    /// How much of one string's bridge motion reaches the others (see
    /// [`BoardShape::coupling`]).
    pub fn set_coupling(&mut self, coupling: f64) {
        for key in &mut self.keys {
            key.receive = (coupling.max(0.0) as f32) / key.resistance;
        }
    }

    /// Level of the room, 0 for the dry instrument.
    pub fn set_room_mix(&mut self, mix: f32) {
        self.room.mix = if mix.is_finite() { mix.clamp(0.0, 1.0) } else { 0.0 };
    }

    /// Silence, with every pedal up.
    pub fn reset(&mut self) {
        for key in &mut self.keys {
            key.voice.clear();
            key.down = false;
            key.held_by_sostenuto = false;
            key.active = false;
            key.own = [0.0; BLOCK];
        }
        self.felt = [[0.0; BLOCK]; BUSES];
        self.felt_peak = [0.0; BUSES];
        self.active_count = 0;
        self.sustain = 0.0;
        self.sostenuto = false;
        self.board.clear();
        self.duplex.clear();
        self.room.clear();
        self.silent_blocks = SILENT_BLOCKS;
        self.fade = 0;
        self.deferred_count = 0;
        self.update_dampers();
    }

    /// Silence at once, the pedals left as they are.
    fn silence(&mut self) {
        let (sustain, sostenuto) = (self.sustain, self.sostenuto);
        self.reset();
        self.sustain = sustain;
        self.sostenuto = sostenuto;
        self.update_dampers();
    }

    /// Hold an event that came during a fade. A pedal that moves again
    /// before anything else came replaces its earlier move.
    fn defer(&mut self, event: PianoEvent) {
        use PianoEvent::*;
        if let Some(last) = self.deferred[..self.deferred_count].last_mut() {
            if matches!((&*last, &event), (Sustain { .. }, Sustain { .. }) | (Sostenuto { .. }, Sostenuto { .. }) | (SoftPedal { .. }, SoftPedal { .. })) {
                *last = event;
                return;
            }
        }
        if self.deferred_count == DEFERRED {
            self.end_fade();
            return self.apply_event(event);
        }
        self.deferred[self.deferred_count] = event;
        self.deferred_count += 1;
    }

    /// The fade is over: silence, then what came during it, in order.
    fn end_fade(&mut self) {
        let (events, count) = (self.deferred, self.deferred_count);
        self.silence();
        for &event in &events[..count] {
            // Nothing has sounded since the fade: another AllSoundOff in
            // the queue silences at once.
            if event == PianoEvent::AllSoundOff {
                self.silence();
            } else {
                self.apply_event(event);
            }
        }
    }

    /// How far the pedal lifts the dampers: nothing for the first third of
    /// its travel, fully up from two thirds.
    fn pedal_lift(&self) -> f64 {
        ((self.sustain as f64 - 0.3) / 0.35).clamp(0.0, 1.0)
    }

    fn update_dampers(&mut self) {
        let lift = self.pedal_lift();
        for key in &mut self.keys {
            let lifted = if key.down || key.held_by_sostenuto { 1.0 } else { lift };
            key.voice.set_damper(1.0 - lifted);
        }
    }

    /// Apply an event at the current sample without rendering or advancing time.
    pub fn apply_event(&mut self, event: PianoEvent) {
        if self.fade > 0 {
            return self.defer(event);
        }
        match event {
            PianoEvent::NoteOn { key, velocity } => {
                if !(FIRST_KEY..=LAST_KEY).contains(&key) {
                    return;
                }
                if velocity == 0 {
                    return self.apply_event(PianoEvent::NoteOff { key });
                }
                let index = (key - FIRST_KEY) as usize;
                let k = &mut self.keys[index];
                k.down = true;
                k.voice.set_damper(0.0);
                k.voice.strike(key_speed(k.touch as f64, velocity));
                if !k.active {
                    k.active = true;
                    self.active[self.active_count] = index as u8;
                    self.active_count += 1;
                }
            }
            PianoEvent::NoteOff { key } => {
                if !(FIRST_KEY..=LAST_KEY).contains(&key) {
                    return;
                }
                let lift = self.pedal_lift();
                let k = &mut self.keys[(key - FIRST_KEY) as usize];
                k.down = false;
                k.voice.set_damper(if k.held_by_sostenuto { 0.0 } else { 1.0 - lift });
            }
            PianoEvent::Sustain { value } => {
                self.sustain = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
                self.update_dampers();
            }
            PianoEvent::SoftPedal { on } => {
                for (key, scale) in self.keys.iter_mut().zip(&self.scale) {
                    key.voice.set_soft(on, scale.midi);
                }
            }
            PianoEvent::Sostenuto { on } => {
                if on && !self.sostenuto {
                    for key in &mut self.keys {
                        key.held_by_sostenuto = key.down;
                    }
                } else if !on {
                    for key in &mut self.keys {
                        key.held_by_sostenuto = false;
                    }
                }
                self.sostenuto = on;
                self.update_dampers();
            }
            PianoEvent::AllSoundOff => {
                if self.active_count == 0 && self.silent_blocks >= SILENT_BLOCKS {
                    self.silence();
                } else {
                    self.fade = self.fade_len;
                }
            }
        }
    }

    fn render_block(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        if self.active_count == 0 && self.silent_blocks >= SILENT_BLOCKS {
            l.fill(0.0);
            r.fill(0.0);
            return;
        }
        for bus in self.bus.iter_mut().chain(self.force.iter_mut()) {
            bus[..n].fill(0.0);
        }
        // Strings whose dampers are off ring along with whatever moves the
        // bridge: wake those the bridge now moves enough to matter.
        let lifted = self.pedal_lift() > 0.0;
        if self.felt_peak.iter().any(|&p| p > 0.0) {
            for (index, key) in self.keys.iter_mut().enumerate() {
                if key.active || !(lifted || key.undamped || key.down || key.held_by_sostenuto) {
                    continue;
                }
                let near = (1.0 - key.bus_mix) * self.felt_peak[key.bus] + key.bus_mix * self.felt_peak[key.bus + 1];
                let moved = key.receive * near.max(FAR_SHARE * self.felt_peak.iter().fold(0.0f32, |m, &p| m.max(p)));
                if moved > WAKE_LEVEL {
                    key.active = true;
                    self.active[self.active_count] = index as u8;
                    self.active_count += 1;
                }
            }
        }
        self.blow = [0.0; BLOCK];
        let mut struck = false;
        let mut i = 0;
        while i < self.active_count {
            let key = &mut self.keys[self.active[i] as usize];
            struck |= key.voice.hammer_active();
            let (near, far) = (1.0 - key.bus_mix, key.bus_mix);
            let open = lifted || key.undamped || key.down || key.held_by_sostenuto;
            if open {
                // The region's force a block ago, less this key's own part
                // of it: its strings already feel their own bridge motion.
                let own_share = near * near + far * far;
                // A third of what the far regions carry arrives too.
                for j in 0..n {
                    let here = near * self.felt[key.bus][j] + far * self.felt[key.bus + 1][j] - own_share * key.own[j];
                    let everywhere = self.felt[0][j] + self.felt[1][j] + self.felt[2][j] + self.felt[3][j] - key.own[j];
                    self.drive[j] = key.receive * (here + FAR_SHARE * (everywhere - here));
                }
                key.voice.process(Some(&self.drive[..n]), &mut self.scratch[..n], &mut self.blow[..n]);
            } else {
                key.voice.process(None, &mut self.scratch[..n], &mut self.blow[..n]);
            }
            struck |= key.voice.hammer_active();
            key.own = [0.0; BLOCK];
            let (here, next) = (1.0 - key.point_mix, key.point_mix);
            for j in 0..n {
                let v = self.scratch[j];
                self.bus[key.point][j] += key.gain * here * v;
                self.bus[key.point + 1][j] += key.gain * next * v;
                let force = key.resistance * v;
                key.own[j] = force;
                self.force[key.bus][j] += near * force;
                self.force[key.bus + 1][j] += far * force;
            }
            if key.voice.peak < SLEEP_LEVEL && !key.voice.hammer_active() {
                key.voice.quiet_blocks += 1;
            } else {
                key.voice.quiet_blocks = 0;
            }
            if key.voice.quiet_blocks > 8 {
                key.voice.clear();
                key.own = [0.0; BLOCK];
                key.active = false;
                self.active_count -= 1;
                self.active[i] = self.active[self.active_count];
            } else {
                i += 1;
            }
        }
        for b in 0..BUSES {
            self.felt[b] = [0.0; BLOCK];
            self.felt[b][..n].copy_from_slice(&self.force[b][..n]);
            self.felt_peak[b] = self.force[b][..n].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        }
        self.duplex.process(&self.force, &mut self.bus, &self.region_point, n);
        self.board.process(&self.bus, &self.blow, struck, l, r);
        if self.shelf_gain != 1.0 {
            let (gain, coef) = (self.shelf_gain, self.shelf_coef);
            for (ear, low) in [&mut *l, &mut *r].into_iter().zip(self.shelf_low.iter_mut()) {
                for x in ear.iter_mut() {
                    *low = *x + coef * (*low - *x);
                    *x = *low + gain * (*x - *low);
                }
            }
        }
        self.room.process(l, r);
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, x| m.max(x.abs()));
        if self.active_count == 0 && peak < SILENCE && !self.duplex.ringing() {
            self.silent_blocks += 1;
            if self.silent_blocks >= SILENT_BLOCKS {
                self.board.clear();
                self.room.clear();
                self.shelf_low = [0.0; 2];
            }
        } else {
            self.silent_blocks = 0;
        }
        let gain = self.master * HEADROOM;
        for x in l.iter_mut().chain(r.iter_mut()) {
            *x = soft_limit(*x * gain, self.knee);
        }
    }

    /// Render `out_l.len()` samples (both slices are overwritten). `events`
    /// are in order of `offset`; one past the end takes effect at the end.
    pub fn process(&mut self, events: &[TimedEvent], out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len());
        let (mut pos, mut next) = (0, 0);
        while pos < len {
            while next < events.len() && events[next].offset as usize <= pos {
                self.apply_event(events[next].event);
                next += 1;
            }
            let mut n = BLOCK.min(len - pos);
            if next < events.len() {
                n = n.min(events[next].offset as usize - pos);
            }
            if self.fade > 0 {
                n = n.min(self.fade as usize);
            }
            self.render_block(&mut out_l[pos..pos + n], &mut out_r[pos..pos + n]);
            if self.fade > 0 {
                // A raised cosine from full level to none.
                let step = std::f32::consts::PI / self.fade_len as f32;
                for (l, r) in out_l[pos..pos + n].iter_mut().zip(&mut out_r[pos..pos + n]) {
                    self.fade -= 1;
                    let gain = 0.5 - 0.5 * (step * self.fade as f32).cos();
                    *l *= gain;
                    *r *= gain;
                }
                if self.fade == 0 {
                    self.end_fade();
                }
            }
            pos += n;
        }
        for event in &events[next..] {
            self.apply_event(event.event);
        }
    }
}

#[cfg(test)]
mod tests;
