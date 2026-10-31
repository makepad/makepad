//! The synthesized sound library as Splash documents: the drum voice model
//! (`sounds/drums.splash`), the synth voice architectures
//! (`sounds/synths.splash`), and the kits and patches written as points
//! of them (`sounds/kits.splash`, `sounds/patches.splash`). They are
//! evaluated once, with `mod.score`, into recipes: `Drums{kit: @house}`
//! and `Synth{preset: @acid}` name them.
//!
//! A layer is an object of audio_synth recipe keys (`{osc: @saw freq: 440
//! cutoff: 2000 cutoff_to: 400 glide: 0.2 a: 0.002 d: 0.3 s: 0 gain:
//! 0.3}`); `freq: 440` follows the note. A score writes the same objects in
//! a Synth's `layers:` and a Drums' `recipes:`.

use crate::vm::{self, Obj, V};
use makepad_audio_synth::recipe::{Layer, Recipe};
use makepad_score_player::presets::{DRUM_SLOTS, SLOTS};
use std::sync::OnceLock;

const DRUMS: &str = include_str!("sounds/drums.splash");
const SYNTHS: &str = include_str!("sounds/synths.splash");
const KITS: &str = include_str!("sounds/kits.splash");
const PATCHES: &str = include_str!("sounds/patches.splash");

/// The kit `Drums{}` plays without a `kit:`.
pub const DEFAULT_KIT: &str = "analog_hybrid";

/// A synthesized kit: a recipe per drum slot.
#[derive(Clone, Debug)]
pub struct KitDef {
    pub name: String,
    pub about: String,
    pub tags: Vec<String>,
    pub slots: Vec<Option<Recipe>>,
}

/// A synth patch.
#[derive(Clone, Debug)]
pub struct PatchDef {
    pub name: String,
    pub about: String,
    pub family: String,
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    pub mono: bool,
    pub recipe: Recipe,
}

#[derive(Debug, Default)]
pub struct Sounds {
    pub kits: Vec<KitDef>,
    pub patches: Vec<PatchDef>,
    /// What did not load (the library's tests keep this empty).
    pub errors: Vec<String>,
}

impl Sounds {
    pub fn kit(&self, name: &str) -> Option<&KitDef> {
        self.kits.iter().find(|k| k.name == name)
    }

    pub fn patch(&self, name: &str) -> Option<&PatchDef> {
        self.patches.iter().find(|p| p.name == name)
    }

    pub fn kit_names(&self) -> Vec<&str> {
        self.kits.iter().map(|k| k.name.as_str()).collect()
    }

    pub fn patch_names(&self) -> Vec<&str> {
        self.patches.iter().map(|p| p.name.as_str()).collect()
    }
}

/// The library, evaluated on first use.
pub fn sounds() -> &'static Sounds {
    static SOUNDS: OnceLock<Sounds> = OnceLock::new();
    SOUNDS.get_or_init(load)
}

fn load() -> Sounds {
    let source = format!("use mod.score.*\n{DRUMS}\n{SYNTHS}\n{KITS}\n{PATCHES}\n{{kits: kits patches: patches}}\n");
    let evaluated = vm::evaluate(&source);
    let mut out = Sounds { errors: evaluated.errors.clone(), ..Sounds::default() };
    let V::Obj(root) = &evaluated.root else {
        out.errors.push("the sound library did not evaluate to {kits patches}".into());
        return out;
    };
    let words = |obj: &Obj, key: &str| obj.get(key).and_then(V::text).map(|t| t.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
    let text = |obj: &Obj, key: &str| obj.get(key).and_then(V::text).unwrap_or("").to_string();
    if let Some(V::Obj(kits)) = root.get("kits") {
        for (name, value) in &kits.fields {
            let V::Obj(kit) = value else { continue };
            match kit_slots(kit) {
                Ok(slots) => out.kits.push(KitDef { name: name.clone(), about: text(kit, "about"), tags: words(kit, "tags"), slots }),
                Err(e) => out.errors.push(format!("kit {name}: {e}")),
            }
        }
    }
    if let Some(V::Obj(patches)) = root.get("patches") {
        for (name, value) in &patches.fields {
            let V::Obj(patch) = value else { continue };
            match patch_recipe(patch) {
                Ok(recipe) => out.patches.push(PatchDef {
                    name: name.clone(),
                    about: text(patch, "about"),
                    family: text(patch, "family"),
                    roles: words(patch, "roles"),
                    tags: words(patch, "tags"),
                    mono: matches!(patch.get("mono"), Some(V::Bool(true))),
                    recipe,
                }),
                Err(e) => out.errors.push(format!("patch {name}: {e}")),
            }
        }
    }
    out
}

/// A patch object's recipe: its `layers` and its recipe-level keys
/// (`volume`, `jitter`, `reverb`, `pitch`).
pub fn patch_recipe(patch: &Obj) -> Result<Recipe, String> {
    let Some(V::Arr(layers)) = patch.get("layers") else { return Err("a patch has `layers: [...]`".into()) };
    let mut recipe = recipe(layers)?;
    for key in ["volume", "jitter", "reverb", "pitch"] {
        if let Some(v) = patch.get(key).and_then(V::num) {
            recipe.set_num(key, v as f32).map_err(|e| e.0)?;
        }
    }
    Ok(recipe)
}

/// A kit object's slots (`{kick: [layers] snare: [layers] ...}`); a slot it
/// does not name is empty.
pub fn kit_slots(kit: &Obj) -> Result<Vec<Option<Recipe>>, String> {
    let mut slots = vec![None; SLOTS];
    for (slot, name) in DRUM_SLOTS.iter().enumerate() {
        if let Some(V::Arr(layers)) = kit.get(name) {
            slots[slot] = Some(recipe(layers).map_err(|e| format!("{name}: {e}"))?);
        }
    }
    Ok(slots)
}

/// A recipe from layer objects.
pub fn recipe(layers: &[V]) -> Result<Recipe, String> {
    let mut recipe = Recipe::default();
    for (i, value) in layers.iter().enumerate() {
        let V::Obj(obj) = value else { return Err(format!("layer {i} is not an object of recipe keys")) };
        let layer = layer(obj).map_err(|e| format!("layer {i}: {e}"))?;
        if recipe.push(layer).is_none() {
            return Err(format!("at most {} layers", makepad_audio_synth::recipe::MAX_LAYERS));
        }
    }
    if recipe.count == 0 {
        return Err("no layers".into());
    }
    Ok(recipe)
}

/// One layer from an object of recipe keys, in their written order.
pub fn layer(obj: &Obj) -> Result<Layer, String> {
    let mut layer = Layer::default();
    for (key, value) in &obj.fields {
        set_key(&mut layer, key, value)?;
    }
    if layer.source == makepad_audio_synth::recipe::Source::None {
        layer.set_str("osc", "sine").map_err(|e| e.0)?;
    }
    Ok(layer)
}

/// One recipe key on a layer: a number, or a name (`@saw`, `@white`, `@lowpass`).
pub fn set_key(layer: &mut Layer, key: &str, value: &V) -> Result<(), String> {
    match value {
        V::Num(n) => layer.set_num(key, *n as f32).map_err(|e| e.0),
        V::Id(name) | V::Str(name) => layer.set_str(key, name).map_err(|e| e.0),
        V::Bool(b) => layer.set_num(key, *b as u8 as f32).map_err(|e| e.0),
        _ => Err(format!("`{key}` is a number or a name")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_loads_whole() {
        let s = sounds();
        assert!(s.errors.is_empty(), "{:#?}", s.errors);
        assert!(s.kits.len() >= 15 && s.patches.len() >= 45, "{} kits {} patches", s.kits.len(), s.patches.len());
        assert!(s.kit(DEFAULT_KIT).is_some());
        for k in &s.kits {
            assert!(k.slots.iter().all(Option::is_some), "{}: every slot", k.name);
            assert!(!k.about.is_empty() && !k.tags.is_empty(), "{}", k.name);
        }
        for p in &s.patches {
            assert!(!p.about.is_empty() && !p.family.is_empty() && !p.roles.is_empty(), "{}", p.name);
        }
    }

    #[test]
    fn a_library_recipe_travels_as_text_unchanged() {
        let s = sounds();
        let all = s.patches.iter().map(|p| p.recipe).chain(s.kits.iter().flat_map(|k| k.slots.iter().flatten().copied()));
        for recipe in all {
            let text = makepad_score_player::presets::recipe_text(&recipe);
            let back = Recipe::parse(&text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
            assert_eq!(format!("{back:?}"), format!("{recipe:?}"), "{text}");
        }
    }
}
