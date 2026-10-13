use super::metadata::{set, string};
use super::*;
use crate::formats::gguf::{GgufArray, GgufValue};
use crate::formats::gguf_write::value_bytes;

/// The compatibility reference defines preprocessing policy, not weights.
/// Every architecture field is checked against the checkpoint before writing.
pub(super) fn make_plan(options: &Options, reference: Option<&GgufFile>) -> Result<Plan> {
    let reference = reference.ok_or("vision conversion requires --reference MMPROJ.gguf")?;
    if options.mix.is_some() || !options.rules.is_empty() {
        return Err("text mix rules do not apply to vision conversion".into());
    }
    let matrix_type = options.matrix_type.unwrap_or(TensorType::F16);
    if !matches!(matrix_type, TensorType::F16 | TensorType::Q8_0) {
        return Err("vision matrix type must be f16 or q8_0".into());
    }
    let config = json(&options.input.join("config.json"))?;
    let config = field(&config, "vision_config")?;
    if field(config, "model_type")?.as_str() != Some("qwen3_5")
        || field(config, "hidden_act")?.as_str() != Some("gelu_pytorch_tanh")
        || !field(config, "deepstack_visual_indexes")?
            .as_array()
            .is_some_and(|a| a.is_empty())
    {
        return Err("expected a dense Qwen3.5 vision tower without deepstack".into());
    }
    let hidden = number(config, "hidden_size")?;
    let ffn = number(config, "intermediate_size")?;
    let layers = number(config, "depth")?;
    let heads = number(config, "num_heads")?;
    let out = number(config, "out_hidden_size")?;
    let patch = number(config, "patch_size")?;
    let merge = number(config, "spatial_merge_size")?;
    let positions = number(config, "num_position_embeddings")?;
    let side = (positions as f64).sqrt() as usize;
    if [hidden, ffn, layers, heads, out, patch, merge, positions].contains(&0)
        || hidden % heads != 0
        || side * side != positions
        || number(config, "in_channels")? != 3
        || number(config, "temporal_patch_size")? != 2
    {
        return Err("unsupported vision dimensions".into());
    }
    let merged = hidden
        .checked_mul(merge)
        .and_then(|n| n.checked_mul(merge))
        .ok_or("merged vision dimension overflow")?;
    let (shards, sources) = checkpoint(&options.input)?;
    let mut used = BTreeSet::new();
    let mut tensors = Vec::new();
    let mut add = |suffix: &str,
                   name: String,
                   source_shape: Vec<usize>,
                   shape: Vec<usize>,
                   ty: TensorType,
                   row_slice|
     -> Result<()> {
        let source_name = format!("model.visual.{suffix}");
        let source = sources
            .get(&source_name)
            .ok_or_else(|| format!("missing tensor {source_name}"))?
            .clone();
        used.insert(source_name.clone());
        if source.entry.shape != source_shape.iter().map(|&n| n as u64).collect::<Vec<_>>() {
            return Err(format!(
                "{source_name}: unexpected shape {:?}, expected {source_shape:?}",
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
        let dimensions: Vec<u64> = shape.into_iter().rev().map(|n| n as u64).collect();
        let spec = TensorSpec {
            name,
            dimensions,
            tensor_type: ty,
        };
        let r = reference.require_tensor(&spec.name)?;
        if r.dimensions != spec.dimensions {
            return Err(format!("reference shape mismatch for {}", spec.name).into());
        }
        if spec.dimensions[0] % block_elements(ty.ggml_type()) as u64 != 0 {
            return Err(format!("unaligned vision row width for {}", spec.name).into());
        }
        tensors.push(Tensor {
            spec,
            source_name,
            source,
            transform: ValueTransform::Identity,
            order: None,
            columns: None,
            row_slice,
        });
        Ok(())
    };
    for layer in 0..layers {
        for (source, target, rows, columns) in [
            ("attn.proj", "attn_out", hidden, hidden),
            ("attn.qkv", "attn_qkv", 3 * hidden, hidden),
            ("mlp.linear_fc1", "ffn_up", ffn, hidden),
            ("mlp.linear_fc2", "ffn_down", hidden, ffn),
        ] {
            let shape = vec![rows, columns];
            add(
                &format!("blocks.{layer}.{source}.weight"),
                format!("v.blk.{layer}.{target}.weight"),
                shape.clone(),
                shape,
                matrix_type,
                None,
            )?;
            add(
                &format!("blocks.{layer}.{source}.bias"),
                format!("v.blk.{layer}.{target}.bias"),
                vec![rows],
                vec![rows],
                TensorType::F32,
                None,
            )?;
        }
        for (source, target) in [("norm1", "ln1"), ("norm2", "ln2")] {
            for suffix in ["weight", "bias"] {
                add(
                    &format!("blocks.{layer}.{source}.{suffix}"),
                    format!("v.blk.{layer}.{target}.{suffix}"),
                    vec![hidden],
                    vec![hidden],
                    TensorType::F32,
                    None,
                )?;
            }
        }
    }
    for (source, target, rows, columns) in [
        ("merger.linear_fc1", "mm.0", merged, merged),
        ("merger.linear_fc2", "mm.2", out, merged),
    ] {
        let shape = vec![rows, columns];
        add(
            &format!("{source}.weight"),
            format!("{target}.weight"),
            shape.clone(),
            shape,
            matrix_type,
            None,
        )?;
        add(
            &format!("{source}.bias"),
            format!("{target}.bias"),
            vec![rows],
            vec![rows],
            TensorType::F32,
            None,
        )?;
    }
    for suffix in ["weight", "bias"] {
        add(
            &format!("merger.norm.{suffix}"),
            format!("v.post_ln.{suffix}"),
            vec![hidden],
            vec![hidden],
            TensorType::F32,
            None,
        )?;
    }
    add(
        "patch_embed.proj.bias",
        "v.patch_embd.bias".into(),
        vec![hidden],
        vec![hidden],
        TensorType::F32,
        None,
    )?;
    // Source [output, channel, time, y, x] becomes two [output, channel, y, x]
    // tensors. The runtime applies both temporal slices to the still frame.
    for time in 0..2 {
        add(
            "patch_embed.proj.weight",
            if time == 0 {
                "v.patch_embd.weight"
            } else {
                "v.patch_embd.weight.1"
            }
            .into(),
            vec![hidden, 3, 2, patch, patch],
            vec![hidden, 3, patch, patch],
            TensorType::F16,
            Some(RowSlice {
                rows: patch,
                stride: 2 * patch,
                offset: time * patch,
            }),
        )?;
    }
    // Position interpolation and normalization read floating-point weights.
    add(
        "pos_embed.weight",
        "v.position_embd.weight".into(),
        vec![positions, hidden],
        vec![positions, hidden],
        TensorType::F32,
        None,
    )?;
    for name in sources.keys().filter(|n| n.starts_with("model.visual.")) {
        if !used.contains(name) {
            return Err(format!("unmapped vision tensor {name}").into());
        }
    }
    if tensors.len() != reference.tensors.len() {
        return Err("reference vision tensor inventory differs from checkpoint".into());
    }
    tensors.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
    let mut expected = Vec::new();
    set(&mut expected, "general.architecture", string("clip"));
    set(&mut expected, "general.type", string("mmproj"));
    set(
        &mut expected,
        "clip.projector_type",
        string("qwen3vl_merger"),
    );
    set(
        &mut expected,
        "clip.has_vision_encoder",
        GgufValue::Bool(true),
    );
    set(&mut expected, "clip.use_gelu", GgufValue::Bool(true));
    for (key, value) in [
        ("projection_dim", out),
        ("image_size", side * patch),
        ("patch_size", patch),
        ("embedding_length", hidden),
        ("feed_forward_length", ffn),
        ("block_count", layers),
        ("attention.head_count", heads),
        ("spatial_merge_size", merge),
    ] {
        set(
            &mut expected,
            &format!("clip.vision.{key}"),
            GgufValue::Uint32(u32::try_from(value)?),
        );
    }
    set(
        &mut expected,
        "clip.vision.attention.layer_norm_epsilon",
        GgufValue::Float32(1e-6),
    );
    set(
        &mut expected,
        "clip.vision.is_deepstack_layers",
        GgufValue::Array(GgufArray::Bool(vec![false; layers])),
    );
    let processor = json(&options.input.join("preprocessor_config.json"))?;
    for key in ["image_mean", "image_std"] {
        let values = field(&processor, key)?
            .as_array()
            .ok_or("expected image normalization array")?;
        let values = values
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|v| v as f32)
                    .ok_or("expected image normalization number")
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if values.len() != 3
            || values
                .iter()
                .any(|x| !x.is_finite() || (key == "image_std" && *x <= 0.0))
        {
            return Err("invalid image normalization".into());
        }
        set(
            &mut expected,
            &format!("clip.vision.{key}"),
            GgufValue::Array(GgufArray::Float32(values)),
        );
    }
    for entry in expected {
        if reference.get_value(&entry.key).map(value_bytes) != Some(value_bytes(&entry.value)) {
            return Err(format!("reference/config vision metadata mismatch: {}", entry.key).into());
        }
    }
    let mut kv = reference.kv.clone();
    if !options.preserve_reference_provenance {
        kv.retain(|e| {
            !e.key.starts_with("quantize.imatrix.")
                && e.key != "general.repo_url"
                && e.key != "general.tags"
        });
        set(&mut kv, "general.quantized_by", string("Makepad"));
    }
    set(
        &mut kv,
        "general.file_type",
        GgufValue::Uint32(mix::file_type(&tensors)),
    );
    set(
        &mut kv,
        "general.quantization_version",
        GgufValue::Uint32(2),
    );
    Ok(Plan {
        shards,
        tensors,
        kv,
        main_layers: layers,
    })
}
