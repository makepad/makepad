//! Parametric character generator: the `character {...}` program macro.
//!
//! One spec (body type, face, hair, outfit layers, accessories, colours)
//! expands into ordinary model operations: a skinned body built from ring
//! tubes with analytic weights, a face on bones (jaw, eyes, lids, brows,
//! mouth corners), hair and clothing shells that share the body's weights,
//! rigid gear on joints and sockets, PBR materials with procedural textures,
//! and a keyframed animation set (locomotion, air, crouch, strafe, aim,
//! fire/reload, hits, deaths, emotes, fighting). The result is an ordinary
//! compiled character: `game.character({model})` drives it.
//!
//! Characters face -Z, Y up; the character's right is +X and `_l` joints
//! sit at -X, the same basis as the humanoid template. The rest pose is an
//! A-pose (arms ~50° from the body) with identity joint rotations, so clip
//! rotations are expressed in world-aligned parent frames.
#[path = "character_body.rs"]
mod body;
#[path = "character_head.rs"]
mod head;
#[path = "character_gear.rs"]
mod gear;
#[path = "character_clips.rs"]
pub mod clips;
#[path = "character_presets.rs"]
mod presets;
#[path = "character_fps.rs"]
mod fps;
pub use fps::{build_fps_arms, FpsArms, Hold};

use crate::character_mesh::*;
use crate::json::{self, Value};
use crate::{Error, Limits, Operation, Result};
pub use presets::{character_preset_names, character_preset};

/// Joint table: (name, parent). Parents precede children. The first 21
/// names match the humanoid template, so template tools keep working.
pub const CHARACTER_JOINTS: [(&str, Option<usize>); 44] = [
    ("hips", None), ("spine", Some(0)), ("chest", Some(1)), ("neck", Some(2)), ("head", Some(3)),
    ("shoulder_l", Some(2)), ("upper_arm_l", Some(5)), ("lower_arm_l", Some(6)), ("hand_l", Some(7)),
    ("shoulder_r", Some(2)), ("upper_arm_r", Some(9)), ("lower_arm_r", Some(10)), ("hand_r", Some(11)),
    ("upper_leg_l", Some(0)), ("lower_leg_l", Some(13)), ("foot_l", Some(14)), ("toe_l", Some(15)),
    ("upper_leg_r", Some(0)), ("lower_leg_r", Some(17)), ("foot_r", Some(18)), ("toe_r", Some(19)),
    ("jaw", Some(4)), ("eye_l", Some(4)), ("eye_r", Some(4)), ("lid_l", Some(4)), ("lid_r", Some(4)),
    ("brow_l", Some(4)), ("brow_r", Some(4)), ("mouth_l", Some(4)), ("mouth_r", Some(4)),
    ("thumb_1_l", Some(8)), ("thumb_2_l", Some(30)), ("index_1_l", Some(8)), ("index_2_l", Some(32)), ("fingers_1_l", Some(8)), ("fingers_2_l", Some(34)),
    ("thumb_1_r", Some(12)), ("thumb_2_r", Some(36)), ("index_1_r", Some(12)), ("index_2_r", Some(38)), ("fingers_1_r", Some(12)), ("fingers_2_r", Some(40)),
    ("hair_1", Some(4)), ("hair_2", Some(42)),
];
/// Bumped whenever generated geometry, materials or clips change, so
/// content-addressed caches of script characters rebuild.
pub const CHARACTER_GENERATOR_VERSION: i64 = 13;

pub fn joint_index(name: &str) -> u32 { CHARACTER_JOINTS.iter().position(|j| j.0 == name).unwrap_or_else(|| panic!("joint {name}")) as u32 }
pub(crate) fn j(name: &str) -> u32 { joint_index(name) }

// ── spec ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tex { Plain, Skin, Fabric, Knit, Denim, Leather, Nylon, Rubber, Metal, Plastic, Camo, Hair, Glossy, Emissive }
impl Tex {
    fn parse(s: &str) -> Option<Tex> {
        Some(match s { "plain" => Tex::Plain, "skin" => Tex::Skin, "fabric" | "cotton" | "cloth" => Tex::Fabric, "knit" | "wool" => Tex::Knit, "denim" => Tex::Denim,
            "leather" => Tex::Leather, "nylon" | "cordura" | "tactical" => Tex::Nylon, "rubber" => Tex::Rubber, "metal" => Tex::Metal, "plastic" | "polymer" => Tex::Plastic,
            "camo" => Tex::Camo, "hair" => Tex::Hair, "glossy" | "visor" | "glass" => Tex::Glossy, "glow" | "emissive" => Tex::Emissive, _ => return None })
    }
}

/// One clothing or gear layer.
#[derive(Clone, Debug)]
pub struct Layer {
    pub kind: String,
    pub color: [f64; 3],
    pub color2: [f64; 3],
    pub tex: Option<Tex>,
    /// Kind-specific style word (sleeves "short"/"long"/"none", pants
    /// "shorts", helmet "tactical"/"racing"/"knight", ...).
    pub style: String,
    pub open: bool,
}

#[derive(Clone, Debug)]
pub struct CharacterSpec {
    pub name: String,
    pub height: f64,
    /// Body height in head heights: 7.5 heroic, 6 stylised, 3 chibi.
    pub heads: f64,
    /// 0 = realistic-stylised proportions … 1 = chibi.
    pub stylize: f64,
    pub fem: f64,
    pub shoulders: f64, pub chest: f64, pub waist: f64, pub hips: f64, pub belly: f64,
    pub muscle: f64, pub arms: f64, pub legs: f64, pub neck: f64, pub hands: f64, pub feet: f64,
    pub head_width: f64, pub jaw: f64, pub chin: f64,
    pub skin: [f64; 3],
    pub eye_color: [f64; 3], pub eye_size: f64, pub eye_spacing: f64, pub eye_tilt: f64, pub lid: f64,
    pub brow: f64, pub brow_angle: f64, pub brow_color: Option<[f64; 3]>,
    pub nose: String, pub nose_size: f64,
    pub mouth_width: f64, pub smile: f64, pub lip_color: Option<[f64; 3]>,
    pub ears: String, pub ear_size: f64,
    pub blush: f64, pub freckles: f64,
    pub hair: String, pub hair_color: [f64; 3], pub hair_color2: Option<[f64; 3]>, pub hair_volume: f64,
    pub facial_hair: String,
    pub hand_style: String,
    pub outfit: Vec<Layer>,
    pub primary: [f64; 3], pub secondary: [f64; 3], pub accent: [f64; 3],
    pub clips: Option<Vec<String>>,
    /// Adds the versus-kit state clips (guard, hit_high, getup, ko, ...).
    pub fighter: bool,
    /// First material ordinal the generator uses (other builders own lower ones).
    pub material_base: u32,
    pub warnings: Vec<String>,
}

fn srgb_to_linear(c: f64) -> f64 { let c = c.clamp(0., 1.); if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } }
/// "#rrggbb" (sRGB) or a linear [r,g,b(,a)] array.
pub(crate) fn color_value(v: &Value) -> Option<[f64; 3]> {
    match v {
        Value::Str(s) => {
            let h = s.trim().trim_start_matches('#').trim_start_matches('x');
            if h.len() < 6 { return None; }
            let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|x| srgb_to_linear(x as f64 / 255.));
            Some([b(0)?, b(2)?, b(4)?])
        }
        Value::Arr(a) if a.len() >= 3 => { let n = |i: usize| num(&a[i]); Some([n(0)?, n(1)?, n(2)?]) }
        _ => None,
    }
}
pub(crate) fn num(v: &Value) -> Option<f64> { match v { Value::F64(f) => Some(*f), Value::Int(i) => Some(*i as f64), _ => None } }
pub(crate) fn hex(s: &str) -> [f64; 3] { color_value(&json::s(s)).unwrap_or([0.5; 3]) }

/// Deep-merge `over` onto `base` (objects merge key by key; anything else
/// replaces), so a preset can be customised one field at a time.
fn merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Obj(b), Value::Obj(o)) => {
            let mut out = b.clone();
            for (k, v) in o {
                match out.iter_mut().find(|(bk, _)| bk == k) { Some((_, bv)) => *bv = merge(bv, v), None => out.push((k.clone(), v.clone())) }
            }
            Value::Obj(out)
        }
        _ => over.clone(),
    }
}

pub(crate) fn merge_pub(base: &Value, over: &Value) -> Value { merge(base, over) }

const TOP_KEYS: &[&str] = &["op", "name", "preset", "height", "heads", "stylize", "body", "skin", "face", "hair", "outfit", "colors", "clips", "hands", "facial_hair", "fighter", "material_base", "generator", "hold", "support"];
const BODY_KEYS: &[&str] = &["fem", "shoulders", "chest", "waist", "hips", "belly", "muscle", "arms", "legs", "neck", "hands", "feet", "head_width", "jaw", "chin"];
const FACE_KEYS: &[&str] = &["eye_color", "eye_size", "eye_spacing", "eye_tilt", "lids", "brows", "brow_angle", "brow_color", "nose", "nose_size", "mouth_width", "smile", "lip_color", "ears", "ear_size", "blush", "freckles"];
const HAIR_KEYS: &[&str] = &["style", "color", "color2", "volume"];

impl CharacterSpec {
    pub fn parse(v: &Value) -> std::result::Result<CharacterSpec, String> {
        let mut warnings = Vec::new();
        let preset_name = v.get("preset").and_then(Value::as_str);
        let v = match preset_name {
            Some(p) => { let base = character_preset(p).ok_or_else(|| format!("unknown character preset '{p}' (known: {})", character_preset_names().join(", ")))?; merge(&base, v) }
            None => v.clone(),
        };
        let unknown = |obj: Option<&Value>, keys: &[&str], what: &str, warnings: &mut Vec<String>| {
            if let Some(Value::Obj(p)) = obj { for (k, _) in p { if !keys.contains(&k.as_str()) { warnings.push(format!("character: unknown {what} key '{k}' (known: {})", keys.join(", "))); } } }
        };
        unknown(Some(&v), TOP_KEYS, "top-level", &mut warnings);
        let body = v.get("body"); let face = v.get("face"); let hair = v.get("hair"); let colors = v.get("colors");
        unknown(body, BODY_KEYS, "body", &mut warnings); unknown(face, FACE_KEYS, "face", &mut warnings);
        if let Some(h) = hair { if h.as_str().is_none() { unknown(Some(h), HAIR_KEYS, "hair", &mut warnings); } }
        let f = |o: Option<&Value>, k: &str, d: f64, lo: f64, hi: f64| o.and_then(|o| o.get(k)).and_then(num).unwrap_or(d).clamp(lo, hi);
        let t = |o: Option<&Value>, k: &str, d: &str| o.and_then(|o| o.get(k)).and_then(Value::as_str).unwrap_or(d).to_string();
        let c = |o: Option<&Value>, k: &str| o.and_then(|o| o.get(k)).and_then(color_value);
        let stylize = f(Some(&v), "stylize", 0.25, 0., 1.);
        let height = f(Some(&v), "height", 1.8, 0.4, 4.);
        let heads = f(Some(&v), "heads", mix(7.2, 2.8, stylize), 2., 9.);
        let skin = c(Some(&v), "skin").unwrap_or(hex("#e0ac8a"));
        let primary = c(colors, "primary").unwrap_or(hex("#3a5ba0"));
        let secondary = c(colors, "secondary").unwrap_or(hex("#2b2f38"));
        let accent = c(colors, "accent").unwrap_or(hex("#f2b233"));
        let (hair_style, hair_color, hair_color2, hair_volume) = match hair {
            Some(Value::Str(s)) => (s.clone(), hex("#3b2618"), None, 1.),
            Some(h) => (t(Some(h), "style", "short"), c(Some(h), "color").unwrap_or(hex("#3b2618")), c(Some(h), "color2"), f(Some(h), "volume", 1., 0.3, 3.)),
            None => ("short".into(), hex("#3b2618"), None, 1.),
        };
        let mut outfit = Vec::new();
        if let Some(list) = v.get("outfit").and_then(Value::as_arr) {
            for (i, l) in list.iter().enumerate() {
                let (kind, o) = match l { Value::Str(s) => (s.clone(), None), Value::Obj(_) => (t(Some(l), "kind", ""), Some(l)), _ => continue };
                if kind.is_empty() { warnings.push(format!("character: outfit[{i}] needs a kind")); continue; }
                if !gear::LAYER_KINDS.contains(&kind.as_str()) { warnings.push(format!("character: outfit kind '{kind}' unknown (known: {})", gear::LAYER_KINDS.join(", "))); continue; }
                let (dc, dc2) = gear::default_colors(&kind, primary, secondary, accent);
                let tex = o.and_then(|o| o.get("material")).and_then(Value::as_str).and_then(|s| { let t = Tex::parse(s); if t.is_none() { warnings.push(format!("character: material '{s}' unknown")); } t });
                outfit.push(Layer { color: c(o, "color").unwrap_or(dc), color2: c(o, "color2").unwrap_or(dc2), tex, style: t(o, "style", ""), open: o.and_then(|o| o.get("open")).and_then(Value::as_bool).unwrap_or(false), kind });
            }
        }
        let clips = v.get("clips").and_then(|c| match c { Value::Arr(a) => Some(a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()), Value::Bool(false) => Some(Vec::new()), _ => None });
        let fem = f(body, "fem", 0., 0., 1.);
        Ok(CharacterSpec {
            name: t(Some(&v), "name", "character"), height, heads, stylize, fem,
            shoulders: f(body, "shoulders", 1., 0.6, 1.6), chest: f(body, "chest", 1., 0.6, 1.6), waist: f(body, "waist", 1., 0.6, 1.8), hips: f(body, "hips", 1., 0.6, 1.6),
            belly: f(body, "belly", 1., 0.6, 2.2), muscle: f(body, "muscle", 1., 0.6, 1.8), arms: f(body, "arms", 1., 0.7, 1.4), legs: f(body, "legs", 1., 0.6, 1.4),
            neck: f(body, "neck", 1., 0.5, 1.8), hands: f(body, "hands", 1., 0.6, 1.8), feet: f(body, "feet", 1., 0.6, 1.8),
            head_width: f(body, "head_width", 1., 0.7, 1.4), jaw: f(body, "jaw", 1., 0.5, 1.8), chin: f(body, "chin", 1., 0.4, 2.),
            skin,
            eye_color: c(face, "eye_color").unwrap_or(hex("#4a6b8a")), eye_size: f(face, "eye_size", 1., 0.5, 2.), eye_spacing: f(face, "eye_spacing", 1., 0.7, 1.4),
            eye_tilt: f(face, "eye_tilt", 0., -20., 20.), lid: f(face, "lids", 0.35, 0., 0.9),
            brow: f(face, "brows", 1., 0., 2.5), brow_angle: f(face, "brow_angle", 0., -30., 30.), brow_color: c(face, "brow_color"),
            nose: t(face, "nose", "button"), nose_size: f(face, "nose_size", 1., 0.3, 2.2),
            mouth_width: f(face, "mouth_width", 1., 0.5, 1.6), smile: f(face, "smile", 0.25, -1., 1.), lip_color: c(face, "lip_color"),
            ears: t(face, "ears", "round"), ear_size: f(face, "ear_size", 1., 0.5, 2.),
            blush: f(face, "blush", 0.35, 0., 1.), freckles: f(face, "freckles", 0., 0., 1.),
            hair: hair_style, hair_color, hair_color2, hair_volume,
            facial_hair: t(Some(&v), "facial_hair", "none"),
            hand_style: t(Some(&v), "hands", if stylize > 0.6 { "mitten" } else { "fingers" }),
            material_base: v.get("material_base").and_then(num).map_or(1, |b| b.clamp(1., 400.) as u32),
            fighter: v.get("fighter").and_then(Value::as_bool).unwrap_or(false),
            outfit, primary, secondary, accent, clips, warnings,
        })
    }
    pub fn has(&self, kind: &str) -> bool { self.outfit.iter().any(|l| l.kind == kind) }
    pub fn layer(&self, kind: &str) -> Option<&Layer> { self.outfit.iter().find(|l| l.kind == kind) }
}

// ── materials ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MatDef { pub color: [f64; 3], pub color2: Option<[f64; 3]>, pub tex: Tex, pub rough: f64, pub metal: f64, pub emissive: f64, pub scale: f64 }
impl MatDef {
    pub fn new(color: [f64; 3], tex: Tex) -> Self {
        let (rough, metal) = match tex { Tex::Skin => (0.62, 0.), Tex::Fabric | Tex::Knit | Tex::Denim => (0.88, 0.), Tex::Leather => (0.5, 0.), Tex::Nylon => (0.8, 0.), Tex::Rubber => (0.92, 0.),
            Tex::Metal => (0.32, 0.9), Tex::Plastic => (0.45, 0.), Tex::Camo => (0.85, 0.), Tex::Hair => (0.58, 0.), Tex::Glossy => (0.08, 0.2), Tex::Emissive => (0.4, 0.), Tex::Plain => (0.6, 0.) };
        Self { color, color2: None, tex, rough, metal, emissive: if tex == Tex::Emissive { 3. } else { 0. }, scale: 1. }
    }
    pub fn rough(mut self, r: f64) -> Self { self.rough = r; self }
    pub fn metal(mut self, m: f64) -> Self { self.metal = m; self }
}
#[derive(Default)]
pub(crate) struct Mats { pub list: Vec<MatDef>, pub base: u32 }
impl Mats {
    /// Material ordinal for a definition (equal definitions share one).
    pub fn id(&mut self, d: MatDef) -> u32 {
        if let Some(i) = self.list.iter().position(|m| *m == d) { return i as u32 + self.base; }
        self.list.push(d); self.list.len() as u32 - 1 + self.base
    }
    pub(crate) fn ops(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let arr = |v: &[f64]| Value::Arr(v.iter().map(|x| Value::F64(*x)).collect());
        for (i, m) in self.list.iter().enumerate() {
            let id = Value::Int(i as i64 + self.base as i64);
            out.push(json::obj(vec![("op", json::s("material")), ("material", id.clone()), ("color", arr(&m.color))]));
            let (base, pattern): ([f64; 4], Option<(&str, [f64; 3], [f64; 3], [f64; 2], f64)>) = {
                let c = m.color; let dk = |k: f64| c.map(|v| v * k);
                let c2 = m.color2;
                let s = m.scale;
                match m.tex {
                    Tex::Skin => ([1.; 4], Some(("perlin", [(c[0] * 1.02).min(1.), c[1] * 0.86, c[2] * 0.82], c, [18. * s, 18. * s], 0.25))),
                    Tex::Fabric => ([1.; 4], Some(("stripes", dk(0.95), c, [640. * s, 640. * s], 0.06))),
                    Tex::Knit => ([1.; 4], Some(("yarn", dk(0.9), c, [48. * s, 48. * s], 0.2))),
                    Tex::Denim => ([1.; 4], Some(("stripes", c2.unwrap_or(dk(0.9)), c, [700. * s, 700. * s], 0.08))),
                    Tex::Leather => ([1.; 4], Some(("fbm", dk(0.8), c, [28. * s, 28. * s], 0.9))),
                    Tex::Nylon => ([1.; 4], Some(("noise", dk(0.96), c, [400. * s, 400. * s], 0.))),
                    Tex::Rubber => ([1.; 4], Some(("noise", dk(0.85), c, [160. * s, 160. * s], 0.6))),
                    Tex::Metal => ([1.; 4], Some(("fbm", dk(0.82), c, [10. * s, 10. * s], 0.25))),
                    Tex::Plastic => ([1.; 4], Some(("perlin", dk(0.93), c, [20. * s, 20. * s], 0.2))),
                    Tex::Camo => ([1.; 4], Some(("fbm", c2.unwrap_or(dk(0.5)), c, [3. * s, 3. * s], 0.9))),
                    Tex::Hair => ([1.; 4], Some(("stripes", c2.unwrap_or(dk(0.8)), c, [140. * s, 1.], 0.3))),
                    _ => ([c[0], c[1], c[2], 1.], None),
                }
            };
            let mut surface = vec![("op", json::s("surface_material")), ("material", id.clone()), ("base_color", arr(&base)), ("metallic", Value::F64(m.metal)), ("roughness", Value::F64(m.rough)), ("alpha", json::s("opaque"))];
            if m.emissive > 0. { surface.push(("emissive", arr(&m.color))); surface.push(("emissive_strength", Value::F64(m.emissive))); }
            out.push(json::obj(surface));
            if let Some((kind, a, b, scale, bump)) = pattern {
                let layer = |ch: &str, l: &str| vec![("op", json::s("surface_layer")), ("material", id.clone()), ("channel", json::s(ch)), ("layer", json::s(l))];
                let size = if matches!(m.tex, Tex::Camo) { 256 } else { 128 };
                let mut l = layer("base_color", "pattern"); l.push(("width", Value::Int(size))); l.push(("height", Value::Int(size)));
                out.push(json::obj(l));
                out.push(json::obj(vec![("op", json::s("surface_pattern")), ("material", id.clone()), ("channel", json::s("base_color")), ("layer", json::s("pattern")),
                    ("pattern", json::obj(vec![("kind", json::s(kind)), ("color_a", arr(&[a[0], a[1], a[2], 1.])), ("color_b", arr(&[b[0], b[1], b[2], 1.])), ("scale", arr(&scale)), ("seed", json::s(((i * 7919) % 9973).to_string()))]))]));
                out.push(json::obj(vec![("op", json::s("surface_mips")), ("material", id.clone()), ("channel", json::s("base_color")), ("layer", json::s("pattern"))]));
                if bump > 0. && matches!(m.tex, Tex::Fabric | Tex::Knit | Tex::Denim | Tex::Leather | Tex::Nylon) {
                    out.push(json::obj(vec![("op", json::s("surface_derive")), ("material", id.clone()), ("source_channel", json::s("base_color")), ("channel", json::s("normal")), ("layer", json::s("bump")), ("strength", Value::F64(bump))]));
                    out.push(json::obj(vec![("op", json::s("surface_mips")), ("material", id.clone()), ("channel", json::s("normal")), ("layer", json::s("bump"))]));
                }
            }
        }
        out
    }
}

/// Generator state shared by the body, head and gear builders.
pub(crate) struct Gen<'a> {
    pub spec: &'a CharacterSpec,
    pub b: body::Body,
    pub mats: Mats,
    pub parts: Vec<Part>,
    pub sockets: Vec<(String, u32, [f64; 3], [f64; 4])>,
}

/// Expanded operations for one `character` program op.
pub struct CharacterBuild {
    pub operations: Vec<Operation>,
    /// JSON operations applied after the meshes (materials, paint, sockets).
    pub late: Vec<Value>,
    pub early: Vec<Value>,
    pub warnings: Vec<String>,
    pub joints: Vec<(String, [f64; 3])>,
}

pub fn build_character(v: &Value, limits: &Limits) -> Result<CharacterBuild> {
    let spec = CharacterSpec::parse(v).map_err(|_| Error::Invalid("character spec"))?;
    build_from_spec(&spec, limits)
}

/// Readable parse errors for program failures.
pub fn character_spec_error(v: &Value) -> Option<String> { CharacterSpec::parse(v).err() }

pub fn build_from_spec(spec: &CharacterSpec, limits: &Limits) -> Result<CharacterBuild> {
    let b = body::Body::new(spec);
    let mut g = Gen { spec, b, mats: Mats { list: Vec::new(), base: spec.material_base }, parts: Vec::new(), sockets: Vec::new() };
    body::build(&mut g);
    head::build(&mut g);
    gear::build(&mut g);
    let skeleton = g.b.skeleton();
    let mut operations = vec![Operation::SetSkeleton { skeleton: skeleton.clone() }];
    let mut late = Vec::new();
    // One object: every primitive then shares its material's textures
    // instead of each part preparing its own copy.
    let (op, paint) = Part::export_merged("character", &g.parts, limits)?;
    operations.push(op);
    late.extend(paint);
    let wanted: Vec<String> = spec.clips.clone().unwrap_or_else(|| {
        let mut v: Vec<String> = clips::CLIP_NAMES.iter().map(|s| s.to_string()).collect();
        if spec.has("gi") || spec.name.contains("fighter") || spec.fighter { v.extend(clips::FIGHT_CLIP_NAMES.iter().map(|s| s.to_string())); }
        else { v.extend(clips::TRAVERSE_CLIP_NAMES.iter().map(|s| s.to_string())); }
        v
    });
    let rig = clips::RigInfo::new(&g.b);
    for name in &wanted {
        match clips::clip(name, &rig) { Some(c) => operations.push(Operation::SetClip { clip: c }), None => return Err(Error::Invalid("unknown character clip")) }
    }
    // Sockets: weapon grip on the right hand, and named gear anchors.
    for (name, joint, pos, rot) in &g.sockets {
        let rest = g.b.joints[*joint as usize];
        let local = crate::transform::sub(*pos, rest);
        let arr = |v: &[f64]| Value::Arr(v.iter().map(|x| Value::F64(*x)).collect());
        late.push(json::obj(vec![("op", json::s("socket")), ("name", json::s(name)), ("attachment", json::obj(vec![("joint", Value::Int(*joint as i64))])),
            ("transform", json::obj(vec![("translation", arr(&local)), ("rotation", arr(rot))]))]));
    }
    let early = g.mats.ops();
    let joints = CHARACTER_JOINTS.iter().zip(&g.b.joints).map(|(j, p)| (j.0.to_string(), *p)).collect();
    Ok(CharacterBuild { operations, late, early, warnings: spec.warnings.clone(), joints })
}

/// World rest positions of every joint for a spec (proportions and face
/// layout only; no geometry is built).
pub fn rest_joints(spec: &CharacterSpec) -> Vec<(String, [f64; 3])> {
    let mut b = body::Body::new(spec);
    head::place_face_joints(spec, &mut b);
    CHARACTER_JOINTS.iter().zip(&b.joints).map(|(j, p)| (j.0.to_string(), *p)).collect()
}
