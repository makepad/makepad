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
    // --- full-catalogue breadth (added after the 4400-model pull) --------
    ("a cat", "cat"),
    ("a dog", "dog"),
    ("penguin", "penguin"),
    ("a boat", "boat"),
    ("pirate ship", "ship"),
    ("a train", "train"),
    ("police car", "police"),
    ("an ambulance", "ambulance"),
    ("pizza", "pizza"),
    ("an apple", "apple"),
    ("a sofa", "sofa"),
    ("a bed", "bed"),
    ("skateboard", "skateboard"),
    ("castle tower", "tower"),
    ("spaceship", "craft"),
    ("astronaut", "astronaut"),
    ("an alien", "alien"),
    ("traffic cone", "cone"),
    ("gravestone", "grave"),
    ("christmas tree", "tree"),
    ("a barrel", "barrel"),
    ("conveyor belt", "belt"),
    ("golf hole", "golf"),
    ("hexagon tile", "hex"),
    ("a rock", "rock"),
    ("a bridge", "bridge"),
    ("staircase", "stairs"),
    ("a fence", "fence"),
    ("street lamp", "light"),
    ("a chair", "chair"),
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
fn every_shipped_sound_is_playable_and_the_agent_is_told_so() {
    let Some(idx) = index() else { return };
    // The in-house Vorbis decoder reached sample-exact on all 556 Kenney
    // sounds, so nothing we ship is indexed-but-unplayable any more. The
    // `undecodable` reporting path stays for a future format we might index
    // before we can play it — this asserts the CURRENT catalogue is clean.
    let stuck = idx.undecodable();
    assert!(
        stuck.is_empty(),
        "unplayable entries: {:?}",
        stuck.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
    // A game must never be able to fire a sound that produces silence, so the
    // agent is only ever told playable:false — never left to guess.
    let results = agent::execute(&idx, &agent::FindParams::new("laser").with_kind_str("sound"));
    assert!(!results.is_empty(), "no laser sounds found");
    let json = agent::results_to_json(&results);
    assert!(!json.contains("\"playable\":false"), "still gated: {json}");
}

#[test]
fn index_build_and_query_stay_fast_at_full_catalogue_scale() {
    use std::time::Instant;
    let t = Instant::now();
    let Some(idx) = index() else { return };
    let build_ms = t.elapsed().as_millis();
    // Whole-catalogue build happens once at startup. This was ~120 ms when
    // the GLB probe silently rejected every file (wrong magic constant), so
    // that figure measured a no-op. With the probe actually reading 5300
    // models it is ~1.8 s alone, and more under parallel test contention.
    // The bound is generous on purpose: it exists to catch an order-of-
    // magnitude regression, not to pin a number this test cannot control.
    // The real fix is caching probe results by path+mtime — see the report.
    assert!(build_ms < 12_000, "index build took {build_ms} ms");

    // Search sits in a chat loop, so it must be imperceptible. A naive
    // all-entries scan measured 73 ms here before the inverted index.
    //
    // MIN-of-N, not an average: this test shares a machine with 23 others, and
    // an averaged wall clock measures the contention rather than the query
    // (2.1 ms run alone, 44 ms under a full parallel suite — a 20x swing with
    // no code change). The fastest of several runs is the one where the query
    // actually got the CPU, which is what we mean to bound. Same protocol the
    // box3d benchmarks use for the same reason.
    let queries = ["truck", "something to drive", "a cat", "medieval castle", "laser"];
    let mut best = u128::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        for q in queries {
            let _ = idx.find(q);
        }
        best = best.min(t.elapsed().as_micros() / queries.len() as u128);
    }
    let us = best;
    assert!(us < 20_000, "average query {us} us — inverted index regressed?");
    eprintln!("scale: {} entries, build {build_ms} ms, {us} us/query", idx.len());
}

#[test]
fn summary_stays_tiny_against_the_real_catalogue() {
    let Some(idx) = index() else { return };
    let s = agent::library_summary(&idx);
    assert!(
        s.len() < 800,
        "summary is {} chars for {} entries:\n{s}",
        s.len(),
        idx.len()
    );
    // It must describe the catalogue, not enumerate it.
    assert!(!s.contains(".glb"));
}

#[test]
fn pack_themes_make_a_whole_pack_reachable_by_setting() {
    let Some(idx) = index() else { return };
    // No castle-kit filename contains "medieval", so any hit at all can only
    // come from the pack theme layer. Reachability is the claim — curated
    // keywords legitimately outrank themes, so this does not assert position.
    let hits = idx.find("medieval");
    assert!(
        hits.iter().any(|h| h.entry.pack == "castle-kit"),
        "pack theme layer not reaching castle-kit"
    );
    // "kingdom" is themed only by castle-kit, so it should lead there.
    let kingdom = idx.find("kingdom");
    assert!(
        kingdom.first().map(|h| h.entry.pack.as_str()) == Some("castle-kit"),
        "theme-only term did not lead to its pack: {:?}",
        kingdom.first().map(|h| h.entry.id.clone())
    );
}

#[test]
fn pack_themes_never_outrank_the_item_that_is_actually_named() {
    let Some(idx) = index() else { return };
    // Every pirate-kit model inherits "boat" from its pack theme; the models
    // actually called boat-* must still come first.
    let top = idx.find("boat");
    let first = top.first().expect("boats exist");
    assert!(
        first.entry.id.contains("boat") || first.entry.id.contains("ship"),
        "theme weighting regressed: {} outranked the real boats",
        first.entry.id
    );
}

// ---- modular kits -------------------------------------------------------

/// A kit must expose a grid pitch and a role vocabulary, or a layout planner
/// has nothing to place tiles against.
#[test]
fn kits_report_a_tile_size_and_their_roles() {
    let Some(idx) = index() else { return };
    let kits = idx.kits();
    assert!(kits.len() >= 15, "expected the kit packs, got {}", kits.len());

    let roads = kits
        .iter()
        .find(|k| k.pack == "city-kit-roads")
        .expect("city-kit-roads is a kit");
    // Kenney road tiles are a 1-unit grid; the median must land ON a real
    // pitch, not between two of them (which is why it is a median, not a mean).
    let size = roads.tile_size.expect("road kit must have a measured size");
    assert!((size - 1.0).abs() < 0.2, "road tile size {size} is not a grid pitch");
    for want in ["straight", "corner", "junction"] {
        assert!(
            roads.roles.iter().any(|(r, n)| r == want && *n > 0),
            "road kit missing {want}: {:?}",
            roads.roles
        );
    }
}

/// Tiles must be reachable BY KIT, because visual coherence within one level
/// is exactly what a naive per-model search destroys.
#[test]
fn kit_tiles_come_back_grouped_and_role_filtered() {
    let Some(idx) = index() else { return };
    let all = idx.kit_tiles("city-kit-roads", None);
    let corners = idx.kit_tiles("city-kit-roads", Some("corner"));
    assert!(!corners.is_empty() && corners.len() < all.len());
    assert!(corners.iter().all(|e| e.pack == "city-kit-roads"));
    assert!(corners
        .iter()
        .all(|e| e.role.is_some_and(|r| r.starts_with("corner"))));
}

/// The composition words people use when BUILDING ("junction", "ramp") must
/// reach tiles, not just the nouns a model is named after.
#[test]
fn composition_words_find_tiles() {
    let Some(idx) = index() else { return };
    for (q, role) in [("junction", "junction"), ("ramp", "ramp"), ("corner", "corner")] {
        let hits = idx.find(q);
        assert!(
            hits.iter().take(10).any(|h| h.entry.role == Some(role)),
            "'{q}' returned no {role} tile in its top 10"
        );
    }
}

/// The agent gets kit-level results compactly: enough to plan a layout,
/// without paying tokens for every tile id.
#[test]
fn find_kit_returns_compact_planning_data() {
    let Some(idx) = index() else { return };
    let kits = agent::execute_kit(&idx, Some("road"), None);
    assert!(!kits.is_empty(), "no road kits");
    let json = agent::kits_to_json(&kits);
    assert!(json.contains("tile_size"), "no grid pitch: {json}");
    assert!(json.contains("junction"), "no roles: {json}");
    assert!(json.len() < 2000, "kit listing too fat for a prompt: {}", json.len());

    // Filtering by role must actually narrow it.
    let with_ramps = agent::execute_kit(&idx, None, Some("ramp"));
    assert!(with_ramps.iter().all(|k| k.roles.iter().any(|(r, _)| r == "ramp")));
    assert!(with_ramps.len() < idx.kits().len());
}

// ---- variety and palettes ----------------------------------------------
//
// These test the index as a COMPOSITION tool rather than a search engine. The
// failure they exist to prevent was visible in a screenshot: five identical
// houses on five identical lots, identical tree clusters beside each. The
// ranking was never wrong — 21 distinct houses came back at equal score — but
// the caller took hit #1 and placed it five times, because nothing in the API
// offered anything else.

use makepad_game_assets::{Spread, VarietyParams};

/// The village case: several houses that are actually different houses.
#[test]
fn a_village_gets_distinct_houses_not_one_house_five_times() {
    let Some(idx) = index() else { return };
    let picks = idx.find_many("suburban house building", &VarietyParams::new(5));
    assert!(picks.len() >= 4, "only {} houses offered", picks.len());
    let ids: Vec<&str> = picks.iter().map(|e| e.id.as_str()).collect();
    let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "repeated a model: {ids:?}");
    // Every one must actually be a building — variety is not licence to drift
    // onto driveways and fences that merely share the pack's theme.
    assert!(
        ids.iter().all(|id| id.contains("building")),
        "variety drifted off-topic: {ids:?}"
    );
}

/// Coherence: a street built from five packs is five art styles. The dominant
/// pack has to be exhausted before the search crosses into another.
#[test]
fn a_scene_stays_in_one_art_pack_when_that_pack_can_supply_it() {
    let Some(idx) = index() else { return };
    let picks = idx.find_many("suburban house building", &VarietyParams::new(5));
    let packs: std::collections::BTreeSet<&str> =
        picks.iter().map(|e| e.pack.as_str()).collect();
    assert_eq!(
        packs.len(),
        1,
        "mixed {} art packs into one street: {:?}",
        packs.len(),
        packs
    );
}

/// The forest case: several species, and the option of several variants of one.
#[test]
fn a_forest_gets_several_species_and_can_get_variants_of_one() {
    let Some(idx) = index() else { return };
    let mixed = idx.find_many("tree", &VarietyParams::new(8));
    assert!(mixed.len() >= 6, "only {} trees offered", mixed.len());

    // Distinct silhouettes, not one tree recoloured: `tree_blocks`,
    // `tree_blocks_dark` and `tree_blocks_fall` are one shape in three
    // palettes and must not count as three kinds.
    let families: std::collections::BTreeSet<String> = mixed
        .iter()
        .map(|e| makepad_game_assets::variety::family_of(e))
        .collect();
    assert!(
        families.len() >= 4,
        "only {} distinct tree shapes in {:?}",
        families.len(),
        mixed.iter().map(|e| &e.id).collect::<Vec<_>>()
    );

    // And the other axis: a row of one kind, for when things should match.
    let row = idx.find_many("tree", &VarietyParams::new(3).spread(Spread::Variants));
    assert!(row.len() >= 2, "no variant row available");
    let row_fams: std::collections::BTreeSet<String> = row
        .iter()
        .map(|e| makepad_game_assets::variety::family_of(e))
        .collect();
    assert_eq!(row_fams.len(), 1, "Variants spread crossed families: {row_fams:?}");
}

/// Selection must be reproducible from (query, seed) alone: multiplayer
/// replicates a scene as parameters, and a re-run of a game must look the same.
#[test]
fn variety_is_seeded_reproducible_and_seed_actually_changes_it() {
    let Some(idx) = index() else { return };
    let take = |seed: u64| -> Vec<String> {
        idx.find_many("tree", &VarietyParams::new(5).seed(seed))
            .iter()
            .map(|e| e.id.clone())
            .collect()
    };
    assert_eq!(take(7), take(7), "same seed gave different picks");
    assert_ne!(take(7), take(42), "seed had no effect");
}

/// A palette is a matched set from ONE pack, grouped coarsely enough to be
/// usable — not a 167-entry listing of singletons.
#[test]
fn a_palette_is_a_coherent_set_grouped_usefully() {
    let Some(idx) = index() else { return };
    let p = idx.palette("village houses", 0).expect("a village pack exists");
    assert!(p.total() >= 20, "palette too thin: {} models", p.total());
    assert!(
        p.groups.len() >= 4 && p.groups.len() <= 40,
        "{} groups is a listing, not a palette",
        p.groups.len()
    );
    // Every id must come from the one pack — that is what makes it coherent.
    assert!(
        p.groups
            .iter()
            .flat_map(|(_, ids)| ids)
            .all(|id| id.contains(&p.pack)),
        "palette leaked models from another pack"
    );
    // Biggest group first, and it must offer several options so the caller can
    // place a different one each time.
    assert!(p.groups[0].1.len() >= 3);
}

/// The agent path must default to distinct models, because that is the caller
/// that was placing hit #1 five times.
#[test]
fn the_agent_tool_returns_distinct_models_by_default() {
    let Some(idx) = index() else { return };
    let results = agent::execute(
        &idx,
        &agent::FindParams::new("suburban house building")
            .with_spread_str("mixed"),
    );
    let ids: std::collections::BTreeSet<&str> =
        results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids.len(), results.len(), "agent got duplicate ids");
    assert!(results.len() >= 4);

    // And the palette tool stays payload-cheap.
    let p = agent::execute_palette(&idx, "village", 0).expect("palette");
    let json = agent::palette_to_json(&p, 4);
    assert!(json.len() < 1400, "palette json too fat: {} bytes", json.len());
    assert!(!json.contains(".glb") && !json.contains("resources/"));
}

/// A landscape rock must not lose "boulder" to a catapult projectile: a
/// confidently wrong top hit is worse than a miss, because a composer places
/// it several times.
#[test]
fn boulder_finds_a_landscape_rock_not_ammunition() {
    let Some(idx) = index() else { return };
    let top: Vec<&str> = idx.find("boulder").iter().take(3).map(|h| h.entry.id.as_str()).collect();
    assert!(
        top.iter().any(|id| id.contains("nature-kit/rock")),
        "boulder still ranks badly: {top:?}"
    );
}
