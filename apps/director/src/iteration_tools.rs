//! Tool transport adapters. All mutations use the same worker as Studio's UI.
use crate::{iteration, iteration_worker::Request};
use makepad_ai_services::wire::{Risk, ServiceCall, ToolDef};
use makepad_strict_json::{self as json, Value};

pub fn tool_defs() -> Vec<ToolDef> {
    let mut tools = iteration::tool_defs();
    for (name, description, names, risk) in [
        ("flow_git_inspect", "Inspect the flow's private local branch and source changes. Local is never pushed.", "flow", Risk::Read),
        ("flow_promotion_preview", "Preview a squash: local to work for a feature, or work to dev for a categorized milestone. Returns exact source/target hashes and merged tree. No branch moves or publication.", "flow,target", Risk::Read),
        ("flow_promote", "Apply a reviewed squash only if exact source/target hashes still match and the merged tree passed validation. Dirty or foreign destination checkouts are refused. Never publishes private ancestry or pushes anything.", "flow,target,source_oid,target_oid,title", Risk::Act),
        ("flow_sync_preview", "Preview directional incoming sync. source is a full public ref (for example refs/remotes/origin/work or refs/heads/dev); target is work/dev or this flow's local branch. Private ancestry must never enter public tiers.", "flow,source,target", Risk::Read),
        ("flow_sync", "Apply the exact reviewed incoming sync into an owned clean checkout. Requires unchanged source/target hashes; never rewrites public history or discards dirty files.", "flow,source,target,source_oid,target_oid", Risk::Act),
        ("flow_fetch", "Fetch only origin/work and origin/dev. Does not merge, change the current app, or push a branch.", "flow", Risk::Act),
        ("flow_diff", "Inspect the upcoming unbuilt source diff, or an immutable artifact's source changes against the previous artifact. Does not check out old source or start a build.", "flow,artifact?", Risk::Read),
    ] {
        let fields:Vec<_> = names.split(',').collect();
        let properties=fields.iter().map(|name| {
            let key=name.trim_end_matches('?');
            (key.to_owned(),json::obj(vec![("type",json::s("string")),("maxLength",Value::Int(if key=="title"{240}else{256}))]))
        }).collect();
        let required=fields.iter().filter(|name|!name.ends_with('?')).map(|name| json::s(*name)).collect();
        let schema=json::obj(vec![("type",json::s("object")),("properties",Value::Obj(properties)),("required",Value::Arr(required)),("additionalProperties",Value::Bool(false))]);
        tools.push(ToolDef::new(name,description,&schema.to_json(),risk));
    }
    let mut properties = vec![];
    for (name, max) in [
        ("flow", 256),
        ("artifact_id", 256),
        ("run_id", 256),
        ("operation_id", 256),
        ("query", 256),
        ("key", 32),
        ("text", 4096),
        ("demo", 120),
    ] {
        properties.push((
            name.to_owned(),
            json::obj(vec![
                ("type", json::s("string")),
                ("maxLength", Value::Int(max)),
            ]),
        ));
    }
    for (name, values) in [
        (
            "action",
            vec!["start", "input", "snapshot", "stop", "status"],
        ),
        (
            "input",
            vec![
                "click",
                "move",
                "down",
                "up",
                "scroll",
                "key_press",
                "key_down",
                "key_up",
                "text",
            ],
        ),
    ] {
        properties.push((
            name.to_owned(),
            json::obj(vec![
                ("type", json::s("string")),
                (
                    "enum",
                    Value::Arr(values.into_iter().map(json::s).collect()),
                ),
            ]),
        ));
    }
    for name in ["x", "y", "dx", "dy"] {
        properties.push((
            name.to_owned(),
            json::obj(vec![("type", json::s("number"))]),
        ));
    }
    for name in ["window", "button"] {
        properties.push((
            name.to_owned(),
            json::obj(vec![
                ("type", json::s("integer")),
                ("minimum", Value::Int(0)),
                (
                    "maximum",
                    Value::Int(if name == "button" { 2 } else { u32::MAX.into() }),
                ),
            ]),
        ));
    }
    for name in ["shift", "ctrl", "alt", "cmd"] {
        properties.push((
            name.to_owned(),
            json::obj(vec![("type", json::s("boolean"))]),
        ));
    }
    let schema = json::obj(vec![
        ("type", json::s("object")),
        ("properties", Value::Obj(properties)),
        (
            "required",
            Value::Arr(vec![json::s("flow"), json::s("action")]),
        ),
        ("additionalProperties", Value::Bool(false)),
    ]);
    tools.push(ToolDef::new("flow_test", "Test a retained artifact in an owned hidden app with recording tiles. start requires artifact_id, a human-closed lane and no pending build; optional demo names a demonstration (1-120 characters, no controls, start only). input uses run_id and mouse x/y, key or text. snapshot saves widget rectangles and PNG; stop finalizes this test's MP4. Poll status with run_id/operation_id before another input; status lists results. Demo titles survive restart. Never controls human apps or infers pass or acceptance.", &schema.to_json(), Risk::Act));
    tools
}
pub fn handles(name: &str) -> bool {
    iteration::handles(name)
        || matches!(
            name,
            "flow_git_inspect"
                | "flow_promotion_preview"
                | "flow_promote"
                | "flow_sync_preview"
                | "flow_sync"
                | "flow_fetch"
                | "flow_diff"
                | "flow_test"
        )
}
pub fn parse(call: &ServiceCall) -> Result<Request, String> {
    if iteration::handles(&call.tool) {
        return iteration::parse(call).map(Request::Flow);
    }
    if call.args.len() > 16 * 1024 {
        return Err("Flow tool arguments exceed their limit".into());
    }
    let args = json::parse(call.args.as_bytes()).map_err(str::to_owned)?;
    if call.tool == "flow_test" {
        let action = crate::iteration_worker::parse_test_request(&args)?;
        let flow = args
            .get("flow")
            .and_then(Value::as_str)
            .ok_or("flow must be a string")?
            .to_owned();
        return Ok(Request::Test { flow, action });
    }
    let Value::Obj(fields) = &args else {
        return Err("Arguments must be an object".into());
    };
    let allowed: &[&str] = match call.tool.as_str() {
        "flow_git_inspect" | "flow_fetch" => &["flow"],
        "flow_promotion_preview" => &["flow", "target"],
        "flow_promote" => &["flow", "target", "source_oid", "target_oid", "title"],
        "flow_sync_preview" => &["flow", "source", "target"],
        "flow_sync" => &["flow", "source", "target", "source_oid", "target_oid"],
        "flow_diff" => &["flow", "artifact"],
        _ => return Err("Unknown flow operation".into()),
    };
    if fields
        .iter()
        .any(|(name, _)| !allowed.contains(&name.as_str()))
    {
        return Err("Unknown flow argument".into());
    }
    let text = |name: &str| -> Result<String, String> {
        let value = args
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} must be a string"))?;
        if value.trim().is_empty()
            || value.len() > 256
            || value.contains('\0')
            || value.contains('\n')
        {
            return Err(format!("Invalid {name}"));
        }
        Ok(value.into())
    };
    let flow = text("flow")?;
    Ok(match call.tool.as_str() {
        "flow_git_inspect" => Request::GitInspect { flow },
        "flow_promotion_preview" => Request::GitPreview {
            flow,
            target: text("target")?,
        },
        "flow_promote" => Request::GitApply {
            flow,
            target: text("target")?,
            source: text("source_oid")?,
            destination: text("target_oid")?,
            title: text("title")?,
        },
        "flow_sync_preview" => Request::SyncPreview {
            flow,
            source: text("source")?,
            target: text("target")?,
        },
        "flow_sync" => Request::SyncApply {
            flow,
            source: text("source")?,
            target: text("target")?,
            source_oid: text("source_oid")?,
            target_oid: text("target_oid")?,
        },
        "flow_fetch" => Request::Fetch { flow },
        "flow_diff" => Request::Diff {
            flow,
            artifact: if args.get("artifact").is_some() {
                Some(text("artifact")?)
            } else {
                None
            },
        },
        _ => return Err("Unknown flow operation".into()),
    })
}
