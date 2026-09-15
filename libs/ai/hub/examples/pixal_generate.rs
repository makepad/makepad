//! Native GPU smoke test / profiler. Run the release binary directly:
//! pixal_generate <verified-weights-dir> <input.png> <output.glb> [1024|1536] [REPEATS]
//! Weights use the filenames below; downloads and hashes still use the registry.
#[cfg(feature = "mesh")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use makepad_ai_hub::backend::{BackendCtx, CancelToken, ContentBackend, GenerateParams};
    use makepad_ai_hub::download::Downloader;
    use makepad_ai_hub::protocol::{GenerateRequestJson, PixalOptionsJson};
    use makepad_ai_hub::registry::Registry;
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 4 {
        return Err(
            "usage: pixal_generate WEIGHTS INPUT.png OUTPUT.glb [1024|1536] [REPEATS]".into(),
        );
    }
    let root = std::path::Path::new(&args[1]);
    let registry = Registry::embedded()?;
    let mut spec = registry
        .find("pixal3d")
        .ok_or("Pixal3D registry entry missing")?
        .clone();
    for file in &mut spec.files {
        let name = match file.role.as_deref().unwrap_or("") {
            "ss-flow" | "shape-flow-512" | "shape-flow-1024" | "texture-flow-1024" => {
                "pixal3d_bf16.safetensors".to_string()
            }
            "dino-conditioner" => "dino_naf.safetensors".to_string(),
            role => format!("{role}.safetensors"),
        };
        file.cache_as = name;
    }
    let downloader = Downloader::new("https://huggingface.co", None)?;
    let cancel = CancelToken::new();
    let mut download =
        |p: makepad_ai_hub::download::DownloadProgress| eprintln!("download {} {}", p.file, p.done);
    let mut load = |stage: &str, progress: f64| eprintln!("load {progress:.3} {stage}");
    let mut context = BackendCtx {
        spec: &spec,
        cache_dir: root,
        downloader: &downloader,
        download_progress: &mut download,
        cancel: &cancel,
        progress: &mut load,
    };
    let mut backend = makepad_ai_hub::trellis_backend::TrellisBackend::new_trellis("pixal3d");
    backend.ensure_loaded(&mut context)?;
    let mut params = GenerateParams::from_request(&GenerateRequestJson {
        model: "pixal3d".into(),
        seed: Some(42),
        texture: Some(true),
        remesh_resolution: Some(256),
        texture_size: Some(1024),
        decimation_target: Some(80_000),
        pixal: Some(PixalOptionsJson {
            resolution: Some(args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1024)),
            camera_fov: Some(49.13),
            structure_seed: Some(56),
            texture_seed: Some(43),
            shape_steps: Some(20),
        }),
        ..Default::default()
    })?;
    params.input_bytes = std::fs::read(&args[2])?;
    let repeats: usize = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(1);
    if !(1..=100).contains(&repeats) {
        return Err("REPEATS must be 1..100".into());
    }
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        for trial in 0..repeats {
            let started = std::time::Instant::now();
            let mut progress = |stage: &str, value: f64| {
                eprintln!("{:.3}s {value:.3} {stage}", started.elapsed().as_secs_f64())
            };
            let artifacts = backend.generate(&params, &mut progress, &cancel)?;
            let elapsed = started.elapsed().as_secs_f64();
            let artifact = artifacts.first().ok_or("no Pixal3D output")?;
            let path = if repeats == 1 {
                args[3].clone()
            } else {
                format!("{}.{trial}.glb", args[3])
            };
            std::fs::write(path, &artifact.bytes)?;
            eprintln!(
                "native Pixal3D trial {trial}: {elapsed:.3}s, {} GLB bytes",
                artifact.bytes.len()
            );
        }
        Ok(())
    })();
    backend.unload()?;
    result
}

#[cfg(not(feature = "mesh"))]
fn main() {
    eprintln!("build with --features mesh");
    std::process::exit(1);
}
