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
use makepad_csg_math::portable::PortableFloat;
#[path = "character_body.rs"]
mod body;
#[path = "character_head.rs"]
mod head;
#[path = "character_gear.rs"]
mod gear;
#[path = "character_clips.rs"]
pub mod clips;
#[path = "character_fps.rs"]
mod fps;
pub use fps::{build_fps_arms, FpsArms, Hold};

use crate::character_mesh::*;
use crate::{Error, Limits, Operation, Result};

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
/// content-addressed caches of script characters rebuild. 42: portable
/// transcendentals (`makepad_csg_math::portable`), the same bits on every
/// platform. 43: the `tread` and `swim` clips.
pub const CHARACTER_GENERATOR_VERSION: i64 = 43;

pub fn joint_index(name: &str) -> u32 { CHARACTER_JOINTS.iter().position(|j| j.0 == name).unwrap_or_else(|| panic!("joint {name}")) as u32 }
pub(crate) fn j(name: &str) -> u32 { joint_index(name) }

// ── spec ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tex { Plain, Skin, Fabric, Knit, Denim, Leather, Nylon, Rubber, Metal, Plastic, Camo, Check, Hair, Glossy, Emissive }
impl Tex {
    fn parse(s: &str) -> Option<Tex> {
        Some(match s { "plain" => Tex::Plain, "skin" => Tex::Skin, "fabric" | "cotton" | "cloth" => Tex::Fabric, "knit" | "wool" => Tex::Knit, "denim" => Tex::Denim,
            "leather" => Tex::Leather, "nylon" | "cordura" | "tactical" => Tex::Nylon, "rubber" => Tex::Rubber, "metal" => Tex::Metal, "plastic" | "polymer" => Tex::Plastic,
            "camo" => Tex::Camo, "check" | "shemagh" | "plaid" => Tex::Check, "hair" => Tex::Hair, "glossy" | "visor" | "glass" => Tex::Glossy, "glow" | "emissive" => Tex::Emissive, _ => return None })
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
    /// `mesh: false` builds only the skeleton and clips: a rig and its
    /// animation for a model whose parts are authored by hand and bound to
    /// the kit's joints.
    pub mesh: bool,
    /// First material ordinal the generator uses (other builders own lower ones).
    pub material_base: u32,
    pub warnings: Vec<String>,
}

fn srgb_to_linear(c: f64) -> f64 { let c = c.clamp(0., 1.); if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).ppowf(2.4) } }
/// "#rrggbb" (sRGB) or a linear [r,g,b(,a)] array.
pub fn hex_color(s: &str) -> Option<[f64; 3]> {
    let h = s.trim().trim_start_matches('#').trim_start_matches('x');
    if h.len() < 6 || !h.is_char_boundary(6) { return None; }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|x| srgb_to_linear(x as f64 / 255.));
    Some([b(0)?, b(2)?, b(4)?])
}
pub(crate) fn hex(s: &str) -> [f64; 3] { hex_color(s).unwrap_or([0.5; 3]) }

const TOP_KEYS: &[&str] = &["op", "name", "preset", "height", "heads", "stylize", "body", "skin", "face", "hair", "outfit", "colors", "clips", "hands", "facial_hair", "fighter", "material_base", "generator", "hold", "support", "support_radius", "grip_radius", "grip_front", "mesh"];
const BODY_KEYS: &[&str] = &["fem", "shoulders", "chest", "waist", "hips", "belly", "muscle", "arms", "legs", "neck", "hands", "feet", "head_width", "jaw", "chin"];
const FACE_KEYS: &[&str] = &["eye_color", "eye_size", "eye_spacing", "eye_tilt", "lids", "brows", "brow_angle", "brow_color", "nose", "nose_size", "mouth_width", "smile", "lip_color", "ears", "ear_size", "blush", "freckles"];
const HAIR_KEYS: &[&str] = &["style", "color", "color2", "volume"];

/// A tree of spec values a character spec is read from: the Splash objects a script
/// passes (the model language implements it), or a preset.
pub trait SpecTree: Clone {
    fn get(&self, key: &str) -> Option<Self>;
    /// The keys of an object (None: not an object).
    fn keys(&self) -> Option<Vec<String>>;
    fn items(&self) -> Option<Vec<Self>>;
    fn num(&self) -> Option<f64>;
    fn text(&self) -> Option<String>;
    fn boolean(&self) -> Option<bool>;
    /// A colour as linear RGB: "#rrggbb" text (sRGB) or a linear [r g b (a)] list.
    fn color(&self) -> Option<[f64; 3]> {
        if let Some(s) = self.text() { return hex_color(&s); }
        let a = self.items()?;
        if a.len() < 3 { return None; }
        Some([a[0].num()?, a[1].num()?, a[2].num()?])
    }
}
/// Spec layers, first wins: a spec over its preset over that preset's preset. Objects merge key
/// by key; anything else the first layer that has it wins.
#[derive(Clone)]
struct Layers<T: SpecTree>(Vec<T>);
impl<T: SpecTree> SpecTree for Layers<T> {
    fn get(&self, key: &str) -> Option<Self> {
        let found: Vec<T> = self.0.iter().filter_map(|l| l.get(key)).collect();
        let first = found.first()?;
        if first.keys().is_none() { return Some(Layers(vec![first.clone()])); }
        Some(Layers(found.into_iter().take_while(|l| l.keys().is_some()).collect()))
    }
    fn keys(&self) -> Option<Vec<String>> {
        let mut keys: Vec<String> = Vec::new();
        for l in &self.0 { for k in l.keys()? { if !keys.contains(&k) { keys.push(k); } } }
        Some(keys)
    }
    fn items(&self) -> Option<Vec<Self>> { self.0[0].items().map(|a| a.into_iter().map(|x| Layers(vec![x])).collect()) }
    fn num(&self) -> Option<f64> { self.0[0].num() }
    fn text(&self) -> Option<String> { self.0[0].text() }
    fn boolean(&self) -> Option<bool> { self.0[0].boolean() }
    fn color(&self) -> Option<[f64; 3]> { self.0[0].color() }
}

/// No spec at all: every field its default.
#[derive(Clone)]
struct NoSpec;
impl SpecTree for NoSpec {
    fn get(&self, _: &str) -> Option<Self> { None }
    fn keys(&self) -> Option<Vec<String>> { Some(Vec::new()) }
    fn items(&self) -> Option<Vec<Self>> { None }
    fn num(&self) -> Option<f64> { None }
    fn text(&self) -> Option<String> { None }
    fn boolean(&self) -> Option<bool> { None }
}

impl CharacterSpec {
    /// A spec from a preset name or a spec object (`preset` loads one through `presets`, the
    /// object's own keys override it field by field). Unknown keys and values are warnings;
    /// ranges clamp.
    pub fn read<T: SpecTree>(v: &T, presets: &dyn Fn(&str) -> Option<T>) -> std::result::Result<CharacterSpec, String> {
        let mut layers = Vec::new();
        let mut next = match v.text() {
            Some(name) => Some(name),
            None => { layers.push(v.clone()); v.get("preset").and_then(|p| p.text()) }
        };
        if v.text().is_none() && v.keys().is_none() {
            return Err("a character spec is a preset name or a spec object".into());
        }
        // Presets may build on presets.
        while let Some(name) = next.take() {
            if layers.len() > 8 { return Err(format!("character preset '{name}': presets nest too deep")); }
            let preset = presets(&name).ok_or_else(|| format!("unknown character preset '{name}'"))?;
            next = preset.get("preset").and_then(|p| p.text()).filter(|p| *p != name);
            layers.push(preset);
        }
        Self::read_merged(&Layers(layers))
    }
    /// Every field its default (a spec object with no keys).
    pub fn defaults() -> CharacterSpec {
        Self::read_merged(&NoSpec).expect("the default spec")
    }
    fn read_merged<T: SpecTree>(v: &T) -> std::result::Result<CharacterSpec, String> {
        let mut warnings = Vec::new();
        let unknown = |obj: Option<&T>, keys: &[&str], what: &str, warnings: &mut Vec<String>| {
            if let Some(p) = obj.and_then(|o| o.keys()) { for k in p { if !keys.contains(&k.as_str()) { warnings.push(format!("character: unknown {what} key '{k}' (known: {})", keys.join(", "))); } } }
        };
        unknown(Some(v), TOP_KEYS, "top-level", &mut warnings);
        let body = v.get("body"); let face = v.get("face"); let hair = v.get("hair"); let colors = v.get("colors");
        let (body, face, hair, colors) = (body.as_ref(), face.as_ref(), hair.as_ref(), colors.as_ref());
        unknown(body, BODY_KEYS, "body", &mut warnings); unknown(face, FACE_KEYS, "face", &mut warnings);
        if let Some(h) = hair { if h.text().is_none() { unknown(Some(h), HAIR_KEYS, "hair", &mut warnings); } }
        let f = |o: Option<&T>, k: &str, d: f64, lo: f64, hi: f64| o.and_then(|o| o.get(k)).and_then(|x| x.num()).unwrap_or(d).clamp(lo, hi);
        let t = |o: Option<&T>, k: &str, d: &str| o.and_then(|o| o.get(k)).and_then(|x| x.text()).unwrap_or_else(|| d.to_string());
        let c = |o: Option<&T>, k: &str| o.and_then(|o| o.get(k)).and_then(|x| x.color());
        let stylize = f(Some(v), "stylize", 0.25, 0., 1.);
        let height = f(Some(v), "height", 1.8, 0.4, 4.);
        let heads = f(Some(v), "heads", mix(7.2, 2.8, stylize), 2., 9.);
        let skin = c(Some(v), "skin").unwrap_or(hex("#e0ac8a"));
        let primary = c(colors, "primary").unwrap_or(hex("#3a5ba0"));
        let secondary = c(colors, "secondary").unwrap_or(hex("#2b2f38"));
        let accent = c(colors, "accent").unwrap_or(hex("#f2b233"));
        let (hair_style, hair_color, hair_color2, hair_volume) = match hair {
            Some(h) if h.text().is_some() => (h.text().unwrap(), hex("#3b2618"), None, 1.),
            Some(h) => (t(Some(h), "style", "short"), c(Some(h), "color").unwrap_or(hex("#3b2618")), c(Some(h), "color2"), f(Some(h), "volume", 1., 0.3, 3.)),
            None => ("short".into(), hex("#3b2618"), None, 1.),
        };
        let mut outfit = Vec::new();
        if let Some(list) = v.get("outfit").and_then(|o| o.items()) {
            for (i, l) in list.iter().enumerate() {
                let (kind, o) = if let Some(s) = l.text() { (s, None) } else if l.keys().is_some() { (t(Some(l), "kind", ""), Some(l)) } else { continue };
                if kind.is_empty() { warnings.push(format!("character: outfit[{i}] needs a kind")); continue; }
                if !gear::LAYER_KINDS.contains(&kind.as_str()) { warnings.push(format!("character: outfit kind '{kind}' unknown (known: {})", gear::LAYER_KINDS.join(", "))); continue; }
                let (dc, dc2) = gear::default_colors(&kind, primary, secondary, accent);
                let tex = o.and_then(|o| o.get("material")).and_then(|m| m.text()).and_then(|s| { let t = Tex::parse(&s); if t.is_none() { warnings.push(format!("character: material '{s}' unknown")); } t });
                outfit.push(Layer { color: c(o, "color").unwrap_or(dc), color2: c(o, "color2").unwrap_or(dc2), tex, style: t(o, "style", ""), open: o.and_then(|o| o.get("open")).and_then(|x| x.boolean()).unwrap_or(false), kind });
            }
        }
        let clips = v.get("clips").and_then(|c| match c.items() { Some(a) => Some(a.iter().filter_map(|x| x.text()).collect()), None if c.boolean() == Some(false) => Some(Vec::new()), None => None });
        let fem = f(body, "fem", 0., 0., 1.);
        Ok(CharacterSpec {
            name: t(Some(v), "name", "character"), height, heads, stylize, fem,
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
            facial_hair: t(Some(v), "facial_hair", "none"),
            hand_style: t(Some(v), "hands", if stylize > 0.6 { "mitten" } else { "fingers" }),
            material_base: v.get("material_base").and_then(|x| x.num()).map_or(1, |b| b.clamp(1., 400.) as u32),
            fighter: v.get("fighter").and_then(|x| x.boolean()).unwrap_or(false),
            mesh: v.get("mesh").and_then(|x| x.boolean()).unwrap_or(true),
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
        let (rough, metal) = match tex { Tex::Skin => (0.5, 0.), Tex::Fabric | Tex::Knit | Tex::Denim => (0.88, 0.), Tex::Leather => (0.62, 0.), Tex::Nylon => (0.8, 0.), Tex::Rubber => (0.92, 0.),
            Tex::Metal => (0.32, 0.9), Tex::Plastic => (0.45, 0.), Tex::Camo => (0.85, 0.), Tex::Check => (0.86, 0.), Tex::Hair => (0.58, 0.), Tex::Glossy => (0.08, 0.2), Tex::Emissive => (0.4, 0.), Tex::Plain => (0.6, 0.) };
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
    pub(crate) fn ops(&self, limits: &Limits) -> Result<Vec<Operation>> {
        use crate::{PatternKind, SurfaceAlpha, SurfaceChannel, SurfaceDerivation, SurfaceLayer, SurfaceMaterial, SurfaceOperation, SurfacePattern};
        let mut out = Vec::new();
        for (i, m) in self.list.iter().enumerate() {
            let id = i as u32 + self.base;
            out.push(Operation::SetMaterial { material: id, value: crate::Material { color: m.color, base_color_png: Vec::new() } });
            let (base, pattern): ([f64; 4], Option<(PatternKind, [f64; 3], [f64; 3], [f64; 2], f64)>) = {
                let c = m.color; let dk = |k: f64| c.map(|v| v * k);
                let c2 = m.color2;
                let s = m.scale;
                match m.tex {
                    Tex::Skin => ([1.; 4], Some((PatternKind::Perlin, [(c[0] * 1.02).min(1.), c[1] * 0.86, c[2] * 0.82], c, [18. * s, 18. * s], 0.25))),
                    Tex::Fabric => ([1.; 4], Some((PatternKind::Stripes, dk(0.95), c, [640. * s, 640. * s], 0.06))),
                    Tex::Knit => ([1.; 4], Some((PatternKind::Yarn, dk(0.82), c, [48. * s, 48. * s], 0.5))),
                    // Twill in its own darker shade (a layer's colour2 is its trim, not the weave).
                    Tex::Denim => ([1.; 4], Some((PatternKind::Stripes, dk(0.82), c, [700. * s, 700. * s], 0.08))),
                    Tex::Leather => ([1.; 4], Some((PatternKind::Fbm, dk(0.72), c, [34. * s, 34. * s], 0.9))),
                    Tex::Nylon => ([1.; 4], Some((PatternKind::Checker, dk(0.93), c, [420. * s, 420. * s], 0.12))),
                    Tex::Rubber => ([1.; 4], Some((PatternKind::Noise, dk(0.85), c, [160. * s, 160. * s], 0.6))),
                    Tex::Metal => ([1.; 4], Some((PatternKind::Fbm, dk(0.82), c, [10. * s, 10. * s], 0.25))),
                    Tex::Plastic => ([1.; 4], Some((PatternKind::Fbm, dk(0.88), c, [24. * s, 24. * s], 0.35))),
                    Tex::Camo => ([1.; 4], Some((PatternKind::Camo, c2.unwrap_or(dk(0.5)), c, [5. * s, 5. * s], 0.25))),
                    // Woven check (shemagh): a coarse two-colour grid.
                    Tex::Check => ([1.; 4], Some((PatternKind::Checker, c2.unwrap_or(dk(0.6)), c, [180. * s, 180. * s], 0.04))),
                    Tex::Hair => ([1.; 4], Some((PatternKind::Stripes, c2.unwrap_or(dk(0.8)), c, [140. * s, 1.], 0.3))),
                    _ => ([c[0], c[1], c[2], 1.], None),
                }
            };
            let mut surface = SurfaceMaterial { base_color: base, metallic: m.metal, roughness: m.rough, alpha: SurfaceAlpha::Opaque, ..SurfaceMaterial::default() };
            if m.emissive > 0. { surface.emissive = m.color; surface.emissive_strength = m.emissive; }
            out.push(Operation::Surface(SurfaceOperation::Material { material: id, value: surface }));
            if let Some((kind, a, b, scale, bump)) = pattern {
                let size = if matches!(m.tex, Tex::Camo) { 256 } else { 128 };
                let base_color = SurfaceChannel::BaseColor;
                out.push(Operation::Surface(SurfaceOperation::Layer { material: id, channel: base_color, layer: SurfaceLayer::blank("pattern", base_color, size, size, limits)? }));
                let pattern = SurfacePattern { kind, color_a: [a[0], a[1], a[2], 1.], color_b: [b[0], b[1], b[2], 1.], scale, seed: ((i * 7919) % 9973) as u64 };
                out.push(Operation::Surface(SurfaceOperation::Pattern { material: id, channel: base_color, layer: "pattern".into(), pattern }));
                out.push(Operation::Surface(SurfaceOperation::Mips { material: id, channel: base_color, layer: "pattern".into() }));
                // Skin gets only a whisper of relief from its mottle (pores and soft
                // unevenness catch the light); a stronger derive reads as lumps.
                let bump = if m.tex == Tex::Skin { 0.05 } else { bump };
                if bump > 0. && matches!(m.tex, Tex::Skin | Tex::Fabric | Tex::Knit | Tex::Denim | Tex::Leather | Tex::Nylon | Tex::Plastic | Tex::Camo | Tex::Check) {
                    let recipe = SurfaceDerivation { source_channel: base_color, source_layer: None, strength: bump, roughness_min: 0.2, roughness_max: 0.9, metallic: 0., wrap: true };
                    out.push(Operation::Surface(SurfaceOperation::Derive { material: id, channel: SurfaceChannel::Normal, layer: "bump".into(), recipe }));
                    out.push(Operation::Surface(SurfaceOperation::Mips { material: id, channel: SurfaceChannel::Normal, layer: "bump".into() }));
                }
            }
        }
        Ok(out)
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
    /// Operations applied after the meshes (paint, sockets).
    pub late: Vec<Operation>,
    /// Operations applied before the meshes (materials).
    pub early: Vec<Operation>,
    pub warnings: Vec<String>,
    pub joints: Vec<(String, [f64; 3])>,
}

pub fn build_from_spec(spec: &CharacterSpec, limits: &Limits) -> Result<CharacterBuild> {
    let b = body::Body::new(spec);
    let mut g = Gen { spec, b, mats: Mats { list: Vec::new(), base: spec.material_base }, parts: Vec::new(), sockets: Vec::new() };
    if spec.mesh {
        body::build(&mut g);
        head::build(&mut g);
        gear::build(&mut g);
    } else {
        head::place_face_joints(spec, &mut g.b);
    }
    let skeleton = g.b.skeleton();
    let mut operations = vec![Operation::SetSkeleton { skeleton: skeleton.clone() }];
    let mut late = Vec::new();
    if spec.mesh {
        // One object: every primitive then shares its material's textures
        // instead of each part preparing its own copy.
        let (op, paint) = Part::export_merged("character", &g.parts, limits)?;
        operations.push(op);
        late.extend(paint);
    }
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
        let transform = crate::Transform { translation: local, rotation: *rot, scale: [1.; 3] };
        late.push(Operation::Scene(crate::SceneOperation::Socket(crate::Socket { name: name.clone(), attachment: crate::Attachment::Joint(*joint), transform })));
    }
    let early = g.mats.ops(limits)?;
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

#[cfg(test)]
pub(crate) mod test_spec {
    use super::*;
    /// Specs written in tests (presets live with the model language, in Splash).
    #[derive(Clone)]
    pub(crate) enum S { O(Vec<(&'static str, S)>), N(f64), T(&'static str), B(bool), L(Vec<S>) }
    impl SpecTree for S {
        fn get(&self, key: &str) -> Option<Self> { match self { S::O(f) => f.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()), _ => None } }
        fn keys(&self) -> Option<Vec<String>> { match self { S::O(f) => Some(f.iter().map(|(k, _)| k.to_string()).collect()), _ => None } }
        fn items(&self) -> Option<Vec<Self>> { match self { S::L(l) => Some(l.clone()), _ => None } }
        fn num(&self) -> Option<f64> { match self { S::N(n) => Some(*n), _ => None } }
        fn text(&self) -> Option<String> { match self { S::T(t) => Some(t.to_string()), _ => None } }
        fn boolean(&self) -> Option<bool> { match self { S::B(b) => Some(*b), _ => None } }
    }
    pub(crate) fn spec(fields: Vec<(&'static str, S)>) -> CharacterSpec { CharacterSpec::read(&S::O(fields), &|_| None).unwrap() }
    /// An armoured crew member (the generator's heaviest outfit).
    pub(crate) fn armoured() -> CharacterSpec {
        let layer = |kind: &'static str, color: &'static str, material: &'static str| S::O(vec![("kind", S::T(kind)), ("color", S::T(color)), ("material", S::T(material))]);
        spec(vec![("name", S::T("armoured")), ("height", S::N(1.83)), ("stylize", S::N(0.05)), ("heads", S::N(7.2)),
            ("body", S::O(vec![("shoulders", S::N(1.16)), ("chest", S::N(1.12)), ("muscle", S::N(1.18)), ("neck", S::N(1.3))])),
            ("outfit", S::L(vec![layer("suit", "#2a3442", "nylon"), layer("armor", "#e8eef4", "glossy"), layer("gloves", "#1c232d", "nylon"), layer("boots", "#1c232d", "leather"),
                S::O(vec![("kind", S::T("helmet")), ("style", S::T("visor")), ("color", S::T("#eef3f7"))])]))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_spec::*;
    fn build(ops: &[crate::ProgramOp]) -> crate::BuiltProgram {
        let limits = Limits { max_joints: 128, ..Default::default() };
        crate::build_program(ops, limits, &mut |_| Err("no imports".into()), None).unwrap_or_else(|e| panic!("{}", e.message))
    }

    #[test]
    fn arms_stay_small() {
        for hold in ["rifle", "pistol", "one"] {
            let arms = FpsArms::read(&S::O(vec![("hold", S::T(hold))])).unwrap();
            let bytes = build(&[crate::ProgramOp::FpsArms(Box::new(armoured()), arms)]).compiled.glb.len();
            assert!(bytes < 1_500_000, "arms {hold}: {bytes} B");
        }
    }

    #[test]
    fn locomotion_arm_keys_are_continuous() {
        let spec = spec(vec![("height", S::N(1.82)), ("stylize", S::N(0.2)),
            ("body", S::O(vec![("muscle", S::N(1.15)), ("shoulders", S::N(1.1)), ("hands", S::N(1.05))]))]);
        let mut b = body::Body::new(&spec);
        head::place_face_joints(&spec, &mut b);
        let rig = clips::RigInfo::new(&b);
        let ang = |a: [f64; 4], b: [f64; 4]| { let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.); 2. * d.pacos().to_degrees() };
        let arms: Vec<u32> = ["shoulder_l", "upper_arm_l", "lower_arm_l", "hand_l", "shoulder_r", "upper_arm_r", "lower_arm_r", "hand_r"].iter().map(|n| joint_index(n)).collect();
        let at = |c: &crate::AnimationClip, jn: u32, u: f64| -> Option<[f64; 4]> {
            let ch = c.channels.iter().find(|ch| ch.joint == jn && ch.path == crate::AnimationPath::Rotation)?;
            let k = ((u * (ch.keys.len() - 1) as f64).round() as usize).min(ch.keys.len() - 1);
            Some(ch.keys[k].value)
        };
        let names = ["idle", "walk", "run", "sprint", "walk_back", "strafe_l", "strafe_r", "crouch_move", "run_back", "strafe_run_l", "strafe_run_r", "crouch_back", "crouch_strafe_l", "crouch_strafe_r"];
        let all: Vec<crate::AnimationClip> = names.iter().map(|n| clips::clip(n, &rig).unwrap()).collect();
        for c in &all {
            for ch in c.channels.iter().filter(|ch| arms.contains(&ch.joint) && ch.path == crate::AnimationPath::Rotation) {
                for w in ch.keys.windows(2) {
                    let a = ang(w[0].value, w[1].value);
                    assert!(a < 25., "{} {}: {:.1}° between keys at {:.3}s", c.name, CHARACTER_JOINTS[ch.joint as usize].0, a, w[1].time);
                }
            }
        }
        // Blend partners at the same phase: walk-run-sprint and idle-walk.
        for (x, y) in [(0usize, 1usize), (1, 2), (2, 3), (1, 4), (1, 5), (1, 6)] {
            for &jn in &arms {
                for k in 0..=20 {
                    let u = k as f64 / 20.;
                    if let (Some(a), Some(b)) = (at(&all[x], jn, u), at(&all[y], jn, u)) {
                        let deg = ang(a, b);
                        assert!(deg < 100., "{}-{} {} at phase {u:.2}: {deg:.1}° apart", names[x], names[y], CHARACTER_JOINTS[jn as usize].0);
                    }
                }
            }
        }
    }

    /// Leg joints never flip between keys in the locomotion and platformer
    /// clips, for a realistic build and a short-legged stylised one (the
    /// hand-built Leap hero's rig).
    #[test]
    fn leg_keys_are_continuous() {
        let ang = |a: [f64; 4], b: [f64; 4]| { let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.); 2. * d.pacos().to_degrees() };
        let legs: Vec<u32> = ["upper_leg_l", "lower_leg_l", "foot_l", "upper_leg_r", "lower_leg_r", "foot_r", "upper_arm_l", "lower_arm_l", "upper_arm_r", "lower_arm_r"].iter().map(|n| joint_index(n)).collect();
        let specs = [
            armoured(),
            spec(vec![("name", S::T("stylised")), ("height", S::N(1.15)), ("stylize", S::N(0.85)), ("heads", S::N(2.9)), ("body", S::O(vec![("hands", S::N(1.1)), ("feet", S::N(1.1))]))]),
            spec(vec![("name", S::T("chibi")), ("height", S::N(1.0)), ("stylize", S::N(0.95)), ("heads", S::N(2.6))]),
            spec(vec![("mesh", S::B(false)), ("height", S::N(1.84)), ("heads", S::N(3.3)), ("stylize", S::N(0.3)), ("body", S::O(vec![("legs", S::N(1.15))]))]),
        ];
        let mut bad = Vec::new();
        for spec in specs {
            let mut b = body::Body::new(&spec);
            head::place_face_joints(&spec, &mut b);
            let rig = clips::RigInfo::new(&b);
            for name in ["walk", "run", "sprint", "crouch_move", "jump", "land", "skid", "run_back", "strafe_run_l", "strafe_run_r", "crouch_back", "crouch_strafe_l", "crouch_strafe_r"] {
                let c = clips::clip(name, &rig).unwrap();
                for ch in c.channels.iter().filter(|ch| legs.contains(&ch.joint) && ch.path == crate::AnimationPath::Rotation) {
                    // Per second of clip, so a dense clip is not penalised.
                    for w in ch.keys.windows(2) {
                        let a = ang(w[0].value, w[1].value);
                        let rate = a / (w[1].time - w[0].time).max(1e-6);
                        if rate >= 1500. { bad.push(format!("{} {} {}: {:.1}° in {:.3}s at {:.3}s", spec.name, name, CHARACTER_JOINTS[ch.joint as usize].0, a, w[1].time - w[0].time, w[1].time)); }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "leg flips:\n{}", bad.join("\n"));
    }

    /// Foot-planted gaits: while a foot is on the ground it slides BACK
    /// against the way the clip travels (under a body moving forward the
    /// planted foot stays put in the world). Forward is where the toes point;
    /// right is forward × up.
    #[test]
    fn stance_feet_slide_against_the_travel() {
        let spec = armoured();
        let mut b = body::Body::new(&spec);
        head::place_face_joints(&spec, &mut b);
        let rig = clips::RigInfo::new(&b);
        let (an, ball) = (b.leg[0][2], b.leg[0][3]);
        let fl = ((ball[0] - an[0]).powi(2) + (ball[2] - an[2]).powi(2)).sqrt();
        let fwd = [(ball[0] - an[0]) / fl, 0., (ball[2] - an[2]) / fl];
        let right = [-fwd[2], 0., fwd[0]];
        let back = [-fwd[0], 0., -fwd[2]];
        let left = [-right[0], 0., -right[2]];
        for (name, travel) in [("walk", fwd), ("run", fwd), ("sprint", fwd), ("crouch_move", fwd), ("walk_back", back), ("strafe_r", right), ("strafe_l", left), ("run_back", back), ("strafe_run_r", right), ("strafe_run_l", left), ("crouch_back", back), ("crouch_strafe_r", right), ("crouch_strafe_l", left)] {
            let c = clips::clip(name, &rig).unwrap();
            let n = c.channels[0].keys.len();
            let mut feet: Vec<[[f64; 3]; 2]> = Vec::with_capacity(n);
            for k in 0..n {
                let mut p = clips::Pose::new(&rig);
                for ch in &c.channels {
                    let v = ch.keys[k].value;
                    match ch.path {
                        crate::AnimationPath::Rotation => { p.set(CHARACTER_JOINTS[ch.joint as usize].0, v); }
                        crate::AnimationPath::Translation if ch.joint == 0 => { p.shift([v[0] - rig.rest[0][0], v[1] - rig.rest[0][1], v[2] - rig.rest[0][2]]); }
                        _ => {}
                    }
                }
                feet.push([p.pos("foot_l"), p.pos("foot_r")]);
            }
            for side in 0..2 {
                let low = feet.iter().map(|f| f[side][1]).fold(f64::MAX, f64::min);
                let planted = |k: usize| feet[k][side][1] < low + 0.01 * rig.u;
                let mut slide = 0.;
                for k in 1..n {
                    if planted(k - 1) && planted(k) {
                        let d = [feet[k][side][0] - feet[k - 1][side][0], 0., feet[k][side][2] - feet[k - 1][side][2]];
                        slide += d[0] * travel[0] + d[2] * travel[2];
                    }
                }
                assert!(slide < -0.05 * rig.leg_len, "{name} foot {side}: the planted foot moves {slide:.3} along the travel (it must slide back)");
            }
        }
    }

    /// Vertex tints go in as one op covering every vertex: one op per colour
    /// cost ~25 ms each, and an uncovered vertex costs a closest-point search
    /// when the model compiles (together ~3 s of a 4.5 s build).
    #[test]
    fn vertex_tints_are_one_op_covering_every_vertex() {
        let spec = armoured();
        let built = build_from_spec(&spec, &Limits { max_joints: 128, ..Default::default() }).unwrap();
        use crate::{Operation, SurfaceOperation};
        let tints: Vec<&SurfaceOperation> = built.late.iter().filter_map(|o| match o { Operation::Surface(s @ (SurfaceOperation::VertexColors { .. } | SurfaceOperation::VertexPaint { .. })) => Some(s), _ => None }).collect();
        assert_eq!(tints.len(), 1);
        let SurfaceOperation::VertexColors { vertices, .. } = tints[0] else { panic!("not one vertex-colours op") };
        let Some(Operation::ImportMesh { .. }) = built.operations.iter().find(|o| matches!(o, Operation::ImportMesh { .. })) else { panic!("no mesh") };
        let n = vertices.len();
        let mut bare = armoured();
        bare.clips = Some(Vec::new());
        let doc = build(&[crate::ProgramOp::Character(Box::new(bare))]).document;
        let mesh_vertices: usize = doc.objects().map(|(_, m)| m.vertices().len()).sum();
        assert_eq!(n, mesh_vertices);
    }
}

impl CharacterSpec {
    /// The canonical encoding (program hashes). Warnings are not part of the spec.
    pub(crate) fn write(&self, w: &mut crate::canon::Writer) -> Result<()> {
        // Every field is written: a new field fails to compile here until it is.
        let CharacterSpec { name: _, height: _, heads: _, stylize: _, fem: _, shoulders: _, chest: _, waist: _, hips: _, belly: _, muscle: _, arms: _, legs: _,
            neck: _, hands: _, feet: _, head_width: _, jaw: _, chin: _, skin: _, eye_color: _, eye_size: _, eye_spacing: _, eye_tilt: _, lid: _, brow: _,
            brow_angle: _, brow_color: _, nose: _, nose_size: _, mouth_width: _, smile: _, lip_color: _, ears: _, ear_size: _, blush: _, freckles: _, hair: _,
            hair_color: _, hair_color2: _, hair_volume: _, facial_hair: _, hand_style: _, outfit: _, primary: _, secondary: _, accent: _, clips: _, fighter: _,
            mesh: _, material_base: _, warnings: _ } = self;
        let rgb = |w: &mut crate::canon::Writer, c: &[f64; 3]| -> Result<()> { for v in c { w.f64(*v)?; } Ok(()) };
        let opt_rgb = |w: &mut crate::canon::Writer, c: &Option<[f64; 3]>| -> Result<()> { match c { None => w.u8(0), Some(c) => { w.u8(1)?; rgb(w, c) } } };
        w.string(&self.name)?;
        for v in [self.height, self.heads, self.stylize, self.fem, self.shoulders, self.chest, self.waist, self.hips, self.belly, self.muscle, self.arms, self.legs,
            self.neck, self.hands, self.feet, self.head_width, self.jaw, self.chin] { w.f64(v)?; }
        rgb(w, &self.skin)?;
        rgb(w, &self.eye_color)?;
        for v in [self.eye_size, self.eye_spacing, self.eye_tilt, self.lid, self.brow, self.brow_angle] { w.f64(v)?; }
        opt_rgb(w, &self.brow_color)?;
        w.string(&self.nose)?; w.f64(self.nose_size)?;
        w.f64(self.mouth_width)?; w.f64(self.smile)?; opt_rgb(w, &self.lip_color)?;
        w.string(&self.ears)?; w.f64(self.ear_size)?;
        w.f64(self.blush)?; w.f64(self.freckles)?;
        w.string(&self.hair)?; rgb(w, &self.hair_color)?; opt_rgb(w, &self.hair_color2)?; w.f64(self.hair_volume)?;
        w.string(&self.facial_hair)?;
        w.string(&self.hand_style)?;
        w.count(self.outfit.len())?;
        for l in &self.outfit {
            w.string(&l.kind)?; rgb(w, &l.color)?; rgb(w, &l.color2)?;
            match l.tex { None => w.u8(0)?, Some(t) => { w.u8(1)?; w.u8(t as u8)?; } }
            w.string(&l.style)?; w.u8(l.open as u8)?;
        }
        rgb(w, &self.primary)?; rgb(w, &self.secondary)?; rgb(w, &self.accent)?;
        match &self.clips { None => w.u8(0)?, Some(c) => { w.u8(1)?; w.count(c.len())?; for s in c { w.string(s)?; } } }
        w.u8(self.fighter as u8)?; w.u8(self.mesh as u8)?; w.u32(self.material_base)
    }
}
