//! A Morph patch: every parameter's value, the mod rows, the LFOs' drawn
//! points, the rack's order, the score's own wavetable frames and the
//! macros' names. One text form (`key=value` words) that a score, a test or
//! a host writes and [`MorphPatch::parse`] reads, checking every key.

use super::{layout, PRESETS};
use crate::kit::matrix::{Matrix, ModRow, MAX_ROWS};
use crate::kit::osc::{OscSource, SampleBuf};
use crate::kit::schema::{num, Curve};
use crate::kit::table::{self, Table, MAX_FRAMES};
use crate::kit::tuning::{self, KeyMap, Scale, Tuning};
use crate::kit::{fx, lfo};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct MorphPatch {
    pub values: Vec<f32>,
    pub matrix: Matrix,
    pub points: [Vec<[f32; 3]>; 8],
    /// The rack, as indices into [`fx::KINDS`].
    pub fx: Vec<u8>,
    /// Partial lists per frame for oscillators whose table is `user`.
    pub frames: [Vec<Vec<f32>>; 3],
    pub macro_names: Vec<String>,
    /// A scale of its own (degrees in cents, the last the period) over the
    /// `tuning` choice; empty = the choice.
    pub scale: Vec<f64>,
}

impl Default for MorphPatch {
    fn default() -> Self {
        MorphPatch { values: layout().schema.defaults(), matrix: Matrix::default(), points: Default::default(), fx: Vec::new(), frames: Default::default(), macro_names: Vec::new(), scale: Vec::new() }
    }
}

impl PartialEq for MorphPatch {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values && self.matrix.rows == other.matrix.rows && self.points == other.points && self.fx == other.fx && self.frames == other.frames && self.macro_names == other.macro_names && self.scale == other.scale
    }
}

fn words(value: &str) -> Vec<&str> {
    value.split(',').map(str::trim).filter(|w| !w.is_empty()).collect()
}

impl MorphPatch {
    /// A patch from its text: `preset=name` first if any, then each word.
    pub fn parse(text: &str) -> Result<MorphPatch, String> {
        let mut patch = MorphPatch::default();
        let mut rows = Vec::new();
        let tokens: Vec<&str> = text.split_whitespace().collect();
        if let Some(name) = tokens.iter().find_map(|t| t.strip_prefix("preset=")) {
            let name = name.trim_start_matches('@');
            let (_, _, preset) = PRESETS.iter().find(|(n, ..)| *n == name).ok_or_else(|| {
                format!("`{name}` is not a Morph preset ({})", PRESETS.iter().map(|p| p.0).collect::<Vec<_>>().join(" "))
            })?;
            patch = MorphPatch::parse(preset)?;
            rows = patch.matrix.rows.clone();
        }
        for token in tokens {
            let (key, value) = token.split_once('=').ok_or_else(|| format!("`{token}` is not key=value"))?;
            if key == "preset" {
                continue;
            }
            if key == "mod" {
                if rows.len() >= MAX_ROWS {
                    return Err(format!("at most {MAX_ROWS} mod rows"));
                }
                rows.push(ModRow::parse(&words(value), &layout().schema)?);
                continue;
            }
            patch.set(key, value)?;
        }
        patch.matrix = Matrix::new(rows);
        Ok(patch)
    }

    /// Set one key from its text (`a_table`, `@formant`; `fx`, `delay,reverb`).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let schema = &layout().schema;
        match key {
            "fx" => {
                self.fx.clear();
                for w in words(value) {
                    let w = w.trim_start_matches('@');
                    let kind = fx::kind(w).ok_or_else(|| format!("`{w}` is not an effect ({})", fx::kind_names()))?;
                    if self.fx.contains(&kind) {
                        return Err(format!("`{w}` is in the rack twice (each effect once)"));
                    }
                    self.fx.push(kind);
                }
                return Ok(());
            }
            "tuning_scale" => {
                // Degrees as cents (`386.3`) or ratios (`5/4`), the last the period.
                let mut text = format!("scale\n{}\n", words(value).len());
                for w in words(value) {
                    text.push_str(w);
                    text.push('\n');
                }
                // Kept to a millionth of a cent so the text form round-trips.
                self.scale = crate::kit::tuning::Scale::parse(&text).map_err(|e| format!("`tuning_scale`: {e}"))?.cents.iter().map(|c| (c * 1e6).round() / 1e6).collect();
                return Ok(());
            }
            "macro_names" => {
                self.macro_names = words(value).iter().take(8).map(|s| s.to_string()).collect();
                return Ok(());
            }
            _ => {}
        }
        if let Some(n) = key.strip_prefix("lfo").and_then(|k| k.strip_suffix("_points")).and_then(|n| n.parse::<usize>().ok()) {
            if !(1..=8).contains(&n) {
                return Err(format!("`{key}`: LFOs are 1-8"));
            }
            let mut pts = Vec::new();
            for p in words(value).iter().take(lfo::MAX_POINTS) {
                let f: Vec<f32> = p.split(':').map(|x| x.trim().parse::<f32>().map_err(|_| format!("`{p}` is not x:y:curve"))).collect::<Result<_, _>>()?;
                if f.len() < 2 {
                    return Err(format!("`{p}` is not x:y:curve"));
                }
                pts.push([f[0].clamp(0.0, 1.0), f[1].clamp(0.0, 1.0), f.get(2).copied().unwrap_or(0.0).clamp(-1.0, 1.0)]);
            }
            pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
            self.points[n - 1] = pts;
            return Ok(());
        }
        if let Some(o) = key.strip_suffix("_frames").and_then(|o| ["a", "b", "c"].iter().position(|x| *x == o)) {
            let mut frames = Vec::new();
            for f in value.split('/').filter(|f| !f.trim().is_empty()).take(MAX_FRAMES) {
                let amps: Vec<f32> = f.split(':').map(|x| x.trim().parse::<f32>().map_err(|_| format!("`{x}` is not a partial's amplitude"))).collect::<Result<_, _>>()?;
                frames.push(amps.into_iter().take(1024).collect());
            }
            self.frames[o] = frames;
            return Ok(());
        }
        let i = schema.index(key).ok_or_else(|| format!("`{key}` is not a Morph parameter"))?;
        let p = &schema.params[i];
        let value = value.trim();
        let v = match p.curve {
            Curve::Choice => match p.option(value) {
                Some(o) => o as f32,
                None => return Err(format!("`{key}` is one of @{}", p.options.join(" @"))),
            },
            Curve::Bool => match value {
                "true" | "on" | "1" => 1.0,
                "false" | "off" | "0" => 0.0,
                _ => return Err(format!("`{key}` is true or false")),
            },
            _ => value.parse::<f32>().map_err(|_| format!("`{key}` is a number ({}..{})", num(p.min), num(p.max)))?,
        };
        self.values[i] = p.clamp(v);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<f32> {
        layout().schema.index(key).map(|i| self.values[i])
    }

    /// The text that parses back to this patch (only what is not default).
    pub fn to_text(&self) -> String {
        let schema = &layout().schema;
        let mut out = Vec::new();
        for (p, v) in schema.params.iter().zip(self.values.iter()) {
            if *v != p.default {
                let t = p.text(*v);
                out.push(format!("{}={}", p.key, t.trim_start_matches('@')));
            }
        }
        for r in &self.matrix.rows {
            out.push(format!("mod={}", r.words(schema).join(",")));
        }
        for (n, pts) in self.points.iter().enumerate() {
            if !pts.is_empty() {
                let p: Vec<String> = pts.iter().map(|p| format!("{}:{}:{}", num(p[0]), num(p[1]), num(p[2]))).collect();
                out.push(format!("lfo{}_points={}", n + 1, p.join(",")));
            }
        }
        if !self.fx.is_empty() {
            out.push(format!("fx={}", self.fx.iter().map(|k| fx::KINDS[*k as usize].0).collect::<Vec<_>>().join(",")));
        }
        for (o, frames) in self.frames.iter().enumerate() {
            if !frames.is_empty() {
                let f: Vec<String> = frames.iter().map(|a| a.iter().map(|x| num(*x)).collect::<Vec<_>>().join(":")).collect();
                out.push(format!("{}_frames={}", ["a", "b", "c"][o], f.join("/")));
            }
        }
        if !self.macro_names.is_empty() {
            out.push(format!("macro_names={}", self.macro_names.join(",")));
        }
        if !self.scale.is_empty() {
            out.push(format!("tuning_scale={}", self.scale.iter().map(|c| format!("{c:.6}")).collect::<Vec<_>>().join(",")));
        }
        out.join(" ")
    }

    /// What the oscillators read: each one's table (built-in, its own
    /// frames, or cut from its sample) and its sample. Builds tables: call
    /// it off the audio thread.
    /// The patch's tuning (None: standard): its own scale, else the
    /// library's `tuning`, with degree 0 on `tuning_root` and A4 at 440 Hz.
    pub fn tuning(&self) -> Option<Arc<Tuning>> {
        let ix = &layout().ix;
        let root = self.values[ix.tuning_root].round() as i32;
        let lib = tuning::library();
        let (name, scale) = if !self.scale.is_empty() {
            ("custom", Scale { description: "custom".into(), cents: self.scale.clone() })
        } else {
            let (name, _, t) = &lib[(self.values[ix.tuning].round().max(0.0) as usize).min(lib.len() - 1)];
            (*name, t.scale.clone())
        };
        let t = Tuning::new(name, scale, KeyMap { middle: root, ..KeyMap::default() });
        (!t.is_standard()).then(|| Arc::new(t))
    }

    pub fn sources(&self, samples: [Option<SampleBuf>; 3]) -> Arc<MorphSources> {
        let ix = &layout().ix;
        let mut osc: Vec<OscSource> = Vec::with_capacity(3);
        for (o, sample) in samples.into_iter().enumerate() {
            let kind = self.values[ix.osc[o].table] as usize;
            let t = match super::TABLES.get(kind).copied() {
                Some("user") if !self.frames[o].is_empty() => Arc::new(Table::from_partials(&self.frames[o])),
                Some("user") => table::sine(),
                Some("import") => match &sample {
                    Some(s) => Arc::new(Table::from_sample(&s.data, s.rate, self.values[ix.osc[o].import] > 0.5)),
                    None => table::sine(),
                },
                _ => table::builtin(kind),
            };
            osc.push(OscSource { table: t, sample });
        }
        let mut it = osc.into_iter();
        let (a, b, c) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        Arc::new(MorphSources { osc: [a, b, c], tuning: self.tuning() })
    }
}

/// The oscillators' tables and samples for a patch, and its tuning.
pub struct MorphSources {
    pub osc: [OscSource; 3],
    /// None: standard tuning.
    pub tuning: Option<Arc<Tuning>>,
}
