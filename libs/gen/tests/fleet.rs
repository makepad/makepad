//! The live gate (opt-in; spends real fleet GPU work): the character preset
//! on the LAN AI Hub fleet at full quality, published into a scratch results folder.
//! Run with `cargo test --release -p makepad-gen --test fleet -- --ignored
//! --nocapture` on a machine that hears the fleet's beacons. It asserts the
//! same facts the validated composite gated on (a skinned GLB with named
//! clips) and prints the evidence: models, nodes, seconds, facts.
#![cfg(not(target_arch = "wasm32"))]

use makepad_flow::engine::results::FolderStore;
use makepad_gen::host::GenHostConfig;
use makepad_gen::service::GenService;
use makepad_gen::tools::{job_json, Phase, RunRequest};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
#[ignore = "opt-in: runs the full character pipeline on the live AI Hub fleet"]
fn live_fleet_character_at_full_quality() {
    // GEN_GATE_DIR keeps the scratch folder for inspection; otherwise it goes.
    let kept = std::env::var_os("GEN_GATE_DIR").map(std::path::PathBuf::from);
    let scratch = kept.clone().unwrap_or_else(|| std::env::temp_dir().join(format!("makepad-gen-fleet-{}", std::process::id())));
    struct Cleanup(Option<std::path::PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(path) = &self.0 {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }
    let _cleanup = Cleanup(kept.is_none().then(|| scratch.clone()));
    let results = FolderStore::open(scratch.join("results"), "gate").unwrap();
    // Beacons arrive every couple of seconds: hear the fleet first.
    let discovery = makepad_ai_hub::discovery::start_listener();
    let heard = Instant::now();
    while heard.elapsed() < Duration::from_secs(5) || discovery.nodes().is_empty() {
        assert!(heard.elapsed() < Duration::from_secs(30), "no AI Hub beacons on this LAN");
        std::thread::sleep(Duration::from_millis(200));
    }
    let service = GenService::start(GenHostConfig {
        root: scratch.join("host"),
        results: Arc::new(results),
        namespace: "gate".into(),
        router: Arc::new(makepad_ai_hub::job::FleetRouter::default()),
    }).unwrap();
    let prompt = std::env::var("GEN_GATE_PROMPT").unwrap_or_else(|_| "a knight in battered plate armour".into());
    let request = RunRequest { preset: "character".into(), prompt: Some(prompt), inputs: Vec::new(), params: Vec::new(), seed: Some(20260930) };
    let started = Instant::now();
    let mut view = service.run(&request, true, &|| false, &mut |stage, permille| eprintln!("[{:>5.1}s] {:>4}‰ {stage}", started.elapsed().as_secs_f64(), permille)).unwrap();
    while view.phase == Phase::Running {
        view = service.wait(&view.id, &|| false, &mut |_, _| {}).unwrap();
    }
    eprintln!("character gate: {}", job_json(&view).to_json());
    assert_eq!(view.phase, Phase::Done, "{:?}", view.error);
    let facts = view.character.as_ref().expect("the animated character's facts");
    assert_eq!(facts.get("skinned"), Some(&makepad_strict_json::Value::Bool(true)));
    assert!(facts.get("clips").and_then(|clips| clips.as_arr()).is_some_and(|clips| !clips.is_empty()), "named clips");
    assert_eq!(view.assets[0].kind, "character");
    // The same run again: the stored character, and no job.
    let again = service.run(&request, true, &|| false, &mut |_, _| {}).unwrap();
    assert!(again.reused, "a repeated fixed-seed run is reused");
    assert_eq!(again.assets[0].asset_id, view.assets[0].asset_id);
}
