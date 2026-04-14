use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{ComfyModelRoots, FluxPromptToImagePlan};
use makepad_diffusion::flux_pipeline::{encode_png_rgb, FluxPipelineMetal};
use std::env;
use std::fs;
use std::path::Path;

fn usage() -> ! {
    eprintln!(
        "usage: flux-warm-bench <workflow.json> <comfy-root-or-model-root> [width height steps warmup_runs measured_runs output.png]"
    );
    std::process::exit(1);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workflow_path = env::args().nth(1).unwrap_or_else(|| usage());
    let root = env::args().nth(2).unwrap_or_else(|| usage());
    let width = env::args().nth(3).map(|value| value.parse::<u32>()).transpose()?;
    let height = env::args().nth(4).map(|value| value.parse::<u32>()).transpose()?;
    let steps = env::args().nth(5).map(|value| value.parse::<usize>()).transpose()?;
    let warmup_runs = env::args().nth(6).map(|value| value.parse::<usize>()).transpose()?;
    let measured_runs = env::args().nth(7).map(|value| value.parse::<usize>()).transpose()?;
    let output_path = env::args().nth(8);

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let width = width.unwrap_or(plan.generation.width);
    let height = height.unwrap_or(width);
    let steps = steps.unwrap_or(plan.generation.steps as usize).max(1);
    let warmup_runs = warmup_runs.unwrap_or(1);
    let measured_runs = measured_runs.unwrap_or(1).max(1);

    let (pipeline, load_timing) = FluxPipelineMetal::load(plan.clone(), Some(width), Some(height))?;
    println!("workflow: {}", workflow.path.display());
    println!("size: {}x{} steps={}", width, height, steps);
    println!("prompt: {}", plan.prompts.t5xxl);
    println!("t5xxl backend: {}", pipeline.t5_backend_name());
    println!("load.runtime_init_ms={:.3}", load_timing.runtime_init_ms);
    println!("load.text_tokenize_ms={:.3}", load_timing.text_tokenize_ms);
    println!("load.text_load_ms={:.3}", load_timing.text_load_ms);
    println!("load.text_compile_ms={:.3}", load_timing.text_compile_ms);
    println!("load.text_execute_ms={:.3}", load_timing.text_execute_ms);
    println!("load.transformer_load_ms={:.3}", load_timing.transformer_load_ms);
    println!("load.transformer_compile_ms={:.3}", load_timing.transformer_compile_ms);
    println!(
        "load.transformer_graph_build_ms={:.3}",
        load_timing.transformer_graph_build_ms
    );
    println!(
        "load.transformer_graph_prepare_ms={:.3}",
        load_timing.transformer_graph_prepare_ms
    );
    println!(
        "load.transformer_session_create_ms={:.3}",
        load_timing.transformer_session_create_ms
    );
    println!("load.vae_load_ms={:.3}", load_timing.vae_load_ms);
    println!("load.vae_compile_ms={:.3}", load_timing.vae_compile_ms);
    println!("load.total_ms={:.3}", load_timing.total_ms);

    let base_seed = pipeline.default_seed();
    for warmup in 0..warmup_runs {
        let seed = base_seed.wrapping_add(warmup as u64);
        let run = pipeline.generate(seed, steps, pipeline.default_guidance())?;
        println!(
            "warmup.run_{}.total_ms={:.3}",
            warmup + 1,
            run.timing.total_ms
        );
    }

    let mut total_ms = 0.0f64;
    let mut total_denoise_ms = 0.0f64;
    let mut total_vae_ms = 0.0f64;
    let mut best_total_ms = f64::INFINITY;
    let mut worst_total_ms = 0.0f64;
    let mut final_image = None;
    for measured in 0..measured_runs {
        let seed = base_seed.wrapping_add((warmup_runs + measured) as u64);
        let run = pipeline.generate(seed, steps, pipeline.default_guidance())?;
        println!(
            "measured.run_{}.total_ms={:.3}",
            measured + 1,
            run.timing.total_ms
        );
        println!(
            "measured.run_{}.denoise_ms={:.3}",
            measured + 1,
            run.timing.denoise_ms
        );
        println!(
            "measured.run_{}.vae_execute_ms={:.3}",
            measured + 1,
            run.timing.vae_execute_ms
        );
        total_ms += run.timing.total_ms;
        total_denoise_ms += run.timing.denoise_ms;
        total_vae_ms += run.timing.vae_execute_ms;
        best_total_ms = best_total_ms.min(run.timing.total_ms);
        worst_total_ms = worst_total_ms.max(run.timing.total_ms);
        final_image = Some(run.image);
    }

    println!(
        "measured.summary.total_ms.mean={:.3}",
        total_ms / measured_runs as f64
    );
    println!(
        "measured.summary.total_ms.best={:.3}",
        best_total_ms
    );
    println!(
        "measured.summary.total_ms.worst={:.3}",
        worst_total_ms
    );
    println!(
        "measured.summary.denoise_ms.mean={:.3}",
        total_denoise_ms / measured_runs as f64
    );
    println!(
        "measured.summary.vae_execute_ms.mean={:.3}",
        total_vae_ms / measured_runs as f64
    );

    if let (Some(output_path), Some(image)) = (output_path.as_ref(), final_image.as_ref()) {
        fs::write(
            Path::new(output_path),
            encode_png_rgb(&image.image, image.width, image.height)?,
        )?;
        println!("output: {}", output_path);
    }

    Ok(())
}
