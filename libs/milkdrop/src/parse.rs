//! The `.milk` reader.
//!
//! A preset is an INI-like list of `key=value` lines under one `[preset00]`
//! section. Three kinds of lines matter:
//!
//! * scalar settings (`fDecay=0.98`, `zoom=1.01`, `wavecode_0_r=1`), stored
//!   under the name the equations use for them (`decay`, `zoom`, wave `r`),
//!   so a preset's base values and its code share one namespace;
//! * numbered code lines (`per_frame_3=`, `per_pixel_1=`,
//!   `wave_0_per_point2=`, `shape_1_per_frame4=`, `per_frame_init_1=`),
//!   joined in line-number order with newlines, so a `//` comment ends at the
//!   end of its own line as it does in MilkDrop;
//! * shader lines (`warp_1=` / `comp_1=`), each prefixed by a backtick.
//!
//! Reading is total: any byte sequence gives a [`Preset`]. Lines that are not
//! `key=value`, repeated keys, and keys nobody knows are kept in
//! [`Preset::unknown`] rather than failing the file.

use std::collections::HashMap;

/// Custom waves and shapes per preset.
pub const WAVES: usize = 4;
pub const SHAPES: usize = 4;

/// One custom wave (`wavecode_N_*` settings, `wave_N_*` code).
#[derive(Clone, Debug, Default)]
pub struct WaveDef {
    /// Settings by equation name: `enabled`, `samples`, `sep`, `spectrum`,
    /// `usedots`, `thick`, `additive`, `scaling`, `smoothing`, `r`..`a`.
    pub values: HashMap<String, f64>,
    pub init: String,
    pub per_frame: String,
    pub per_point: String,
}

/// One custom shape (`shapecode_N_*` settings, `shape_N_*` code).
#[derive(Clone, Debug, Default)]
pub struct ShapeDef {
    /// Settings by equation name: `enabled`, `sides`, `additive`,
    /// `thickoutline`, `textured`, `num_inst`, `x`, `y`, `rad`, `ang`,
    /// `tex_ang`, `tex_zoom`, `r`..`a`, `r2`..`a2`, `border_r`..`border_a`.
    pub values: HashMap<String, f64>,
    pub init: String,
    pub per_frame: String,
}

impl WaveDef {
    pub fn get(&self, name: &str, default: f64) -> f64 {
        self.values.get(name).copied().unwrap_or(default)
    }
    pub fn enabled(&self) -> bool {
        self.get("enabled", 0.0) != 0.0
    }
}

impl ShapeDef {
    pub fn get(&self, name: &str, default: f64) -> f64 {
        self.values.get(name).copied().unwrap_or(default)
    }
    pub fn enabled(&self) -> bool {
        self.get("enabled", 0.0) != 0.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Preset {
    /// `MILKDROP_PRESET_VERSION`: 100-ish for MilkDrop 1 presets, 200+ for
    /// MilkDrop 2 (which may carry shaders). 0 when absent.
    pub version: u32,
    /// `PSVERSION_WARP` / `PSVERSION_COMP` (0 = no shader of that kind).
    pub ps_warp: u32,
    pub ps_comp: u32,
    /// Base values of the per-frame variables, by equation name.
    pub values: HashMap<String, f64>,
    pub per_frame_init: String,
    pub per_frame: String,
    pub per_pixel: String,
    pub waves: [WaveDef; WAVES],
    pub shapes: [ShapeDef; SHAPES],
    /// The warp and composite shader sources, backticks removed, lines
    /// joined with newlines. `None` when the preset has none.
    pub warp_shader: Option<String>,
    pub comp_shader: Option<String>,
    /// Lines this reader did not use, verbatim, in file order.
    pub unknown: Vec<String>,
}

impl Preset {
    pub fn get(&self, name: &str, default: f64) -> f64 {
        self.values.get(name).copied().unwrap_or(default)
    }

    /// Read a preset from bytes (UTF-8, anything else replaced).
    pub fn parse(bytes: &[u8]) -> Preset {
        let text = String::from_utf8_lossy(bytes);
        let mut preset = Preset::default();
        let mut code = CodeLines::default();
        for raw in text.lines() {
            let line = raw.trim_end_matches('\r');
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('[') {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                preset.unknown.push(line.to_string());
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            if !preset.read_line(&key, value, &mut code) {
                preset.unknown.push(line.to_string());
            }
        }
        code.finish(&mut preset);
        preset
    }

    /// Use one `key=value` line; `false` when nothing claims it.
    fn read_line(&mut self, key: &str, value: &str, code: &mut CodeLines) -> bool {
        match key {
            "milkdrop_preset_version" => {
                self.version = number(value) as u32;
                return true;
            }
            "psversion_warp" => {
                self.ps_warp = number(value) as u32;
                return true;
            }
            "psversion_comp" => {
                self.ps_comp = number(value) as u32;
                return true;
            }
            "psversion" => {
                // The combined version: both kinds unless one is given apart.
                let v = number(value) as u32;
                if self.ps_warp == 0 {
                    self.ps_warp = v;
                }
                if self.ps_comp == 0 {
                    self.ps_comp = v;
                }
                return true;
            }
            _ => {}
        }
        // Numbered code lines.
        if let Some((kind, index)) = numbered(key) {
            let text = value.to_string();
            match kind {
                "per_frame_init_" => code.push(CodeKey::FrameInit, index, text),
                "per_frame_" => code.push(CodeKey::Frame, index, text),
                "per_pixel_" => code.push(CodeKey::Pixel, index, text),
                "warp_" => code.push(CodeKey::Warp, index, shader_line(value)),
                "comp_" => code.push(CodeKey::Comp, index, shader_line(value)),
                _ => return false,
            }
            return true;
        }
        if let Some(rest) = key.strip_prefix("wave_").or_else(|| key.strip_prefix("shape_")) {
            let is_wave = key.starts_with("wave_");
            if let Some((slot, rest)) = split_slot(rest) {
                let limit = if is_wave { WAVES } else { SHAPES };
                if slot >= limit {
                    return false;
                }
                for (prefix, part) in [("init", 0usize), ("per_frame", 1), ("per_point", 2)] {
                    if let Some(num) = rest.strip_prefix(prefix) {
                        if let Ok(index) = num.parse::<u32>() {
                            if !is_wave && part == 2 {
                                return false;
                            }
                            let key = if is_wave { CodeKey::Wave(slot, part) } else { CodeKey::Shape(slot, part) };
                            code.push(key, index, value.to_string());
                            return true;
                        }
                    }
                }
                return false;
            }
        }
        if let Some(rest) = key.strip_prefix("wavecode_") {
            if let Some((slot, name)) = split_slot(rest) {
                if slot < WAVES {
                    self.waves[slot].values.insert(wave_setting(name).to_string(), number(value));
                    return true;
                }
            }
            return false;
        }
        if let Some(rest) = key.strip_prefix("shapecode_") {
            if let Some((slot, name)) = split_slot(rest) {
                if slot < SHAPES {
                    self.shapes[slot].values.insert(shape_setting(name).to_string(), number(value));
                    return true;
                }
            }
            return false;
        }
        if let Some(name) = setting_name(key) {
            self.values.insert(name.to_string(), number(value));
            return true;
        }
        false
    }

    /// Whether either shader samples a blur texture: the blur chain only
    /// runs for presets that read it.
    pub fn uses_blur(&self) -> bool {
        [&self.warp_shader, &self.comp_shader].iter().any(|s| {
            s.as_deref().is_some_and(|s| {
                let lower = s.to_ascii_lowercase();
                lower.contains("getblur") || lower.contains("sampler_blur")
            })
        })
    }
}

/// `per_frame_12` → ("per_frame_", 12). Longest prefix first, since
/// `per_frame_init_` also starts with `per_frame_`.
fn numbered(key: &str) -> Option<(&'static str, u32)> {
    for prefix in ["per_frame_init_", "per_frame_", "per_pixel_", "warp_", "comp_"] {
        if let Some(num) = key.strip_prefix(prefix) {
            if let Ok(index) = num.parse::<u32>() {
                return Some((prefix, index));
            }
        }
    }
    None
}

/// `3_per_frame2` → (3, "per_frame2").
fn split_slot(rest: &str) -> Option<(usize, &str)> {
    let (slot, rest) = rest.split_once('_')?;
    Some((slot.parse().ok()?, rest))
}

/// Shader lines start with a backtick; a missing one is tolerated.
fn shader_line(value: &str) -> String {
    value.strip_prefix('`').unwrap_or(value).to_string()
}

/// A number the way MilkDrop reads one: leading float, rest ignored, 0 when
/// there is none.
pub fn number(value: &str) -> f64 {
    let value = value.trim();
    let mut end = 0;
    let bytes = value.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < bytes.len() {
        let c = bytes[end];
        match c {
            b'0'..=b'9' => seen_digit = true,
            b'+' | b'-' if end == 0 || matches!(bytes[end - 1], b'e' | b'E') => {}
            b'.' if !seen_dot && !seen_exp => seen_dot = true,
            b'e' | b'E' if seen_digit && !seen_exp => seen_exp = true,
            _ => break,
        }
        end += 1;
    }
    // A trailing exponent marker or sign without digits does not parse.
    let mut slice = &value[..end];
    while !slice.is_empty() && slice.parse::<f64>().is_err() {
        slice = &slice[..slice.len() - 1];
    }
    slice.parse().unwrap_or(0.0)
}

/// Preset-level INI keys → the equation variable they initialise.
fn setting_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "frating" => "rating",
        "fgammaadj" => "gamma",
        "fdecay" => "decay",
        "fvideoechozoom" => "echo_zoom",
        "fvideoechoalpha" => "echo_alpha",
        "nvideoechoorientation" => "echo_orient",
        "nwavemode" => "wave_mode",
        "badditivewaves" => "wave_additive",
        "bwavedots" => "wave_usedots",
        "bwavethick" => "wave_thick",
        "bmodwavealphabyvolume" => "modwavealphabyvolume",
        "bmaximizewavecolor" => "wave_brighten",
        "btexwrap" => "wrap",
        "bdarkencenter" => "darken_center",
        "bredbluestereo" => "red_blue",
        "bbrighten" => "brighten",
        "bdarken" => "darken",
        "bsolarize" => "solarize",
        "binvert" => "invert",
        "fwavealpha" => "wave_a",
        "fwavescale" => "wave_scale",
        "fwavesmoothing" => "wave_smoothing",
        "fwaveparam" => "wave_mystery",
        "fmodwavealphastart" => "modwavealphastart",
        "fmodwavealphaend" => "modwavealphaend",
        "fwarpanimspeed" => "warpanimspeed",
        "fwarpscale" => "warpscale",
        "fzoomexponent" => "zoomexp",
        "fshader" => "fshader",
        "nmotionvectorsx" => "mv_x",
        "nmotionvectorsy" => "mv_y",
        "b1n" => "blur1_min",
        "b2n" => "blur2_min",
        "b3n" => "blur3_min",
        "b1x" => "blur1_max",
        "b2x" => "blur2_max",
        "b3x" => "blur3_max",
        "b1ed" => "blur1_edge_darken",
        _ => return DIRECT.iter().copied().find(|name| *name == key),
    })
}

/// Settings stored under their own name.
const DIRECT: &[&str] = &[
    "zoom", "rot", "cx", "cy", "dx", "dy", "warp", "sx", "sy", "wave_r", "wave_g", "wave_b", "wave_x",
    "wave_y", "ob_size", "ob_r", "ob_g", "ob_b", "ob_a", "ib_size", "ib_r", "ib_g", "ib_b", "ib_a",
    "mv_dx", "mv_dy", "mv_l", "mv_r", "mv_g", "mv_b", "mv_a", "zoomexp", "wave_a", "decay", "gamma",
    "echo_zoom", "echo_alpha", "echo_orient",
];

/// `wavecode_N_*` keys → wave equation names.
fn wave_setting(name: &str) -> &str {
    match name {
        "bspectrum" => "spectrum",
        "busedots" => "usedots",
        "bdrawthick" => "thick",
        "badditive" => "additive",
        other => other,
    }
}

/// `shapecode_N_*` keys → shape equation names.
fn shape_setting(name: &str) -> &str {
    match name {
        "thickoutline" => "thick",
        other => other,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CodeKey {
    FrameInit,
    Frame,
    Pixel,
    Warp,
    Comp,
    /// (slot, 0 = init, 1 = per frame, 2 = per point)
    Wave(usize, usize),
    Shape(usize, usize),
}

/// Numbered lines collected in file order, sorted by number at the end (a
/// preset may list them out of order; MilkDrop reads them by number).
#[derive(Default)]
struct CodeLines {
    lines: Vec<(CodeKey, u32, String)>,
}

impl CodeLines {
    fn push(&mut self, key: CodeKey, index: u32, text: String) {
        self.lines.push((key, index, text));
    }

    fn finish(mut self, preset: &mut Preset) {
        // Stable: equal numbers keep file order.
        self.lines.sort_by_key(|(key, index, _)| (*key, *index));
        let mut warp: Option<String> = None;
        let mut comp: Option<String> = None;
        for (key, _, text) in self.lines {
            let target: &mut String = match key {
                CodeKey::FrameInit => &mut preset.per_frame_init,
                CodeKey::Frame => &mut preset.per_frame,
                CodeKey::Pixel => &mut preset.per_pixel,
                CodeKey::Warp => warp.get_or_insert_with(String::new),
                CodeKey::Comp => comp.get_or_insert_with(String::new),
                CodeKey::Wave(slot, 0) => &mut preset.waves[slot].init,
                CodeKey::Wave(slot, 1) => &mut preset.waves[slot].per_frame,
                CodeKey::Wave(slot, _) => &mut preset.waves[slot].per_point,
                CodeKey::Shape(slot, 0) => &mut preset.shapes[slot].init,
                CodeKey::Shape(slot, _) => &mut preset.shapes[slot].per_frame,
            };
            target.push_str(&text);
            target.push('\n');
        }
        // A shader of only blank lines is no shader.
        preset.warp_shader = warp.filter(|s| !s.trim().is_empty());
        preset.comp_shader = comp.filter(|s| !s.trim().is_empty());
        if preset.ps_warp == 0 && preset.warp_shader.is_some() {
            preset.ps_warp = 2;
        }
        if preset.ps_comp == 0 && preset.comp_shader.is_some() {
            preset.ps_comp = 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "MILKDROP_PRESET_VERSION=201\nPSVERSION=3\nPSVERSION_WARP=2\n[preset00]\nfDecay=0.980\nzoom=1.01000\nnWaveMode=5\nwavecode_0_enabled=1\nwavecode_0_bSpectrum=1\nshapecode_2_sides=6\nshapecode_2_thickOutline=1\nper_frame_2=zoom = zoom + 0.1; // two\nper_frame_1=rot = 0.02;\nper_pixel_1=zoom = zoom*rad;\nwave_0_per_point1=x = sample;\nshape_2_per_frame1=ang = time;\nwarp_1=`shader_body\nwarp_2=`{ ret = GetPixel(uv); }\nsomething odd\nmystery=4\n";

    #[test]
    fn a_preset_reads_into_values_code_and_shaders() {
        let p = Preset::parse(SAMPLE.as_bytes());
        assert_eq!(p.version, 201);
        assert_eq!((p.ps_warp, p.ps_comp), (2, 3));
        assert_eq!(p.get("decay", 0.0), 0.98);
        assert_eq!(p.get("zoom", 0.0), 1.01);
        assert_eq!(p.get("wave_mode", 0.0), 5.0);
        // Lines join in number order, one per line, so comments end there.
        assert_eq!(p.per_frame, "rot = 0.02;\nzoom = zoom + 0.1; // two\n");
        assert_eq!(p.per_pixel, "zoom = zoom*rad;\n");
        assert!(p.waves[0].enabled());
        assert_eq!(p.waves[0].get("spectrum", 0.0), 1.0);
        assert_eq!(p.waves[0].per_point, "x = sample;\n");
        assert_eq!(p.shapes[2].get("sides", 0.0), 6.0);
        assert_eq!(p.shapes[2].get("thick", 0.0), 1.0);
        assert_eq!(p.shapes[2].per_frame, "ang = time;\n");
        assert_eq!(p.warp_shader.as_deref(), Some("shader_body\n{ ret = GetPixel(uv); }\n"));
        assert!(p.comp_shader.is_none());
        assert_eq!(p.unknown, vec!["something odd".to_string(), "mystery=4".to_string()]);
    }

    #[test]
    fn numbers_read_like_milkdrop() {
        assert_eq!(number("0.5"), 0.5);
        assert_eq!(number(" -1.25e2 junk"), -125.0);
        assert_eq!(number(".75"), 0.75);
        assert_eq!(number("1e"), 1.0);
        assert_eq!(number("abc"), 0.0);
        assert_eq!(number(""), 0.0);
    }

    #[test]
    fn any_bytes_give_a_preset() {
        let p = Preset::parse(&[0xff, 0xfe, b'=', b'\n', b'=', b'=', b'\n']);
        assert!(p.warp_shader.is_none());
        let p = Preset::parse(b"per_frame_x=1\nwave_9_per_frame1=a=1\nshapecode_7_x=1\n");
        assert_eq!(p.unknown.len(), 3);
    }
}
