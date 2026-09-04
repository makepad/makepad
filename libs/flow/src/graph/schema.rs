use super::*;
use makepad_strict_json::Value as Json;

/// Project the flow's Input and Output nodes into callable JSON schemas.
pub fn tool_schema(graph: &Graph) -> ToolSchema {
    let mut defs = Vec::new();
    for tool in &graph.tools {
        let mut properties = Vec::new();
        let mut required = Vec::new();
        for input_id in &tool.inputs {
            let Some(node) = graph.nodes.iter().find(|node| &node.id == input_id) else {
                continue;
            };
            let ty = node.outputs.first().map(|port| port.ty).unwrap_or(PortType::Json);
            let mut schema = schema_for(ty);
            if let Some(doc) = node.doc.as_ref().or(node.label.as_ref()) {
                if let Json::Obj(fields) = &mut schema {
                    fields.push(("description".to_string(), Json::Str(doc.clone())));
                }
            }
            if let Some(default) = node.params.iter().find_map(|(name, value)| {
                (name == "default" && !matches!(value, Literal::Null)).then_some(value)
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
        let result_fields = tool
            .outputs
            .iter()
            .filter_map(|output_id| {
                let node = graph.nodes.iter().find(|node| &node.id == output_id)?;
                let ty = if node.kind == "publish" {
                    node.outputs.first().map(|output| output.ty)?
                } else {
                    node.inputs.first().map(|input| input.ty)?
                };
                Some((node.id.clone(), ty))
            })
            .collect();
        defs.push(ToolDef {
            name: tool.name.clone(),
            description: graph.brief.clone(),
            parameters,
            result_fields,
        });
    }
    ToolSchema { tools: defs }
}

fn schema_for(ty: PortType) -> Json {
    match ty {
        PortType::Text => object_type("string"),
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
    }
}

fn object_type(name: &str) -> Json {
    Json::Obj(vec![("type".to_string(), Json::Str(name.to_string()))])
}

fn literal_json(value: &Literal) -> Json {
    match value {
        Literal::Null => Json::Null,
        Literal::Bool(value) => Json::Bool(*value),
        Literal::Num(value) if value.fract() == 0.0 => Json::Int(*value as i64),
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
