//! The one parameter schema of the synth kit. Every module (oscillator,
//! filter, envelope, LFO, arp, effect) declares its parameters into a
//! [`Builder`] under a prefix (`a_`, `f1_`, `env2_`, `lfo3_`); a layout (an
//! engine's patch shape, e.g. [`crate::morph`]) is the list of modules it
//! declares. The engine, the score, the panels, the code view's lenses and
//! the AI all read the same table: key, label, group, range, default,
//! curve, unit, choices, whether a mod row may target it, and one short
//! line for the AI.
//!
//! Modulation works in a parameter's knob travel (its curve): a mod row of
//! amount 0.5 moves it half its travel.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    /// Equal ratios for equal travel (frequencies, times); `min > 0`.
    Log,
    /// Whole numbers.
    Int,
    /// One of `options` (stored as its index).
    Choice,
    /// Off or on (0 or 1).
    Bool,
}

impl Curve {
    pub fn name(self) -> &'static str {
        match self {
            Curve::Linear => "linear",
            Curve::Log => "log",
            Curve::Int => "int",
            Curve::Choice => "choice",
            Curve::Bool => "bool",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    Hz,
    Secs,
    Ms,
    Beats,
    Cents,
    Semis,
    Db,
    /// 0..1 shown as percent.
    Percent,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Hz => "Hz",
            Unit::Secs => "s",
            Unit::Ms => "ms",
            Unit::Beats => "beats",
            Unit::Cents => "ct",
            Unit::Semis => "st",
            Unit::Db => "dB",
            Unit::Percent => "%",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamDef {
    pub key: String,
    /// Upper-case silkscreen label (`CUTOFF`).
    pub label: String,
    /// The group (one module instance: `osc_a`, `filter1`, `env2`, `fx_delay`).
    pub group: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub curve: Curve,
    pub unit: Unit,
    pub options: &'static [&'static str],
    /// A mod row may target it.
    pub modulatable: bool,
    /// Drawn from its middle (pan, detune, bipolar amounts).
    pub bipolar: bool,
    pub about: &'static str,
}

impl ParamDef {
    /// Knob travel 0..1 for a value.
    #[inline]
    pub fn norm(&self, v: f32) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        let v = v.clamp(self.min, self.max);
        match self.curve {
            Curve::Log if self.min > 0.0 => (v / self.min).ln() / (self.max / self.min).ln(),
            _ => (v - self.min) / (self.max - self.min),
        }
    }

    /// The value at travel `t` (clamped to 0..1).
    #[inline]
    pub fn denorm(&self, t: f32) -> f32 {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        match self.curve {
            Curve::Log if self.min > 0.0 => self.min * (self.max / self.min).powf(t),
            Curve::Int | Curve::Choice | Curve::Bool => (self.min + t * (self.max - self.min)).round(),
            _ => self.min + t * (self.max - self.min),
        }
    }

    /// A value brought into range (choices and whole numbers rounded).
    pub fn clamp(&self, v: f32) -> f32 {
        let v = if v.is_finite() { v } else { self.default };
        let v = v.clamp(self.min, self.max);
        match self.curve {
            Curve::Int | Curve::Choice | Curve::Bool => v.round(),
            _ => v,
        }
    }

    /// The index of a choice by name (`@` optional).
    pub fn option(&self, name: &str) -> Option<usize> {
        let name = name.trim().trim_start_matches('@');
        self.options.iter().position(|o| *o == name)
    }

    /// The value as a score writes it: `@ladder`, `true`, `1200`, `0.35`.
    pub fn text(&self, v: f32) -> String {
        match self.curve {
            Curve::Choice => format!("@{}", self.options.get(v.round().max(0.0) as usize).copied().unwrap_or("")),
            Curve::Bool => (if v > 0.5 { "true" } else { "false" }).to_string(),
            Curve::Int => format!("{}", v.round() as i64),
            _ => num(v),
        }
    }

    /// One token-lean line for the AI:
    /// `f1_cutoff 20..20000 Hz log =4000 ~` (`~`: a mod destination).
    pub fn line(&self) -> String {
        let range = match self.curve {
            Curve::Choice => format!("@{}", self.options.join("|@")),
            Curve::Bool => "bool".to_string(),
            _ => format!("{}..{}{}{}", num(self.min), num(self.max), if self.unit == Unit::None { "" } else { " " }, self.unit.name()),
        };
        let curve = if self.curve == Curve::Log { " log" } else { "" };
        let m = if self.modulatable { " ~" } else { "" };
        let about = if self.about.is_empty() { String::new() } else { format!(": {}", self.about) };
        format!("{} {range}{curve} ={}{m}{about}", self.key, self.text(self.default))
    }
}

/// A number as short as it can be written.
pub fn num(v: f32) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" { "0".into() } else { s }
}

/// A group of parameters (one module instance), with its line for the AI.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub about: &'static str,
}

/// A layout's whole table.
pub struct Schema {
    pub params: Vec<ParamDef>,
    pub groups: Vec<Group>,
    by_key: HashMap<String, usize>,
}

impl Schema {
    pub fn index(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }

    pub fn param(&self, key: &str) -> Option<&ParamDef> {
        self.index(key).map(|i| &self.params[i])
    }

    pub fn defaults(&self) -> Vec<f32> {
        self.params.iter().map(|p| p.default).collect()
    }

    /// The parameters of one group, in order.
    pub fn group<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a ParamDef> + 'a {
        self.params.iter().filter(move |p| p.group == name)
    }

    /// One line a parameter, grouped; `groups` picks some (`osc_*` matches
    /// a prefix); empty is all of them.
    pub fn describe(&self, groups: &[&str]) -> String {
        let mut out = String::new();
        for g in &self.groups {
            let wanted = groups.is_empty() || groups.iter().any(|w| g.name == *w || (w.ends_with('*') && g.name.starts_with(w.trim_end_matches('*'))));
            if !wanted {
                continue;
            }
            out.push_str(&format!("[{}] {}\n", g.name, g.about));
            for p in self.group(&g.name) {
                out.push_str("  ");
                out.push_str(&p.line());
                out.push('\n');
            }
        }
        out
    }
}

/// Where modules declare their parameters.
pub struct Builder {
    params: Vec<ParamDef>,
    groups: Vec<Group>,
    group: String,
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

impl Builder {
    pub fn new() -> Builder {
        Builder { params: Vec::new(), groups: Vec::new(), group: String::new() }
    }

    /// Start (or return to) a group.
    pub fn group(&mut self, name: &str, about: &'static str) {
        self.group = name.to_string();
        if !self.groups.iter().any(|g| g.name == name) {
            self.groups.push(Group { name: name.to_string(), about });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add(&mut self, key: String, label: &str, min: f32, max: f32, default: f32, curve: Curve, unit: Unit, options: &'static [&'static str], modulatable: bool, about: &'static str) -> usize {
        let bipolar = min < 0.0 && max > 0.0;
        self.params.push(ParamDef { key, label: label.to_uppercase(), group: self.group.clone(), min, max, default, curve, unit, options, modulatable, bipolar, about });
        self.params.len() - 1
    }

    pub fn num(&mut self, key: String, label: &str, min: f32, max: f32, default: f32, unit: Unit, about: &'static str) -> usize {
        self.add(key, label, min, max, default, Curve::Linear, unit, &[], true, about)
    }

    pub fn log(&mut self, key: String, label: &str, min: f32, max: f32, default: f32, unit: Unit, about: &'static str) -> usize {
        self.add(key, label, min, max, default, Curve::Log, unit, &[], true, about)
    }

    pub fn int(&mut self, key: String, label: &str, min: f32, max: f32, default: f32, about: &'static str) -> usize {
        self.add(key, label, min, max, default, Curve::Int, Unit::None, &[], false, about)
    }

    pub fn choice(&mut self, key: String, label: &str, options: &'static [&'static str], default: usize, about: &'static str) -> usize {
        self.add(key, label, 0.0, (options.len() - 1) as f32, default as f32, Curve::Choice, Unit::None, options, false, about)
    }

    pub fn flag(&mut self, key: String, label: &str, default: bool, about: &'static str) -> usize {
        self.add(key, label, 0.0, 1.0, default as u8 as f32, Curve::Bool, Unit::None, &[], false, about)
    }

    pub fn finish(self) -> Schema {
        let by_key = self.params.iter().enumerate().map(|(i, p)| (p.key.clone(), i)).collect();
        Schema { params: self.params, groups: self.groups, by_key }
    }
}

/// A patch's values read through modulation: the base value of `i`, or
/// the voice's modulated one.
pub trait Values {
    fn val(&self, i: usize) -> f32;
    #[inline]
    fn on(&self, i: usize) -> bool {
        self.val(i) > 0.5
    }
    #[inline]
    fn index(&self, i: usize) -> usize {
        self.val(i).max(0.0) as usize
    }
}

impl Values for [f32] {
    #[inline]
    fn val(&self, i: usize) -> f32 {
        self[i]
    }
}
