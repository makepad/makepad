//! What the AI reads to write a style or a MilkDrop preset: the style schema
//! (the `script_mod!` in `hw.rs`, read at build time: every template, its
//! parent, its fields, uniforms, instances and hook functions with the
//! comments above them), plus the parts that are not in that file (the
//! palette keys' meaning, the ReliefView fields every surface has, the
//! finish kinds). The references are compact texts for the formats.

use makepad_widgets::makepad_micro_serde::*;

const HW_SOURCE: &str = include_str!("hw.rs");

/// One field of a template, as the schema lists it.
#[derive(Clone, Debug, SerJson)]
pub struct SchemaField {
    /// The path under the template: `draw_bg.pattern`, `height`.
    pub path: String,
    /// `uniform`, `instance`, `value` or `fn`.
    pub kind: String,
    /// The default expression (for `fn`: the signature).
    pub default: String,
    pub doc: String,
}

#[derive(Clone, Debug, SerJson)]
pub struct SchemaTemplate {
    pub name: String,
    pub parent: String,
    pub doc: String,
    pub fields: Vec<SchemaField>,
}

/// Every `mod.widgets.<Name> = <Parent>{ … }` in `hw.rs`, with the fields
/// at its top level and one level into its `draw_*` blocks.
pub fn templates() -> Vec<SchemaTemplate> {
    let Some(start) = HW_SOURCE.find("script_mod! {") else { return Vec::new() };
    let body = &HW_SOURCE[start..];
    let lines: Vec<&str> = body.lines().collect();
    let mut out: Vec<SchemaTemplate> = Vec::new();
    let mut doc = String::new();
    let mut depth: i32 = 0;
    // (depth the template's braces opened at, template index)
    let mut current: Option<(i32, usize)> = None;
    // The draw block we are in: (depth it opened at, its name).
    let mut block: Option<(i32, String)> = None;
    for raw in lines.iter().skip(1) {
        let line = raw.trim();
        if line.starts_with("}") && depth <= 0 && current.is_none() && line == "}" {
            // The end of script_mod!.
            break;
        }
        if let Some(text) = line.strip_prefix("//") {
            if !text.starts_with('!') {
                if !doc.is_empty() {
                    doc.push(' ');
                }
                doc.push_str(text.trim());
            }
            continue;
        }
        if let Some(inner) = line.strip_prefix("/**").and_then(|l| l.strip_suffix("*/")) {
            if !doc.is_empty() {
                doc.push(' ');
            }
            doc.push_str(inner.trim());
            continue;
        }
        if line.is_empty() {
            doc.clear();
        }
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;
        if current.is_none() {
            if let Some(rest) = line.strip_prefix("mod.widgets.") {
                if let Some((name, value)) = rest.split_once('=') {
                    let name = name.trim();
                    if !name.contains('.') && name.starts_with("Hw") && value.contains('{') {
                        let parent = value.trim().trim_end_matches('{').trim_end_matches("{}").trim().trim_start_matches("set_type_default() do").trim().trim_start_matches("mod.widgets.").to_string();
                        let mut doc = std::mem::take(&mut doc);
                        // A section rule (`---- surfaces ----`) is not the template's doc.
                        if let Some(rest) = doc.strip_prefix("----") {
                            doc = rest.split_once("----").map(|(_, d)| d.trim().to_string()).unwrap_or_default();
                        }
                        out.push(SchemaTemplate { name: name.to_string(), parent, doc, fields: Vec::new() });
                        current = Some((depth, out.len() - 1));
                    }
                }
            }
        } else if let Some((tdepth, at)) = current {
            let level = depth - tdepth;
            if let Some((name, value)) = line.split_once(':') {
                // `draw_bg +: {` merges into the parent's block: a block too.
                let merged = name.trim_end().ends_with('+');
                let name = name.trim().trim_end_matches('+').trim();
                let value = if merged { "{" } else { value };
                let is_ident = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                let value = value.trim();
                if is_ident && (level == 1 || (level == 2 && block.is_some())) {
                    let path = match (&block, level) {
                        (Some((_, b)), 2) => format!("{b}.{name}"),
                        _ => name.to_string(),
                    };
                    let (kind, default) = if let Some(sig) = value.strip_prefix("fn") {
                        ("fn", format!("fn{}", sig.trim_end_matches('{').trim_end()))
                    } else if let Some(v) = value.strip_prefix("uniform(") {
                        ("uniform", v.strip_suffix(')').unwrap_or(v).to_string())
                    } else if let Some(v) = value.strip_prefix("instance(") {
                        ("instance", v.strip_suffix(')').unwrap_or(v).to_string())
                    } else if value.starts_with('+') || value.ends_with('{') {
                        ("block", String::new())
                    } else {
                        ("value", value.to_string())
                    };
                    if kind == "block" {
                        if level == 1 && name.starts_with("draw_") {
                            block = Some((depth + 1, name.to_string()));
                        }
                    } else {
                        out[at].fields.push(SchemaField { path, kind: kind.into(), default, doc: std::mem::take(&mut doc) });
                    }
                }
            }
            doc.clear();
        }
        depth += opens - closes;
        if let Some((bdepth, _)) = &block {
            if depth < *bdepth {
                block = None;
            }
        }
        if let Some((tdepth, _)) = current {
            if depth <= tdepth {
                current = None;
                block = None;
            }
        }
    }
    out
}

/// What every Hw surface (a `ReliefView`) takes besides its template's own
/// fields, and what the palette keys paint.
pub const PALETTE: &[(&str, &str)] = &[
    ("screen_bg", "colour: the lists' glass (track table, playlists, Up Next), the spectrum well"),
    ("screen_text", "colour: list text, the spectrum's full-height mix"),
    ("screen_muted", "colour: secondary list text, the spectrum scale"),
    ("accent", "colour: selection, playing row, spectrum bars and waveform, progress; default for key glow, LEDs, menu highlights"),
    ("panel_text", "colour: type on recessed panels (headings, sidebar buttons); dark on a light floor, light on a dark one"),
    ("dark_floor", "1 when the device's floor is dark enough for display type to sit on it without a window; else from panel_text"),
    ("panel_muted", "colour: secondary type on recessed panels"),
    ("menu_bg", "colour: popup menus (style picker, row menus)"),
    ("spill_color", "colour: the SCREEN's light on the device is graded toward this"),
    ("spill_mono", "0..1: 0 keeps the picture's own colours, 1 = monochrome"),
    ("spill_tint_all", "1 = spill_color/spill_mono grade every emitter (LEDs, keys, meters), not only the screen"),
    ("spill_knee", "soft ceiling on neighbour light (0 = none); stops white flashes washing out chrome"),
    ("spill_dark", "a black surface's share of neighbour light (default 0.5)"),
    ("caption_bg", "optional colour: the window caption bar"),
    ("caption_text", "optional colour: the window title"),
    ("legend_on", "colour: key legends (fav, lock, PRESETS) while on"),
    ("legend_off", "colour: those legends while off"),
    ("caps", "1 = titles and headings in capitals"),
    ("map0", "colour map for the picture, stop 0 (dark)"),
    ("map1", "colour map stop 1"),
    ("map2", "colour map stop 2"),
    ("map3", "colour map stop 3 (bright)"),
    ("map_strength", "0..1: how strongly the colour map is blended in"),
    ("map_default", "1 = the colour map is on unless the person turns it off"),
    ("screen_filter", "screen filter kind (0 none)"),
    ("screen_filter_strength", "0..1"),
    ("screen_cell", "filter cell size, pt"),
    ("screen_curve", "CRT curvature"),
];

pub const SURFACE_FIELDS: &[(&str, &str)] = &[
    ("draw_bg.ground", "colour: the face's base colour"),
    ("draw_bg.light_ink", "vec4: lit end of the shading ramps (bevels, rim, gloss)"),
    ("draw_bg.shadow_ink", "vec4: shadowed end of the shading ramps"),
    ("draw_bg.glow_ink", "vec4: the part's own glow and an LED's light colour"),
    ("draw_bg.light", "vec4(x right, y down, z out, intensity): the key light"),
    ("draw_bg.relief", "vec4: bevel width (pt), profile (0 soft, 1 round), raise, specular"),
    ("draw_bg.finish", "vec4: occlusion, rim, gloss, roughness"),
    ("draw_bg.tune", "vec4: face gradient, hairline, occlusion reach, sink"),
    ("draw_bg.shadow", "vec4: cast shadow strength, blur, falloff (0 linear, 1 exp), contact occlusion"),
    ("draw_bg.inner", "vec4: inner shadow, inner blur, ground lip, glow"),
    ("draw_bg.spill", "vec2: neighbour light (x) and neighbour occlusion (y) taken from the relief buffer; dark faces ~1.2-1.6, light metal 2+"),
    ("draw_bg.grain", "float 0..0.05: fine noise"),
    ("draw_bg.pattern", "vec4(kind, scale, strength, anisotropy): finish kind 0 plain, 1 brushed metal, 2 wood (pattern_ink = late wood), 3 lacquer band, 4 hammertone, 5 fine grain, 6 plates+screws (pattern_seam), 7 hazard stripes (pattern_seam.x period, .y band)"),
    ("draw_bg.pattern_ink", "vec4(rgb, mix): the finish's second colour"),
    ("draw_bg.pattern_seam", "vec4: plates: seam spacing x, y, seam width, screw radius; stripes: period, band width"),
    ("draw_bg.sheen", "vec4: reflected band strength, width, position, tilt"),
    ("draw_bg.surface", "HOOK fn(p: vec2, base: vec3) -> vec3: the face's flat colour before lighting; p = points in the quad, self.world.xy = window position; helpers self.hw_hash(q), self.hw_noise(q)"),
    ("draw_bg.pixel", "HOOK fn() -> vec4: replace the whole part; return self.layer(colour) keeps bevel, shadows, AO and spill"),
    ("depth", "float pt: elevation (negative sinks)"),
    ("inset", "float pt: margin for the part's own shadow"),
    ("radius", "float pt: corner radius"),
    ("press", "float pt: depth change while held"),
    ("emissive", "vec4(rgb, intensity): light given to neighbours while lit"),
];

/// How a style file is written, for the brief and the schema overview.
pub const STYLE_RULES: &str = "A style is one Splash file of assignments over the Hw templates: `mod.widgets.<Template>.<path> = <value>` (colours `#xRRGGBB`, `vec4(r, g, b, a)` in 0..1, numbers with a decimal point), palette keys `mod.widgets.hw_palette.<key> = …`, hook functions `mod.widgets.HwKey.draw_bg.surface = fn(p: vec2, base: vec3) -> vec3 { … }`. Set each template you change explicitly (a value on HwSurface does not reach the derived ones). Also set `mod.widgets.Window.pass.clear_color` to the chassis ground. Shader rules: clamp and mix take matching vector arguments, no fwidth, atan2(y, x), never read draw_pass.time, no `&&` inside if (nest ifs). The file starts with `use mod.prelude.widgets_internal.*` and ends with `true`. Main templates: HwChassis (the front), HwPlate (raised wings), HwPanel (recessed list panels), HwBezel (screen surround), HwDisplay (dark readout windows), HwGlass (over the picture), HwKey/HwTransportKey/HwPlayKey (keys; mechanical keys: draw_bg.mech), HwKeyButton/HwKeyText (legends), HwDash/HwLed (lights), HwKnobFace, HwMeterFace, HwSpectrum, HwMilk, HwSegmented, HwMenuLayer, HwCaption/HwTitle/HwText/HwPanelTitle/HwPanelText/HwDisplayText (type), HwNavButton, HwTextInput, HwSplitter/HwDeckSplitter.";

/// The MilkDrop `.milk` format as libs/milkdrop reads it.
pub const MILK_REFERENCE: &str = "A .milk preset is INI-like `key=value` lines under `[preset00]`. Header: MILKDROP_PRESET_VERSION=201, PSVERSION=2 (or 0 without shaders). Base values: fDecay (0.9-1), zoom, rot, warp, cx, cy, dx, dy, sx, sy, fWarpScale, fWarpAnimSpeed, fGammaAdj, fVideoEchoZoom, fVideoEchoAlpha, nVideoEchoOrientation, bBrighten/bDarken/bSolarize/bInvert, bTexWrap, bDarkenCenter; main wave: nWaveMode (0-7), fWaveAlpha, fWaveScale, fWaveSmoothing, wave_r/g/b, wave_x/y, bAdditiveWaves, bWaveDots, bWaveThick, bModWaveAlphaByVolume; borders ob_size/ob_r/ob_g/ob_b/ob_a, ib_size/ib_r..ib_a; motion vectors nMotionVectorsX/Y, mv_r/g/b/a. Code lines are numbered and joined in order: per_frame_init_N=, per_frame_N= (runs each frame: set zoom, rot, wave_r, q1..q32 …), per_pixel_N= (per mesh vertex: reads x, y, rad, ang; sets zoom, rot, dx, dy, sx, sy, warp, cx, cy). EEL syntax: `a = b * 2; c = if(above(bass, 1.2), 1, 0);` functions sin cos tan asin acos atan atan2 sqrt pow exp log log10 abs sign min max sqr rand(n) int above below equal if band bor bnot sigmoid. Audio: bass mid treb (1 = average), bass_att mid_att treb_att (smoothed), vol; time, frame, fps, progress; q1..q32 pass values from per_frame to per_pixel, shapes, waves and shaders. Custom waves (0-3): wavecode_N_enabled=1, wavecode_N_samples, wavecode_N_spectrum, wavecode_N_additive, wavecode_N_usedots, wavecode_N_thick, wavecode_N_r/g/b/a, wave_N_per_frameK=, wave_N_per_pointK= (sample, value1, value2 → x, y, r, g, b, a). Custom shapes (0-3): shapecode_N_enabled=1, sides, additive, thickOutline, textured, x, y, rad, ang, r g b a (centre), r2 g2 b2 a2 (edge), border_r..border_a, num_inst; shape_N_per_frameK=. Shaders (MilkDrop 2 HLSL dialect, each line prefixed by a backtick): warp_1=`shader_body {` … `ret = tex2D(sampler_main, uv).xyz * 0.97;` … `}` and comp_1=`shader_body { ret = tex2D(sampler_main, uv).xyz; }`; samplers sampler_main, sampler_blur1..3 (GetBlur1(uv)), sampler_noise_lq/mq/hq; uv, uv_orig, rad, ang, time, bass, q1..q32 (also _qa.x…), texsize. Keep shaders small; errors come back from get_preset_errors and the preset falls back to the MilkDrop 1 look.";
