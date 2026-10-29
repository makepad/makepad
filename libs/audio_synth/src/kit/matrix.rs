//! The kit's mod matrix: rows of source → destination with an amount, a
//! curve that bends the source, a polarity (unipolar sweeps up from where
//! the knob is, bipolar around it), an aux source that scales the row (or
//! 1 - aux), and an output scale. Destinations are any modulatable
//! parameter of the layout's schema; amounts are in knob travel.

use super::env::bend;
use super::schema::{num, Schema};

pub const SOURCES: &[&str] = &[
    "none", "env1", "env2", "env3", "env4", "lfo1", "lfo2", "lfo3", "lfo4", "lfo5", "lfo6", "lfo7", "lfo8", "lfo1y", "lfo2y", "lfo3y",
    "lfo4y", "lfo5y", "lfo6y", "lfo7y", "lfo8y", "vel", "note", "rand", "alt", "mod", "bend", "aftertouch", "pressure", "slide", "m1", "m2",
    "m3", "m4", "m5", "m6", "m7", "m8", "const", "noise", "gate", "rand2", "release_vel", "voices",
];
pub const N_SOURCES: usize = 44;
pub const SRC_ENV1: usize = 1;
pub const SRC_LFO1: usize = 5;
pub const SRC_LFO1Y: usize = 13;
pub const SRC_VEL: usize = 21;
pub const SRC_NOTE: usize = 22;
pub const SRC_RAND: usize = 23;
pub const SRC_ALT: usize = 24;
pub const SRC_MOD: usize = 25;
pub const SRC_BEND: usize = 26;
pub const SRC_AFTERTOUCH: usize = 27;
pub const SRC_PRESSURE: usize = 28;
pub const SRC_SLIDE: usize = 29;
pub const SRC_M1: usize = 30;
pub const SRC_CONST: usize = 38;
pub const SRC_NOISE: usize = 39;
pub const SRC_GATE: usize = 40;
pub const SRC_RAND2: usize = 41;
pub const SRC_RELEASE_VEL: usize = 42;
pub const SRC_VOICES: usize = 43;
pub const MAX_ROWS: usize = 64;

pub const ABOUT: &str = "sources: env1-4 (env1 is the amp), lfo1-8 and lfo1y-8y (an LFO's second output: a path's y, else a quarter-cycle on), vel, note (key -1..1 around c4), rand and rand2 (per note), alt (+-1 on alternate notes), mod (wheel), bend, aftertouch, pressure and slide (per-note expression), m1-m8 (macros), const, noise, gate (1 while held), release_vel, voices (sounding / poly). A row: [@src @dst amount] then in any order @aux (a second source that scales the row; @inv: by 1-aux), a curve -1..1 (bends the source: + slow start), @uni or @bi (unipolar sweeps up from the knob, bipolar around it; default the source's own), and a last number: output scale. amount is in the dst's knob travel (-1..1).";

/// Whether a source swings -1..1 by nature (LFOs follow their `uni`).
pub fn bipolar(src: usize) -> bool {
    (SRC_LFO1..SRC_VEL).contains(&src) || matches!(src, SRC_NOTE | SRC_ALT | SRC_BEND | SRC_NOISE)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModRow {
    pub src: u8,
    pub dst: u16,
    pub amount: f32,
    pub aux: u8,
    pub aux_inv: bool,
    pub curve: f32,
    /// 0 the source's own, 1 unipolar, 2 bipolar.
    pub polarity: u8,
    pub output: f32,
}

impl ModRow {
    /// A row from its words: `lfo1 a_pos 0.4 m1 inv 0.3 bi 0.5`.
    pub fn parse(words: &[&str], schema: &Schema) -> Result<ModRow, String> {
        let atom = |w: &str| w.trim().trim_start_matches('@').to_string();
        let source = |w: &str| SOURCES.iter().position(|s| *s == atom(w));
        if words.len() < 3 {
            return Err("a mod row is [@source @destination amount ...]".into());
        }
        let src = source(words[0]).ok_or_else(|| format!("`{}` is not a mod source ({})", atom(words[0]), SOURCES.join(" ")))?;
        let dst_key = atom(words[1]);
        let dst = schema.index(&dst_key).ok_or_else(|| format!("`{dst_key}` is not a parameter"))?;
        if !schema.params[dst].modulatable {
            return Err(format!("`{dst_key}` cannot be modulated (choices and counts are set, not moved)"));
        }
        let amount: f32 = words[2].trim().parse().map_err(|_| format!("`{}` is not an amount (-1..1)", words[2]))?;
        let mut row = ModRow { src: src as u8, dst: dst as u16, amount: amount.clamp(-1.0, 1.0), aux: 0, aux_inv: false, curve: 0.0, polarity: 0, output: 1.0 };
        let mut numbers = 0;
        for w in &words[3..] {
            let a = atom(w);
            if a.is_empty() {
                continue;
            }
            if a == "inv" {
                row.aux_inv = true;
            } else if a == "uni" {
                row.polarity = 1;
            } else if a == "bi" {
                row.polarity = 2;
            } else if let Some(s) = source(&a) {
                row.aux = s as u8;
            } else if let Ok(v) = a.parse::<f32>() {
                if numbers == 0 {
                    row.curve = v.clamp(-1.0, 1.0);
                } else {
                    row.output = v.clamp(-4.0, 4.0);
                }
                numbers += 1;
            } else {
                return Err(format!("`{a}` in a mod row is not an aux source, a number, @uni, @bi or @inv"));
            }
        }
        Ok(row)
    }

    /// The row as words (`lfo1 a_pos 0.4 m1 inv 0.3`), what `parse` reads.
    pub fn words(&self, schema: &Schema) -> Vec<String> {
        let mut out = vec![SOURCES[self.src as usize].to_string(), schema.params[self.dst as usize].key.clone(), num(self.amount)];
        if self.aux != 0 {
            out.push(SOURCES[self.aux as usize].to_string());
        }
        if self.aux_inv {
            out.push("inv".into());
        }
        match self.polarity {
            1 => out.push("uni".into()),
            2 => out.push("bi".into()),
            _ => {}
        }
        if self.curve != 0.0 || self.output != 1.0 {
            out.push(num(self.curve));
        }
        if self.output != 1.0 {
            out.push(num(self.output));
        }
        out
    }
}

/// A patch's rows, ready to apply: each row's slot among the distinct
/// destinations.
#[derive(Clone, Debug, Default)]
pub struct Matrix {
    pub rows: Vec<ModRow>,
    /// The distinct destinations, and each row's index into them.
    pub dsts: Vec<u16>,
    slot: Vec<u16>,
}

impl Matrix {
    pub fn new(rows: Vec<ModRow>) -> Matrix {
        let mut dsts: Vec<u16> = Vec::new();
        let mut slot = Vec::with_capacity(rows.len());
        for r in &rows {
            let i = match dsts.iter().position(|d| *d == r.dst) {
                Some(i) => i,
                None => {
                    dsts.push(r.dst);
                    dsts.len() - 1
                }
            };
            slot.push(i as u16);
        }
        Matrix { rows, dsts, slot }
    }

    /// Whether any row reads `src`.
    pub fn reads(&self, src: usize) -> bool {
        self.rows.iter().any(|r| r.src as usize == src || r.aux as usize == src)
    }

    /// The modulated values: for every destination, `vals[dst]` = the base
    /// value moved by its rows in knob travel. `sources` are the current
    /// source values, `bip` whether each is bipolar now; `acc` is scratch
    /// (at least `dsts.len()` long).
    pub fn apply(&self, schema: &Schema, base: &[f32], sources: &[f32; N_SOURCES], bip: &[bool; N_SOURCES], acc: &mut [f32], vals: &mut [f32]) {
        let acc = &mut acc[..self.dsts.len()];
        acc.fill(0.0);
        for (r, slot) in self.rows.iter().zip(self.slot.iter()) {
            let s = sources[r.src as usize];
            let nat = bip[r.src as usize];
            let mut u = if nat { s * 0.5 + 0.5 } else { s };
            u = u.clamp(0.0, 1.0);
            if r.curve != 0.0 {
                u = bend(u, r.curve);
            }
            let bi = r.polarity == 2 || (r.polarity == 0 && nat);
            let v = if bi { 2.0 * u - 1.0 } else { u };
            let mut a = 1.0;
            if r.aux != 0 {
                let x = sources[r.aux as usize];
                a = if bip[r.aux as usize] { x * 0.5 + 0.5 } else { x }.clamp(0.0, 1.0);
                if r.aux_inv {
                    a = 1.0 - a;
                }
            }
            acc[*slot as usize] += v * r.amount * a * r.output;
        }
        for (d, add) in self.dsts.iter().zip(acc.iter()) {
            let p = &schema.params[*d as usize];
            let i = *d as usize;
            vals[i] = p.denorm(p.norm(base[i]) + add);
        }
    }
}
