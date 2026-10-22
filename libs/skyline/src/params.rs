//! Every skyline parameter, with its panel group, label, range, step and default, plus the seed.
//!
//! One table drives both ends: the panel builds a slider per row, grouped in table order, and a
//! parameter's key is also the name of the shader uniform that carries it ([`Params::set_uniforms`]
//! sets every row on a draw shader; a shader declares only the ones it reads). `web` is the web
//! page's key, so its saved parameter files load as they are.
//!
//! Values are `f64`, as the page's are, so the CPU layout (zones, ridges, columns, props) is computed
//! from the same doubles and comes out the same for the same seed.
use makepad_widgets::makepad_draw::*;

/// `key` is the uniform's name (snake case) and the key of the app's own saved parameters, `web` the web page's.
pub struct ParamDef {
    pub key: &'static str, pub id: LiveId, pub web: &'static str, pub label: &'static str, pub group: &'static str,
    pub min: f64, pub max: f64, pub step: f64, pub default: f64,
}

macro_rules! params {
    ($($group:literal { $($name:ident $key:ident $web:literal $label:literal $min:literal $max:literal $step:literal $def:literal;)* })*) => {
        /// A parameter: its index in [`PARAMS`] and [`Params::v`] (not every one is named in code).
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum P { $($($name,)*)* }
        pub const PARAMS: &[ParamDef] = &[$($(ParamDef {
            key: stringify!($key), id: LiveId::from_str(stringify!($key)), web: $web, label: $label, group: $group,
            min: $min, max: $max, step: $step, default: $def,
        },)*)*];
    };
}

params! {
    "Land & sea" {
        Land land "land" "Land covers" 0. 1. 0.01 0.80;
        LandC land_c "landC" "Land at" 0. 1. 0.01 0.40;
        Horizon horizon "horizon" "Horizon" 0.3 0.95 0.01 0.80;
        Size size "size" "Size" 0.5 2. 0.05 1.;
        SeaD sea_d "seaD" "Sea distance" 0. 1. 0.01 0.;
    }
    "Mountains" {
        MtH mt_h "mtH" "Height" 0.02 0.5 0.005 0.20;
        MtW mt_w "mtW" "Covers" 0. 1. 0.01 0.45;
        MtC mt_c "mtC" "At" 0. 1. 0.01 0.15;
        MtRug mt_rug "mtRug" "Ruggedness" 0. 1. 0.01 0.5;
    }
    "City" {
        CtH ct_h "ctH" "Tower height" 0.2 2.4 0.05 1.;
        CtD ct_d "ctD" "Density" 0. 1. 0.01 0.7;
        CtW ct_w "ctW" "Covers" 0. 1. 0.01 0.30;
        CtC ct_c "ctC" "At" 0. 1. 0.01 0.55;
        CtR ct_r "ctR" "Distance" 0.5 2. 0.01 1.;
    }
    "Trees" {
        TrD tr_d "trD" "Density" 0. 1. 0.01 0.7;
        TrH tr_h "trH" "Height" 0.005 0.08 0.001 0.035;
        TrCon tr_con "trCon" "Conifer share" 0. 1. 0.01 0.7;
        TrW tr_w "trW" "Covers" 0. 1. 0.01 0.55;
        TrC tr_c "trC" "At" 0. 1. 0.01 0.30;
        Treeline treeline "treeline" "Treeline" 0. 0.3 0.005 0.05;
    }
    "Props" {
        PrPyl pr_pyl "prPyl" "Power lines" 0. 1. 0.01 0.4;
        PrTurb pr_turb "prTurb" "Wind turbines" 0. 1. 0.01 0.3;
        PrCrane pr_crane "prCrane" "Cranes" 0. 1. 0.01 0.4;
        PrInd pr_ind "prInd" "Industry" 0. 1. 0.01 0.3;
        PrChurch pr_church "prChurch" "Churches" 0. 1. 0.01 0.5;
        PrTown pr_town "prTown" "Town" 0. 1. 0.01 0.5;
        PrLamp pr_lamp "prLamp" "Street lamps" 0. 1. 0.01 0.5;
        PrBridge pr_bridge "prBridge" "Bridge" 0. 1. 0.01 0.5;
        PrFair pr_fair "prFair" "Fairground" 0. 1. 0.01 0.3;
        PrRefl pr_refl "prRefl" "Water reflections" 0. 1. 0.01 0.6;
        PrRipple pr_ripple "prRipple" "Ripple" 0. 1. 0.01 0.4;
        PrMast pr_mast "prMast" "Radio masts" 0. 1. 0.01 0.4;
        PrFar pr_far "prFar" "Distant lights" 0. 1. 0.01 0.5;
        PrFarm pr_farm "prFarm" "Farms" 0. 1. 0.01 0.4;
        PrCoast pr_coast "prCoast" "Lighthouse, ships" 0. 1. 0.01 0.5;
        PrLake pr_lake "prLake" "Lakes" 0. 1. 0.01 0.;
    }
    "Light" {
        Lights lights "lights" "Intensity" 0. 10. 0.1 3.;
        Temp temp "temp" "Colour temp K" 1800. 9000. 50. 3200.;
        Sharp sharp "sharp" "Sharpness" 0. 1. 0.01 1.;
        WinD win_d "winD" "Lit windows" 0. 1. 0.01 0.45;
        Glow glow "glow" "City glow" 0. 2. 0.01 0.5;
        Haze haze "haze" "Haze" 0. 1. 0.01 0.5;
    }
    "Camera" {
        Off off "off" "Off-centre" 0. 0.85 0.01 0.;
        OffDir off_dir "offDir" "Near side at" 0. 1. 0.005 0.55;
    }
    "View" {
        Vfov vfov "vfov" "Strip height \u{b0}" 10. 90. 1. 32.;
        Zoom zoom "zoom" "Detail width" 0.05 1. 0.01 0.22;
        Pan pan "pan" "Detail at" 0. 1. 0.001 0.55;
    }
}

pub const COUNT: usize = PARAMS.len();
pub const SEED_MAX: u32 = 9_999_999;
/// The silhouette colour and the haze colour of the night (the page's `uSil`, `uHazeCol`).
pub const SIL: [f32; 3] = [0.004, 0.006, 0.012];
pub const HAZE_COL: [f32; 3] = [0.16, 0.18, 0.28];

/// The caches' hash: `bits` folded into `h` one word at a time ([`Params::hash`], `SkyLight::hash`).
pub fn hash_bits(mut h: u64, bits: impl Iterator<Item = u64>) -> u64 {
    for b in bits { h = (h ^ b).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(29); }
    h
}

#[derive(Clone, PartialEq, Debug)]
pub struct Params { pub v: [f64; COUNT], pub seed: u32 }

impl Default for Params {
    fn default() -> Self { Self { v: std::array::from_fn(|i| PARAMS[i].default), seed: 7 } }
}

impl std::ops::Index<P> for Params {
    type Output = f64;
    fn index(&self, p: P) -> &f64 { &self.v[p as usize] }
}

impl Params {
    /// The value a slider at `x` stands for: clamped and snapped to the step, then rounded to four
    /// decimals as the page's `snap()` does, so a dragged value is the same double the page holds.
    pub fn snap(i: usize, x: f64) -> f64 {
        let d = &PARAMS[i];
        let x = x.clamp(d.min, d.max);
        let n = ((x - d.min) / d.step).round();
        format!("{:.4}", d.min + n * d.step).parse().unwrap_or(x)
    }

    /// Sets parameter `i` (snapped); true when it changed.
    pub fn set(&mut self, i: usize, x: f64) -> bool { let x = Self::snap(i, x); std::mem::replace(&mut self.v[i], x) != x }
    pub fn set_seed(&mut self, seed: u32) -> bool { let seed = seed.min(SEED_MAX); std::mem::replace(&mut self.seed, seed) != seed }

    /// Every parameter onto the uniform of the same name, plus `seed`; a shader that does not
    /// declare a uniform ignores it.
    pub fn set_uniforms(&self, cx: &Cx, vars: &mut DrawVars) {
        for (d, x) in PARAMS.iter().zip(self.v) { vars.set_uniform(cx, d.id, &[x as f32]); }
        vars.set_uniform(cx, live_id!(seed), &[self.seed as f32]);
    }

    /// A hash of the seed and the listed parameters: the key of a cache built from them.
    pub fn hash(&self, keys: &[P], extra: &[f64]) -> u64 {
        hash_bits(0xcbf2_9ce4_8422_2325u64 ^ self.seed as u64, keys.iter().map(|p| self[*p]).chain(extra.iter().copied()).map(f64::to_bits))
    }

    /// A seed from the clock (the dice).
    pub fn random_seed() -> u32 {
        let mut x = (Cx::time_now() * 1e9) as u64 ^ 0x9e37_79b9_7f4a_7c15;
        x = (x ^ (x >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
        ((x ^ (x >> 33)) % 100_000) as u32
    }

    /// Reads the page's saved parameter file (`{"format":"skyline-params","params":{...}}`, or a bare
    /// object of keys; the page's keys or this table's): every known key with a number, snapped like
    /// a slider. Returns how many were set.
    pub fn load_web_json(&mut self, text: &str) -> Result<usize, String> {
        let (mut n, mut rest) = (0, text.find("\"params\"").map_or(text, |i| &text[i + 8..]));
        while let Some(q0) = rest.find('"') {
            let after = &rest[q0 + 1..];
            let Some(q1) = after.find('"') else { break };
            let key = &after[..q1];
            let tail = after[q1 + 1..].trim_start();
            rest = &after[q1 + 1..];
            let Some(tail) = tail.strip_prefix(':') else { continue };
            let num: String = tail.trim_start().chars().take_while(|c| c.is_ascii_digit() || "+-.eE".contains(*c)).collect();
            let Ok(x) = num.parse::<f64>() else { continue };
            if !x.is_finite() { continue; }
            if key == "seed" { (self.seed, n) = ((x.trunc().max(0.0) as u32).min(SEED_MAX), n + 1); }
            else if let Some(i) = PARAMS.iter().position(|d| d.web == key || d.key == key) { (self.v[i], n) = (Self::snap(i, x), n + 1); }
        }
        if n == 0 { Err("no known settings found".into()) } else { Ok(n) }
    }
}
