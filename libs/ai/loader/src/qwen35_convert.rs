//! Offline Qwen3.5 checkpoint conversion for text with MTP, and vision.
//! No inference backend is involved. A reference file can supply an exact
//! storage-type and tokenizer compatibility profile, never weight values.

mod metadata;
mod mix;
mod validate;
mod vision;

pub use mix::{matrix_type, TypeRule};

use crate::formats::gguf::{GgufFile, GgufKeyValue};
use crate::formats::gguf_write::{self, TensorSpec};
use crate::quant::{bf16_to_f32, block_elements, block_size, f16_to_f32, TensorType};
use crate::{MlxDType, MlxSafetensorsHeader, MlxTensorEntry};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Options {
    pub input: PathBuf,
    pub output: PathBuf,
    pub reference: Option<PathBuf>,
    pub threads: usize,
    /// Retain historical provenance only when explicitly requested.
    pub preserve_reference_provenance: bool,
    /// Override text matrix types, or select F16/Q8_0 vision matrices.
    pub matrix_type: Option<TensorType>,
    /// Named mixed-precision profile, followed by explicit overrides.
    pub mix: Option<String>,
    pub rules: Vec<TypeRule>,
    /// Convert the vision tower using an mmproj compatibility reference.
    pub vision: bool,
}

#[derive(Clone, Copy, Debug)]
enum ValueTransform {
    Identity,
    AddOne,
    NegativeExp,
}

#[derive(Clone, Copy, Debug)]
struct HeadOrder {
    axis: usize,
    prefix: usize,
    keys: usize,
    values_per_key: usize,
    dim: usize,
}

impl HeadOrder {
    fn source_index(self, destination: usize) -> usize {
        if destination < self.prefix {
            return destination;
        }
        let index = destination - self.prefix;
        let head = index / self.dim;
        self.prefix
            + ((head % self.keys) * self.values_per_key + head / self.keys) * self.dim
            + index % self.dim
    }
}

#[derive(Clone)]
struct Source {
    shard: usize,
    start: u64,
    entry: MlxTensorEntry,
}

#[derive(Clone, Copy, Debug)]
struct RowSlice {
    rows: usize,
    stride: usize,
    offset: usize,
}

struct Tensor {
    spec: TensorSpec,
    source_name: String,
    source: Source,
    transform: ValueTransform,
    order: Option<HeadOrder>,
    columns: Option<Vec<usize>>,
    row_slice: Option<RowSlice>,
}

impl Tensor {
    fn width(&self) -> usize {
        self.spec.dimensions[0] as usize
    }
    fn rows(&self) -> usize {
        self.spec.dimensions[1..].iter().product::<u64>() as usize
    }
    fn row_bytes(&self) -> usize {
        self.width() / block_elements(self.spec.tensor_type.ggml_type())
            * block_size(self.spec.tensor_type.ggml_type())
    }
}

#[derive(Clone, Copy)]
struct Job {
    tensor: usize,
    row: usize,
    rows: usize,
}

struct Plan {
    shards: Vec<PathBuf>,
    tensors: Vec<Tensor>,
    kv: Vec<GgufKeyValue>,
    main_layers: usize,
}

fn json(path: &Path) -> Result<JsonValue> {
    JsonValue::deserialize_json(&fs::read_to_string(path)?)
        .map_err(|e| format!("{}: {e:?}", path.display()).into())
}

fn field<'a>(j: &'a JsonValue, name: &str) -> Result<&'a JsonValue> {
    j.get(name)
        .ok_or_else(|| format!("missing config field {name}").into())
}

fn number(j: &JsonValue, name: &str) -> Result<usize> {
    let n = field(j, name)?
        .as_u64()
        .ok_or_else(|| format!("{name} must be an unsigned integer"))?;
    Ok(usize::try_from(n)?)
}

fn checkpoint(dir: &Path) -> Result<(Vec<PathBuf>, BTreeMap<String, Source>)> {
    let index_path = dir.join("model.safetensors.index.json");
    let mut indexed = BTreeMap::new();
    let mut names = BTreeSet::new();
    if index_path.exists() {
        let index = json(&index_path)?;
        for (name, value) in field(&index, "weight_map")?
            .as_object()
            .ok_or("weight_map is not an object")?
        {
            let shard = value.as_str().ok_or("shard name is not a string")?;
            if Path::new(shard).components().count() != 1 || !shard.ends_with(".safetensors") {
                return Err(format!("invalid shard name {shard}").into());
            }
            names.insert(dir.join(shard));
            indexed.insert(name.clone(), dir.join(shard));
        }
    } else {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "safetensors") {
                names.insert(path);
            }
        }
    }
    if names.is_empty() {
        return Err("no safetensors shards found".into());
    }
    let shards: Vec<_> = names.into_iter().collect();
    let mut sources = BTreeMap::new();
    for (shard, path) in shards.iter().enumerate() {
        let header = MlxSafetensorsHeader::load(path)?;
        for (name, entry) in &header.tensors {
            if index_path.exists() && indexed.get(name) != Some(path) {
                return Err(format!("index/shard disagreement for {name}").into());
            }
            if entry.data_offsets[1] < entry.data_offsets[0] {
                return Err(format!("reversed data offsets for {name}").into());
            }
            let source = Source {
                shard,
                start: header.payload_base_offset() + entry.data_offsets[0],
                entry: entry.clone(),
            };
            if sources.insert(name.clone(), source).is_some() {
                return Err(format!("duplicate source tensor {name}").into());
            }
        }
    }
    if index_path.exists() && indexed.len() != sources.len() {
        return Err("index names missing from shards".into());
    }
    Ok((shards, sources))
}

fn preferred_type(
    name: &str,
    dimensions: &[u64],
    layer: usize,
    layers: usize,
    attention_index: usize,
) -> TensorType {
    let use_six = |i: usize, n: usize| i < n / 8 || i >= 7 * n / 8 || ((i - n / 8) % 3 == 2);
    if dimensions.len() == 1 || name.contains("ssm_conv1d") {
        TensorType::F32
    } else if name.contains("ssm_alpha") || name.contains("ssm_beta") {
        TensorType::Q8_0
    } else if name == "token_embd.weight"
        || name == "output.weight"
        || name.contains("attn_qkv")
        || name.contains("ssm_out")
        || (name.contains("ffn_down") && use_six(layer, layers))
        || (name.contains("attn_v.weight") && use_six(attention_index, layers))
    {
        TensorType::Q6K
    } else {
        TensorType::Q5K
    }
}

fn make_plan(options: &Options, reference: Option<&GgufFile>) -> Result<Plan> {
    if options.vision {
        return vision::make_plan(options, reference);
    }
    if options.matrix_type.is_some_and(|ty| {
        !matches!(
            ty,
            TensorType::Q4K | TensorType::Q5K | TensorType::Q6K | TensorType::Q8_0
        )
    }) {
        return Err("text matrix type must be q4_k, q5_k, q6_k or q8_0".into());
    }
    let config = json(&options.input.join("config.json"))?;
    let text = config.get("text_config").unwrap_or(&config);
    if field(text, "model_type")?.as_str() != Some("qwen3_5_text") {
        return Err("expected a dense Qwen3.5 text config".into());
    }
    let main_layers = number(text, "num_hidden_layers")?;
    let tied = text.get("tie_word_embeddings").and_then(JsonValue::as_bool) == Some(true);
    let mut rules = mix::Rules::new(options, main_layers)?;
    let mtp_layers = number(text, "mtp_num_hidden_layers")?;
    if mtp_layers != 1 {
        return Err("this converter requires exactly one MTP decoder layer".into());
    }
    if text
        .get("mtp_use_dedicated_embeddings")
        .and_then(JsonValue::as_bool)
        == Some(true)
    {
        return Err("dedicated MTP embeddings are not supported".into());
    }
    let hidden = number(text, "hidden_size")?;
    let ffn = number(text, "intermediate_size")?;
    let heads = number(text, "num_attention_heads")?;
    let kv_heads = number(text, "num_key_value_heads")?;
    let head_dim = number(text, "head_dim")?;
    let key_heads = number(text, "linear_num_key_heads")?;
    let value_heads = number(text, "linear_num_value_heads")?;
    let key_dim = number(text, "linear_key_head_dim")?;
    let value_dim = number(text, "linear_value_head_dim")?;
    let conv = number(text, "linear_conv_kernel_dim")?;
    let interval = number(text, "full_attention_interval")?;
    if [
        hidden,
        ffn,
        heads,
        kv_heads,
        head_dim,
        key_heads,
        value_heads,
        key_dim,
        value_dim,
        conv,
        interval,
        main_layers,
    ]
    .contains(&0)
        || value_heads % key_heads != 0
        || heads % kv_heads != 0
    {
        return Err("invalid attention dimensions".into());
    }
    if text.get("attn_output_gate").and_then(JsonValue::as_bool) != Some(true) {
        return Err("attention output gate is required".into());
    }
    if let Some(types) = text.get("layer_types").and_then(JsonValue::as_array) {
        if types.len() != main_layers
            || types.iter().enumerate().any(|(i, t)| {
                t.as_str()
                    != Some(if (i + 1) % interval == 0 {
                        "full_attention"
                    } else {
                        "linear_attention"
                    })
            })
        {
            return Err("layer_types disagree with full_attention_interval".into());
        }
    }
    let (shards, mut sources) = checkpoint(&options.input)?;
    let prefix = if sources.contains_key("model.language_model.embed_tokens.weight") {
        "model.language_model."
    } else {
        "model."
    };
    let mut tensors = Vec::new();
    let mut add = |source_name: String,
                   name: String,
                   source_shape: Vec<usize>,
                   shape: Vec<usize>,
                   transform: ValueTransform,
                   order: Option<HeadOrder>,
                   layer: usize|
     -> Result<()> {
        let source = sources
            .remove(&source_name)
            .ok_or_else(|| format!("missing tensor {source_name}"))?;
        let expected: Vec<u64> = source_shape.iter().map(|n| *n as u64).collect();
        if source.entry.shape != expected {
            return Err(format!(
                "{source_name}: shape {:?}, expected {expected:?}",
                source.entry.shape
            )
            .into());
        }
        if !matches!(
            source.entry.dtype,
            MlxDType::BF16 | MlxDType::F16 | MlxDType::F32
        ) {
            return Err(format!("{source_name}: expected BF16/F16/F32").into());
        }
        let dimensions: Vec<u64> = shape.into_iter().rev().map(|d| d as u64).collect();
        let mut ty = preferred_type(&name, &dimensions, layer, main_layers, layer / interval);
        if let Some(reference) = reference {
            if layer < main_layers || !name.starts_with("blk.") {
                let r = reference.require_tensor(&name)?;
                if r.dimensions != dimensions {
                    return Err(format!("reference shape mismatch for {name}").into());
                }
                ty = r.tensor_type;
            } else if !name.contains("nextn.") {
                let analog = name.replacen(
                    &format!("blk.{main_layers}."),
                    &format!("blk.{}.", main_layers - 1),
                    1,
                );
                if let Some(r) = reference.get_tensor(&analog) {
                    ty = r.tensor_type;
                }
            }
        }
        if dimensions.len() == 2 && !name.contains("ssm_conv1d") {
            if let Some(override_type) = options.matrix_type {
                ty = override_type;
            }
            ty = rules.select(&name, layer, main_layers, tied, ty);
        }
        if dimensions[0] % block_elements(ty.ggml_type()) as u64 != 0 {
            return Err(format!("unaligned row width for {name}").into());
        }
        let columns = order.filter(|o| o.axis == 1).map(|o| {
            (0..dimensions[0] as usize)
                .map(|c| o.source_index(c))
                .collect()
        });
        tensors.push(Tensor {
            spec: TensorSpec {
                name,
                dimensions,
                tensor_type: ty,
            },
            source_name,
            source,
            transform,
            order,
            columns,
            row_slice: None,
        });
        Ok(())
    };
    use ValueTransform::{AddOne, Identity, NegativeExp};
    let vocab = number(text, "vocab_size")?;
    add(
        format!("{prefix}embed_tokens.weight"),
        "token_embd.weight".into(),
        vec![vocab, hidden],
        vec![vocab, hidden],
        Identity,
        None,
        0,
    )?;
    add(
        format!("{prefix}norm.weight"),
        "output_norm.weight".into(),
        vec![hidden],
        vec![hidden],
        AddOne,
        None,
        0,
    )?;
    if text.get("tie_word_embeddings").and_then(JsonValue::as_bool) != Some(true) {
        add(
            "lm_head.weight".into(),
            "output.weight".into(),
            vec![vocab, hidden],
            vec![vocab, hidden],
            Identity,
            None,
            0,
        )?;
    }
    for layer in 0..=main_layers {
        let source_prefix = if layer == main_layers {
            "mtp.layers.0.".into()
        } else {
            format!("{prefix}layers.{layer}.")
        };
        let mut layer_add = |suffix: &str, target: &str, shape: Vec<usize>, transform, order| {
            add(
                format!("{source_prefix}{suffix}"),
                format!("blk.{layer}.{target}"),
                shape.clone(),
                shape,
                transform,
                order,
                layer,
            )
        };
        for (source, target) in [
            ("input_layernorm.weight", "attn_norm.weight"),
            (
                "post_attention_layernorm.weight",
                "post_attention_norm.weight",
            ),
        ] {
            layer_add(source, target, vec![hidden], AddOne, None)?;
        }
        for (source, target, shape) in [
            ("gate", "ffn_gate", vec![ffn, hidden]),
            ("up", "ffn_up", vec![ffn, hidden]),
            ("down", "ffn_down", vec![hidden, ffn]),
        ] {
            layer_add(
                &format!("mlp.{source}_proj.weight"),
                &format!("{target}.weight"),
                shape,
                Identity,
                None,
            )?;
        }
        if layer == main_layers || (layer + 1) % interval == 0 {
            for (source, target, shape) in [
                ("q_proj", "attn_q", vec![2 * heads * head_dim, hidden]),
                ("k_proj", "attn_k", vec![kv_heads * head_dim, hidden]),
                ("v_proj", "attn_v", vec![kv_heads * head_dim, hidden]),
                ("o_proj", "attn_output", vec![hidden, heads * head_dim]),
            ] {
                layer_add(
                    &format!("self_attn.{source}.weight"),
                    &format!("{target}.weight"),
                    shape,
                    Identity,
                    None,
                )?;
            }
            for norm in ["q", "k"] {
                layer_add(
                    &format!("self_attn.{norm}_norm.weight"),
                    &format!("attn_{norm}_norm.weight"),
                    vec![head_dim],
                    AddOne,
                    None,
                )?;
            }
        } else {
            let order = |axis, prefix, dim| {
                Some(HeadOrder {
                    axis,
                    prefix,
                    dim,
                    keys: key_heads,
                    values_per_key: value_heads / key_heads,
                })
            };
            let qk = 2 * key_heads * key_dim;
            let inner = value_heads * value_dim;
            for (source, target, shape, permutation) in [
                (
                    "in_proj_qkv",
                    "attn_qkv",
                    vec![qk + inner, hidden],
                    order(0, qk, value_dim),
                ),
                (
                    "in_proj_z",
                    "attn_gate",
                    vec![inner, hidden],
                    order(0, 0, value_dim),
                ),
                (
                    "in_proj_a",
                    "ssm_alpha",
                    vec![value_heads, hidden],
                    order(0, 0, 1),
                ),
                (
                    "in_proj_b",
                    "ssm_beta",
                    vec![value_heads, hidden],
                    order(0, 0, 1),
                ),
                (
                    "out_proj",
                    "ssm_out",
                    vec![hidden, inner],
                    order(1, 0, value_dim),
                ),
            ] {
                layer_add(
                    &format!("linear_attn.{source}.weight"),
                    &format!("{target}.weight"),
                    shape,
                    Identity,
                    permutation,
                )?;
            }
            layer_add(
                "linear_attn.A_log",
                "ssm_a",
                vec![value_heads],
                NegativeExp,
                order(1, 0, 1),
            )?;
            layer_add(
                "linear_attn.dt_bias",
                "ssm_dt.bias",
                vec![value_heads],
                Identity,
                order(1, 0, 1),
            )?;
            layer_add(
                "linear_attn.norm.weight",
                "ssm_norm.weight",
                vec![value_dim],
                Identity,
                None,
            )?;
            add(
                format!("{source_prefix}linear_attn.conv1d.weight"),
                format!("blk.{layer}.ssm_conv1d.weight"),
                vec![qk + inner, 1, conv],
                vec![qk + inner, conv],
                Identity,
                order(0, qk, value_dim),
                layer,
            )?;
        }
    }
    for (source, target, shape, transform) in [
        ("fc", "eh_proj", vec![hidden, 2 * hidden], Identity),
        ("pre_fc_norm_embedding", "enorm", vec![hidden], AddOne),
        ("pre_fc_norm_hidden", "hnorm", vec![hidden], AddOne),
        ("norm", "shared_head_norm", vec![hidden], AddOne),
    ] {
        add(
            format!("mtp.{source}.weight"),
            format!("blk.{main_layers}.nextn.{target}.weight"),
            shape.clone(),
            shape,
            transform,
            None,
            main_layers,
        )?;
    }
    for name in sources.keys() {
        if !name.starts_with("model.visual.") {
            return Err(format!("unmapped checkpoint tensor {name}").into());
        }
    }
    rules.finish()?;
    if let Some(reference) = reference {
        let main_count = tensors
            .iter()
            .filter(|t| !t.spec.name.starts_with(&format!("blk.{main_layers}.")))
            .count();
        if main_count != reference.tensors.len() {
            return Err("reference tensor inventory differs from checkpoint".into());
        }
    }
    tensors.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
    let kv = metadata::build(options, text, reference, mix::file_type(&tensors))?;
    Ok(Plan {
        shards,
        tensors,
        kv,
        main_layers,
    })
}

fn jobs(plan: &Plan) -> Vec<Job> {
    let mut result = Vec::new();
    for (tensor, t) in plan.tensors.iter().enumerate() {
        // Scheduling grain only: every row is processed, including the tail.
        let rows_per_job = (1024 * 1024 / (t.width() * 4)).max(1);
        let mut row = 0;
        while row < t.rows() {
            let rows = rows_per_job.min(t.rows() - row);
            result.push(Job { tensor, row, rows });
            row += rows;
        }
    }
    result
}

fn read_source(files: &mut [File], tensor: &Tensor, job: Job) -> Result<Vec<f32>> {
    let width = tensor.width();
    let element_bytes = tensor.source.entry.dtype.byte_width() as usize;
    let row_bytes = width
        .checked_mul(element_bytes)
        .ok_or("source row size overflow")?;
    let mut raw = vec![0u8; job.rows * row_bytes];
    let file = &mut files[tensor.source.shard];
    let source_row = |row| {
        let row = tensor
            .order
            .filter(|o| o.axis == 0)
            .map_or(row, |o| o.source_index(row));
        tensor
            .row_slice
            .map_or(row, |s| (row / s.rows) * s.stride + s.offset + row % s.rows)
    };
    let mut row = 0;
    while row < job.rows {
        let start = source_row(job.row + row);
        let mut run = 1;
        while row + run < job.rows && source_row(job.row + row + run) == start + run {
            run += 1;
        }
        file.seek(SeekFrom::Start(
            tensor.source.start + (start * row_bytes) as u64,
        ))?;
        file.read_exact(&mut raw[row * row_bytes..(row + run) * row_bytes])?;
        row += run;
    }
    let mut values = vec![0.0f32; job.rows * width];
    for (r, row) in values.chunks_exact_mut(width).enumerate() {
        for (c, value) in row.iter_mut().enumerate() {
            let column = tensor.columns.as_ref().map_or(c, |columns| columns[c]);
            let at = (r * width + column) * element_bytes;
            let x = match tensor.source.entry.dtype {
                MlxDType::BF16 => bf16_to_f32(u16::from_le_bytes(raw[at..at + 2].try_into()?)),
                MlxDType::F16 => f16_to_f32(u16::from_le_bytes(raw[at..at + 2].try_into()?)),
                MlxDType::F32 => f32::from_le_bytes(raw[at..at + 4].try_into()?),
                _ => unreachable!(),
            };
            *value = match tensor.transform {
                ValueTransform::Identity => x,
                ValueTransform::AddOne => x + 1.0,
                ValueTransform::NegativeExp => -x.exp(),
            };
            if !value.is_finite() {
                return Err(format!("non-finite weight in {}", tensor.source_name).into());
            }
        }
    }
    Ok(values)
}

fn open_sources(plan: &Plan) -> Result<Vec<File>> {
    plan.shards.iter().map(|p| Ok(File::open(p)?)).collect()
}

fn reference(options: &Options) -> Result<Option<GgufFile>> {
    options
        .reference
        .as_ref()
        .map(GgufFile::open)
        .transpose()
        .map_err(Into::into)
}

/// Converts text including MTP, or the vision tower, through a worker pool.
/// Memory use scales with worker count and row width, not checkpoint size.
pub fn convert(options: &Options) -> Result<()> {
    if options.threads == 0 {
        return Err("threads must be positive".into());
    }
    let reference = reference(options)?;
    let plan = make_plan(options, reference.as_ref())?;
    let jobs = jobs(&plan);
    let specs: Vec<_> = plan.tensors.iter().map(|t| t.spec.clone()).collect();
    if options.output.exists() {
        return Err(format!("output already exists: {}", options.output.display()).into());
    }
    let mut temporary = options.output.as_os_str().to_owned();
    temporary.push(".partial");
    let temporary = PathBuf::from(temporary);
    let layout = gguf_write::create(&temporary, &plan.kv, &specs)?;
    let next = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let start = Instant::now();
    eprintln!(
        "converting {} tensors ({} blocks, {}), {} bytes, {} workers",
        specs.len(),
        plan.main_layers,
        if options.vision {
            "vision"
        } else {
            "plus 1 MTP"
        },
        layout.file_size,
        options.threads
    );
    std::thread::scope(|scope| -> Result<()> {
        let mut workers = Vec::new();
        for _ in 0..options.threads {
            workers.push(scope.spawn(|| -> Result<()> {
                let mut files = open_sources(&plan)?;
                let mut output = OpenOptions::new().write(true).open(&temporary)?;
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&job) = jobs.get(index) else {
                        break;
                    };
                    let tensor = &plan.tensors[job.tensor];
                    let values = read_source(&mut files, tensor, job)?;
                    let mut bytes = vec![0u8; job.rows * tensor.row_bytes()];
                    crate::quant_encode::encode(tensor.spec.tensor_type, &values, &mut bytes)
                        .map_err(|e| format!("{} row {}: {e}", tensor.spec.name, job.row))?;
                    gguf_write::write_range(
                        &mut output,
                        &layout,
                        job.tensor,
                        (job.row * tensor.row_bytes()) as u64,
                        &bytes,
                    )?;
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if done % 512 == 0 || done == jobs.len() {
                        eprintln!(
                            "converted {done}/{} row groups in {:.1}s",
                            jobs.len(),
                            start.elapsed().as_secs_f64()
                        );
                    }
                }
                Ok(())
            }));
        }
        let mut failure = None;
        for worker in workers {
            let result = worker
                .join()
                .map_err(|_| "conversion worker panicked".into())
                .and_then(|r| r);
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    })?;
    OpenOptions::new()
        .write(true)
        .open(&temporary)?
        .sync_all()?;
    // A hard link publishes without replacing a file created concurrently.
    fs::hard_link(&temporary, &options.output)?;
    fs::remove_file(&temporary)?;
    eprintln!(
        "wrote {} in {:.3}s",
        options.output.display(),
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

pub use validate::validate;
