//! The `flux2` backend: FLUX.2-klein-4B instruction+reference image edit
//! through libs/diffusion. CUDA-only; fail closed on Metal/CPU.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use makepad_diffusion::backend::gpu_device_available;
use makepad_diffusion::flux2_pipeline::{
    flux2_klein_paths_from_root, Flux2EditRequest, Flux2KleinPipeline,
};
use makepad_diffusion::flux2_vae::flux2_image_from_rgb_u8;
use makepad_zune_core::options::DecoderOptions;
use makepad_zune_png::PngDecoder;
use std::io::BufReader;
use std::path::PathBuf;

pub fn flux2_cuda_provisioned() -> bool {
    gpu_device_available()
}

pub struct Flux2Backend {
    model_id: String,
    cache_dir: Option<PathBuf>,
    pipeline: Option<Flux2KleinPipeline>,
}

impl Flux2Backend {
    pub fn new(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            cache_dir: None,
            pipeline: None,
        }
    }
}

impl ContentBackend for Flux2Backend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn is_resident(&self) -> bool {
        self.pipeline.is_some()
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        self.pipeline = None;
        Ok(())
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        if !gpu_device_available() {
            return Err(AssetAiError::Unavailable(
                "flux2-klein-4b requires CUDA; refusing CPU/Metal fallback".into(),
            ));
        }
        ctx.ensure_files().map_err(|err| {
            AssetAiError::Backend(format!("flux2-klein-4b download: {err}"))
        })?;
        let root = ctx.cache_dir.join("flux2-klein-4b");
        if self.pipeline.is_some() && self.cache_dir.as_ref() == Some(&root) {
            return Ok(());
        }
        let paths = flux2_klein_paths_from_root(&root).map_err(|err| {
            AssetAiError::Backend(format!("flux2-klein-4b paths: {err}"))
        })?;
        let pipeline = Flux2KleinPipeline::load(paths)
            .map_err(|err| AssetAiError::Backend(format!("flux2-klein-4b load: {err}")))?;
        self.cache_dir = Some(root);
        self.pipeline = Some(pipeline);
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if !gpu_device_available() {
            return Err(AssetAiError::Unavailable(
                "flux2-klein-4b requires CUDA; refusing CPU/Metal fallback".into(),
            ));
        }
        cancel.check()?;
        progress("load", 0.05);
        let pipe = self
            .pipeline
            .as_mut()
            .ok_or_else(|| AssetAiError::Backend("flux2-klein-4b not loaded".into()))?;
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(
                "flux2-klein-4b edit requires input_b64 (reference PNG)".into(),
            ));
        }
        let (rgb, width, height) = decode_png_rgb(&params.input_bytes)?;
        let reference = flux2_image_from_rgb_u8(&rgb, width, height)
            .map_err(|err| AssetAiError::Backend(format!("flux2 ref: {err}")))?;
        let out_w = params.width.unwrap_or(width as u32).max(16) / 16 * 16;
        let out_h = params.height.unwrap_or(height as u32).max(16) / 16 * 16;
        cancel.check()?;
        progress("edit", 0.15);
        let request = Flux2EditRequest {
            prompt: params.prompt.clone(),
            width: out_w,
            height: out_h,
            steps: params.steps.unwrap_or(4) as usize,
            seed: params.seed,
            references: vec![reference],
            noise: None,
            teacher_ref_tokens: None,
            teacher_embeds: None,
        };
        let result = pipe
            .edit(&request)
            .map_err(|err| AssetAiError::Backend(format!("flux2 edit: {err}")))?;
        cancel.check()?;
        progress("png", 0.95);
        Ok(vec![ArtifactData {
            content_type: "image/png",
            ext: "png",
            bytes: result.png,
        }])
    }
}

fn decode_png_rgb(bytes: &[u8]) -> Result<(Vec<u8>, usize, usize), AssetAiError> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let reader = BufReader::new(cursor);
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(reader, options);
    decoder
        .decode_headers()
        .map_err(|err| AssetAiError::Params(format!("flux2 png: {err:?}")))?;
    let info = decoder
        .info()
        .cloned()
        .ok_or_else(|| AssetAiError::Params("flux2 png: no info".into()))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| AssetAiError::Params("flux2 png: no colorspace".into()))?;
    let pixels = decoder
        .decode_raw()
        .map_err(|err| AssetAiError::Params(format!("flux2 png: {err:?}")))?;
    let components = colorspace.num_components();
    let (w, h) = (info.width as usize, info.height as usize);
    if components < 3 {
        return Err(AssetAiError::Params(format!(
            "flux2 png: need rgb, got {components} channels"
        )));
    }
    let mut rgb = vec![0u8; w * h * 3];
    for (i, chunk) in pixels.chunks_exact(components).enumerate() {
        rgb[i * 3..i * 3 + 3].copy_from_slice(&chunk[..3]);
    }
    Ok((rgb, w, h))
}
