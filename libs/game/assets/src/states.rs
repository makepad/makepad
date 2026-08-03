//! Animation-state and modular-tile vocabulary.
//!
//! Two problems, one shape: an asset's *usefulness* is described by words that
//! never appear in its filename. A character is useful because it can attack;
//! a road tile is useful because it is a corner. Both facts live in structure
//! the AI cannot see — clip names inside the GLB, and a kit's naming
//! convention — so both are translated into ordinary search keywords here.

/// Query words → the clip-name fragments that satisfy them.
///
/// Clip vocabularies differ per pack (`Running_A` vs `Run` vs `Gallop`), so a
/// game asking for "a character that can run" must match all of them. Matching
/// is case-insensitive substring against each clip name.
pub const STATE_WORDS: &[(&str, &[&str])] = &[
    ("idle", &["idle", "stand"]),
    ("walk", &["walk"]),
    ("run", &["run", "sprint", "gallop", "dash"]),
    ("sprint", &["sprint", "run"]),
    ("jump", &["jump", "leap", "hop"]),
    ("fall", &["fall", "falling", "air"]),
    ("land", &["land", "jump_land", "landing"]),
    ("attack", &["attack", "slash", "chop", "stab", "slice", "punch", "kick", "swing", "melee", "headbutt"]),
    ("punch", &["punch", "unarmed"]),
    ("kick", &["kick"]),
    ("shoot", &["shoot", "ranged", "aiming", "throw", "spellcast"]),
    ("block", &["block", "shield", "defend"]),
    ("dodge", &["dodge", "roll", "evade"]),
    ("hurt", &["hit", "hitreact", "recievehit", "receivehit", "damage", "hurt"]),
    ("die", &["death", "die", "defeat", "dead"]),
    ("dance", &["dance", "cheer", "celebrate"]),
    ("sit", &["sit", "sitdown", "chair"]),
    ("wave", &["wave", "greet", "interact"]),
    ("swim", &["swim"]),
    ("fly", &["fly", "flying", "hover"]),
    ("eat", &["eat", "eating", "graze"]),
    ("carry", &["carry", "pickup", "pick_up"]),
    ("climb", &["climb"]),
    ("sleep", &["sleep", "lie", "rest"]),
];

/// Modular-tile roles → filename fragments that identify them.
///
/// Grounded in the actual Kenney naming seen in the downloaded kits:
/// `road-bend`, `corridor-intersection`, `building-corner-window`,
/// `block-grass-slope`. Order matters — the more specific variants are tested
/// before the general ones, so `corner-inner` never resolves to `corner`.
pub const TILE_ROLES: &[(&str, &[&str])] = &[
    ("corner-inner", &["corner-inner", "cornerinner", "inner-corner"]),
    ("corner-outer", &["corner-outer", "cornerouter", "outer-corner"]),
    ("junction", &["intersection", "junction", "crossroad", "crossing", "-cross", "split"]),
    ("corner", &["corner", "bend", "curve", "turn", "-elbow"]),
    ("end", &["-end", "end-", "cap", "deadend", "stub"]),
    ("ramp", &["ramp", "slope", "incline", "-hill"]),
    ("stairs", &["stair", "steps"]),
    ("door", &["door", "gate", "entrance", "archway"]),
    ("window", &["window"]),
    ("bridge", &["bridge"]),
    ("roof", &["roof", "ceiling"]),
    ("wall", &["wall", "fence", "railing", "barrier"]),
    ("pillar", &["pillar", "column", "post"]),
    ("floor", &["floor", "ground", "tile", "platform", "block-"]),
    ("straight", &["straight", "-line", "middle", "section", "corridor", "road", "track"]),
];

/// Packs that are MODULAR KITS: their models are designed to snap together on
/// a grid, so they are level vocabulary rather than standalone props.
///
/// Curated rather than inferred. A filename heuristic mislabels both ways —
/// `castle-kit` is named "kit" but is mostly scenery, while `city` genuinely
/// tiles — and 52 packs is few enough that being right matters more than
/// being clever.
pub const KIT_PACKS: &[&str] = &[
    "city-kit-roads",
    "city-kit-commercial",
    "city-kit-suburban",
    "city-kit-industrial",
    "modular-buildings",
    "modular-dungeon-kit",
    "modular-cave-kit",
    "modular-space-kit",
    "hexagon-kit",
    "brick-kit",
    "building-kit",
    "platformer-kit",
    "tower-defense-kit",
    "mini-dungeon",
    "mini-arena",
    "retro-urban-kit",
    "coaster-kit",
    "marble-kit",
    "minigolf-kit",
    "train-kit",
    "racing-kit",
    "city",
    "arena",
];

pub fn is_kit(pack: &str) -> bool {
    KIT_PACKS.contains(&pack)
}

/// Which roles may legitimately connect to which. Data, not prose, because a
/// composition layer consumes it.
///
/// Deliberately coarse: Kenney filenames say what a piece IS, never which of
/// its edges are open, so anything finer would be invented. A layout planner
/// uses this to know a corridor may follow a corner; it must still decide
/// rotation from its own grid.
pub const ROLE_ADJACENCY: &[(&str, &[&str])] = &[
    ("straight", &["straight", "corner", "junction", "end", "ramp", "door", "bridge"]),
    ("corner", &["straight", "junction", "corner", "end"]),
    ("junction", &["straight", "corner", "end", "junction"]),
    ("end", &["straight", "corner", "junction"]),
    ("ramp", &["straight", "floor", "stairs"]),
    ("stairs", &["floor", "straight", "ramp"]),
    ("floor", &["floor", "wall", "ramp", "stairs", "door"]),
    ("wall", &["wall", "corner", "door", "window", "pillar"]),
    ("door", &["wall", "straight", "floor"]),
    ("window", &["wall"]),
    ("roof", &["roof", "wall"]),
    ("pillar", &["wall", "floor"]),
    ("bridge", &["straight", "end"]),
];

pub fn connects_to(role: &str) -> &'static [&'static str] {
    ROLE_ADJACENCY
        .iter()
        .find(|(r, _)| *r == role)
        .map(|(_, v)| *v)
        .unwrap_or(&[])
}

/// Composition words a person uses when BUILDING rather than when naming a
/// thing. These are query-side: they expand to role names above.
pub const COMPOSITION_WORDS: &[(&str, &str)] = &[
    ("road", "straight"),
    ("track", "straight"),
    ("path", "straight"),
    ("straight bit", "straight"),
    ("corner piece", "corner"),
    ("bend", "corner"),
    ("t junction", "junction"),
    ("crossroads", "junction"),
    ("wall section", "wall"),
    ("dungeon floor", "floor"),
    ("roof piece", "roof"),
    ("dead end", "end"),
    ("ramp", "ramp"),
    ("doorway", "door"),
    ("slope", "ramp"),
    ("crossroad", "junction"),
    ("intersection", "junction"),
    ("floor tile", "floor"),
    ("platform", "floor"),
];

/// State keywords implied by a model's clip list, e.g. a model whose clips
/// include `1H_Melee_Attack_Chop` becomes findable as "attack".
pub fn states_from_clips(clips: &[String]) -> Vec<String> {
    let lower: Vec<String> = clips.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut out = Vec::new();
    for (word, frags) in STATE_WORDS {
        if lower
            .iter()
            .any(|c| frags.iter().any(|f| c.contains(&f.to_ascii_lowercase())))
        {
            out.push((*word).to_string());
        }
    }
    out
}

/// The tile role a filename implies, if any. First match wins, and the more
/// specific inner/outer corners are tested before plain "corner".
pub fn role_of(stem: &str) -> Option<&'static str> {
    let s = stem.to_ascii_lowercase();
    for role in ["corner-inner", "corner-outer"] {
        if let Some((_, frags)) = TILE_ROLES.iter().find(|(r, _)| *r == role) {
            if frags.iter().any(|f| s.contains(f)) {
                return Some(role);
            }
        }
    }
    TILE_ROLES
        .iter()
        .find(|(r, frags)| {
            !r.starts_with("corner-") && frags.iter().any(|f| s.contains(f))
        })
        .map(|(r, _)| *r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_vocabularies_differ_per_pack_but_map_to_the_same_words() {
        // KayKit names it Running_A, Quaternius names it Run, an animal
        // gallops. "run" must find all three or the query is useless.
        let kaykit = vec!["Running_A".into(), "1H_Melee_Attack_Chop".into(), "Death_A".into()];
        let quaternius = vec!["Run".into(), "Punch".into(), "Death".into()];
        let animal = vec!["Gallop".into(), "Attack_Headbutt".into(), "Eating".into()];
        for set in [&kaykit, &quaternius, &animal] {
            let s = states_from_clips(set);
            assert!(s.contains(&"run".to_string()), "no run in {s:?}");
            assert!(s.contains(&"attack".to_string()), "no attack in {s:?}");
        }
        assert!(states_from_clips(&animal).contains(&"eat".to_string()));
        assert!(states_from_clips(&kaykit).contains(&"die".to_string()));
    }

    #[test]
    fn a_model_with_no_clips_claims_no_states() {
        assert!(states_from_clips(&[]).is_empty());
    }

    #[test]
    fn tile_roles_come_from_kenney_naming() {
        assert_eq!(role_of("road-corner"), Some("corner"));
        assert_eq!(role_of("road-straight"), Some("straight"));
        assert_eq!(role_of("road-intersection"), Some("junction"));
        assert_eq!(role_of("wall-doorway"), Some("door"));
        assert_eq!(role_of("stairs-corner-inner"), Some("corner-inner"));
        assert_eq!(role_of("banner"), None);
    }
}
