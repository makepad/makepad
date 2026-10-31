use super::*;
use crate::formats::gguf::{GgufArray, GgufString, GgufValue};
use crate::formats::gguf_write::value_bytes;

pub(super) fn string(s: impl AsRef<str>) -> GgufValue {
    GgufValue::String(GgufString::new(s.as_ref().as_bytes().to_vec()))
}

pub(super) fn set(kv: &mut Vec<GgufKeyValue>, key: &str, value: GgufValue) {
    if let Some(entry) = kv.iter_mut().find(|e| e.key == key) {
        entry.value = value;
    } else {
        kv.push(GgufKeyValue {
            key: key.into(),
            value,
        });
    }
}

pub(super) fn build(
    options: &Options,
    config: &JsonValue,
    reference: Option<&GgufFile>,
    file_type: u32,
) -> Result<Vec<GgufKeyValue>> {
    let mut generated = Vec::new();
    set(&mut generated, "general.architecture", string("qwen35"));
    set(&mut generated, "general.type", string("model"));
    set(&mut generated, "general.name", string("Qwen3.5-MTP"));
    set(&mut generated, "general.license", string("apache-2.0"));
    set(&mut generated, "general.quantized_by", string("Makepad"));
    for (key, source) in [
        ("block_count", "num_hidden_layers"),
        ("context_length", "max_position_embeddings"),
        ("embedding_length", "hidden_size"),
        ("feed_forward_length", "intermediate_size"),
        ("attention.head_count", "num_attention_heads"),
        ("attention.head_count_kv", "num_key_value_heads"),
        ("attention.key_length", "head_dim"),
        ("attention.value_length", "head_dim"),
        ("ssm.conv_kernel", "linear_conv_kernel_dim"),
        ("ssm.state_size", "linear_key_head_dim"),
        ("ssm.group_count", "linear_num_key_heads"),
        ("ssm.time_step_rank", "linear_num_value_heads"),
        ("full_attention_interval", "full_attention_interval"),
    ] {
        set(
            &mut generated,
            &format!("qwen35.{key}"),
            GgufValue::Uint32(u32::try_from(number(config, source)?)?),
        );
    }
    let inner = number(config, "linear_num_value_heads")?
        .checked_mul(number(config, "linear_value_head_dim")?)
        .ok_or("inner dimension overflow")?;
    set(
        &mut generated,
        "qwen35.ssm.inner_size",
        GgufValue::Uint32(u32::try_from(inner)?),
    );
    let rope = field(config, "rope_parameters")?;
    let float = |j: &JsonValue, key: &str| -> Result<f32> {
        let x = field(j, key)?
            .as_f64()
            .ok_or_else(|| format!("{key} must be numeric"))? as f32;
        if !x.is_finite() || x <= 0.0 {
            return Err(format!("{key} must be finite and positive").into());
        }
        Ok(x)
    };
    set(
        &mut generated,
        "qwen35.rope.freq_base",
        GgufValue::Float32(float(rope, "rope_theta")?),
    );
    set(
        &mut generated,
        "qwen35.attention.layer_norm_rms_epsilon",
        GgufValue::Float32(float(config, "rms_norm_eps")?),
    );
    let rotary = number(config, "head_dim")? as f32 * float(rope, "partial_rotary_factor")?;
    if rotary.fract() != 0.0 || rotary > number(config, "head_dim")? as f32 {
        return Err("invalid rotary dimension".into());
    }
    set(
        &mut generated,
        "qwen35.rope.dimension_count",
        GgufValue::Uint32(rotary as u32),
    );
    let mut sections: Vec<i32> = field(rope, "mrope_section")?
        .as_array()
        .ok_or("mrope_section must be an array")?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|x| i32::try_from(x).ok())
                .ok_or("invalid rotary section")
        })
        .collect::<std::result::Result<_, _>>()?;
    if sections.len() != 3 && sections.len() != 4 {
        return Err("expected three or four rotary sections".into());
    }
    sections.resize(4, 0);
    set(
        &mut generated,
        "qwen35.rope.dimension_sections",
        GgufValue::Array(GgufArray::Int32(sections)),
    );
    tokenizer(
        &options.input,
        number(config, "vocab_size")?,
        &mut generated,
    )?;
    set(
        &mut generated,
        "general.quantization_version",
        GgufValue::Uint32(2),
    );
    set(
        &mut generated,
        "general.file_type",
        GgufValue::Uint32(file_type),
    );
    let mut kv = if let Some(reference) = reference {
        if reference.get_value("general.architecture").map(value_bytes)
            != Some(value_bytes(&string("qwen35")))
        {
            return Err("reference architecture is not qwen35".into());
        }
        if reference.get_value("qwen35.nextn_predict_layers").is_some() {
            return Err("reference must contain only the main model".into());
        }
        for entry in generated.iter().filter(|e| e.key.starts_with("qwen35.")) {
            if reference.get_value(&entry.key).map(value_bytes) != Some(value_bytes(&entry.value)) {
                return Err(format!("reference/config metadata mismatch: {}", entry.key).into());
            }
        }
        // The compatibility profile may add tokens only in unused vocabulary
        // slots. It cannot change the checkpoint's existing token identities.
        let token_array = |kv: &[GgufKeyValue]| -> Result<Vec<GgufString>> {
            match &kv
                .iter()
                .find(|e| e.key == "tokenizer.ggml.tokens")
                .ok_or("missing token array")?
                .value
            {
                GgufValue::Array(GgufArray::String(tokens)) => Ok(tokens.clone()),
                _ => Err("invalid token array".into()),
            }
        };
        let src = token_array(&generated)?;
        let dst = token_array(&reference.kv)?;
        if src.len() != dst.len() {
            return Err("reference vocabulary size mismatch".into());
        }
        for (id, (a, b)) in src.iter().zip(&dst).enumerate() {
            if a != b && a.as_bytes() != format!("[PAD{id}]").as_bytes() {
                return Err(format!("reference changes existing token {id}").into());
            }
        }
        for entry in generated.iter().filter(|e| e.key.starts_with("tokenizer.")) {
            if reference.get_value(&entry.key).map(value_bytes) != Some(value_bytes(&entry.value)) {
                eprintln!("tokenizer compatibility override: {}", entry.key);
            }
        }
        reference.kv.clone()
    } else {
        generated
    };
    if !options.preserve_reference_provenance {
        kv.retain(|e| {
            !e.key.starts_with("quantize.imatrix.")
                && e.key != "general.repo_url"
                && e.key != "general.tags"
        });
        set(&mut kv, "general.quantized_by", string("Makepad"));
    }
    set(&mut kv, "general.file_type", GgufValue::Uint32(file_type));
    let total = number(config, "num_hidden_layers")?
        .checked_add(number(config, "mtp_num_hidden_layers")?)
        .ok_or("layer count overflow")?;
    set(
        &mut kv,
        "qwen35.block_count",
        GgufValue::Uint32(u32::try_from(total)?),
    );
    set(
        &mut kv,
        "qwen35.nextn_predict_layers",
        GgufValue::Uint32(u32::try_from(number(config, "mtp_num_hidden_layers")?)?),
    );
    Ok(kv)
}

fn tokenizer(dir: &Path, size: usize, kv: &mut Vec<GgufKeyValue>) -> Result<()> {
    let source = json(&dir.join("tokenizer.json"))?;
    let config = json(&dir.join("tokenizer_config.json"))?;
    let model = field(&source, "model")?;
    let vocab = field(model, "vocab")?
        .as_object()
        .ok_or("vocab must be an object")?;
    let mut tokens: Vec<_> = (0..size).map(|i| format!("[PAD{i}]")).collect();
    let mut types = vec![5i32; size];
    for (token, id) in vocab {
        let id = usize::try_from(id.as_u64().ok_or("invalid token id")?)?;
        if id >= size || types[id] != 5 {
            return Err("duplicate or out of range vocabulary id".into());
        }
        tokens[id] = token.clone();
        types[id] = 1;
    }
    for token in field(&source, "added_tokens")?
        .as_array()
        .ok_or("added_tokens must be an array")?
    {
        let id = number(token, "id")?;
        if id >= size {
            return Err("added token out of range".into());
        }
        let content = field(token, "content")?
            .as_str()
            .ok_or("token content must be a string")?;
        tokens[id] = content.into();
        types[id] = if field(token, "special")?
            .as_bool()
            .ok_or("special must be bool")?
        {
            3
        } else {
            4
        };
    }
    for (key, field_name) in [
        ("eos_token_id", "eos_token"),
        ("padding_token_id", "pad_token"),
    ] {
        let name = field(&config, field_name)?
            .as_str()
            .ok_or("special token name must be a string")?;
        let id = tokens
            .iter()
            .position(|t| t == name)
            .ok_or("special token missing from vocabulary")?;
        set(
            kv,
            &format!("tokenizer.ggml.{key}"),
            GgufValue::Uint32(u32::try_from(id)?),
        );
    }
    let merges = field(model, "merges")?
        .as_array()
        .ok_or("merges must be an array")?
        .iter()
        .map(|m| -> Result<GgufString> {
            let text = if let Some(s) = m.as_str() {
                s.to_string()
            } else if let Some(pair) = m.as_array() {
                if pair.len() != 2 {
                    return Err("merge pair must have two tokens".into());
                }
                format!(
                    "{} {}",
                    pair[0].as_str().ok_or("invalid merge")?,
                    pair[1].as_str().ok_or("invalid merge")?
                )
            } else {
                return Err("invalid merge entry".into());
            };
            Ok(GgufString::new(text.into_bytes()))
        })
        .collect::<Result<Vec<_>>>()?;
    set(kv, "tokenizer.ggml.model", string("gpt2"));
    set(kv, "tokenizer.ggml.pre", string("qwen35"));
    set(
        kv,
        "tokenizer.ggml.tokens",
        GgufValue::Array(GgufArray::String(
            tokens
                .into_iter()
                .map(|t| GgufString::new(t.into_bytes()))
                .collect(),
        )),
    );
    set(
        kv,
        "tokenizer.ggml.token_type",
        GgufValue::Array(GgufArray::Int32(types)),
    );
    set(
        kv,
        "tokenizer.ggml.merges",
        GgufValue::Array(GgufArray::String(merges)),
    );
    let template = fs::read_to_string(dir.join("chat_template.jinja"))?;
    set(kv, "tokenizer.chat_template", string(template));
    Ok(())
}
