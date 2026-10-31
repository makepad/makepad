//! Native Depth-Anything (V2 family) relative depth: DINOv2 ViT + DPT head.
//!
//! The same network shape as DA3METRIC-LARGE (`da3.rs`), at any ViT size,
//! with the plain DPT head of Depth-Anything-V2: one relative-disparity
//! output (larger = nearer, final ReLU), no sky head. It shares DA3's
//! building blocks; only the sizes differ, and those are read from the
//! checkpoint itself, so every checkpoint of that architecture loads:
//!
//! * Depth-Anything-V2-Small (ViT-S/14, 24.8M, Apache-2.0) — the realtime tier;
//! * Distill-Any-Depth-Small (same shape, Apache-2.0 weights);
//! * Depth-Anything-V2-Base / -Large (CC-BY-NC-4.0) — slower, better.
//!
//! Weights come from the original `.pth` (`depth_anything_v2_vits.pth`) or an
//! f32 safetensors export with the original tensor names (any prefix such as
//! `module.` is detected). The transformers `-hf` layout is not supported.
//!
//! The speed / accuracy knob is the input size: any multiple of 14 (the
//! models train at 518). Same-shape requests replay a captured CUDA graph,
//! which matters most for the small model, whose cost is launch-bound.

use crate::backend::{
    gpu_add, gpu_attention_packed_f32, gpu_attention_packed_flash_bf16,
    gpu_birefnet_image_to_patches, gpu_birefnet_tokens_to_planar, gpu_concat_cols,
    gpu_concat_rows, gpu_download, gpu_gelu_erf, gpu_graph_capture, gpu_graph_launch,
    gpu_layer_norm_mod, gpu_linear_f32_resident, gpu_slice_cols, gpu_slice_rows,
    gpu_upload_into, GpuStepGraph, GpuTensor,
};
use crate::da3::{
    f32_to_bf16_bytes, interpolate_position_embedding, norm_mods, relu, resize, stride_two,
    tensor, upload, CacheScope, Conv2d, ConvTransposeNoOverlap, Da3Precision, Da3WeightFile,
    DinoLayer, DinoLayerBf16, DinoLinears, FusionBlock, Planar, StrideCache, TensorSource,
    DA3_BASE_PATCH_SIDE, DA3_NORM_EPS, DA3_PATCH,
};
use crate::depth_anything_temporal::{FrameCache, MotionModule, TemporalStream};
use crate::torch_pth::PthStateDict;
use crate::{DiffusionError, Result};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

const CACHE_NAMESPACE: &str = "depth-anything";
const HEAD_DIM: usize = 64;

/// Network sizes, read from the checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepthAnythingConfig {
    pub hidden: usize,
    pub depth: usize,
    pub heads: usize,
    pub mlp: usize,
    /// Blocks whose (normed) output feeds the DPT head.
    pub taps: [usize; 4],
    /// DPT `projects.N` widths.
    pub out_channels: [usize; 4],
    /// DPT fusion width.
    pub features: usize,
}

impl DepthAnythingConfig {
    /// "ViT-S" / "ViT-B" / "ViT-L" / "ViT-G", from the width.
    pub fn variant(&self) -> &'static str {
        match self.hidden {
            384 => "ViT-S",
            768 => "ViT-B",
            1024 => "ViT-L",
            1536 => "ViT-G",
            _ => "ViT-?",
        }
    }
}

/// Relative disparity (larger = nearer) at the input resolution.
#[derive(Clone, Debug)]
pub struct DepthAnythingPrediction {
    pub disparity: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

/// A torch `.pth` state dict behind the shared tensor-source interface.
struct PthTensors(RefCell<PthStateDict>);

impl TensorSource for PthTensors {
    fn tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        self.0
            .borrow_mut()
            .f32(name)
            .map_err(|err| DiffusionError::model(format!("depth-anything tensor {name:?}: {err}")))
    }

    fn tensor_names(&self) -> Vec<String> {
        self.0.borrow().names().cloned().collect()
    }
}

pub struct DepthAnything {
    config: DepthAnythingConfig,
    patch_w: GpuTensor,
    patch_b: GpuTensor,
    cls: GpuTensor,
    base_pos: Vec<f32>,
    layers: Vec<DinoLayer>,
    final_norm: GpuTensor,
    projects: Vec<Conv2d>,
    resize0: ConvTransposeNoOverlap,
    resize1: ConvTransposeNoOverlap,
    resize3: Conv2d,
    scratch_layers: Vec<Conv2d>,
    fusion: Vec<FusionBlock>, // index 0 == refinenet1
    output_conv1: Conv2d,
    out_conv: Conv2d,
    out_final: Conv2d,
    pos_cache: Mutex<Option<((usize, usize), GpuTensor)>>,
    stride_cache: StrideCache,
    precision: Da3Precision,
    /// Process-unique id keying the captured-graph cache (see da3.rs).
    generation: u64,
    /// Video-Depth-Anything: motion modules after layer_3, layer_4,
    /// refinenet4 and refinenet3, plus the streaming window. Empty for the
    /// per-frame models.
    motion: Vec<MotionModule>,
    stream: Mutex<TemporalStream>,
}

impl DepthAnything {
    /// Load with bf16 transformer linears (f32 accumulation).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_precision(path, Da3Precision::FullBf16)
    }

    pub fn load_with_precision(path: impl AsRef<Path>, precision: Da3Precision) -> Result<Self> {
        let path = path.as_ref();
        let is_pth = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pth") || ext.eq_ignore_ascii_case("pt"));
        // Per-checkpoint cache keys: two checkpoints share tensor names.
        let scope = CacheScope {
            namespace: CACHE_NAMESPACE,
            key_prefix: format!("{}:", path.display()),
        };
        if is_pth {
            let dict = PthStateDict::load(path)
                .map_err(|err| DiffusionError::model(format!("{}: {err}", path.display())))?;
            Self::prepare(&PthTensors(RefCell::new(dict)), &scope, precision)
        } else {
            Self::prepare(&Da3WeightFile::load(path)?, &scope, precision)
        }
    }

    /// Video-Depth-Anything: consecutive frames share a temporal window
    /// (call [`Self::reset_stream`] on a cut or a seek).
    pub fn is_temporal(&self) -> bool {
        !self.motion.is_empty()
    }

    /// Forget the temporal window: the next frame starts a new stream.
    pub fn reset_stream(&self) {
        if let Ok(mut stream) = self.stream.lock() {
            stream.reset();
        }
    }

    pub fn config(&self) -> &DepthAnythingConfig {
        &self.config
    }

    pub fn precision(&self) -> Da3Precision {
        self.precision
    }

    fn prepare(
        weights: &dyn TensorSource,
        scope: &CacheScope,
        precision: Da3Precision,
    ) -> Result<Self> {
        let names: HashSet<String> = weights.tensor_names().into_iter().collect();
        let find_prefix = |suffix: &str| {
            names
                .iter()
                .filter_map(|name| name.strip_suffix(suffix))
                .min_by_key(|prefix| prefix.len())
                .map(str::to_string)
                .ok_or_else(|| {
                    DiffusionError::model(format!(
                        "depth-anything: no `*{suffix}` tensor (not a Depth-Anything DINOv2 + DPT checkpoint?)"
                    ))
                })
        };
        // e.g. "pretrained." and "depth_head." in the original checkpoints.
        let bb = find_prefix("cls_token")?;
        let hd = find_prefix("projects.0.weight")?;
        let len_of = |name: &str| weights.tensor_f32(name).map(|values| values.len());

        let hidden = len_of(&format!("{bb}cls_token"))?;
        let mut depth = 0;
        while names.contains(&format!("{bb}blocks.{depth}.norm1.weight")) {
            depth += 1;
        }
        if hidden % HEAD_DIM != 0 || depth == 0 {
            return Err(DiffusionError::model(format!(
                "depth-anything: unsupported backbone (width {hidden}, {depth} blocks)"
            )));
        }
        let taps = match depth {
            12 => [2, 5, 8, 11],
            24 => [4, 11, 17, 23],
            40 => [9, 19, 29, 39],
            other => {
                return Err(DiffusionError::model(format!(
                    "depth-anything: no DPT tap layout for a {other}-block backbone"
                )))
            }
        };
        let mut out_channels = [0usize; 4];
        for (index, channels) in out_channels.iter_mut().enumerate() {
            *channels = len_of(&format!("{hd}projects.{index}.bias"))?;
        }
        let config = DepthAnythingConfig {
            hidden,
            depth,
            heads: hidden / HEAD_DIM,
            mlp: len_of(&format!("{bb}blocks.0.mlp.fc1.bias"))?,
            taps,
            out_channels,
            features: len_of(&format!("{hd}scratch.refinenet1.out_conv.bias"))?,
        };
        let (hidden, mlp, features) = (config.hidden, config.mlp, config.features);

        let patch_dim = 3 * DA3_PATCH * DA3_PATCH;
        let patch_w = tensor(weights, &format!("{bb}patch_embed.proj.weight"), hidden * patch_dim)?;
        let patch_b = tensor(weights, &format!("{bb}patch_embed.proj.bias"), hidden)?;
        let cls = tensor(weights, &format!("{bb}cls_token"), hidden)?;
        let base_pos = tensor(
            weights,
            &format!("{bb}pos_embed"),
            (1 + DA3_BASE_PATCH_SIDE * DA3_BASE_PATCH_SIDE) * hidden,
        )?;

        let mut layers = Vec::with_capacity(depth);
        for index in 0..depth {
            let prefix = format!("{bb}blocks.{index}");
            let key = scope.key(&prefix);
            // LayerScale folds into the output projections.
            let scale1 = tensor(weights, &format!("{prefix}.ls1.gamma"), hidden)?;
            let scale2 = tensor(weights, &format!("{prefix}.ls2.gamma"), hidden)?;
            let mut proj_w = tensor(weights, &format!("{prefix}.attn.proj.weight"), hidden * hidden)?;
            let mut proj_b = tensor(weights, &format!("{prefix}.attn.proj.bias"), hidden)?;
            let mut fc2_w = tensor(weights, &format!("{prefix}.mlp.fc2.weight"), hidden * mlp)?;
            let mut fc2_b = tensor(weights, &format!("{prefix}.mlp.fc2.bias"), hidden)?;
            for row in 0..hidden {
                for value in &mut proj_w[row * hidden..(row + 1) * hidden] {
                    *value *= scale1[row];
                }
                proj_b[row] *= scale1[row];
                for value in &mut fc2_w[row * mlp..(row + 1) * mlp] {
                    *value *= scale2[row];
                }
                fc2_b[row] *= scale2[row];
            }
            let qkv_w = tensor(weights, &format!("{prefix}.attn.qkv.weight"), 3 * hidden * hidden)?;
            let qkv_b = tensor(weights, &format!("{prefix}.attn.qkv.bias"), 3 * hidden)?;
            let fc1_w = tensor(weights, &format!("{prefix}.mlp.fc1.weight"), mlp * hidden)?;
            let fc1_b = tensor(weights, &format!("{prefix}.mlp.fc1.bias"), mlp)?;
            let linears = match precision {
                Da3Precision::StrictF32 => DinoLinears::F32 {
                    qkv_w: upload(&qkv_w, 3 * hidden, hidden)?,
                    qkv_b: upload(&qkv_b, 1, 3 * hidden)?,
                    proj_w: upload(&proj_w, hidden, hidden)?,
                    proj_b: upload(&proj_b, 1, hidden)?,
                    fc1_w: upload(&fc1_w, mlp, hidden)?,
                    fc1_b: upload(&fc1_b, 1, mlp)?,
                    fc2_w: upload(&fc2_w, hidden, mlp)?,
                    fc2_b: upload(&fc2_b, 1, hidden)?,
                },
                Da3Precision::FullBf16 => DinoLinears::Bf16(DinoLayerBf16 {
                    qkv: f32_to_bf16_bytes(&qkv_w),
                    qkv_key: format!("{key}.attn.qkv:bf16"),
                    qkv_bias: qkv_b,
                    proj: f32_to_bf16_bytes(&proj_w),
                    proj_key: format!("{key}.attn.proj:bf16"),
                    proj_bias: proj_b,
                    fc1: f32_to_bf16_bytes(&fc1_w),
                    fc1_key: format!("{key}.mlp.fc1:bf16"),
                    fc1_bias: fc1_b,
                    fc2: f32_to_bf16_bytes(&fc2_w),
                    fc2_key: format!("{key}.mlp.fc2:bf16"),
                    fc2_bias: fc2_b,
                }),
            };
            layers.push(DinoLayer {
                norm1: norm_mods(weights, &format!("{prefix}.norm1"), hidden)?,
                norm2: norm_mods(weights, &format!("{prefix}.norm2"), hidden)?,
                linears,
                hidden,
                mlp,
                namespace: scope.namespace,
            });
        }

        let oc = config.out_channels;
        let mut projects = Vec::with_capacity(4);
        let mut scratch_layers = Vec::with_capacity(4);
        for index in 0..4 {
            projects.push(Conv2d::load_in(
                scope,
                weights,
                &format!("{hd}projects.{index}"),
                hidden,
                oc[index],
                1,
                1,
                true,
            )?);
            scratch_layers.push(Conv2d::load_in(
                scope,
                weights,
                &format!("{hd}scratch.layer{}_rn", index + 1),
                oc[index],
                features,
                3,
                3,
                false,
            )?);
        }
        let mut fusion = Vec::with_capacity(4);
        for index in 1..=4 {
            fusion.push(FusionBlock::load_in(
                scope,
                weights,
                &format!("{hd}scratch.refinenet{index}"),
                features,
                index != 4,
            )?);
        }
        // Video-Depth-Anything carries a temporal head.
        let mut motion = Vec::new();
        if names.contains(&format!("{hd}motion_modules.0.temporal_transformer.proj_in.weight")) {
            for (index, channels) in [oc[2], oc[3], features, features].into_iter().enumerate() {
                motion.push(MotionModule::load(
                    scope,
                    weights,
                    &format!("{hd}motion_modules.{index}"),
                    channels,
                )?);
            }
        }
        let neck = features / 2;
        let result = Self {
            patch_w: upload(&patch_w, hidden, patch_dim)?,
            patch_b: upload(&patch_b, 1, hidden)?,
            cls: upload(&cls, 1, hidden)?,
            base_pos,
            layers,
            final_norm: norm_mods(weights, &format!("{bb}norm"), hidden)?,
            projects,
            resize0: ConvTransposeNoOverlap::load_in(
                scope,
                weights,
                &format!("{hd}resize_layers.0"),
                oc[0],
                4,
            )?,
            resize1: ConvTransposeNoOverlap::load_in(
                scope,
                weights,
                &format!("{hd}resize_layers.1"),
                oc[1],
                2,
            )?,
            resize3: Conv2d::load_in(
                scope,
                weights,
                &format!("{hd}resize_layers.3"),
                oc[3],
                oc[3],
                3,
                3,
                true,
            )?,
            scratch_layers,
            fusion,
            output_conv1: Conv2d::load_in(
                scope,
                weights,
                &format!("{hd}scratch.output_conv1"),
                features,
                neck,
                3,
                3,
                true,
            )?,
            out_conv: Conv2d::load_in(
                scope,
                weights,
                &format!("{hd}scratch.output_conv2.0"),
                neck,
                32,
                3,
                3,
                true,
            )?,
            out_final: Conv2d::load_in(
                scope,
                weights,
                &format!("{hd}scratch.output_conv2.2"),
                32,
                1,
                1,
                1,
                true,
            )?,
            config,
            motion,
            stream: Mutex::new(TemporalStream::default()),
            pos_cache: Mutex::new(None),
            stride_cache: Mutex::new(None),
            precision,
            generation: {
                static NEXT_GENERATION: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
        };
        Ok(result)
    }

    /// ImageNet-normalized planar RGB, both sides multiples of 14.
    pub fn forward_normalized(
        &self,
        pixels: &[f32],
        width: usize,
        height: usize,
    ) -> Result<DepthAnythingPrediction> {
        if width == 0
            || height == 0
            || width % DA3_PATCH != 0
            || height % DA3_PATCH != 0
            || pixels.len() != 3 * width * height
        {
            return Err(DiffusionError::workflow(
                "depth-anything normalized input shape mismatch",
            ));
        }
        if self.is_temporal() {
            // The window changes every frame: eager, no captured graph.
            let mut stream = self
                .stream
                .lock()
                .map_err(|_| DiffusionError::workflow("depth-anything stream poisoned"))?;
            let window = stream.window((width, height));
            let image = upload(pixels, 3, width * height)?;
            let mut cache = Vec::with_capacity(8);
            let out = self.forward_device(&image, width, height, Some((&window, &mut cache)))?;
            let output = gpu_download(&out.tensor).map_err(DiffusionError::model)?;
            stream.push(cache);
            return Ok(DepthAnythingPrediction {
                disparity: output,
                width,
                height,
            });
        }
        let output = match self.forward_graph(pixels, width, height)? {
            Some(output) => output,
            None => {
                let image = upload(pixels, 3, width * height)?;
                let out = self.forward_device(&image, width, height, None)?;
                gpu_download(&out.tensor).map_err(DiffusionError::model)?
            }
        };
        Ok(DepthAnythingPrediction {
            disparity: output,
            width,
            height,
        })
    }

    /// Two eager warm runs, then a captured graph replayed per same-shape
    /// frame (da3.rs pattern). Ok(None): run eager instead.
    fn forward_graph(&self, pixels: &[f32], width: usize, height: usize) -> Result<Option<Vec<f32>>> {
        struct GraphState {
            model: u64,
            shape: (usize, usize),
            image: GpuTensor,
            warm_runs: u32,
            capture_failed: bool,
            graph: Option<(GpuStepGraph, GpuTensor)>,
        }
        thread_local! {
            static GRAPH: RefCell<Option<GraphState>> = const { RefCell::new(None) };
        }
        GRAPH.with(|cell| {
            let mut slot = cell.borrow_mut();
            let matches = slot
                .as_ref()
                .is_some_and(|state| state.model == self.generation && state.shape == (width, height));
            if matches {
                let state = slot.as_ref().expect("matching depth-anything graph state");
                gpu_upload_into(&state.image, pixels).map_err(DiffusionError::model)?;
            } else {
                *slot = None;
                *slot = Some(GraphState {
                    model: self.generation,
                    shape: (width, height),
                    image: upload(pixels, 3, width * height)?,
                    warm_runs: 0,
                    capture_failed: false,
                    graph: None,
                });
            }
            let state = slot.as_mut().expect("depth-anything graph state");
            if let Some((graph, out)) = &state.graph {
                gpu_graph_launch(graph).map_err(DiffusionError::model)?;
                return Ok(Some(gpu_download(out).map_err(DiffusionError::model)?));
            }
            if state.capture_failed {
                return Ok(None);
            }
            if state.warm_runs < 2 {
                state.warm_runs += 1;
                let out = self.forward_device(&state.image, width, height, None)?;
                return Ok(Some(gpu_download(&out.tensor).map_err(DiffusionError::model)?));
            }
            let captured = gpu_graph_capture(|| {
                self.forward_device(&state.image, width, height, None)
                    .map_err(|err| err.to_string())
            });
            match captured {
                Ok((graph, out)) => {
                    gpu_graph_launch(&graph).map_err(DiffusionError::model)?;
                    let values = gpu_download(&out.tensor).map_err(DiffusionError::model)?;
                    state.graph = Some((graph, out.tensor));
                    Ok(Some(values))
                }
                Err(err) => {
                    eprintln!("depth-anything graph capture failed ({err}); running eager");
                    state.capture_failed = true;
                    Ok(None)
                }
            }
        })
    }

    /// Device-resident forward: normalized planar image -> disparity plane.
    /// `temporal`: the window frames' caches, and where this frame's go.
    fn forward_device(
        &self,
        image: &GpuTensor,
        width: usize,
        height: usize,
        mut temporal: Option<(&[std::rc::Rc<FrameCache>], &mut FrameCache)>,
    ) -> Result<Planar> {
        let cfg = &self.config;
        let patch_w = width / DA3_PATCH;
        let patch_h = height / DA3_PATCH;
        let patch_count = patch_w * patch_h;
        let tiles = gpu_birefnet_image_to_patches(image, width, height, DA3_PATCH, DA3_PATCH)
            .map_err(DiffusionError::model)?;
        let red = gpu_slice_rows(&tiles, 0, patch_count).map_err(DiffusionError::model)?;
        let green =
            gpu_slice_rows(&tiles, patch_count, patch_count).map_err(DiffusionError::model)?;
        let blue =
            gpu_slice_rows(&tiles, 2 * patch_count, patch_count).map_err(DiffusionError::model)?;
        let patch_input =
            gpu_concat_cols(&[&red, &green, &blue]).map_err(DiffusionError::model)?;
        let patches = gpu_linear_f32_resident(&patch_input, &self.patch_w, Some(&self.patch_b))
            .map_err(DiffusionError::model)?;
        let hidden = gpu_concat_rows(&self.cls, &patches).map_err(DiffusionError::model)?;
        let mut pos_cache = self
            .pos_cache
            .lock()
            .map_err(|_| DiffusionError::workflow("depth-anything position cache poisoned"))?;
        if pos_cache.as_ref().map(|(shape, _)| *shape) != Some((patch_w, patch_h)) {
            let pos = interpolate_position_embedding(&self.base_pos, patch_w, patch_h, cfg.hidden)?;
            *pos_cache = Some(((patch_w, patch_h), upload(&pos, 1 + patch_count, cfg.hidden)?));
        }
        let pos = &pos_cache.as_ref().expect("position cache filled").1;
        let mut hidden = gpu_add(&hidden, pos).map_err(DiffusionError::model)?;
        drop(pos_cache);

        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let mut features = Vec::with_capacity(4);
        for (index, layer) in self.layers.iter().enumerate() {
            let normed = gpu_layer_norm_mod(&hidden, &layer.norm1, 0, cfg.hidden, DA3_NORM_EPS)
                .map_err(DiffusionError::model)?;
            let qkv = layer.qkv(&normed)?;
            let q = gpu_slice_cols(&qkv, 0, cfg.hidden).map_err(DiffusionError::model)?;
            let k = gpu_slice_cols(&qkv, cfg.hidden, cfg.hidden).map_err(DiffusionError::model)?;
            let v =
                gpu_slice_cols(&qkv, 2 * cfg.hidden, cfg.hidden).map_err(DiffusionError::model)?;
            let attention = match self.precision {
                Da3Precision::FullBf16 => {
                    gpu_attention_packed_flash_bf16(&q, &k, &v, cfg.heads, scale)
                }
                Da3Precision::StrictF32 => gpu_attention_packed_f32(&q, &k, &v, cfg.heads, scale),
            }
            .map_err(DiffusionError::model)?;
            let update = layer.proj(&attention)?;
            hidden = gpu_add(&hidden, &update).map_err(DiffusionError::model)?;

            let normed = gpu_layer_norm_mod(&hidden, &layer.norm2, 0, cfg.hidden, DA3_NORM_EPS)
                .map_err(DiffusionError::model)?;
            let ff = layer.fc1(&normed)?;
            let ff = gpu_gelu_erf(&ff).map_err(DiffusionError::model)?;
            let ff = layer.fc2(&ff)?;
            hidden = gpu_add(&hidden, &ff).map_err(DiffusionError::model)?;

            if cfg.taps.contains(&index) {
                let normalized =
                    gpu_layer_norm_mod(&hidden, &self.final_norm, 0, cfg.hidden, DA3_NORM_EPS)
                        .map_err(DiffusionError::model)?;
                // Class token dropped: the DPT head takes patch rows only.
                features.push(
                    gpu_slice_rows(&normalized, 1, patch_count).map_err(DiffusionError::model)?,
                );
            }
        }
        if features.len() != 4 {
            return Err(DiffusionError::workflow("depth-anything missing DINO feature taps"));
        }

        let mut resized = Vec::with_capacity(4);
        for (index, feature) in features.into_iter().enumerate() {
            let planar = Planar {
                tensor: gpu_birefnet_tokens_to_planar(&feature).map_err(DiffusionError::model)?,
                width: patch_w,
                height: patch_h,
            };
            let projected = self.projects[index].forward(&planar)?;
            resized.push(match index {
                0 => self.resize0.forward(projected)?,
                1 => self.resize1.forward(projected)?,
                2 => projected,
                _ => stride_two(&self.stride_cache, self.resize3.forward(&projected)?)?,
            });
        }

        // One motion module: window caches for its two blocks, new entries out.
        let mut temporal_step = |module: usize, x: Planar| -> Result<Planar> {
            let Some((window, cache)) = temporal.as_mut() else {
                return Ok(x);
            };
            let block0: Vec<&GpuTensor> = window.iter().map(|f| &f[module * 2]).collect();
            let block1: Vec<&GpuTensor> = window.iter().map(|f| &f[module * 2 + 1]).collect();
            let (out, entries) = self.motion[module].forward(x, [&block0, &block1])?;
            cache.extend(entries);
            Ok(out)
        };
        if !self.motion.is_empty() {
            let layer_4 = resized.pop().expect("layer_4");
            let layer_3 = resized.pop().expect("layer_3");
            let layer_3 = temporal_step(0, layer_3)?;
            let layer_4 = temporal_step(1, layer_4)?;
            resized.push(layer_3);
            resized.push(layer_4);
        }

        let mut lateral = Vec::with_capacity(4);
        for (conv, input) in self.scratch_layers.iter().zip(&resized) {
            lateral.push(conv.forward(input)?);
        }
        let mut fused =
            self.fusion[3].forward(lateral.remove(3), None, (lateral[2].width, lateral[2].height))?;
        if !self.motion.is_empty() {
            fused = temporal_step(2, fused)?;
        }
        fused = self.fusion[2].forward(
            fused,
            Some(&lateral[2]),
            (lateral[1].width, lateral[1].height),
        )?;
        if !self.motion.is_empty() {
            fused = temporal_step(3, fused)?;
        }
        fused = self.fusion[1].forward(
            fused,
            Some(&lateral[1]),
            (lateral[0].width, lateral[0].height),
        )?;
        fused = self.fusion[0].forward(
            fused,
            Some(&lateral[0]),
            (lateral[0].width * 2, lateral[0].height * 2),
        )?;

        let neck = self.output_conv1.forward(&fused)?;
        let neck = resize(&neck, width, height, true)?;
        let out = relu(&self.out_conv.forward(&neck)?)?;
        relu(&self.out_final.forward(&out)?)
    }
}
