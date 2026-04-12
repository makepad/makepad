use crate::{DiffusionError, Result};
use makepad_micro_serde::{DeJson, JsonValue};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

type JsonObject = HashMap<String, JsonValue>;
type WorkflowNodes = HashMap<String, JsonValue>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FluxWorkflowKind {
    SplitModel,
    Checkpoint,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FluxWorkflowFiles {
    pub checkpoint_name: Option<String>,
    pub unet_name: Option<String>,
    pub vae_name: Option<String>,
    pub clip_l_name: Option<String>,
    pub t5xxl_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FluxPrompts {
    pub clip_l: String,
    pub t5xxl: String,
    pub negative: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FluxGenerationConfig {
    pub width: u32,
    pub height: u32,
    pub batch_size: u32,
    pub seed: u64,
    pub steps: u32,
    pub cfg: f32,
    pub denoise: f32,
    pub guidance: f32,
    pub sampler_name: String,
    pub scheduler: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FluxWorkflow {
    pub path: PathBuf,
    pub kind: FluxWorkflowKind,
    pub files: FluxWorkflowFiles,
    pub prompts: FluxPrompts,
    pub generation: FluxGenerationConfig,
}

impl FluxWorkflow {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let text = fs::read_to_string(&path)
            .map_err(|err| DiffusionError::io(&path, err.to_string()))?;
        Self::from_json_str(&path, &text)
    }

    pub fn from_json_str(path_hint: impl AsRef<Path>, text: &str) -> Result<Self> {
        let path_hint = path_hint.as_ref().to_path_buf();
        let nodes = WorkflowNodes::deserialize_json(text)
            .map_err(|err| DiffusionError::json(&path_hint, format!("{:?}", err)))?;

        let (_, ksampler) = find_first_node(&nodes, "KSampler").ok_or_else(|| {
            DiffusionError::workflow("expected a KSampler node in the ComfyUI workflow")
        })?;
        let model_node = input_ref_node(&nodes, ksampler, "model", &path_hint)?;
        let positive_node = input_ref_node(&nodes, ksampler, "positive", &path_hint)?;
        let negative_node = input_ref_node(&nodes, ksampler, "negative", &path_hint)?;
        let latent_node = input_ref_node(&nodes, ksampler, "latent_image", &path_hint)?;

        let mut files = FluxWorkflowFiles::default();
        let kind = match node_class_type(model_node, &path_hint)? {
            "UNETLoader" => {
                files.unet_name = Some(input_string(model_node, "unet_name", &path_hint)?);
                FluxWorkflowKind::SplitModel
            }
            "CheckpointLoaderSimple" => {
                files.checkpoint_name = Some(input_string(model_node, "ckpt_name", &path_hint)?);
                FluxWorkflowKind::Checkpoint
            }
            other => {
                return Err(DiffusionError::workflow(format!(
                    "unsupported model loader node '{}'",
                    other
                )));
            }
        };

        if let Some(clip_node) = input_ref_node_opt(&nodes, positive_node, "clip", &path_hint)? {
            if node_class_type(clip_node, &path_hint)? == "DualCLIPLoader" {
                files.clip_l_name = Some(input_string(clip_node, "clip_name1", &path_hint)?);
                files.t5xxl_name = Some(input_string(clip_node, "clip_name2", &path_hint)?);
            }
        }

        if let Some((_, vae_decode_node)) = find_first_node(&nodes, "VAEDecode") {
            if let Some(vae_node) = input_ref_node_opt(&nodes, vae_decode_node, "vae", &path_hint)? {
                if node_class_type(vae_node, &path_hint)? == "VAELoader" {
                    files.vae_name = Some(input_string(vae_node, "vae_name", &path_hint)?);
                }
            }
        }

        let (mut prompts, guidance) = parse_positive_prompts(&nodes, positive_node, &path_hint)?;
        prompts.negative = parse_negative_prompt(&nodes, negative_node, &path_hint)?;

        let generation = FluxGenerationConfig {
            width: input_u32(latent_node, "width", &path_hint)?,
            height: input_u32(latent_node, "height", &path_hint)?,
            batch_size: input_u32(latent_node, "batch_size", &path_hint)?,
            seed: input_u64(ksampler, "seed", &path_hint)?,
            steps: input_u32(ksampler, "steps", &path_hint)?,
            cfg: input_f32(ksampler, "cfg", &path_hint)?,
            denoise: input_f32(ksampler, "denoise", &path_hint)?,
            guidance: guidance.unwrap_or(0.0),
            sampler_name: input_string(ksampler, "sampler_name", &path_hint)?,
            scheduler: input_string(ksampler, "scheduler", &path_hint)?,
        };

        Ok(Self {
            path: path_hint,
            kind,
            files,
            prompts,
            generation,
        })
    }
}

fn parse_positive_prompts(
    nodes: &WorkflowNodes,
    node: &JsonObject,
    path_hint: &Path,
) -> Result<(FluxPrompts, Option<f32>)> {
    match node_class_type(node, path_hint)? {
        "CLIPTextEncodeFlux" => Ok((
            FluxPrompts {
                clip_l: normalize_prompt(&input_string(node, "clip_l", path_hint)?),
                t5xxl: normalize_prompt(&input_string(node, "t5xxl", path_hint)?),
                negative: String::new(),
            },
            Some(input_f32(node, "guidance", path_hint)?),
        )),
        "FluxGuidance" => {
            let conditioning_node = input_ref_node(nodes, node, "conditioning", path_hint)?;
            let prompt = parse_single_text_prompt(conditioning_node, path_hint)?;
            Ok((
                FluxPrompts {
                    clip_l: prompt.clone(),
                    t5xxl: prompt,
                    negative: String::new(),
                },
                Some(input_f32(node, "guidance", path_hint)?),
            ))
        }
        "CLIPTextEncode" => {
            let prompt = parse_single_text_prompt(node, path_hint)?;
            Ok((
                FluxPrompts {
                    clip_l: prompt.clone(),
                    t5xxl: prompt,
                    negative: String::new(),
                },
                None,
            ))
        }
        other => Err(DiffusionError::workflow(format!(
            "unsupported positive conditioning node '{}'",
            other
        ))),
    }
}

fn parse_negative_prompt(
    _nodes: &WorkflowNodes,
    node: &JsonObject,
    path_hint: &Path,
) -> Result<String> {
    match node_class_type(node, path_hint)? {
        "ConditioningZeroOut" => Ok(String::new()),
        "CLIPTextEncode" => parse_single_text_prompt(node, path_hint),
        "CLIPTextEncodeFlux" => Ok(normalize_prompt(&input_string(node, "clip_l", path_hint)?)),
        other => Err(DiffusionError::workflow(format!(
            "unsupported negative conditioning node '{}'",
            other
        ))),
    }
}

fn parse_single_text_prompt(node: &JsonObject, path_hint: &Path) -> Result<String> {
    Ok(normalize_prompt(&input_string(node, "text", path_hint)?))
}

fn normalize_prompt(text: &str) -> String {
    text.trim_end_matches(|ch| ch == '\r' || ch == '\n')
        .to_string()
}

fn find_first_node<'a>(nodes: &'a WorkflowNodes, class_type: &str) -> Option<(&'a str, &'a JsonObject)> {
    nodes.iter().find_map(|(id, value)| {
        let node = value.object()?;
        match node.get("class_type") {
            Some(JsonValue::String(node_class)) if node_class == class_type => Some((id.as_str(), node)),
            _ => None,
        }
    })
}

fn input_ref_node<'a>(
    nodes: &'a WorkflowNodes,
    node: &JsonObject,
    key: &str,
    path_hint: &Path,
) -> Result<&'a JsonObject> {
    let node_id = input_ref_id(node, key, path_hint)?;
    node_object(nodes, &node_id, path_hint)
}

fn input_ref_node_opt<'a>(
    nodes: &'a WorkflowNodes,
    node: &JsonObject,
    key: &str,
    path_hint: &Path,
) -> Result<Option<&'a JsonObject>> {
    let Some(node_id) = input_ref_id_opt(node, key, path_hint)? else {
        return Ok(None);
    };
    Ok(Some(node_object(nodes, &node_id, path_hint)?))
}

fn node_object<'a>(nodes: &'a WorkflowNodes, node_id: &str, path_hint: &Path) -> Result<&'a JsonObject> {
    let value = nodes.get(node_id).ok_or_else(|| {
        DiffusionError::workflow(format!(
            "workflow {} references missing node '{}'",
            path_hint.display(),
            node_id
        ))
    })?;
    value.object().ok_or_else(|| {
        DiffusionError::workflow(format!(
            "workflow node '{}' in {} is not a JSON object",
            node_id,
            path_hint.display()
        ))
    })
}

fn node_class_type<'a>(node: &'a JsonObject, path_hint: &Path) -> Result<&'a str> {
    let value = node
        .get("class_type")
        .ok_or_else(|| DiffusionError::workflow(format!("missing class_type in {}", path_hint.display())))?;
    json_str(value).ok_or_else(|| {
        DiffusionError::workflow(format!(
            "class_type must be a string in {}",
            path_hint.display()
        ))
    })
}

fn input_string(node: &JsonObject, key: &str, path_hint: &Path) -> Result<String> {
    let value = input_value(node, key, path_hint)?;
    json_str(value)
        .map(str::to_owned)
        .ok_or_else(|| DiffusionError::workflow(format!("input '{}' must be a string", key)))
}

fn input_u32(node: &JsonObject, key: &str, path_hint: &Path) -> Result<u32> {
    let value = input_value(node, key, path_hint)?;
    let value = json_u64(value).ok_or_else(|| {
        DiffusionError::workflow(format!(
            "input '{}' in {} must be an unsigned integer",
            key,
            path_hint.display()
        ))
    })?;
    u32::try_from(value)
        .map_err(|_| DiffusionError::workflow(format!("input '{}' exceeds u32 range", key)))
}

fn input_u64(node: &JsonObject, key: &str, path_hint: &Path) -> Result<u64> {
    let value = input_value(node, key, path_hint)?;
    json_u64(value).ok_or_else(|| {
        DiffusionError::workflow(format!(
            "input '{}' in {} must be an unsigned integer",
            key,
            path_hint.display()
        ))
    })
}

fn input_f32(node: &JsonObject, key: &str, path_hint: &Path) -> Result<f32> {
    let value = input_value(node, key, path_hint)?;
    json_f32(value).ok_or_else(|| {
        DiffusionError::workflow(format!(
            "input '{}' in {} must be numeric",
            key,
            path_hint.display()
        ))
    })
}

fn input_ref_id(node: &JsonObject, key: &str, path_hint: &Path) -> Result<String> {
    input_ref_id_opt(node, key, path_hint)?.ok_or_else(|| {
        DiffusionError::workflow(format!(
            "input '{}' in {} must be a node reference",
            key,
            path_hint.display()
        ))
    })
}

fn input_ref_id_opt(node: &JsonObject, key: &str, path_hint: &Path) -> Result<Option<String>> {
    let Some(value) = inputs_object(node, path_hint)?.get(key) else {
        return Ok(None);
    };
    json_ref_node_id(value).map(Some).ok_or_else(|| {
        DiffusionError::workflow(format!(
            "input '{}' in {} must be a [node_id, output_index] reference",
            key,
            path_hint.display()
        ))
    })
}

fn input_value<'a>(node: &'a JsonObject, key: &str, path_hint: &Path) -> Result<&'a JsonValue> {
    inputs_object(node, path_hint)?
        .get(key)
        .ok_or_else(|| DiffusionError::workflow(format!("missing input '{}' in {}", key, path_hint.display())))
}

fn inputs_object<'a>(node: &'a JsonObject, path_hint: &Path) -> Result<&'a JsonObject> {
    node.get("inputs")
        .and_then(JsonValue::object)
        .ok_or_else(|| DiffusionError::workflow(format!("missing inputs object in {}", path_hint.display())))
}

fn json_str(value: &JsonValue) -> Option<&str> {
    match value {
        JsonValue::String(text) => Some(text),
        JsonValue::BareIdent(text) => Some(text),
        _ => None,
    }
}

fn json_u64(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::U64(number) => Some(*number),
        JsonValue::U128(number) => u64::try_from(*number).ok(),
        JsonValue::I64(number) => u64::try_from(*number).ok(),
        JsonValue::I128(number) => u64::try_from(*number).ok(),
        JsonValue::F64(number) if *number >= 0.0 => Some(*number as u64),
        _ => None,
    }
}

fn json_f32(value: &JsonValue) -> Option<f32> {
    match value {
        JsonValue::U64(number) => Some(*number as f32),
        JsonValue::U128(number) => Some(*number as f32),
        JsonValue::I64(number) => Some(*number as f32),
        JsonValue::I128(number) => Some(*number as f32),
        JsonValue::F64(number) => Some(*number as f32),
        _ => None,
    }
}

fn json_ref_node_id(value: &JsonValue) -> Option<String> {
    let array = match value {
        JsonValue::Array(array) => array,
        _ => return None,
    };
    let node_id = array.first()?;
    json_str(node_id).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{FluxWorkflow, FluxWorkflowKind};

    #[test]
    fn parses_split_flux_workflow() {
        let workflow = FluxWorkflow::from_json_str(
            "flux_dev_full_text_to_image.json",
            include_str!("../../../examples/comfyui/flux_dev_full_text_to_image.json"),
        )
        .unwrap();

        assert_eq!(workflow.kind, FluxWorkflowKind::SplitModel);
        assert_eq!(workflow.files.unet_name.as_deref(), Some("flux1-dev.safetensors"));
        assert_eq!(workflow.files.vae_name.as_deref(), Some("ae.safetensors"));
        assert_eq!(workflow.files.clip_l_name.as_deref(), Some("clip_l.safetensors"));
        assert_eq!(workflow.files.t5xxl_name.as_deref(), Some("t5xxl_fp16.safetensors"));
        assert_eq!(workflow.prompts.clip_l, "test");
        assert_eq!(workflow.prompts.t5xxl, "test");
        assert_eq!(workflow.prompts.negative, "");
        assert_eq!(workflow.generation.guidance, 3.5);
        assert_eq!(workflow.generation.width, 1024);
        assert_eq!(workflow.generation.height, 1024);
    }

    #[test]
    fn parses_checkpoint_flux_workflow() {
        let workflow = FluxWorkflow::from_json_str(
            "flux_dev.json",
            include_str!("../../../examples/comfyui/flux_dev.json"),
        )
        .unwrap();

        assert_eq!(workflow.kind, FluxWorkflowKind::Checkpoint);
        assert_eq!(
            workflow.files.checkpoint_name.as_deref(),
            Some("flux1-dev-fp8.safetensors")
        );
        assert_eq!(workflow.prompts.clip_l, "kids driving cars photorealistic");
        assert_eq!(workflow.prompts.t5xxl, "kids driving cars photorealistic");
        assert_eq!(workflow.prompts.negative, "");
        assert_eq!(workflow.generation.guidance, 3.5);
        assert_eq!(workflow.generation.width, 1088);
        assert_eq!(workflow.generation.height, 1920);
    }
}
