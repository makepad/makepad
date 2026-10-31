//! A module played tick by tick, the way the trackers play it, into notes:
//! each note its channel, instrument and sample, where it starts (in beats:
//! a row is `1 / rows_per_beat` of a beat, so a speed change is a tempo
//! change and the rows stay on the grid), and its state every tick (the
//! pitch with its slides, the vibrato and arpeggio apart, the level with
//! every volume, envelope and fade applied, the pan). The order list is
//! followed with its jumps, breaks and pattern loops until the song ends or
//! comes back to a row it played (its loop).

use super::{Envelope, Fx, Kind, Module, Vol, NOTE_CUT, NOTE_FADE, NOTE_NONE, NOTE_OFF, ORDER_END, ORDER_SKIP};
use std::collections::HashSet;

/// One tick of a sounding note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tick {
    /// Beats from the song's start.
    pub beat: f64,
    /// The pitch with its slides (a MIDI number, fractional).
    pub pitch: f32,
    /// The vibrato's and the arpeggio's offsets now, in semitones.
    pub vibrato: f32,
    pub arpeggio: f32,
    /// The arpeggio playing (`xy`), 0 for none.
    pub arp_param: u8,
    /// 0..1, everything applied.
    pub level: f32,
    /// -1..1.
    pub pan: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub channel: usize,
    /// 0-based.
    pub instrument: usize,
    pub sample: usize,
    pub beat: f64,
    /// The pitch it was struck at (the key map applied).
    pub key: f32,
    /// The frame it starts at (a sample offset).
    pub offset: usize,
    pub ticks: Vec<Tick>,
    /// Where it stops sounding (the next note on its channel, a cut, the
    /// end of the sample or the fade).
    pub end: f64,
    /// The vibrato and arpeggio effects it played under, for the Score's
    /// note options: (speed in Hz, depth in semitones), (the two offsets).
    pub vibrato: Option<(f32, f32)>,
    pub arpeggio: Option<(u8, u8)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Played {
    pub notes: Vec<Note>,
    /// (beat, bpm): the first at 0.
    pub tempo: Vec<(f64, f64)>,
    /// Where each order played starts: (beat, order index, pattern).
    pub orders: Vec<(f64, usize, u16)>,
    /// The song's length in beats.
    pub end: f64,
    pub rows_per_beat: u32,
    /// Ticks a beat at the song's start (for an arpeggio's step).
    pub ticks_per_beat: f64,
    /// Seconds.
    pub seconds: f64,
}

/// The longest song played (a module whose loop is not found).
const MAX_SECONDS: f64 = 20.0 * 60.0;

fn vib_wave(pos: u8) -> f32 {
    (pos as f32 / 64.0 * std::f32::consts::TAU).sin()
}

#[derive(Clone, Default)]
struct Mem {
    porta: u8,
    porta_up: u8,
    porta_down: u8,
    tone: u8,
    vib_speed: u8,
    vib_depth: u8,
    vol_slide: u8,
    arp: u8,
    offset: u8,
    retrig: u8,
    chan_slide: u8,
    pan_slide: u8,
    global_slide: u8,
    tremor: u8,
    trem_speed: u8,
    trem_depth: u8,
}

/// A note sounding on a channel (or behind it, after a new note action).
#[derive(Clone)]
struct Voice {
    note: usize,
    instrument: usize,
    sample: usize,
    key_on: bool,
    fading: bool,
    fade: f32,
    env_tick: f32,
    pan_env_tick: f32,
    /// Frames played, to find a one-shot's end.
    pos: f64,
    /// The pitch in units: an Amiga period * 4, or 64ths of a semitone.
    units: f64,
    volume: f32,
}

#[derive(Clone, Default)]
struct Chan {
    inst: usize,
    voice: Option<Voice>,
    behind: Vec<Voice>,
    target: Option<f64>,
    vib_pos: u8,
    trem_pos: u8,
    tremor_on: bool,
    tremor_count: u8,
    pan: f32,
    chan_vol: f32,
    mem: Mem,
    loop_row: usize,
    loop_count: u8,
    offset_high: u8,
    nna_override: Option<u8>,
}

struct Player<'a> {
    m: &'a Module,
    chans: Vec<Chan>,
    notes: Vec<Note>,
    global: f32,
    speed: u32,
    tempo: f64,
    beat: f64,
    seconds: f64,
    tick_beats: f64,
    rows_per_beat: u32,
}

impl<'a> Player<'a> {
    fn linear(&self) -> bool {
        self.m.linear
    }

    /// Units of a note.
    fn units_of(&self, note: f32) -> f64 {
        if self.linear() {
            note as f64 * 64.0
        } else {
            1712.0 * 2f64.powf(-(note as f64 - 60.0) / 12.0)
        }
    }

    fn pitch_of(&self, units: f64) -> f32 {
        if self.linear() {
            (units / 64.0) as f32
        } else {
            (60.0 + 12.0 * (1712.0 / units.max(1.0)).log2()) as f32
        }
    }

    /// Move the pitch up by `units` (down for negative).
    fn slide(&self, v: &mut Voice, units: f64) {
        if self.linear() {
            v.units = (v.units + units).clamp(0.0, 127.0 * 64.0);
        } else {
            v.units = (v.units - units).clamp(1712.0 / 2f64.powf(67.0 / 12.0), 1712.0 * 2f64.powf(60.0 / 12.0));
        }
    }

    fn sample_volume(&self, v: &Voice) -> f32 {
        self.m.samples.get(v.sample).map_or(1.0, |s| s.volume * 64.0)
    }

    /// Strike `note` on channel `c` with instrument `inst` (0-based).
    fn strike(&mut self, c: usize, note: i16, offset: usize, row_beat: f64) {
        let inst = self.chans[c].inst;
        let Some(instrument) = self.m.instruments.get(inst) else { return };
        let Some(Some((sample, key))) = instrument.keymap.get(note.clamp(0, 127) as usize).copied() else {
            self.stop(c, row_beat);
            return;
        };
        if self.m.samples.get(sample).is_none_or(|s| s.left.is_empty()) {
            self.stop(c, row_beat);
            return;
        }
        // The note before: cut, or behind the channel (IT's new note actions).
        let nna = self.chans[c].nna_override.unwrap_or(instrument.nna);
        if let Some(mut old) = self.chans[c].voice.take() {
            let keep = self.m.kind == Kind::It && nna != 0 && self.m.instruments.get(old.instrument).is_some_and(|i| i.volume_env.is_some() || i.fadeout > 0.0);
            if keep {
                match nna {
                    2 => old.key_on = false,
                    3 => old.fading = true,
                    _ => {}
                }
                let ch = &mut self.chans[c];
                ch.behind.push(old);
                if ch.behind.len() > 3 {
                    let gone = ch.behind.remove(0);
                    self.notes[gone.note].end = self.notes[gone.note].end.min(row_beat);
                }
            } else {
                self.notes[old.note].end = row_beat;
            }
        }
        let key = key as f32;
        let units = self.units_of(key);
        let volume = self.m.samples[sample].volume * 64.0;
        self.notes.push(Note { channel: c, instrument: inst, sample, beat: row_beat, key, offset, ticks: Vec::new(), end: f64::INFINITY, vibrato: None, arpeggio: None });
        let ch = &mut self.chans[c];
        if let Some(p) = instrument.pan.or(self.m.samples[sample].pan.filter(|_| self.m.kind == Kind::It || self.m.kind == Kind::Xm)) {
            ch.pan = p as f32 / 255.0 * 2.0 - 1.0;
        }
        ch.vib_pos = 0;
        ch.trem_pos = 0;
        ch.target = None;
        ch.voice = Some(Voice { note: self.notes.len() - 1, instrument: inst, sample, key_on: true, fading: false, fade: 1.0, env_tick: 0.0, pan_env_tick: 0.0, pos: offset as f64, units, volume });
    }

    fn stop(&mut self, c: usize, beat: f64) {
        if let Some(v) = self.chans[c].voice.take() {
            self.notes[v.note].end = self.notes[v.note].end.min(beat);
        }
    }

    fn key_off(&mut self, c: usize, beat: f64) {
        let has_env = self.chans[c].voice.as_ref().and_then(|v| self.m.instruments.get(v.instrument)).is_some_and(|i| i.volume_env.is_some());
        match self.chans[c].voice.as_mut() {
            Some(v) if has_env => v.key_on = false,
            Some(v) if self.m.kind == Kind::It => {
                v.key_on = false;
                v.fading = true;
            }
            Some(_) => self.stop(c, beat),
            None => {}
        }
    }

    /// The level and pan of a voice now, and whether it still sounds; the
    /// envelopes and the fade move on a tick.
    fn advance(&self, v: &mut Voice, chan_pan: f32, chan_vol: f32, extra: f32) -> (f32, f32, bool) {
        let inst = self.m.instruments.get(v.instrument);
        let env: Option<&Envelope> = inst.and_then(|i| i.volume_env.as_ref());
        let mut level = (v.volume.clamp(0.0, 64.0) + extra).clamp(0.0, 64.0) / 64.0;
        let mut alive = true;
        if let Some(env) = env {
            level *= env.at(v.env_tick);
            if !v.key_on && env.ended(v.env_tick) && env.points.last().is_some_and(|p| p.1 <= 0.0) {
                alive = false;
            }
            v.env_tick = env.advance(v.env_tick, v.key_on);
            // XM: an envelope's key off starts the fadeout too.
            if !v.key_on {
                v.fading = true;
            }
        }
        if v.fading {
            level *= v.fade;
            let out = inst.map_or(1.0, |i| i.fadeout);
            if out <= 0.0 && self.m.kind == Kind::It && env.is_none() {
                alive = false;
            }
            v.fade = (v.fade - out).max(0.0);
            if v.fade <= 0.0 && out > 0.0 {
                alive = false;
            }
        }
        let sample = &self.m.samples[v.sample];
        level *= sample.global_volume * inst.map_or(1.0, |i| i.global_volume) * chan_vol * self.global * self.m.channel_volumes.first().map_or(1.0, |_| 1.0);
        let mut pan = chan_pan;
        if let Some(penv) = inst.and_then(|i| i.pan_env.as_ref()) {
            let p = penv.at(v.pan_env_tick) * 2.0 - 1.0;
            pan = (pan + p * (1.0 - pan.abs())).clamp(-1.0, 1.0);
            v.pan_env_tick = penv.advance(v.pan_env_tick, v.key_on);
        }
        // The sample's end.
        let hz = sample.rate * 2f64.powf((self.pitch_of(v.units) as f64 - 60.0) / 12.0);
        v.pos += hz * self.tick_seconds();
        if sample.looped.is_none() && sample.sustain.is_none() && v.pos >= sample.left.len() as f64 {
            alive = false;
        }
        (level, pan, alive)
    }

    fn tick_seconds(&self) -> f64 {
        2.5 / self.tempo
    }

    fn bpm(&self) -> f64 {
        24.0 * self.tempo / (self.rows_per_beat as f64 * self.speed as f64)
    }

    fn vol_slide_s3m(&mut self, c: usize, param: u8, tick: u32) {
        let p = if param == 0 { self.chans[c].mem.vol_slide } else { param };
        self.chans[c].mem.vol_slide = p;
        let (x, y) = ((p >> 4) as f32, (p & 15) as f32);
        let delta = if x == 15.0 && y > 0.0 {
            if tick == 0 { -y } else { 0.0 }
        } else if y == 15.0 && x > 0.0 {
            if tick == 0 { x } else { 0.0 }
        } else if x == 0.0 {
            if tick > 0 { -y } else { 0.0 }
        } else if y == 0.0 {
            if tick > 0 { x } else { 0.0 }
        } else {
            0.0
        };
        if let Some(v) = self.chans[c].voice.as_mut() {
            v.volume = (v.volume + delta).clamp(0.0, 64.0);
        }
    }

    fn vol_slide_mod(&mut self, c: usize, param: u8, tick: u32) {
        let p = if param == 0 && self.m.kind == Kind::Xm { self.chans[c].mem.vol_slide } else { param };
        self.chans[c].mem.vol_slide = p;
        if tick == 0 {
            return;
        }
        let (x, y) = ((p >> 4) as f32, (p & 15) as f32);
        let delta = if x > 0.0 { x } else { -y };
        if let Some(v) = self.chans[c].voice.as_mut() {
            v.volume = (v.volume + delta).clamp(0.0, 64.0);
        }
    }

    fn vol_slide(&mut self, c: usize, param: u8, tick: u32) {
        if self.m.s3m_slides {
            self.vol_slide_s3m(c, param, tick)
        } else {
            self.vol_slide_mod(c, param, tick)
        }
    }

    fn tone_porta(&mut self, c: usize, param: u8, tick: u32) {
        let p = if param == 0 { self.chans[c].mem.tone } else { param };
        self.chans[c].mem.tone = p;
        if tick == 0 {
            return;
        }
        let Some(target) = self.chans[c].target else { return };
        let step = p as f64 * 4.0;
        let linear = self.linear();
        if let Some(v) = self.chans[c].voice.as_mut() {
            // Up in pitch is up in units (linear) or down (periods).
            let up = if linear { target > v.units } else { target < v.units };
            let moved = if linear == up { v.units + step } else { v.units - step };
            let passed = if up == linear { moved >= target } else { moved <= target };
            v.units = if passed { target } else { moved };
            if passed {
                self.chans[c].target = None;
            }
        }
    }

    fn vibrato(&mut self, c: usize, speed: u8, depth: u8) {
        let ch = &mut self.chans[c];
        if speed > 0 {
            ch.mem.vib_speed = speed;
        }
        if depth > 0 {
            ch.mem.vib_depth = depth;
        }
    }

    /// The vibrato's depth in semitones at a channel's pitch (an Amiga
    /// period's swing is wider at low notes).
    fn vib_semis(&self, depth: u8, fine: bool, units: f64) -> f32 {
        let d = depth as f64 / if fine { 4.0 } else { 1.0 };
        if self.linear() {
            (d * 4.0 / 64.0) as f32
        } else {
            (12.0 * ((units + 8.0 * d) / units.max(1.0)).log2()) as f32
        }
    }
}

/// Play a module.
pub fn play(m: &Module) -> Played {
    let mut rows_per_beat = m.rows_per_beat.map_or(4, |r| r as u32).clamp(1, 32);
    let bpm0 = 24.0 * m.tempo as f64 / (rows_per_beat as f64 * m.speed.max(1) as f64);
    // A beat the module's own, or four rows, then doubled or halved into
    // a tempo a score plays at.
    let mut bpm = bpm0;
    while bpm > 200.0 && rows_per_beat < 32 {
        rows_per_beat *= 2;
        bpm /= 2.0;
    }
    while bpm < 50.0 && rows_per_beat > 1 && rows_per_beat % 2 == 0 {
        rows_per_beat /= 2;
        bpm *= 2.0;
    }
    let channels = m.channels;
    let mut p = Player {
        m,
        chans: (0..channels).map(|c| Chan { pan: m.pans.get(c).map_or(0.0, |&v| v as f32 / 255.0 * 2.0 - 1.0), chan_vol: m.channel_volumes.get(c).copied().unwrap_or(1.0), ..Default::default() }).collect(),
        notes: Vec::new(),
        global: m.global_volume,
        speed: m.speed.max(1) as u32,
        tempo: m.tempo.max(32) as f64,
        beat: 0.0,
        seconds: 0.0,
        tick_beats: 0.0,
        rows_per_beat,
    };
    let mut tempo_map: Vec<(f64, f64)> = vec![(0.0, p.bpm())];
    let mut orders_played: Vec<(f64, usize, u16)> = Vec::new();
    let mut visited: HashSet<(usize, usize)> = HashSet::new();
    let ticks_per_beat = p.speed as f64 * rows_per_beat as f64;
    let mut order = 0usize;
    let mut row = 0usize;
    let mut last_order = usize::MAX;
    'song: loop {
        // The pattern at this order.
        let pat_index = loop {
            match m.orders.get(order) {
                None => break 'song,
                Some(&ORDER_END) => break 'song,
                Some(&ORDER_SKIP) => {
                    order += 1;
                    row = 0;
                }
                Some(&pi) => break pi,
            }
        };
        let empty;
        let pattern = match m.patterns.get(pat_index as usize) {
            Some(pat) => pat,
            None => {
                empty = super::Pattern::empty(64, channels);
                &empty
            }
        };
        if row >= pattern.rows {
            row = 0;
            order += 1;
            continue;
        }
        let looping = p.chans.iter().any(|c| c.loop_count > 0);
        if !looping && !visited.insert((order, row)) {
            break;
        }
        if p.seconds > MAX_SECONDS {
            break;
        }
        if order != last_order || row == 0 {
            orders_played.push((p.beat, order, pat_index));
            last_order = order;
        }
        let cells: Vec<super::Cell> = (0..channels).map(|c| *pattern.cell(row, c, channels)).collect();
        // Row-wide effects first: speed, tempo, the pattern delay.
        let mut delay_rows = 0u32;
        for cell in &cells {
            for fx in [cell.fx, cell.fx2] {
                match fx {
                    Fx::Speed(s) if s > 0 => p.speed = s as u32,
                    Fx::Tempo(t) if t >= 32 => p.tempo = t as f64,
                    Fx::PatternDelay(d) if delay_rows == 0 => delay_rows = d as u32,
                    _ => {}
                }
            }
        }
        let bpm = p.bpm();
        if (tempo_map.last().map_or(0.0, |t| t.1) - bpm).abs() > 0.01 {
            if tempo_map.last().is_some_and(|t| (t.0 - p.beat).abs() < 1e-9) {
                tempo_map.last_mut().unwrap().1 = bpm;
            } else {
                tempo_map.push((p.beat, bpm));
            }
        }
        let row_beat = p.beat;
        let row_ticks = p.speed * (1 + delay_rows);
        // A row lasts one row of beats however many ticks it has.
        p.tick_beats = 1.0 / (rows_per_beat as f64 * row_ticks as f64);
        let mut jump: Option<usize> = None;
        let mut brk: Option<usize> = None;
        let mut loop_to: Option<usize> = None;
        for t in 0..row_ticks {
            let tick = t % p.speed;
            let tick_beat = row_beat + t as f64 * p.tick_beats;
            for c in 0..channels {
                let cell = cells[c];
                let delay = [cell.fx, cell.fx2].iter().find_map(|f| if let Fx::NoteDelay(d) = f { Some(*d as u32) } else { None }).unwrap_or(0);
                // The cell's note, instrument and volume act on its first
                // tick (or its delayed one).
                if delay < p.speed && t == delay {
                    row_start(&mut p, c, &cell, tick_beat);
                }
                effects(&mut p, c, &cell, tick, t, tick_beat, &mut jump, &mut brk, &mut loop_to, row);
                // Record the tick.
                let (chan_pan, chan_vol) = (p.chans[c].pan, p.chans[c].chan_vol);
                let tremolo = trem_offset(&p, c, &cell, tick);
                let mut voice = p.chans[c].voice.take();
                if let Some(v) = voice.as_mut() {
                    let (level, pan, alive) = p.advance(v, chan_pan, chan_vol, tremolo);
                    let (vib, arp) = pitch_mods(&p, c, &cell, tick, v.units);
                    let arp_param = if let Fx::Arpeggio(_) = cell.fx { p.chans[c].mem.arp } else { 0 };
                    let pitch = p.pitch_of(v.units);
                    let n = &mut p.notes[v.note];
                    n.ticks.push(Tick { beat: tick_beat, pitch, vibrato: vib, arpeggio: arp, arp_param, level, pan });
                    if !alive {
                        n.end = n.end.min(tick_beat + p.tick_beats);
                        voice = None;
                    }
                }
                p.chans[c].voice = voice;
                // The voices behind the channel go on by themselves.
                let mut behind = std::mem::take(&mut p.chans[c].behind);
                behind.retain_mut(|v| {
                    let (level, pan, alive) = p.advance(v, chan_pan, chan_vol, 0.0);
                    let pitch = p.pitch_of(v.units);
                    let n = &mut p.notes[v.note];
                    n.ticks.push(Tick { beat: tick_beat, pitch, vibrato: 0.0, arpeggio: 0.0, arp_param: 0, level, pan });
                    if !alive || level <= 0.0005 {
                        n.end = n.end.min(tick_beat + p.tick_beats);
                    }
                    alive && level > 0.0005
                });
                p.chans[c].behind = behind;
            }
            p.seconds += p.tick_seconds();
            // A tempo slide moves the tempo every tick but the first.
            for cell in &cells {
                if let Fx::TempoSlide(d) = cell.fx {
                    if tick > 0 {
                        p.tempo = (p.tempo + d as f64).clamp(32.0, 255.0);
                    }
                }
            }
        }
        p.beat = row_beat + 1.0 / rows_per_beat as f64;
        // Where the song goes next.
        if let Some(to) = loop_to {
            row = to;
            continue;
        }
        match (jump, brk) {
            (Some(j), b) => {
                order = j;
                row = b.unwrap_or(0);
            }
            (None, Some(b)) => {
                order += 1;
                row = b;
            }
            (None, None) => {
                row += 1;
                if row >= pattern.rows {
                    row = 0;
                    order += 1;
                }
            }
        }
    }
    let end = p.beat;
    for c in 0..channels {
        p.stop(c, end);
        for v in std::mem::take(&mut p.chans[c].behind) {
            p.notes[v.note].end = p.notes[v.note].end.min(end);
        }
    }
    let mut notes = p.notes;
    for n in notes.iter_mut() {
        if !n.end.is_finite() {
            n.end = end;
        }
        // The note ends where it falls silent for good.
        if let Some(last) = n.ticks.iter().rposition(|t| t.level > 0.001) {
            let silent_from = n.ticks.get(last + 1).map(|t| t.beat);
            if let Some(b) = silent_from {
                n.end = n.end.min(b);
            }
            n.ticks.truncate(last + 2);
        } else {
            n.ticks.clear();
        }
    }
    notes.retain(|n| !n.ticks.is_empty() && n.end > n.beat);
    Played { notes, tempo: tempo_map, orders: orders_played, end, rows_per_beat, ticks_per_beat, seconds: p.seconds }
}

/// A cell's note, instrument and volume on the tick it acts.
fn row_start(p: &mut Player, c: usize, cell: &super::Cell, beat: f64) {
    let porta = matches!(cell.fx, Fx::TonePorta(_) | Fx::TonePortaVolSlide(_)) || matches!(cell.vol, Vol::TonePorta(_));
    if cell.inst > 0 {
        let inst = cell.inst as usize - 1;
        if inst < p.m.instruments.len() {
            p.chans[c].inst = inst;
        }
        // An instrument number resets the volume to its sample's.
        let sample = if cell.note >= 0 {
            p.m.instruments.get(p.chans[c].inst).and_then(|i| i.keymap.get(cell.note as usize).copied().flatten()).map(|(s, _)| s)
        } else {
            p.chans[c].voice.as_ref().map(|v| v.sample)
        };
        if let Some(s) = sample.and_then(|s| p.m.samples.get(s)) {
            let vol = s.volume * 64.0;
            if let Some(v) = p.chans[c].voice.as_mut() {
                v.volume = vol;
                if cell.note < 0 || porta {
                    // XM: the envelopes restart with the instrument.
                    v.env_tick = 0.0;
                    v.fade = 1.0;
                    v.fading = false;
                    v.key_on = true;
                }
            }
        }
    }
    match cell.note {
        NOTE_NONE => {}
        NOTE_OFF => p.key_off(c, beat),
        NOTE_CUT => p.stop(c, beat),
        NOTE_FADE => {
            if let Some(v) = p.chans[c].voice.as_mut() {
                v.fading = true;
            }
        }
        note if porta && p.chans[c].voice.is_some() => {
            let key = p.m.instruments.get(p.chans[c].inst).and_then(|i| i.keymap.get(note as usize).copied().flatten()).map_or(note as f32, |(_, k)| k as f32);
            p.chans[c].target = Some(p.units_of(key));
        }
        note => {
            let mut offset = 0;
            for fx in [cell.fx, cell.fx2] {
                if let Fx::Offset(o) = fx {
                    let o = if o == 0 { p.chans[c].mem.offset } else { o };
                    p.chans[c].mem.offset = o;
                    offset = (p.chans[c].offset_high as usize) << 16 | (o as usize) << 8;
                }
            }
            p.strike(c, note, offset, beat);
            // A note with an instrument number takes the sample's volume;
            // without one, MOD and S3M keep the channel's.
            if cell.inst == 0 {
                let vol = p.chans[c].voice.as_ref().map(|v| p.sample_volume(v));
                if let (Some(v), Some(vol)) = (p.chans[c].voice.as_mut(), vol) {
                    v.volume = vol;
                }
            }
        }
    }
    match cell.vol {
        Vol::Set(v) => {
            if let Some(voice) = p.chans[c].voice.as_mut() {
                voice.volume = v as f32;
            }
        }
        Vol::Pan(v) => p.chans[c].pan = v as f32 / 64.0 * 2.0 - 1.0,
        _ => {}
    }
}

/// The tremolo and tremor's change to the volume now (in 0..64 units).
fn trem_offset(p: &Player, c: usize, cell: &super::Cell, _tick: u32) -> f32 {
    let ch = &p.chans[c];
    let mut out = 0.0;
    if matches!(cell.fx, Fx::Tremolo(..)) {
        out += vib_wave(ch.trem_pos) * ch.mem.trem_depth as f32 * 4.0 / 4.0;
    }
    if matches!(cell.fx, Fx::Tremor(_)) && !ch.tremor_on {
        out -= 64.0;
    }
    out
}

/// The vibrato's and arpeggio's offsets in semitones on this tick.
fn pitch_mods(p: &Player, c: usize, cell: &super::Cell, tick: u32, units: f64) -> (f32, f32) {
    let ch = &p.chans[c];
    let mut vib = 0.0;
    let vib_on = matches!(cell.fx, Fx::Vibrato(..) | Fx::FineVibrato(..) | Fx::VibratoVolSlide(_)) || matches!(cell.vol, Vol::VibratoDepth(_));
    if vib_on {
        let fine = matches!(cell.fx, Fx::FineVibrato(..));
        vib = vib_wave(ch.vib_pos) * p.vib_semis(ch.mem.vib_depth, fine, units);
    }
    let mut arp = 0.0;
    if let Fx::Arpeggio(_) = cell.fx {
        let a = ch.mem.arp;
        arp = match tick % 3 {
            0 => 0.0,
            1 => (a >> 4) as f32,
            _ => (a & 15) as f32,
        };
    }
    (vib, arp)
}

#[allow(clippy::too_many_arguments)]
fn effects(p: &mut Player, c: usize, cell: &super::Cell, tick: u32, t: u32, beat: f64, jump: &mut Option<usize>, brk: &mut Option<usize>, loop_to: &mut Option<usize>, row: usize) {
    let first = t == 0;
    // The volume column.
    match cell.vol {
        Vol::Slide(d) if tick > 0 => {
            if let Some(v) = p.chans[c].voice.as_mut() {
                v.volume = (v.volume + d as f32).clamp(0.0, 64.0);
            }
        }
        Vol::FineSlide(d) if first => {
            if let Some(v) = p.chans[c].voice.as_mut() {
                v.volume = (v.volume + d as f32).clamp(0.0, 64.0);
            }
        }
        Vol::PanSlide(d) if tick > 0 => p.chans[c].pan = (p.chans[c].pan + d as f32 / 32.0).clamp(-1.0, 1.0),
        Vol::TonePorta(s) => p.tone_porta(c, s, tick),
        Vol::VibratoSpeed(s) if first => p.chans[c].mem.vib_speed = s,
        Vol::VibratoDepth(d) if first => p.vibrato(c, 0, d),
        Vol::PortaUp(u) | Vol::PortaDown(u) if tick > 0 => {
            let up = matches!(cell.vol, Vol::PortaUp(_));
            let mut v = p.chans[c].voice.take();
            if let Some(v) = v.as_mut() {
                p.slide(v, if up { u as f64 * 4.0 } else { -(u as f64) * 4.0 });
            }
            p.chans[c].voice = v;
        }
        _ => {}
    }
    for fx in [cell.fx, cell.fx2] {
        match fx {
            Fx::None => {}
            Fx::Arpeggio(a) => {
                if first {
                    let a = if a == 0 { p.chans[c].mem.arp } else { a };
                    p.chans[c].mem.arp = a;
                    if let Some(v) = p.chans[c].voice.as_ref() {
                        let n = &mut p.notes[v.note];
                        n.arpeggio = n.arpeggio.or(Some((a >> 4, a & 15)));
                    }
                }
            }
            Fx::PortaUp(x) | Fx::PortaDown(x) => {
                let up = matches!(fx, Fx::PortaUp(_));
                let mem = if up { &mut p.chans[c].mem.porta_up } else { &mut p.chans[c].mem.porta_down };
                let x = if x == 0 && p.m.kind != Kind::Mod { *mem } else { x };
                *mem = x;
                if tick > 0 {
                    let mut v = p.chans[c].voice.take();
                    if let Some(v) = v.as_mut() {
                        p.slide(v, if up { x as f64 * 4.0 } else { -(x as f64) * 4.0 });
                    }
                    p.chans[c].voice = v;
                }
            }
            Fx::FinePortaUp(x) | Fx::FinePortaDown(x) | Fx::ExtraFinePortaUp(x) | Fx::ExtraFinePortaDown(x) => {
                if first {
                    let up = matches!(fx, Fx::FinePortaUp(_) | Fx::ExtraFinePortaUp(_));
                    let scale = if matches!(fx, Fx::FinePortaUp(_) | Fx::FinePortaDown(_)) { 4.0 } else { 1.0 };
                    let mut v = p.chans[c].voice.take();
                    if let Some(v) = v.as_mut() {
                        p.slide(v, if up { x as f64 * scale } else { -(x as f64) * scale });
                    }
                    p.chans[c].voice = v;
                }
            }
            Fx::S3mPortaUp(x) | Fx::S3mPortaDown(x) => {
                let up = matches!(fx, Fx::S3mPortaUp(_));
                let x = if x == 0 { p.chans[c].mem.porta } else { x };
                p.chans[c].mem.porta = x;
                let units = if x >= 0xF0 {
                    if first { (x & 15) as f64 * 4.0 } else { 0.0 }
                } else if x >= 0xE0 {
                    if first { (x & 15) as f64 } else { 0.0 }
                } else if tick > 0 {
                    x as f64 * 4.0
                } else {
                    0.0
                };
                if units > 0.0 {
                    let mut v = p.chans[c].voice.take();
                    if let Some(v) = v.as_mut() {
                        p.slide(v, if up { units } else { -units });
                    }
                    p.chans[c].voice = v;
                }
            }
            Fx::TonePorta(x) => p.tone_porta(c, x, tick),
            Fx::TonePortaVolSlide(s) => {
                p.tone_porta(c, 0, tick);
                p.vol_slide(c, s, tick);
            }
            Fx::Vibrato(s, d) | Fx::FineVibrato(s, d) => {
                if first {
                    p.vibrato(c, s, d);
                }
            }
            Fx::VibratoVolSlide(s) => p.vol_slide(c, s, tick),
            Fx::Tremolo(s, d) => {
                if first {
                    if s > 0 {
                        p.chans[c].mem.trem_speed = s;
                    }
                    if d > 0 {
                        p.chans[c].mem.trem_depth = d;
                    }
                }
            }
            Fx::Pan(v) => {
                if first {
                    p.chans[c].pan = v as f32 / 255.0 * 2.0 - 1.0;
                }
            }
            Fx::Offset(_) => {}
            Fx::HighOffset(h) => {
                if first {
                    p.chans[c].offset_high = h;
                }
            }
            Fx::VolSlide(s) => p.vol_slide_mod(c, s, tick),
            Fx::S3mVolSlide(s) => p.vol_slide_s3m(c, s, tick),
            Fx::FineVolUp(x) | Fx::FineVolDown(x) => {
                if first {
                    let d = if matches!(fx, Fx::FineVolUp(_)) { x as f32 } else { -(x as f32) };
                    if let Some(v) = p.chans[c].voice.as_mut() {
                        v.volume = (v.volume + d).clamp(0.0, 64.0);
                    }
                }
            }
            Fx::Jump(o) => {
                if first {
                    *jump = Some(o as usize);
                }
            }
            Fx::Break(r) => {
                if first {
                    *brk = Some(r as usize);
                }
            }
            Fx::Volume(v) => {
                if first {
                    if let Some(voice) = p.chans[c].voice.as_mut() {
                        voice.volume = v as f32;
                    }
                }
            }
            Fx::Speed(_) | Fx::Tempo(_) | Fx::TempoSlide(_) | Fx::PatternDelay(_) | Fx::NoteDelay(_) => {}
            Fx::Retrig(x) | Fx::MultiRetrig(x) => {
                let multi = matches!(fx, Fx::MultiRetrig(_));
                let x = if x == 0 { p.chans[c].mem.retrig } else { x };
                p.chans[c].mem.retrig = x;
                let every = if multi { (x & 15) as u32 } else { x as u32 };
                if every > 0 && t > 0 && t % every == 0 {
                    let key = p.chans[c].voice.as_ref().map(|v| (p.notes[v.note].key, v.volume));
                    if let Some((key, vol)) = key {
                        let vol = if multi {
                            match x >> 4 {
                                1..=5 => vol - (1 << ((x >> 4) - 1)) as f32,
                                6 => vol * 2.0 / 3.0,
                                7 => vol / 2.0,
                                9..=13 => vol + (1 << ((x >> 4) - 9)) as f32,
                                14 => vol * 1.5,
                                15 => vol * 2.0,
                                _ => vol,
                            }
                        } else {
                            vol
                        };
                        // The key as struck; the key map gives the sample back.
                        let note = p.m.instruments.get(p.chans[c].inst).and_then(|i| i.keymap.iter().position(|k| k.is_some_and(|k| k.1 as f32 == key))).unwrap_or(key as usize) as i16;
                        p.strike(c, note, 0, beat);
                        if let Some(v) = p.chans[c].voice.as_mut() {
                            v.volume = vol.clamp(0.0, 64.0);
                        }
                    }
                }
            }
            Fx::NoteCut(x) => {
                if tick == x as u32 {
                    if let Some(v) = p.chans[c].voice.as_mut() {
                        v.volume = 0.0;
                    }
                }
            }
            Fx::PatternLoop(x) => {
                if first {
                    let ch = &mut p.chans[c];
                    if x == 0 {
                        ch.loop_row = row;
                    } else if ch.loop_count == 0 {
                        ch.loop_count = x;
                        *loop_to = Some(ch.loop_row);
                    } else {
                        ch.loop_count -= 1;
                        if ch.loop_count > 0 {
                            *loop_to = Some(ch.loop_row);
                        }
                    }
                }
            }
            Fx::GlobalVolume(v) => {
                if first {
                    p.global = v as f32 / 64.0;
                }
            }
            Fx::GlobalVolSlide(s) => {
                let s = if s == 0 { p.chans[c].mem.global_slide } else { s };
                p.chans[c].mem.global_slide = s;
                if tick > 0 {
                    let (x, y) = ((s >> 4) as f32, (s & 15) as f32);
                    let d = if x > 0.0 { x } else { -y };
                    p.global = (p.global + d / 64.0).clamp(0.0, 1.0);
                }
            }
            Fx::KeyOff(x) => {
                if tick == x as u32 {
                    p.key_off(c, beat);
                }
            }
            Fx::ChannelVolume(v) => {
                if first {
                    p.chans[c].chan_vol = v as f32 / 64.0;
                }
            }
            Fx::ChannelVolSlide(s) => {
                let s = if s == 0 { p.chans[c].mem.chan_slide } else { s };
                p.chans[c].mem.chan_slide = s;
                let (x, y) = ((s >> 4) as f32, (s & 15) as f32);
                let d = if x == 15.0 && y > 0.0 {
                    if first { -y } else { 0.0 }
                } else if y == 15.0 && x > 0.0 {
                    if first { x } else { 0.0 }
                } else if tick > 0 {
                    if x > 0.0 { x } else { -y }
                } else {
                    0.0
                };
                p.chans[c].chan_vol = (p.chans[c].chan_vol + d / 64.0).clamp(0.0, 1.0);
            }
            Fx::PanSlide(s) => {
                let s = if s == 0 { p.chans[c].mem.pan_slide } else { s };
                p.chans[c].mem.pan_slide = s;
                if tick > 0 {
                    let (x, y) = ((s >> 4) as f32, (s & 15) as f32);
                    // IT: x moves left, y right.
                    let d = if x > 0.0 { -x } else { y };
                    p.chans[c].pan = (p.chans[c].pan + d / 32.0).clamp(-1.0, 1.0);
                }
            }
            Fx::Tremor(x) => {
                let x = if x == 0 { p.chans[c].mem.tremor } else { x };
                p.chans[c].mem.tremor = x;
                let (on, off) = ((x >> 4) + 1, (x & 15) + 1);
                let ch = &mut p.chans[c];
                if first && t == 0 && ch.tremor_count == 0 {
                    ch.tremor_on = true;
                }
                ch.tremor_count += 1;
                let limit = if ch.tremor_on { on } else { off };
                if ch.tremor_count >= limit {
                    ch.tremor_on = !ch.tremor_on;
                    ch.tremor_count = 0;
                }
            }
            Fx::Finetune(_) => {}
            Fx::EnvelopePosition(x) => {
                if first {
                    if let Some(v) = p.chans[c].voice.as_mut() {
                        v.env_tick = x as f32;
                    }
                }
            }
            Fx::NoteAction(x) => {
                if first {
                    match x {
                        0 => {
                            for v in std::mem::take(&mut p.chans[c].behind) {
                                p.notes[v.note].end = p.notes[v.note].end.min(beat);
                            }
                        }
                        1 => p.chans[c].behind.iter_mut().for_each(|v| v.key_on = false),
                        2 => p.chans[c].behind.iter_mut().for_each(|v| v.fading = true),
                        3..=6 => p.chans[c].nna_override = Some(x - 3),
                        _ => {}
                    }
                }
            }
        }
    }
    // The vibrato and tremolo move on every tick but the first.
    let vib = matches!(cell.fx, Fx::Vibrato(..) | Fx::FineVibrato(..) | Fx::VibratoVolSlide(_)) || matches!(cell.vol, Vol::VibratoDepth(_));
    if vib {
        let (speed, depth) = (p.chans[c].mem.vib_speed, p.chans[c].mem.vib_depth);
        let fine = matches!(cell.fx, Fx::FineVibrato(..));
        let units = p.chans[c].voice.as_ref().map(|v| v.units);
        if let (Some(v), Some(units)) = (p.chans[c].voice.as_ref(), units) {
            let hz = speed as f32 / 64.0 / p.tick_seconds() as f32;
            let semis = p.vib_semis(depth, fine, units);
            let n = &mut p.notes[v.note];
            if n.vibrato.is_none_or(|(_, d)| d < semis) {
                n.vibrato = Some((hz, semis));
            }
        }
        if tick > 0 || p.m.kind == Kind::It && !p.m.it_old_effects {
            p.chans[c].vib_pos = p.chans[c].vib_pos.wrapping_add(speed * 4) & 255;
        }
    }
    if matches!(cell.fx, Fx::Tremolo(..)) && tick > 0 {
        let s = p.chans[c].mem.trem_speed;
        p.chans[c].trem_pos = p.chans[c].trem_pos.wrapping_add(s * 4) & 255;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::tracker::{Cell, Instrument, Kind, Loop, Module, Pattern, Sample};

    pub(crate) fn module(cells: &[(usize, usize, Cell)], rows: usize) -> Module {
        let mut pat = Pattern::empty(rows, 2);
        for &(row, ch, cell) in cells {
            pat.cells[row * 2 + ch] = cell;
        }
        let mut s = Sample::silent("square".into());
        s.left = (0..64).map(|i| if i < 32 { 0.5 } else { -0.5 }).collect();
        s.looped = Some(Loop { from: 0, to: 64, pingpong: false });
        Module {
            kind: Kind::Mod,
            title: "test".into(),
            tracker: String::new(),
            channels: 2,
            orders: vec![0],
            restart: 0,
            patterns: vec![pat],
            instruments: vec![Instrument::of_sample("square".into(), 0)],
            samples: vec![s],
            speed: 6,
            tempo: 125,
            global_volume: 1.0,
            linear: false,
            pans: vec![0x40, 0xC0],
            channel_volumes: vec![1.0; 2],
            rows_per_beat: None,
            message: String::new(),
            s3m_slides: false,
            it_old_effects: false,
            it_compat_gxx: false,
        }
    }

    fn note(n: i16, fx: Fx) -> Cell {
        Cell { note: n, inst: 1, vol: Vol::None, fx, fx2: Fx::None }
    }

    #[test]
    fn rows_are_sixteenths_and_notes_end_at_the_next() {
        let m = module(&[(0, 0, note(60, Fx::None)), (4, 0, note(64, Fx::None)), (8, 1, note(48, Fx::Volume(32)))], 16);
        let played = play(&m);
        assert_eq!(played.rows_per_beat, 4);
        assert!((played.tempo[0].1 - 125.0).abs() < 1e-9);
        assert_eq!(played.notes.len(), 3);
        assert_eq!((played.notes[0].beat, played.notes[0].end, played.notes[0].key), (0.0, 1.0, 60.0));
        assert_eq!((played.notes[1].beat, played.notes[1].key), (1.0, 64.0));
        assert!((played.notes[2].ticks[0].level - 0.5).abs() < 1e-6);
        assert!(played.notes[2].ticks[0].pan > 0.0 && played.notes[0].ticks[0].pan < 0.0);
        assert!((played.end - 4.0).abs() < 1e-9);
    }

    #[test]
    fn slides_vibrato_arpeggio_and_volume() {
        let m = module(
            &[(0, 0, note(60, Fx::PortaUp(8))), (1, 0, Cell { fx: Fx::VolSlide(0x02), ..Cell::EMPTY }), (4, 1, note(60, Fx::Arpeggio(0x47))), (8, 1, Cell { fx: Fx::Vibrato(4, 8), ..Cell::EMPTY })],
            16,
        );
        let played = play(&m);
        let up = &played.notes[0];
        // Five ticks of 8 Amiga periods up from 428: ~1.5 semitones.
        let after = up.ticks[5].pitch;
        assert!(after > 61.0 && after < 62.5, "{after}");
        // The volume slide: 5 ticks of -2 on row 1.
        let lvl = up.ticks[11].level;
        assert!((lvl - (64.0 - 10.0) / 64.0).abs() < 0.02, "{lvl}");
        let arp = &played.notes[1];
        assert_eq!(arp.arpeggio, Some((4, 7)));
        let offsets: Vec<f32> = arp.ticks[..6].iter().map(|t| t.arpeggio).collect();
        assert_eq!(offsets, vec![0.0, 4.0, 7.0, 0.0, 4.0, 7.0]);
        assert!(arp.vibrato.is_some_and(|(hz, d)| hz > 1.0 && d > 0.2 && d < 1.0));
    }

    #[test]
    fn jumps_and_loops_end_the_song() {
        // Row 3 jumps back to order 0: the song is 4 rows long.
        let m = module(&[(0, 0, note(60, Fx::None)), (3, 0, Cell { fx: Fx::Jump(0), ..Cell::EMPTY })], 16);
        let played = play(&m);
        assert!((played.end - 1.0).abs() < 1e-9);
        // A pattern loop plays rows 0-1 three times.
        let m = module(&[(0, 0, Cell { fx: Fx::PatternLoop(0), ..note(60, Fx::None) }), (1, 0, Cell { fx: Fx::PatternLoop(2), ..Cell::EMPTY }), (2, 0, Cell { fx: Fx::Break(0), ..Cell::EMPTY })], 16);
        let played = play(&m);
        assert_eq!(played.notes.len(), 3);
        assert!((played.end - 7.0 / 4.0).abs() < 1e-9, "{}", played.end);
    }

    #[test]
    fn speed_changes_are_tempo_changes() {
        let m = module(&[(0, 0, note(60, Fx::Speed(3))), (4, 0, note(60, Fx::Speed(6)))], 8);
        let played = play(&m);
        // Speed 3 doubles 125 to 250: eight rows a beat.
        assert_eq!(played.rows_per_beat, 4);
        assert_eq!(played.tempo, vec![(0.0, 250.0), (1.0, 125.0)]);
    }
}
