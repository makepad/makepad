//! Query-quality suite, run against the REAL downloaded library.
//!
//! These are written the way a kid or an agent actually phrases a request, not
//! the way the filenames are spelled. The suite reports every miss rather than
//! being tuned until it is green — a miss list tells us where the aliases are
//! thin, which is the useful output.
//!
//! Skips with a hint when the library isn't downloaded.

use makepad_game_assets::{agent, AssetIndex, AssetKind};
use std::path::PathBuf;

fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../apps/arcade/resources")
}

fn index() -> Option<AssetIndex> {
    let idx = AssetIndex::build(&resources());
    if idx.is_empty() {
        eprintln!("SKIP: no assets installed — run apps/arcade/download_assets.sh");
        return None;
    }
    Some(idx)
}

/// (query, substring that must appear in the id of a top-3 hit)
const SUITE: &[(&str, &str)] = &[
    // --- models: plain naming -------------------------------------------
    ("i want a lorry", "truck"),
    ("red truck", "truck-red"),
    ("motorbike", "motorcycle"),
    ("a house", "building-small"),
    ("trees for a forest", "trees"),
    ("big tree", "tree"),
    ("a sword", "sword"),
    ("knight", "knight"),
    ("statue", "statue"),
    ("fountain", "fountain"),
    ("coin", "coin"),
    ("a flag", "flag"),
    ("stairs", "stairs"),
    ("a gate", "gate"),
    // --- models: function over identity ---------------------------------
    ("something to drive", "vehicle"),
    ("something to drive on", "road"),
    ("somewhere to hide", "wall"),
    ("something to hide behind", "wall"),
    ("somewhere for my guy to stand", "platform"),
    ("something to jump on", "platform"),
    ("somewhere to live", "building"),
    ("something to shoot at", "enemy"),
    ("something to shoot with", "blaster"),
    ("something to win", "trophy"),
    ("something to collect", "coin"),
    // --- models: kid vocabulary and misspellings ------------------------
    ("a big scary monster", "enemy"),
    ("baddie", "enemy"),
    ("my guy", "character"),
    ("vehical", "vehicle"),
    ("hosue", "building-small"),
    ("motercycle", "motorcycle"),
    // --- models: theme and quality --------------------------------------
    ("city street", "road"),
    ("race track corner", "track-corner"),
    ("medieval castle wall", "wall"),
    ("grass for the ground", "grass"),
    // --- audio: event and trigger ---------------------------------------
    ("sound when you crash into a wall", "impact"),
    ("laser gun", "laser"),
    ("footsteps on wood", "footstep_wood"),
    ("explosion", "explosion"),
    ("jump sound", "jump"),
    ("click sound for a button", "click"),
    ("error sound", "error"),
    ("sound when you get hit", "impact"),
    ("coins", "coin"),
    ("power up sound", "powerup"),
    // --- audio: material -------------------------------------------------
    ("glass smashing", "impactGlass"),
    ("metal clang", "impactMetal"),
    ("footsteps on grass", "footstep_grass"),
    // --- audio: music and mood ------------------------------------------
    ("happy win music", "jingles"),
    ("victory fanfare", "jingles"),
    ("retro 8-bit sound", "digital-audio"),
    ("spaceship engine", "spaceEngine"),
];

#[test]
fn natural_language_queries_find_the_right_asset() {
    let Some(idx) = index() else { return };
    let mut misses = Vec::new();
    for (query, expect) in SUITE {
        let hits = idx.find(query);
        let top3: Vec<&str> = hits.iter().take(3).map(|h| h.entry.id.as_str()).collect();
        // Case-insensitive: Kenney mixes camelCase (phaseJump) and snake_case.
        let want = expect.to_lowercase();
        if !top3.iter().any(|id| id.to_lowercase().contains(&want)) {
            misses.push((query, *expect, top3.join(", ")));
        }
    }
    if !misses.is_empty() {
        eprintln!("\n--- alias MISS list ({}/{}) ---", misses.len(), SUITE.len());
        for (q, want, got) in &misses {
            eprintln!("  {q:40} want ~{want:20} got: {got}");
        }
        eprintln!();
    }
    // The bar: the suite exists to expose thin aliases, so a handful of misses
    // is informative rather than fatal — but a majority failing means the
    // table is broken.
    assert!(
        misses.len() * 4 < SUITE.len(),
        "{}/{} queries missed — alias table regressed",
        misses.len(),
        SUITE.len()
    );
}

#[test]
fn ranking_is_deterministic_across_runs() {
    let Some(idx) = index() else { return };
    for q in ["truck", "something to drive", "laser", "happy win music"] {
        let a: Vec<String> = idx.find(q).iter().map(|h| h.entry.id.clone()).collect();
        let b: Vec<String> = idx.find(q).iter().map(|h| h.entry.id.clone()).collect();
        assert_eq!(a, b, "ranking for {q:?} is not stable");
    }
}

#[test]
fn ids_are_stable_across_index_builds() {
    let Some(a) = index() else { return };
    let b = AssetIndex::build(&resources());
    let ids_a: Vec<&str> = a.entries().iter().map(|e| e.id.as_str()).collect();
    let ids_b: Vec<&str> = b.entries().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids_a, ids_b);
}

#[test]
fn kind_filter_separates_models_sounds_and_music() {
    let Some(idx) = index() else { return };
    let sounds = agent::execute(
        &idx,
        &agent::FindParams::new("laser").with_kind_str("sound"),
    );
    assert!(!sounds.is_empty());
    assert!(sounds.iter().all(|r| r.kind == AssetKind::Sound));

    let models = agent::execute(
        &idx,
        &agent::FindParams::new("truck").with_kind_str("model"),
    );
    assert!(!models.is_empty());
    assert!(models.iter().all(|r| r.kind == AssetKind::Model));

    let music = agent::execute(
        &idx,
        &agent::FindParams::new("jingle").with_kind_str("music"),
    );
    assert!(!music.is_empty(), "music jingles should be findable");
    assert!(music.iter().all(|r| r.kind == AssetKind::Music));
}

#[test]
fn music_is_never_returned_as_a_hit_sound() {
    let Some(idx) = index() else { return };
    // A game asking for an impact must not be handed a 30-second track.
    let hits = agent::execute(
        &idx,
        &agent::FindParams::new("sound when you get hit").with_kind_str("sound"),
    );
    assert!(hits.iter().all(|r| r.kind != AssetKind::Music));
}

#[test]
fn resolve_rejects_a_hallucinated_id_and_suggests_near_misses() {
    let Some(idx) = index() else { return };
    let err = idx
        .resolve_or_explain("kenney/racing/vehicle-truck-orange")
        .unwrap_err();
    assert!(err.contains("did you mean"), "no suggestions in: {err}");
    assert!(err.contains("truck"), "suggestions unrelated: {err}");

    // A real id resolves.
    let real = &idx.entries()[0].id.clone();
    assert!(idx.resolve_or_explain(real).is_ok());
}

#[test]
fn local_librarian_gets_a_confident_answer_for_an_obvious_request() {
    let Some(idx) = index() else { return };
    let action = agent::local_spawn(&idx, "put a truck in the game").expect("should resolve");
    assert!(action.model_id.contains("truck"), "got {}", action.model_id);
    assert!(
        action.confidence > 0.3,
        "confidence too low ({}) for an obvious request",
        action.confidence
    );
}

#[test]
fn agent_results_are_compact_and_hide_index_internals() {
    let Some(idx) = index() else { return };
    let results = agent::execute(&idx, &agent::FindParams::new("red truck"));
    let json = agent::results_to_json(&results);
    assert!(json.starts_with('['));
    // Paths and packs are not the agent's business.
    assert!(!json.contains("resources/"), "leaked a path: {json}");
    assert!(!json.contains(".glb"), "leaked a filename: {json}");
    // Compact enough to paste into a prompt.
    assert!(json.len() < 800, "result blob too large: {} bytes", json.len());
}

#[test]
fn undecodable_audio_is_flagged_rather_than_silently_broken() {
    let Some(idx) = index() else { return };
    let stuck = idx.undecodable();
    // Kenney audio is ogg and this tree has no vorbis decoder — the index must
    // say so rather than let a game fire a sound that never plays.
    assert!(!stuck.is_empty());
    assert!(stuck.iter().all(|e| e.format == "ogg"));
    let results = agent::execute(&idx, &agent::FindParams::new("laser").with_kind_str("sound"));
    let json = agent::results_to_json(&results);
    assert!(json.contains("\"playable\":false"), "agent not told: {json}");
}
