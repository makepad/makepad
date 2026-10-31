//! Tracker modules (MOD, S3M, XM, IT): one model the four formats are read
//! into, and the replay ([`replay`]) that plays a module's patterns tick
//! by tick into notes with their pitch and level over time.
//!
//! The model keeps what playing needs: the order list, the patterns as
//! cells (a note, an instrument, a volume-column command and an effect,
//! the effect already named in one vocabulary for all four formats), the
//! instruments (a key map onto samples and the volume envelope) and the
//! samples as floats with their loops. Notes are MIDI numbers: the note at
//! which a sample plays at its own reference rate (MOD C-2, S3M and XM
//! C-4, IT C-5) is 60, so [`Sample::rate`] is the sample's rate at 60.

pub mod arrange;
pub mod it;
pub mod protracker;
pub mod render;
pub mod replay;
pub mod s3m;
pub mod xm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mod,
    S3m,
    Xm,
    It,
}

impl Kind {
    pub fn ext(self) -> &'static str {
        match self {
            Kind::Mod => "mod",
            Kind::S3m => "s3m",
            Kind::Xm => "xm",
            Kind::It => "it",
        }
    }
}

/// No note in the cell.
pub const NOTE_NONE: i16 = -1;
/// Key off (`===`, XM/IT): the envelope's release; a cut where there is none.
pub const NOTE_OFF: i16 = -2;
/// Note cut (`^^^`): silence now.
pub const NOTE_CUT: i16 = -3;
/// Note fade (IT `~~~`): the instrument's fadeout starts.
pub const NOTE_FADE: i16 = -4;

/// The volume column, in one vocabulary (S3M's volume, XM's and IT's
/// commands).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Vol {
    #[default]
    None,
    /// 0..64.
    Set(u8),
    /// Each tick but the first, up (positive) or down.
    Slide(i8),
    /// On the first tick.
    FineSlide(i8),
    /// 0..64 (32 is the centre).
    Pan(u8),
    PanSlide(i8),
    TonePorta(u8),
    VibratoSpeed(u8),
    VibratoDepth(u8),
    /// IT's portamento in the volume column (units of 4).
    PortaUp(u8),
    PortaDown(u8),
}

/// An effect, in one vocabulary for the four formats. The variants that
/// behave differently between formats say whose behaviour they have.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Fx {
    #[default]
    None,
    /// `xy`: the note, +x and +y semitones, a tick each.
    Arpeggio(u8),
    /// MOD/XM `1xx`/`2xx`: each tick but the first, `xx` units (an Amiga
    /// period, or a 16th of a semitone on a linear-frequency module).
    PortaUp(u8),
    PortaDown(u8),
    /// On the first tick.
    FinePortaUp(u8),
    FinePortaDown(u8),
    /// On the first tick, a quarter of a fine step.
    ExtraFinePortaUp(u8),
    ExtraFinePortaDown(u8),
    /// S3M/IT `Fxx`/`Exx` (`FFx` fine, `FEx` extra fine), memory shared.
    S3mPortaUp(u8),
    S3mPortaDown(u8),
    TonePorta(u8),
    /// (speed, depth).
    Vibrato(u8, u8),
    FineVibrato(u8, u8),
    /// The volume slide byte (MOD/XM `Axy` style, or S3M `Dxy` style on
    /// S3M and IT); the porta or vibrato continues.
    TonePortaVolSlide(u8),
    VibratoVolSlide(u8),
    Tremolo(u8, u8),
    /// 0..255.
    Pan(u8),
    /// The sample starts at `xx` * 256 frames (plus [`Fx::HighOffset`]).
    Offset(u8),
    HighOffset(u8),
    /// MOD/XM `Axy`: up x or down y each tick but the first.
    VolSlide(u8),
    /// S3M/IT `Dxy` (`DFx`/`DxF` fine), with memory.
    S3mVolSlide(u8),
    FineVolUp(u8),
    FineVolDown(u8),
    /// To that order.
    Jump(u8),
    /// To that row of the next pattern.
    Break(u8),
    /// 0..64.
    Volume(u8),
    Speed(u8),
    Tempo(u8),
    /// IT `T0x`/`T1x`: down or up x each tick but the first.
    TempoSlide(i8),
    /// `xy` (XM `E9y`: every y ticks; S3M/IT `Qxy`: x the volume change).
    Retrig(u8),
    MultiRetrig(u8),
    NoteCut(u8),
    NoteDelay(u8),
    PatternLoop(u8),
    /// Rows the row is repeated.
    PatternDelay(u8),
    /// 0..64.
    GlobalVolume(u8),
    GlobalVolSlide(u8),
    /// XM `Kxx`: key off at tick xx.
    KeyOff(u8),
    /// IT `Mxx`: 0..64.
    ChannelVolume(u8),
    ChannelVolSlide(u8),
    PanSlide(u8),
    Tremor(u8),
    /// XM `E5x`/S3M `S2x`: the sample's finetune (-8..7 eighths of a semitone).
    Finetune(u8),
    /// XM `Lxx`: the envelope position.
    EnvelopePosition(u8),
    /// IT `S7x`: past note actions, new note action, envelopes on and off.
    NoteAction(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    /// A MIDI number, or [`NOTE_NONE`], [`NOTE_OFF`], [`NOTE_CUT`],
    /// [`NOTE_FADE`].
    pub note: i16,
    /// 1-based; 0 is none.
    pub inst: u16,
    pub vol: Vol,
    pub fx: Fx,
    /// A second effect (XM's volume column is the first; IT and S3M have one).
    pub fx2: Fx,
}

impl Cell {
    pub const EMPTY: Cell = Cell { note: NOTE_NONE, inst: 0, vol: Vol::None, fx: Fx::None, fx2: Fx::None };
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub rows: usize,
    /// `rows * channels`, row-major.
    pub cells: Vec<Cell>,
}

impl Pattern {
    pub fn empty(rows: usize, channels: usize) -> Pattern {
        Pattern { rows, cells: vec![Cell::EMPTY; rows * channels] }
    }

    pub fn cell(&self, row: usize, channel: usize, channels: usize) -> &Cell {
        &self.cells[row * channels + channel]
    }
}

/// An envelope: points (tick, value 0..1), a sustain range and a loop
/// (point indices).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    pub points: Vec<(u16, f32)>,
    pub sustain: Option<(usize, usize)>,
    pub looped: Option<(usize, usize)>,
}

impl Envelope {
    /// The value at `tick`.
    pub fn at(&self, tick: f32) -> f32 {
        let pts = &self.points;
        let Some(first) = pts.first() else { return 1.0 };
        if tick <= first.0 as f32 {
            return first.1;
        }
        for w in pts.windows(2) {
            if tick < w[1].0 as f32 {
                let span = (w[1].0 as f32 - w[0].0 as f32).max(1.0);
                return w[0].1 + (w[1].1 - w[0].1) * (tick - w[0].0 as f32) / span;
            }
        }
        pts[pts.len() - 1].1
    }

    /// The tick after `tick` as the envelope moves on (held at a sustain
    /// point while the key is down, wrapping at the loop).
    pub fn advance(&self, tick: f32, key_on: bool) -> f32 {
        let tick_of = |i: usize| self.points.get(i).map_or(0.0, |p| p.0 as f32);
        let next = tick + 1.0;
        if key_on {
            if let Some((from, to)) = self.sustain {
                let (a, b) = (tick_of(from), tick_of(to));
                if next > b {
                    return if b > a { a } else { b };
                }
            }
        }
        if let Some((from, to)) = self.looped {
            let (a, b) = (tick_of(from), tick_of(to));
            if next > b && b >= a {
                return a;
            }
        }
        next
    }

    /// The envelope has ended (past its last point, no loop).
    pub fn ended(&self, tick: f32) -> bool {
        self.looped.is_none() && self.points.last().is_some_and(|p| tick > p.0 as f32)
    }
}

/// How a sample's loop plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loop {
    /// Frames `from..to`.
    pub from: usize,
    pub to: usize,
    pub pingpong: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub name: String,
    pub left: Vec<f32>,
    pub right: Option<Vec<f32>>,
    /// The rate it plays at as MIDI note 60, in Hz.
    pub rate: f64,
    /// 0..1.
    pub volume: f32,
    /// IT's sample global volume, 0..1.
    pub global_volume: f32,
    /// 0..255, when the sample sets one.
    pub pan: Option<u8>,
    pub looped: Option<Loop>,
    /// IT's sustain loop (while the key is down).
    pub sustain: Option<Loop>,
}

impl Sample {
    pub fn silent(name: String) -> Sample {
        Sample { name, left: Vec::new(), right: None, rate: 8363.0, volume: 1.0, global_volume: 1.0, pan: None, looped: None, sustain: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Instrument {
    pub name: String,
    /// The sample (0-based) and the note each MIDI key plays; `None` is
    /// silence.
    pub keymap: Vec<Option<(usize, u8)>>,
    pub volume_env: Option<Envelope>,
    pub pan_env: Option<Envelope>,
    /// Volume (0..1) lost per tick after key off (0: none).
    pub fadeout: f32,
    pub global_volume: f32,
    /// 0..255.
    pub pan: Option<u8>,
    /// IT's new note action: 0 cut, 1 continue, 2 note off, 3 note fade.
    pub nna: u8,
}

impl Instrument {
    /// An instrument that is one sample on every key (MOD, S3M).
    pub fn of_sample(name: String, sample: usize) -> Instrument {
        Instrument { name, keymap: (0..128).map(|k| Some((sample, k as u8))).collect(), volume_env: None, pan_env: None, fadeout: 0.0, global_volume: 1.0, pan: None, nna: 0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub kind: Kind,
    pub title: String,
    /// The tracker that wrote it, when the file says.
    pub tracker: String,
    pub channels: usize,
    /// Pattern numbers; [`ORDER_SKIP`] is skipped, [`ORDER_END`] ends.
    pub orders: Vec<u16>,
    pub restart: usize,
    pub patterns: Vec<Pattern>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<Sample>,
    pub speed: u8,
    pub tempo: u8,
    /// 0..1.
    pub global_volume: f32,
    /// Linear frequency slides (XM, IT); else Amiga periods.
    pub linear: bool,
    /// Each channel's pan 0..255 and volume 0..1.
    pub pans: Vec<u8>,
    pub channel_volumes: Vec<f32>,
    /// Rows a beat is, when the file says (IT's highlight).
    pub rows_per_beat: Option<u8>,
    /// The song message (IT) and the sample names joined, where credits
    /// usually are.
    pub message: String,
    /// IT: S3M/IT volume slides and portamento share effect memory.
    pub s3m_slides: bool,
    /// IT's "old effects" and "compatible Gxx" flags.
    pub it_old_effects: bool,
    pub it_compat_gxx: bool,
}

pub const ORDER_SKIP: u16 = 0xFFFE;
pub const ORDER_END: u16 = 0xFFFF;

impl Module {
    /// The text a module carries that names people: the title, the song
    /// message, the sample and instrument names.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut push = |s: &str| {
            let s = s.trim();
            if !s.is_empty() {
                out.push_str(s);
                out.push('\n');
            }
        };
        push(&self.title);
        push(&self.message);
        for i in &self.instruments {
            push(&i.name);
        }
        for s in &self.samples {
            push(&s.name);
        }
        out
    }
}

/// The format of `bytes`, by its signature (and the file name's extension
/// for a MOD, whose tag is at the end of a header with no magic).
pub fn detect(bytes: &[u8], name: &str) -> Option<Kind> {
    if bytes.len() >= 4 && &bytes[..4] == b"IMPM" {
        return Some(Kind::It);
    }
    if bytes.len() >= 17 && &bytes[..17] == b"Extended Module: " {
        return Some(Kind::Xm);
    }
    if bytes.len() >= 48 && &bytes[44..48] == b"SCRM" {
        return Some(Kind::S3m);
    }
    if protracker::channels_of(bytes).is_some() {
        return Some(Kind::Mod);
    }
    let lower = name.to_ascii_lowercase();
    if (lower.ends_with(".mod") || lower.starts_with("mod.")) && bytes.len() > 600 {
        return Some(Kind::Mod);
    }
    None
}

/// Read a module.
pub fn read(bytes: &[u8], name: &str) -> Result<Module, String> {
    match detect(bytes, name) {
        Some(Kind::Mod) => protracker::read(bytes),
        Some(Kind::S3m) => s3m::read(bytes),
        Some(Kind::Xm) => xm::read(bytes),
        Some(Kind::It) => it::read(bytes),
        None => Err("not a tracker module (MOD, S3M, XM or IT)".into()),
    }
}

/// Little-endian reads that give zero past the end (a truncated module
/// still reads what it has).
pub(crate) struct Bytes<'a>(pub &'a [u8]);

impl Bytes<'_> {
    pub fn u8(&self, at: usize) -> u8 {
        self.0.get(at).copied().unwrap_or(0)
    }
    pub fn u16(&self, at: usize) -> u16 {
        u16::from_le_bytes([self.u8(at), self.u8(at + 1)])
    }
    pub fn u16be(&self, at: usize) -> u16 {
        u16::from_be_bytes([self.u8(at), self.u8(at + 1)])
    }
    pub fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes([self.u8(at), self.u8(at + 1), self.u8(at + 2), self.u8(at + 3)])
    }
    /// A fixed-size text field: up to the first NUL, control characters as spaces.
    pub fn text(&self, at: usize, len: usize) -> String {
        let end = (at + len).min(self.0.len());
        let field = if at < end { &self.0[at..end] } else { &[][..] };
        let field = field.split(|b| *b == 0).next().unwrap_or(&[]);
        field.iter().map(|&b| if b < 32 || b == 127 { ' ' } else { b as char }).collect::<String>().trim_end().to_string()
    }
    pub fn slice(&self, at: usize, len: usize) -> &[u8] {
        let a = at.min(self.0.len());
        let b = at.saturating_add(len).min(self.0.len());
        &self.0[a..b]
    }
}

/// Unroll a ping-pong loop into a forward one (the loop played forwards
/// then backwards, appended), so the engine plays it as a plain loop.
pub fn unroll_pingpong(s: &mut Sample) {
    for which in [false, true] {
        let lp = if which { s.sustain } else { s.looped };
        let Some(lp) = lp.filter(|l| l.pingpong && l.to > l.from + 1 && l.to <= s.left.len()) else { continue };
        let unroll = |ch: &mut Vec<f32>| {
            let tail: Vec<f32> = ch[lp.from..lp.to].iter().rev().copied().collect();
            ch.truncate(lp.to);
            ch.extend(tail);
        };
        // Only the main loop is unrolled in place; a sustain loop that is
        // ping-pong plays forwards (rare, and the difference is small).
        if which {
            if let Some(l) = s.sustain.as_mut() {
                l.pingpong = false;
            }
            continue;
        }
        unroll(&mut s.left);
        if let Some(r) = s.right.as_mut() {
            unroll(r);
        }
        s.looped = Some(Loop { from: lp.from, to: lp.from + 2 * (lp.to - lp.from), pingpong: false });
    }
}
