//! The rigged cast, verified against the loader that will actually load it.
//!
//! The index's own GLB probe is a lightweight reader — and it was silently
//! wrong for the entire library until recently (its magic constant spelled
//! "gLUF", so every file failed the check and every model indexed as
//! unrigged, unanimated and sizeless). That bug survived because the test
//! fixture wrote the same wrong magic, so probe and test agreed with each
//! other while both disagreed with reality.
//!
//! These tests exist so that cannot happen again: they parse each character
//! with `makepad_game_render::skin`, the code the app runs, and assert the
//! index's claims match it. A character that indexes but will not load is
//! worse than one that is absent, because only the second is visible.

use std::path::{Path, PathBuf};

use makepad_game_assets::AssetIndex;

fn resources() -> Option<PathBuf> {
    let p = Path::new("../../../apps/arcade/resources");
    let p = if p.is_dir() {
        p.to_path_buf()
    } else {
        PathBuf::from("apps/arcade/resources")
    };
    p.is_dir().then_some(p)
}

/// Every entry the index calls rigged must parse through the real skin loader,
/// and report the same joint and clip counts the index advertises.
#[test]
fn every_rigged_entry_loads_and_matches_the_index() {
    let Some(root) = resources() else { return };
    let idx = AssetIndex::build(&root);
    let rigged: Vec<_> = idx.entries().iter().filter(|e| e.rigged).collect();
    if rigged.is_empty() {
        // No packs downloaded — run apps/arcade/download_assets.sh.
        return;
    }

    for e in &rigged {
        let bytes = match std::fs::read(&e.path) {
            Ok(b) => b,
            Err(err) => panic!("{}: indexed but unreadable: {err}", e.id),
        };
        let model = match makepad_game_render::skin::SkinnedModel::parse_glb(&bytes) {
            Ok(m) => m,
            Err(err) => panic!("{}: indexed as rigged but the loader rejects it: {err}", e.id),
        };
        assert_eq!(
            model.joint_count() as u32,
            e.joints,
            "{}: index says {} joints, loader found {}",
            e.id,
            e.joints,
            model.joint_count()
        );
        assert_eq!(
            model.clips.len(),
            e.clips.len(),
            "{}: index says {} clips, loader found {}",
            e.id,
            e.clips.len(),
            model.clips.len()
        );
        assert!(
            model.joint_count() > 0,
            "{}: rigged with zero joints is not a rig",
            e.id
        );
    }
}

/// The KayKit cast shares one skeleton ACROSS TWO SEPARATE DOWNLOADS, which is
/// the property that makes it worth having: a clip authored for the knight
/// plays on the skeleton warrior, so an AI can recast a part without touching
/// animation code. If a future pack bump breaks that, the whole "one animation
/// path serves the cast" assumption goes with it.
#[test]
fn the_kaykit_cast_shares_one_rig_across_both_packs() {
    let Some(root) = resources() else { return };
    let idx = AssetIndex::build(&root);
    let kaykit: Vec<_> = idx
        .entries()
        .iter()
        .filter(|e| e.rigged && e.id.starts_with("kaykit/"))
        .collect();
    if kaykit.is_empty() {
        return;
    }

    let joints = kaykit[0].joints;
    for e in &kaykit {
        assert_eq!(
            e.joints, joints,
            "{} has {} joints, expected the shared {joints}",
            e.id, e.joints
        );
    }

    // Adventurers and skeletons are different repositories; both must be here,
    // or the test is only checking one pack against itself.
    assert!(
        kaykit.iter().any(|e| e.id.contains("skeleton_")),
        "no skeletons — the cross-pack half of the shared-rig claim is untested"
    );
    assert!(
        kaykit.iter().any(|e| !e.id.contains("skeleton_")),
        "no adventurers"
    );

    // Skeletons carry the adventurers' clips plus undead extras, so the
    // superset direction is what must hold.
    let adv: Vec<&String> = kaykit
        .iter()
        .find(|e| e.id.ends_with("/knight"))
        .map(|e| e.clips.iter().collect())
        .unwrap_or_default();
    if let Some(skel) = kaykit.iter().find(|e| e.id.ends_with("/skeleton_warrior")) {
        for clip in &adv {
            assert!(
                skel.clips.contains(clip),
                "skeleton_warrior is missing the knight's {clip:?} — the casts are no longer interchangeable"
            );
        }
    }
}

/// A cast advertises only the states EVERY member can perform.
#[test]
fn cast_states_are_the_intersection_not_the_union() {
    let Some(root) = resources() else { return };
    let idx = AssetIndex::build(&root);
    for cast in idx.casts() {
        for id in &cast.members {
            let e = idx.entries().iter().find(|e| &e.id == id).unwrap();
            let mine = makepad_game_assets::states::states_from_clips(&e.clips);
            for s in &cast.shared_states {
                assert!(
                    mine.contains(s),
                    "cast of {} joints claims {s:?}, but {id} cannot do it",
                    cast.joints
                );
            }
        }
    }
}

/// The states a game actually asks for by name must resolve to real characters.
#[test]
fn characters_are_findable_by_what_they_can_do() {
    let Some(root) = resources() else { return };
    let idx = AssetIndex::build(&root);
    if idx.entries().iter().all(|e| !e.rigged) {
        return;
    }
    for query in [
        "a character that can attack",
        "someone who can run and jump",
        "a skeleton enemy",
        "a character that can die",
    ] {
        let hits = idx.find(query);
        assert!(
            hits.iter().take(8).any(|h| h.entry.rigged),
            "{query:?} returned no rigged character in its top hits"
        );
    }
}

/// The cast tool's JSON must actually parse.
///
/// Hand-rolled JSON emission is where brace arithmetic goes wrong silently —
/// this exact function shipped a doubled closing brace, and nothing noticed
/// because the string still *looked* like JSON in a log. A structural check is
/// cheap; a malformed tool result costs the AI a whole turn.
#[test]
fn cast_json_is_well_formed_and_small() {
    let Some(root) = resources() else { return };
    let idx = AssetIndex::build(&root);
    let casts = makepad_game_assets::agent::execute_cast(&idx, None);
    if casts.is_empty() {
        return;
    }
    let json = makepad_game_assets::agent::casts_to_json(&casts, 6);

    // Balance and nesting, without pulling in a JSON parser.
    let mut depth_obj = 0i32;
    let mut depth_arr = 0i32;
    let mut in_str = false;
    for ch in json.chars() {
        match ch {
            '"' => in_str = !in_str,
            '{' if !in_str => depth_obj += 1,
            '}' if !in_str => depth_obj -= 1,
            '[' if !in_str => depth_arr += 1,
            ']' if !in_str => depth_arr -= 1,
            _ => {}
        }
        assert!(depth_obj >= 0 && depth_arr >= 0, "unbalanced close in: {json}");
    }
    assert_eq!(depth_obj, 0, "unclosed/extra object brace in: {json}");
    assert_eq!(depth_arr, 0, "unclosed/extra array bracket in: {json}");
    assert!(!in_str, "unterminated string in: {json}");
    assert!(!json.contains("}}"), "doubled closing brace in: {json}");

    // Every turn pays for this, so it must stay a summary rather than a dump:
    // the 41-joint cast alone has 95 clips per member.
    assert!(json.len() < 4096, "cast JSON is {} bytes — too fat for a prompt", json.len());
}

/// A character's texture must be resolvable, or it renders untextured — the
/// failure mode that cost three attempts on the Kenney packs, where an atlas
/// existed on disk at a path no model referenced.
///
/// KayKit differs from Kenney: its GLBs EMBED the atlas, so there is no
/// external path to get wrong. The sidecar PNGs are fetched anyway because the
/// skin loader deliberately ignores materials and the app binds its own
/// texture. This asserts both halves hold.
#[test]
fn character_textures_resolve() {
    let Some(root) = resources() else { return };
    let idx = AssetIndex::build(&root);
    let kaykit: Vec<_> = idx
        .entries()
        .iter()
        .filter(|e| e.rigged && e.id.starts_with("kaykit/"))
        .collect();
    if kaykit.is_empty() {
        return;
    }
    let dir = root.join("characters");
    for e in &kaykit {
        let bytes = std::fs::read(&e.path).expect("readable");
        let model = makepad_game_render::skin::SkinnedModel::parse_glb(&bytes).expect("parses");
        assert!(
            model.vertex_count() > 0,
            "{}: no vertices, so nothing to texture",
            e.id
        );
    }
    // The app loads <name>_texture.png beside the GLB; every character must
    // have one it can reach (Rogue_Hooded re-skins rogue's).
    for name in [
        "knight_texture.png",
        "barbarian_texture.png",
        "mage_texture.png",
        "rogue_texture.png",
        "skeleton_texture.png",
    ] {
        assert!(
            dir.join(name).is_file(),
            "{name} missing — characters would render untextured"
        );
    }
}
