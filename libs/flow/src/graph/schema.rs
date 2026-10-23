use super::*;
use makepad_strict_json::Value as Json;

/// Project a builtin-only flow's Input and Output nodes into callable JSON
/// schemas. Registered types require `tool_schema_with_registry`.
pub fn tool_schema(graph: &Graph) -> ToolSchema {
    tool_schema_with_registry(graph, &Registry::with_builtins()).expect("tool schema requires registered types")
}

pub fn tool_schema_with_registry(graph: &Graph, registry: &Registry) -> Result<ToolSchema, String> {
    let mut defs = Vec::new();
    for tool in &graph.tools {
        let mut properties = Vec::new();
        let mut required = Vec::new();
        for input_id in &tool.inputs {
            let Some(node) = graph.nodes.iter().find(|node| &node.id == input_id) else {
                continue;
            };
            let ty = node.outputs.first().map(|port| port.ty.clone()).unwrap_or(PortType::Json);
            let mut schema = match node.outputs.first().and_then(|port| port.many) {
                // An ordered input is an array of its element type, at most
                // its bound long.
                Some(bound) => Json::Obj(vec![
                    ("type".to_string(), Json::Str("array".to_string())),
                    ("items".to_string(), schema_for(&ty, registry)?),
                    ("maxItems".to_string(), Json::Int(bound as i64)),
                ]),
                None => schema_for(&ty, registry)?,
            };
            if let Some(doc) = node.doc.as_ref().or(node.label.as_ref()) {
                if let Json::Obj(fields) = &mut schema {
                    fields.push(("description".to_string(), Json::Str(doc.clone())));
                }
            }
            if let Some(default) = node.params.iter().find_map(|(name, value)| {
                (name == "value" && !matches!(value, Literal::Null)).then_some(value)
            }) {
                if let Json::Obj(fields) = &mut schema {
                    fields.push(("default".to_string(), literal_json(default)));
                }
            }
            properties.push((node.id.clone(), schema));
            required.push(Json::Str(node.id.clone()));
        }
        let parameters = Json::Obj(vec![
            ("type".to_string(), Json::Str("object".to_string())),
            ("properties".to_string(), Json::Obj(properties)),
            ("required".to_string(), Json::Arr(required)),
            ("additionalProperties".to_string(), Json::Bool(false)),
        ])
        .to_json();
        debug_assert!(makepad_strict_json::parse(parameters.as_bytes()).is_ok());
        let result_fields: Vec<_> = tool
            .outputs
            .iter()
            .filter_map(|output_id| {
                let node = graph.nodes.iter().find(|node| &node.id == output_id)?;
                let ty = if node.kind == "publish" {
                    node.outputs.first().map(|output| output.ty.clone())?
                } else {
                    node.inputs.first().map(|input| input.ty.clone())?
                };
                Some((node.id.clone(), ty))
            })
            .collect();
        for (_, ty) in &result_fields {
            registry.types().require_artifact(ty).map_err(|error| error.to_string())?;
        }
        defs.push(ToolDef {
            name: tool.name.clone(),
            description: graph.brief.clone(),
            parameters,
            result_fields,
        });
    }
    Ok(ToolSchema { tools: defs })
}

fn schema_for(ty: &PortType, registry: &Registry) -> Result<Json, String> {
    registry.types().require_artifact(ty).map_err(|error| error.to_string())?;
    Ok(match ty {
        PortType::Text => object_type("string"),
        PortType::Registered(_) => Json::Obj(vec![
            ("type".to_string(), Json::Str("string".to_string())),
            ("contentEncoding".to_string(), Json::Str("base64".to_string())),
            ("x-flow-type".to_string(), Json::Str(ty.as_str().to_string())),
        ]),
        PortType::Image | PortType::Audio | PortType::Video | PortType::Mesh | PortType::Bytes => {
            Json::Obj(vec![
                ("type".to_string(), Json::Str("string".to_string())),
                (
                    "contentEncoding".to_string(),
                    Json::Str("base64".to_string()),
                ),
            ])
        }
        PortType::Json => Json::Obj(Vec::new()),
        PortType::List => Json::Obj(vec![
            ("type".to_string(), Json::Str("array".to_string())),
            ("items".to_string(), Json::Obj(Vec::new())),
        ]),
    })
}

fn object_type(name: &str) -> Json {
    Json::Obj(vec![("type".to_string(), Json::Str(name.to_string()))])
}

fn literal_json(value: &Literal) -> Json {
    match value {
        Literal::Null => Json::Null,
        Literal::Bool(value) => Json::Bool(*value),
        Literal::Num(value) if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value < -(i64::MIN as f64) => Json::Int(*value as i64),
        Literal::Num(value) => Json::F64(*value),
        Literal::Str(value) | Literal::Id(value) => Json::Str(value.clone()),
        Literal::Arr(values) => Json::Arr(values.iter().map(literal_json).collect()),
        Literal::Obj(values) => Json::Obj(
            values
                .iter()
                .map(|(name, value)| (name.clone(), literal_json(value)))
                .collect(),
        ),
    }
}
