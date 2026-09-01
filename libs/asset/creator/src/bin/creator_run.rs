//! makepad-creator-run: the detached client for long pipeline runs
//! (aicore §9 — "a run that must outlive a window is a client that does not
//! close"). Drives one library pipeline against a hub node and writes the
//! finished artifacts beside it; close your laptop lid AFTER starting this,
//! not instead of it.
//!
//! ```text
//! makepad-creator-run --base http://10.0.0.217:8765 --stages text,image \
//!     --prompt "a rusted lighthouse" [--model-0 qwen3.5-9b] [--out DIR]
//! ```
//!
//! v1 drives a linear chain: each stage's domain in order, the first stage
//! takes the prompt, every later stage splices its predecessor (text →
//! prompt, artifact → input image). The named-pipeline catalogue replaces
//! `--stages` as chains migrate in.

use makepad_asset_creator::engine::{
    run, EngineConfig, RunEvent, Splice, StageOrder,
};
use makepad_asset_creator::pipeline::{PipelineSpec, StageSpec, DEFAULT_STAGE_WEIGHT};
use makepad_asset_creator::makepad_ai_hub::client::LocalService;
use makepad_asset_creator::makepad_ai_hub::protocol::GenerateRequestJson;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Arc;

fn main() {
    if let Err(err) = run_cli() {
        eprintln!("makepad-creator-run: {err}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<(), String> {
    let mut base = String::new();
    let mut stages: Vec<String> = Vec::new();
    let mut prompt = String::new();
    let mut out_dir = PathBuf::from(".");
    let mut models: Vec<(usize, String)> = Vec::new();
    let mut seed: u64 = 1;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--base" => base = value("--base")?,
            "--stages" => {
                stages = value("--stages")?.split(',').map(|s| s.trim().to_string()).collect()
            }
            "--prompt" => prompt = value("--prompt")?,
            "--out" => out_dir = PathBuf::from(value("--out")?),
            "--seed" => seed = value("--seed")?.parse().map_err(|_| "bad --seed")?,
            other => {
                if let Some(rest) = other.strip_prefix("--model-") {
                    let index: usize = rest.parse().map_err(|_| format!("bad {other}"))?;
                    models.push((index, value(other)?));
                } else {
                    return Err(format!("unknown argument {other:?}"));
                }
            }
        }
    }
    if base.is_empty() || stages.is_empty() || prompt.is_empty() {
        return Err("usage: --base URL --stages a,b,c --prompt TEXT [--model-N id] [--out DIR] [--seed N]".into());
    }

    let spec = PipelineSpec {
        name: stages.join("-"),
        stages: stages
            .iter()
            .enumerate()
            .map(|(i, domain)| StageSpec {
                key: format!("s{i}-{domain}"),
                domain: domain.clone(),
                deps: if i == 0 {
                    Vec::new()
                } else {
                    vec![format!("s{}-{}", i - 1, stages[i - 1])]
                },
                weight: DEFAULT_STAGE_WEIGHT,
                seed,
                on_fail_skip: false,
            })
            .collect(),
    };
    let orders: Vec<StageOrder> = spec
        .stages
        .iter()
        .enumerate()
        .map(|(i, stage)| {
            let mut request = GenerateRequestJson::default();
            request.seed = Some(seed);
            request.model = models
                .iter()
                .find(|(index, _)| *index == i)
                .map(|(_, model)| model.clone())
                .unwrap_or_default();
            if i == 0 {
                request.prompt = Some(prompt.clone());
            }
            let splices = if i == 0 {
                Vec::new()
            } else {
                let dep = stage.deps[0].clone();
                if stages[i - 1] == "text" {
                    vec![Splice::PromptFromText(dep)]
                } else {
                    vec![Splice::InputImageFrom(dep)]
                }
            };
            StageOrder { spec: stage.clone(), request, splices }
        })
        .collect();

    let base_url = base.clone();
    let provider = makepad_asset_creator::engine::SingleProvider(move || {
        Box::new(LocalService::new(&base_url)) as Box<_>
    });
    let (events_tx, events_rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let printer = std::thread::spawn(move || {
        for event in events_rx {
            match event {
                RunEvent::StageStarted { key, job_id } => println!("▶ {key} → {job_id}"),
                RunEvent::StageProgress { key, stage, progress } => {
                    if let Some(p) = progress {
                        println!("  {key}: {} {:.0}%", stage.unwrap_or_default(), p * 100.0)
                    }
                }
                RunEvent::StageDone { key, .. } => println!("✓ {key}"),
                RunEvent::StageFailed { key, error } => println!("✗ {key}: {error}"),
                RunEvent::StageSkipped { key, error } => println!("⤼ {key} skipped: {error}"),
                RunEvent::RunFinished { state } => println!("run: {state:?}"),
            }
        }
    });
    let outputs = run(
        &spec,
        &orders,
        &provider,
        &EngineConfig::default(),
        &events_tx,
        &cancel,
    )
    .map_err(|e| e.to_string())?;
    drop(events_tx);
    let _ = printer.join();

    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    for (key, output) in &outputs {
        if let Some(artifact) = &output.artifact {
            let ext = match artifact.content_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "video/mp4" => "mp4",
                "audio/wav" => "wav",
                "model/gltf-binary" => "glb",
                t if t.starts_with("text/") => "txt",
                _ => "bin",
            };
            let path = out_dir.join(format!("{key}.{ext}"));
            std::fs::write(&path, &artifact.bytes).map_err(|e| e.to_string())?;
            println!("wrote {}", path.display());
        } else if let Some(text) = &output.text {
            println!("{key}: {text}");
        }
    }
    Ok(())
}
