//! Tests that need no downloaded assets: synthetic fixture dirs, budget bounds
//! and graceful degradation. These must never skip, so CI has real coverage on
//! a fresh checkout.

use makepad_game_assets::{agent, AssetIndex, AssetKind};
use std::fs;
use std::path::{Path, PathBuf};

/// A throwaway directory under the target dir — no /tmp, no cleanup races.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a minimal GLB whose JSON chunk declares a skin and POSITION bounds,
/// so the probe has something real to read.
fn fake_glb(path: &Path, rigged: bool) {
    let json = if rigged {
        r#"{"asset":{"version":"2.0"},"skins":[{"joints":[0]}],"accessors":[{"min":[-1.0,0.0,-1.0],"max":[1.0,2.0,1.0]}]}"#
    } else {
        r#"{"asset":{"version":"2.0"},"accessors":[{"min":[0.0,0.0,0.0],"max":[1.0,1.0,1.0]}]}"#
    };
    let mut json = json.as_bytes().to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let mut out = Vec::new();
    // "glTF" little-endian. This fixture previously wrote 0x4655_4C67 —
    // which spells "gLUF" — matching a typo in the probe's own constant, so
    // the two agreed with each other while every REAL Kenney GLB was
    // rejected and indexed with no bounds and no rigged flag. Writing the
    // true magic is what makes this test able to fail.
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((20 + json.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json);
    fs::write(path, out).unwrap();
}

#[test]
fn missing_library_is_empty_not_an_error() {
    let idx = AssetIndex::build(Path::new("/definitely/not/here"));
    assert!(idx.is_empty());
    assert!(!idx.missing().is_empty(), "should report what it looked for");
    assert!(idx.find("truck").is_empty());
    assert!(agent::local_spawn(&idx, "a truck").is_none());
    // The prompt blurb must still be usable and must tell the model what to do.
    let summary = agent::library_summary(&idx);
    assert!(summary.contains("download_assets.sh"));
}

#[test]
fn index_builds_over_a_synthetic_dir() {
    let root = scratch("synthetic");
    let pack = root.join("models/kenney/racing");
    fs::create_dir_all(&pack).unwrap();
    fake_glb(&pack.join("vehicle-truck-red.glb"), false);
    fake_glb(&pack.join("vehicle-motorcycle.glb"), true);

    let idx = AssetIndex::build(&root);
    assert_eq!(idx.len(), 2);

    let truck = idx.resolve("kenney/racing/vehicle-truck-red").expect("id scheme");
    assert_eq!(truck.kind, AssetKind::Model);
    assert_eq!(truck.name, "Red Truck", "curated name should win over the stem");
    assert!(truck.keywords.iter().any(|k| k == "lorry"), "curation missing");
    assert_eq!(truck.size, Some([1.0, 1.0, 1.0]), "bounds from the GLB");
    assert!(!truck.rigged);

    let bike = idx.resolve("kenney/racing/vehicle-motorcycle").unwrap();
    assert!(bike.rigged, "skins array should mark it rigged");
}

#[test]
fn uncurated_files_are_still_indexed_and_findable() {
    let root = scratch("uncurated");
    let pack = root.join("models/kenney/mystery");
    fs::create_dir_all(&pack).unwrap();
    fake_glb(&pack.join("space-rocket-big.glb"), false);

    let idx = AssetIndex::build(&root);
    assert_eq!(idx.len(), 1);
    // Filename tokens are the floor: no curation, but still reachable.
    let hits = idx.find("rocket");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry.name, "Space Rocket Big");
}

#[test]
fn summary_stays_small_for_a_large_catalogue() {
    let root = scratch("large");
    // 400 entries across 8 packs — well past anything we ship.
    for p in 0..8 {
        let pack = root.join(format!("models/kenney/pack{p}"));
        fs::create_dir_all(&pack).unwrap();
        for i in 0..50 {
            fake_glb(&pack.join(format!("thing-{i}.glb")), false);
        }
    }
    let idx = AssetIndex::build(&root);
    assert_eq!(idx.len(), 400);

    let summary = agent::library_summary(&idx);
    // ~4 chars/token, so 200 tokens is roughly 800 characters. The summary must
    // not scale with the catalogue — it names categories and a few examples.
    assert!(
        summary.len() < 800,
        "summary is {} chars, too big for a system prompt:\n{summary}",
        summary.len()
    );
    assert!(summary.contains("find_model"), "must state the contract");
    assert!(summary.contains("never guess"), "must forbid guessing ids");
}

#[test]
fn summary_does_not_grow_when_the_library_doubles() {
    let build = |packs: usize| {
        let root = scratch(&format!("grow{packs}"));
        for p in 0..packs {
            let pack = root.join(format!("models/kenney/pack{p}"));
            fs::create_dir_all(&pack).unwrap();
            for i in 0..20 {
                fake_glb(&pack.join(format!("thing-{i}.glb")), false);
            }
        }
        agent::library_summary(&AssetIndex::build(&root))
    };
    let small = build(2);
    let large = build(8);
    // Only the counts change, so growth must be a handful of digits.
    assert!(
        large.len() < small.len() + 40,
        "summary grew {} -> {} chars with the catalogue",
        small.len(),
        large.len()
    );
}

#[test]
fn tool_descriptor_is_well_formed() {
    let d = agent::FIND_MODEL;
    assert_eq!(d.name, "find_model");
    assert!(d.params.iter().any(|p| p.name == "query" && p.required));
    // Every optional param must be genuinely optional.
    assert_eq!(d.params.iter().filter(|p| p.required).count(), 1);
    for p in d.params {
        assert!(!p.description.is_empty(), "{} has no description", p.name);
        assert!(matches!(p.ty, "string" | "boolean" | "integer"));
    }
}

#[test]
fn max_results_is_clamped() {
    let root = scratch("clamp");
    let pack = root.join("models/kenney/racing");
    fs::create_dir_all(&pack).unwrap();
    for i in 0..30 {
        fake_glb(&pack.join(format!("vehicle-truck-{i}.glb")), false);
    }
    let idx = AssetIndex::build(&root);
    let mut params = agent::FindParams::new("truck");
    params.max_results = Some(9999);
    assert!(agent::execute(&idx, &params).len() <= 20);
    params.max_results = Some(0);
    assert_eq!(agent::execute(&idx, &params).len(), 1);
}

#[test]
fn credits_list_only_sources_actually_present() {
    let root = scratch("credits");
    let pack = root.join("models/kenney/racing");
    fs::create_dir_all(&pack).unwrap();
    fake_glb(&pack.join("vehicle-truck-red.glb"), false);
    let idx = AssetIndex::build(&root);
    let credits = agent::credits(&idx);
    assert_eq!(credits.len(), 1);
    assert!(credits[0].contains("Kenney"));
}
