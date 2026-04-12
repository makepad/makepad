use makepad_diffusion::comfy::FluxWorkflow;
use makepad_diffusion::flux::{ComfyModelRoots, FluxPromptToImagePlan};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {}", err);
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_default();
    let workflow_path = match args.next() {
        Some(path) => path,
        None => {
            eprintln!(
                "usage: {} <workflow.json> <comfyui-root>",
                std::path::Path::new(&program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("flux-smoke")
            );
            std::process::exit(2);
        }
    };
    let comfy_root = match args.next() {
        Some(path) => path,
        None => {
            eprintln!(
                "usage: {} <workflow.json> <comfyui-root>",
                std::path::Path::new(&program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("flux-smoke")
            );
            std::process::exit(2);
        }
    };

    let workflow = FluxWorkflow::from_file(&workflow_path)?;
    let roots = ComfyModelRoots::new(&comfy_root);
    let plan = FluxPromptToImagePlan::from_workflow(&workflow, &roots)?;
    let headers = plan.bundle.inspect_headers()?;
    let inspection = headers.inspect_bundle()?;

    println!("workflow: {}", plan.workflow_path.display());
    println!("kind: {:?}", plan.kind);
    println!(
        "image: {}x{} batch={} seed={}",
        plan.generation.width,
        plan.generation.height,
        plan.generation.batch_size,
        plan.generation.seed
    );
    println!(
        "sampling: sampler={} scheduler={} steps={} cfg={} denoise={} guidance={}",
        plan.generation.sampler_name,
        plan.generation.scheduler,
        plan.generation.steps,
        plan.generation.cfg,
        plan.generation.denoise,
        plan.generation.guidance
    );
    println!("prompt.clip_l: {}", plan.prompts.clip_l);
    println!("prompt.t5xxl: {}", plan.prompts.t5xxl);
    println!("prompt.negative: {}", plan.prompts.negative);
    println!(
        "latents: {}x{}x{} packed={}x{} tokens={}",
        plan.latent_shape.latent_width,
        plan.latent_shape.latent_height,
        plan.latent_shape.latent_channels,
        plan.latent_shape.packed_width,
        plan.latent_shape.packed_height,
        plan.latent_shape.image_token_count
    );
    println!(
        "transformer: hidden={} heads={} head_dim={} depth={} single_depth={} axes={:?}",
        plan.transformer.hidden_size,
        plan.transformer.num_heads,
        plan.transformer.head_dim(),
        plan.transformer.depth,
        plan.transformer.depth_single_blocks,
        plan.transformer.axes_dim
    );
    println!(
        "transformer.header: style={:?} hidden={} context={} in={} out={} vec={} guidance={} depth={} single_depth={} heads={}",
        inspection.transformer.tensor_name_style,
        inspection.transformer.config.hidden_size,
        inspection.transformer.config.context_in_dim,
        inspection.transformer.config.in_channels,
        inspection.transformer.config.out_channels,
        inspection.transformer.config.vec_in_dim,
        inspection.transformer.config.guidance_embed,
        inspection.transformer.config.depth,
        inspection.transformer.config.depth_single_blocks,
        inspection.transformer.config.num_heads
    );
    println!("diffusion: {}", plan.bundle.diffusion_model_path.display());
    println!(
        "diffusion tensors: {}",
        headers.diffusion_model.tensors.len()
    );
    if let Some(path) = &plan.bundle.vae_path {
        println!("vae: {}", path.display());
    }
    if let Some(header) = &headers.vae {
        println!("vae tensors: {}", header.tensors.len());
    }
    if let Some(path) = &plan.bundle.clip_l_path {
        println!("clip_l: {}", path.display());
    }
    if let Some(header) = &headers.clip_l {
        println!("clip_l tensors: {}", header.tensors.len());
    }
    if let Some(config) = &inspection.clip_l {
        println!(
            "clip_l config: vocab={} hidden={} layers={} positions={} mlp={}",
            config.vocab_size,
            config.hidden_size,
            config.layer_count,
            config.max_position_embeddings,
            config.intermediate_size
        );
    }
    if let Some(path) = &plan.bundle.t5xxl_path {
        println!("t5xxl: {}", path.display());
    }
    if let Some(header) = &headers.t5xxl {
        println!("t5xxl tensors: {}", header.tensors.len());
    }
    if let Some(config) = &inspection.t5xxl {
        println!(
            "t5xxl config: vocab={} d_model={} layers={} ffn={}",
            config.vocab_size,
            config.model_dim,
            config.layer_count,
            config.feedforward_dim
        );
    }

    Ok(())
}
