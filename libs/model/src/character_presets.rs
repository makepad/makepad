//! Named character presets. Each is an ordinary spec; `preset` in a spec
//! loads one and the spec's own keys override it field by field.
use crate::json::{self, Value};

const PRESETS: &[(&str, &str)] = &[
    // Tactical counter-terrorist: navy, helmet + headset, plate carrier.
    ("ct_operator", r##"{"name":"ct_operator","height":1.82,"stylize":0.18,"body":{"shoulders":1.2,"chest":1.18,"muscle":1.3,"waist":1.08,"jaw":1.45,"chin":1.3,"neck":1.35},
      "skin":"#d6a283","face":{"eye_color":"#4d6a86","eye_size":0.85,"brows":1.6,"brow_angle":8,"nose":"straight","nose_size":1.15,"smile":-0.05,"blush":0.1,"lids":0.45},
      "hair":{"style":"buzz","color":"#2f241c"},"facial_hair":"stubble",
      "colors":{"primary":"#253a5e","secondary":"#1f2733","accent":"#9fb7d6"},
      "outfit":[{"kind":"shirt","color":"#2a3f63"},{"kind":"cargo","color":"#26324a"},{"kind":"plate_carrier","color":"#1b1d22","color2":"#c9b48a"},{"kind":"armband","color":"#3aa0ff"},
        {"kind":"helmet","style":"tactical","color":"#1d2a40"},{"kind":"headset","color":"#191c22"},{"kind":"gloves","color":"#15171b"},
        {"kind":"boots","color":"#1b1b1d"},{"kind":"belt","color":"#15171b"},{"kind":"knee_pads","color":"#1a2233"},{"kind":"holster","color":"#15171b"}]}"##),
    ("ct_medic", r##"{"preset":"ct_operator","name":"ct_medic","skin":"#8d5a3b","hair":{"style":"short","color":"#141010"},"facial_hair":"none",
      "face":{"eye_color":"#3b2a1d","nose":"broad"},
      "outfit":[{"kind":"shirt","color":"#2a3f63"},{"kind":"cargo","color":"#26324a"},{"kind":"plate_carrier","color":"#1f2d45","color2":"#e04848"},{"kind":"armband","color":"#3aa0ff"},
        {"kind":"helmet","style":"tactical","color":"#1d2a40"},{"kind":"goggles","color":"#1d2a40","color2":"#e8c46a"},{"kind":"gloves","color":"#15171b"},
        {"kind":"boots","color":"#1b1b1d"},{"kind":"belt","color":"#15171b"},{"kind":"backpack","color":"#22314b","color2":"#e04848"}]}"##),
    // Terrorist: earth tones, balaclava, chest rig, field jacket.
    ("t_raider", r##"{"name":"t_raider","height":1.78,"stylize":0.18,"body":{"shoulders":1.06,"muscle":1.08,"belly":1.05,"jaw":1.3,"neck":1.2},
      "skin":"#c89272","face":{"eye_color":"#3a2a1c","brows":1.35,"brow_angle":14},
      "hair":{"style":"short","color":"#1c140e"},
      "colors":{"primary":"#6b5a3e","secondary":"#4a4234","accent":"#b0452f"},
      "outfit":[{"kind":"shirt","color":"#5c5a4a"},{"kind":"cargo","color":"#5a4c36","material":"camo","color2":"#3b3526"},
        {"kind":"jacket","color":"#b59a66","color2":"#40362a","style":"field","open":true},{"kind":"chest_rig","color":"#4a4630"},{"kind":"scarf","color":"#d23a2a"},{"kind":"armband","color":"#e8c22a"},
        {"kind":"balaclava","color":"#23211d"},{"kind":"gloves","color":"#2d261e"},{"kind":"boots","color":"#3a2b1e"},{"kind":"belt","color":"#2b241c"}]}"##),
    ("t_scout", r##"{"preset":"t_raider","name":"t_scout","height":1.74,"body":{"muscle":0.9,"shoulders":0.95},
      "outfit":[{"kind":"hoodie","color":"#b89c6a"},{"kind":"armband","color":"#e8c22a"},{"kind":"cargo","color":"#4d4637"},{"kind":"chest_rig","color":"#5a5438"},
        {"kind":"balaclava","color":"#3a3228"},{"kind":"goggles","color":"#2b2620","color2":"#d99a3a"},{"kind":"gloves","color":"#2d261e"},
        {"kind":"boots","color":"#3a2b1e","style":"low"},{"kind":"scarf","color":"#9c3b2a"}]}"##),
    // Platformer hero: a chibi explorer with a big grin.
    ("hero_pip", r##"{"name":"hero_pip","height":1.15,"stylize":0.85,"heads":2.9,"body":{"hands":1.1,"feet":1.1},
      "skin":"#ffd2b0","face":{"eye_color":"#2e7fd8","eye_size":1.15,"brows":1.2,"nose":"button","nose_size":0.8,"smile":0.75,"blush":0.7,"lids":0.12},
      "hair":{"style":"spiky","color":"#f28a2e","color2":"#c9621a","volume":1.1},
      "colors":{"primary":"#2f6fe0","secondary":"#ffc94a","accent":"#e23b3b"},
      "outfit":[{"kind":"tshirt","color":"#2f6fe0"},{"kind":"shorts","color":"#ffc94a","material":"denim"},{"kind":"sneakers","color":"#e23b3b","color2":"#ffffff"},
        {"kind":"gloves","color":"#f4f6fa"},{"kind":"scarf","color":"#e23b3b"},{"kind":"backpack","color":"#ff8a1f","color2":"#2a2230"}]}"##),
    ("hero_mia", r##"{"preset":"hero_pip","name":"hero_mia","body":{"fem":0.8},"hair":{"style":"ponytail","color":"#6b3a1e","volume":1.2},
      "face":{"eye_color":"#3fa35b","eye_tilt":6},
      "outfit":[{"kind":"shirt","color":"#e8426a","style":"short"},{"kind":"overalls","color":"#3b6fd1"},{"kind":"sneakers","color":"#ffffff","color2":"#e8426a"},{"kind":"cap","color":"#e8426a"}]}"##),
    // Platformer enemies: goblin-ish critters.
    ("gob_grunt", r##"{"name":"gob_grunt","height":0.95,"stylize":0.9,"heads":2.6,"body":{"belly":1.5,"arms":1.15,"legs":0.85,"hands":1.3},
      "skin":"#7fae3e","face":{"eye_color":"#f2d230","eye_size":1.3,"brows":1.6,"brow_angle":22,"nose":"pointy","nose_size":1.4,"smile":-0.35,"ears":"pointy","ear_size":1.8,"blush":0.0,"lids":0.4,"eye_tilt":-8},
      "hair":"bald","hands":"mitten",
      "colors":{"primary":"#6b4a2b","secondary":"#3d2c1c","accent":"#c8452c"},
      "outfit":[{"kind":"tank","color":"#6b4a2b","material":"leather"},{"kind":"shorts","color":"#3d2c1c","material":"leather"},{"kind":"belt","color":"#2b1d12"},{"kind":"wraps","color":"#b9a37a"}]}"##),
    ("gob_shaman", r##"{"preset":"gob_grunt","name":"gob_shaman","height":1.0,"skin":"#5c8fa8","body":{"belly":1.0},
      "face":{"eye_color":"#ff5a3a","brow_angle":10,"smile":0.3},"hair":{"style":"mohawk","color":"#e2e2e2"},
      "outfit":[{"kind":"sweater","color":"#6d3f7a"},{"kind":"shorts","color":"#3d2c1c"},{"kind":"scarf","color":"#e0b040"},{"kind":"wraps","color":"#d8c9a0"}]}"##),
    // Fighters: a broad brawler and a lean martial artist.
    ("fighter_brawler", r##"{"name":"fighter_brawler","height":1.9,"stylize":0.3,"heads":6.8,"body":{"shoulders":1.32,"chest":1.3,"muscle":1.6,"waist":1.12,"neck":1.5,"arms":1.05,"hands":1.2,"jaw":1.5},
      "skin":"#b27352","face":{"eye_color":"#3a2413","brows":1.6,"brow_angle":14,"nose":"broad","nose_size":1.3,"smile":0.1,"blush":0.1,"lids":0.45},
      "hair":{"style":"mohawk","color":"#d8342a"},"facial_hair":"beard",
      "colors":{"primary":"#d8342a","secondary":"#1e1e22","accent":"#f2c230"},
      "outfit":[{"kind":"shorts","color":"#d8342a","style":"long"},{"kind":"belt","color":"#f2c230"},{"kind":"wraps","color":"#f0ece2"},{"kind":"boots","color":"#1e1e22","style":"low"}]}"##),
    ("fighter_monk", r##"{"name":"fighter_monk","height":1.74,"stylize":0.3,"heads":6.9,"body":{"shoulders":0.92,"muscle":0.95,"waist":0.9,"fem":0.55,"legs":1.08},
      "skin":"#f0c7a0","face":{"eye_color":"#2b2b35","eye_tilt":10,"brows":0.9,"brow_angle":8,"nose":"straight","nose_size":0.8,"smile":0.05,"lids":0.35},
      "hair":{"style":"ponytail","color":"#101014","volume":1.1},
      "colors":{"primary":"#f4f1ea","secondary":"#1b1b1f","accent":"#2f5fd0"},
      "outfit":[{"kind":"gi","color":"#f4f1ea","color2":"#141418"},{"kind":"wraps","color":"#f4f1ea"},{"kind":"headband","color":"#d02f2f"},{"kind":"shoes","color":"#1b1b1f","color2":"#1b1b1f"}]}"##),
    // Dojo: KAEDE, tall lean martial artist; BRANDT, broad armoured brawler.
    ("kaede", r##"{"name":"kaede","fighter":true,"height":1.8,"stylize":0.28,"heads":7.2,"body":{"fem":0.85,"shoulders":0.95,"muscle":1.0,"waist":0.85,"legs":1.12,"arms":1.04},
      "skin":"#f1c9a5","face":{"eye_color":"#3a2a22","eye_tilt":9,"brows":1.0,"brow_angle":10,"nose":"straight","nose_size":0.75,"smile":0.0,"lids":0.38,"lip_color":"#b8545a"},
      "hair":{"style":"ponytail","color":"#0e0d10","color2":"#2a2330","volume":1.25},
      "colors":{"primary":"#b3162a","secondary":"#101014","accent":"#e2b64a"},
      "outfit":[{"kind":"gi","color":"#b3162a","color2":"#101014","style":"long"},{"kind":"wraps","color":"#efe9dc"},{"kind":"shin_wraps","color":"#efe9dc"},
        {"kind":"headband","color":"#e2b64a"},{"kind":"barefoot"}]}"##),
    ("brandt", r##"{"name":"brandt","fighter":true,"height":1.95,"stylize":0.32,"heads":6.0,"body":{"shoulders":1.4,"chest":1.35,"muscle":1.7,"waist":1.15,"neck":1.7,"arms":1.02,"hands":1.25,"jaw":1.8,"chin":1.6,"head_width":1.08,"legs":0.92},
      "skin":"#c98b62","face":{"eye_color":"#4a6a5a","brows":2.3,"brow_angle":20,"eye_size":0.8,"nose":"broad","nose_size":1.25,"smile":-0.1,"lids":0.5,"blush":0.1},
      "hair":{"style":"buzz","color":"#3a2a1c"},"facial_hair":"beard",
      "colors":{"primary":"#138a8a","secondary":"#16262c","accent":"#e6b43a"},
      "outfit":[{"kind":"suit","color":"#138a8a","material":"nylon"},{"kind":"armor","color":"#d9a52e","color2":"#f2d27a"},{"kind":"shoulder_pads","color":"#d9a52e"},
        {"kind":"gauntlets","color":"#2a3a40","color2":"#3ff2e8"},{"kind":"belt","color":"#16262c"},{"kind":"boots","color":"#16262c"},{"kind":"greaves","color":"#d9a52e","material":"metal"}]}"##),
    // Racing driver in a suit and full-face helmet; `racer` shows the face.
    ("racer", r##"{"name":"racer","height":1.78,"stylize":0.22,"body":{"jaw":1.2,"muscle":1.05},"skin":"#e2b08e","face":{"eye_color":"#3a5d7a","smile":0.4,"brows":1.3,"eye_size":0.9},
      "hair":{"style":"swept","color":"#5a3a22"},
      "colors":{"primary":"#e63328","secondary":"#18181c","accent":"#f5f5f5"},
      "outfit":[{"kind":"suit","color":"#e63328","color2":"#f5f5f5"},{"kind":"gloves","color":"#18181c"},{"kind":"shoes","color":"#18181c","color2":"#e63328"},{"kind":"belt","color":"#18181c"}]}"##),
    ("racer_helmet", r##"{"preset":"racer","name":"racer_helmet","outfit":[{"kind":"suit","color":"#e63328","color2":"#f5f5f5"},{"kind":"gloves","color":"#18181c"},{"kind":"shoes","color":"#18181c","color2":"#e63328"},
      {"kind":"helmet","style":"racing","color":"#f5f5f5","color2":"#1a2a4a"}]}"##),
    // Plain bases for customisation.
    ("adult", r##"{"name":"adult","height":1.78,"stylize":0.2,"outfit":["tshirt","pants","sneakers"]}"##),
    ("kid", r##"{"name":"kid","height":1.2,"stylize":0.6,"heads":4.2,"outfit":["tshirt","shorts","sneakers"]}"##),
    ("chibi", r##"{"name":"chibi","height":1.0,"stylize":0.95,"heads":2.6,"outfit":["tshirt","shorts","sneakers"]}"##),
];

pub fn character_preset_names() -> Vec<&'static str> { PRESETS.iter().map(|p| p.0).collect() }
pub fn character_preset(name: &str) -> Option<Value> {
    let text = PRESETS.iter().find(|p| p.0 == name)?.1;
    let v = json::parse(text.as_bytes()).ok()?;
    // Presets may build on presets.
    match v.get("preset").and_then(Value::as_str) {
        Some(base) if base != name => {
            let b = character_preset(base)?;
            let mut out = super::merge_pub(&b, &v);
            if let Value::Obj(pairs) = &mut out { pairs.retain(|(k, _)| k != "preset"); }
            Some(out)
        }
        _ => Some(v),
    }
}
