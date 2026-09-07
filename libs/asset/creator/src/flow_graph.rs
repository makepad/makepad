//! Translate legacy work orders into ordinary, inspectable Flow definitions.
//! Request values belong to an instance; definitions contain only the DAG.
use crate::engine::{Splice, StageOrder};
use crate::pipeline::PipelineSpec;
use makepad_flow::client::FlowClient;
use makepad_flow::{CreateInstanceRequest, InputValueDto, PortType};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::collections::HashMap;

pub struct CompiledCreator {
    pub name: String,
    pub source: String,
    pub inputs: CreateInstanceRequest,
    pub stages: Vec<(String, String)>,
}

fn quote(text: &str) -> String {
    text.to_string().serialize_json()
}
fn ident(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
}
fn output_type(domain: &str) -> Result<PortType, String> {
    use PortType::*;
    Ok(match domain {
        "text" | "vision" | "stt" => Text,
        "image" | "edit" | "inpaint" | "control" | "upscale" | "matte" | "depth" | "segment" => {
            Image
        }
        "video" | "enhance" => Video,
        "audio" | "speech" | "music" | "stems" => Audio,
        "mesh" | "paint" | "rig" | "motion" => Mesh,
        "world" | "splat" | "body" | "beats" | "notes" => Bytes,
        _ => return Err(format!("unsupported creator domain {domain}")),
    })
}
fn media_type(mime: &str) -> PortType {
    if mime.starts_with("image/") {
        PortType::Image
    } else if mime.starts_with("audio/") {
        PortType::Audio
    } else if mime.starts_with("video/") {
        PortType::Video
    } else if mime.starts_with("model/") {
        PortType::Mesh
    } else {
        PortType::Bytes
    }
}

pub fn compile(
    client: &FlowClient,
    spec: &PipelineSpec,
    orders: &[StageOrder],
) -> Result<CompiledCreator, String> {
    crate::pipeline::validate(spec)?;
    if orders.len() != spec.stages.len() {
        return Err("creator stage/order count mismatch".into());
    }
    let index: HashMap<_, _> = spec
        .stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key.as_str(), i))
        .collect();
    let types: Vec<_> = spec
        .stages
        .iter()
        .map(|s| output_type(&s.domain))
        .collect::<Result<_, _>>()?;
    let mut source = String::from("use mod.flow.*\n\n");
    let mut nodes = Vec::new();
    let mut values = HashMap::new();
    let mut stages = Vec::new();
    for (i, order) in orders.iter().enumerate() {
        let stage = &spec.stages[i];
        if order.spec.key != stage.key
            || order.spec.domain != stage.domain
            || order.spec.deps != stage.deps
        {
            return Err(format!("work order {i} does not match its declared stage"));
        }
        let node = format!("stage_{i}");
        let req = format!("request_{i}");
        source.push_str(&format!(
            "let {req} = Input{{ type: @json default: nil }}\n"
        ));
        nodes.push(req.clone());
        let mut request = order.request.clone();
        request.seed = Some(request.seed.unwrap_or(stage.seed));
        let mut edges: Vec<(String, PortType, String)> = Vec::new();
        let mut media = Vec::new();
        if let Some(data) = request.input_b64.take() {
            media.push((
                "primary".to_string(),
                request
                    .input_content_type
                    .take()
                    .unwrap_or_else(|| "image/png".into()),
                data,
            ));
        }
        for input in request.inputs.take().unwrap_or_default() {
            media.push((input.name, input.content_type, input.data_b64));
        }
        for (j, (port, mime, data)) in media.into_iter().enumerate() {
            if !ident(&port)
                || ["request", "prompt", "text", "lyrics"].contains(&port.as_str())
                || port.starts_with("dependency_")
            {
                return Err(format!("unsupported named media input {port:?}"));
            }
            let ty = media_type(&mime);
            let bytes = makepad_ai_hub::makepad_base64::base64_decode(data.as_bytes())
                .map_err(|e| format!("input base64: {e:?}"))?;
            let digest = client
                .put_value(ty, &mime, &bytes)
                .map_err(|e| e.to_string())?
                .digest;
            let input = format!("media_{i}_{j}");
            source.push_str(&format!(
                "let {input} = Input{{ type: @{} default: nil }}\n",
                ty.as_str()
            ));
            nodes.push(input.clone());
            values.insert(
                input.clone(),
                HashMap::from([(
                    ty.as_str().into(),
                    InputValueDto {
                        ty,
                        text: None,
                        json: None,
                        digest: Some(digest),
                    },
                )]),
            );
            edges.push((port, ty, format!("{input}.out(@{})", ty.as_str())));
        }
        for splice in &order.splices {
            let (dep, port, expected) = match splice {
                Splice::PromptFromText(dep) => (dep, "prompt", Some(PortType::Text)),
                Splice::InputImageFrom(dep) => (dep, "primary", None),
                Splice::NamedInputFrom {
                    dep,
                    name,
                    content_type,
                } => (dep, name.as_str(), Some(media_type(content_type))),
            };
            let d = *index
                .get(dep.as_str())
                .ok_or_else(|| format!("unknown splice dependency {dep}"))?;
            if !stage.deps.contains(dep) || d >= i {
                return Err(format!("splice {dep} is not a declared earlier dependency"));
            }
            // The legacy name also carries a mesh into paint/rig, or audio
            // into a downstream stage. Preserve the artifact's actual type.
            if expected.is_none() && !types[d].is_media() {
                return Err(format!("primary input from {dep} must be media"));
            }
            let expected = expected.unwrap_or(types[d]);
            if types[d] != expected || !ident(port) {
                return Err(format!("invalid {port} splice from {dep}"));
            }
            edges.retain(|(name, _, _)| name != port);
            edges.push((port.into(), expected, format!("stage_{d}.out(@result)")));
        }
        for (j, dep) in stage.deps.iter().enumerate() {
            let d = index[dep.as_str()];
            edges.push((
                format!("dependency_{j}"),
                types[d],
                format!("stage_{d}.out(@result)"),
            ));
        }
        // Media lives on the data plane; the small exact request stays JSON.
        let json =
            JsonValue::deserialize_json(&request.serialize_json()).map_err(|e| format!("{e:?}"))?;
        values.insert(
            req.clone(),
            HashMap::from([(
                "json".into(),
                InputValueDto {
                    ty: PortType::Json,
                    text: None,
                    json: Some(json),
                    digest: None,
                },
            )]),
        );
        source.push_str(&format!(
            "let {node} = Gen{{ domain: {} seed: nil label: {}\n ports: {{ in: {{ request: @json",
            quote(&stage.domain),
            quote(&stage.key)
        ));
        for (port, ty, _) in &edges {
            source.push_str(&format!(" {port}: @{}", ty.as_str()));
        }
        source.push_str(&format!(
            " }} out: {{ result: @{} }} }}\n request: {req}.json()\n",
            types[i].as_str()
        ));
        if stage.on_fail_skip {
            if types[i] != PortType::Text {
                return Err(format!(
                    "skippable media stage {} needs an explicit Flow fallback",
                    stage.key
                ));
            }
            source.push_str(" on_fail: @skip result: \"\"\n");
        }
        for (port, _, edge) in edges {
            source.push_str(&format!(" {port}: {edge}\n"));
        }
        source.push_str("}\n");
        nodes.push(node.clone());
        let out = format!("output_{i}");
        source.push_str(&format!(
            "let {out} = Output{{ type: @{} value: {node}.out(@result) }}\n",
            types[i].as_str()
        ));
        nodes.push(out);
        stages.push((node, stage.key.clone()));
    }
    source.push_str(&format!(
        "Flow{{ label: {} {} }}\n",
        quote(&spec.name),
        nodes.join(", ")
    ));
    makepad_flow::graph::evaluate(&source, "creator.splash").map_err(|e| e.to_string())?;
    let mut sha = makepad_ai_hub::sha256::Sha256::new();
    sha.update(source.as_bytes());
    let name = format!(
        "creator-{}",
        &makepad_ai_hub::sha256::to_hex(&sha.finish())[..32]
    );
    Ok(CompiledCreator {
        name,
        source,
        stages,
        inputs: CreateInstanceRequest {
            label: Some(spec.name.clone()),
            inputs: Some(values),
            pin: Some(false),
        },
    })
}
