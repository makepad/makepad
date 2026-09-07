//! Transport bounds for editable documents. The authoring engine validates
//! operation semantics on its worker before changing a candidate document.
use super::*;

#[cfg(test)]
mod joint_budget_tests {
    use super::*;
    #[test]
    fn model_open_joint_budget_requires_a_bounded_integer() {
        for value in ["1","64","128"] {let args=json::parse(format!(r#"{{"document":"yarn","max_joints":{value}}}"#).as_bytes()).unwrap();assert!(validate(ModelDocumentTool::Open,&args).is_ok());}
        for value in ["0","129","-1","64.5","\"128\"","null","true"] {let args=json::parse(format!(r#"{{"document":"yarn","max_joints":{value}}}"#).as_bytes()).unwrap();assert!(validate(ModelDocumentTool::Open,&args).is_err(),"{value}");}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelDocumentTool { Open, Apply, Concepts, Texture, Render, Inspect, History, Close, Publish, Jobs, Cancel }

impl ModelDocumentTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Concepts => "model.concepts", Self::Open => "model.open", Self::Apply => "model.apply",
            Self::Texture => "model.texture", Self::Render => "model.render",
            Self::Inspect => "model.inspect", Self::History => "model.history",
            Self::Close => "model.close", Self::Publish => "model.publish", Self::Jobs => "model.jobs", Self::Cancel => "model.cancel",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "model.concepts" => Self::Concepts, "model.open" => Self::Open, "model.apply" => Self::Apply,
            "model.texture" => Self::Texture, "model.render" => Self::Render,
            "model.inspect" => Self::Inspect, "model.history" => Self::History,
            "model.close" => Self::Close, "model.publish" => Self::Publish, "model.jobs" => Self::Jobs, "model.cancel" => Self::Cancel,
            _ => return None,
        })
    }
}

fn identifier(args: &Value, key: &'static str) -> Result<String, String> {
    let value = need_str(args, key, 96)?;
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(format!("{key} must be nonempty text without controls"));
    }
    Ok(value)
}

fn head(value: &Value) -> Result<(), String> {
    check_known(value, &["generation", "content"], "document expected head")?;
    let generation = need_str(value, "generation", 20)?;
    if generation.parse::<u64>().ok().is_none_or(|n| n.to_string() != generation) {
        return Err("generation must be a canonical u64 decimal string".into());
    }
    let hash = need_str(value, "content", 64)?;
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("content must be a lowercase 64-character hex digest".into());
    }
    Ok(())
}

pub(super) fn validate(tool: ModelDocumentTool, args: &Value) -> Result<(), String> {
    use ModelDocumentTool::*;
    let allowed: &[&str] = match tool {
        Concepts => &["request_id","prompts","model","seed"],
        Open => &["document", "alias", "preview_scale", "max_joints"],
        Apply => &["document", "request_id", "expected", "operations", "result_mode"],
        Texture => &["document", "request_id", "expected", "material", "layer", "prompt", "model", "width", "height", "seed", "channels", "reference", "seamless", "derive_maps"],
        Inspect => &["document", "object", "domain", "offset", "limit"],
        Render => &["document", "expected", "request_id", "motion"],
        History => &["document", "expected", "action"],
        Close => &["document", "expected"],
        Publish => &["document", "title", "alias", "expected", "expected_alias", "request_id"],
        Jobs => &["job", "wait_ms"],
        Cancel => &["job"],
    };
    check_known(args, allowed, tool.name())?;
    if matches!(tool, Jobs | Cancel) {
        identifier(args, "job")?;
        if tool == Jobs && optional_u64(args, "wait_ms")?.unwrap_or(10_000) > 10_000 {
            return Err("wait_ms must be within 0..10000".into());
        }
        return Ok(());
    }
    if tool==Concepts {
        identifier(args,"request_id")?;
        let prompts=args.get("prompts").and_then(Value::as_arr).ok_or("prompts must be an array")?;
        if prompts.is_empty()||prompts.len()>3||prompts.iter().any(|p|p.as_str().is_none_or(|s|s.trim().is_empty()||s.len()>1500)){return Err("choose 1..3 nonempty prompts, each at most 1500 bytes".into());}
        if let Some(model)=optional_str(args,"model")?{if model.is_empty()||model.len()>128{return Err("invalid image model".into());}}
        if let Some(seed)=optional_str(args,"seed")?{if seed.parse::<u64>().ok().is_none_or(|n|n.to_string()!=seed){return Err("seed must be a canonical u64 decimal string".into());}}
        return Ok(());
    }
    identifier(args, "document")?;
    if tool==Open{
        if optional_u64(args,"max_joints")?.is_some_and(|n|!(1..=128).contains(&n)){return Err("max_joints must be an integer within 1..128".into());}
        if let Some(value)=args.get("preview_scale"){let n=json_num(value).ok_or("preview_scale must be numeric")?;if !n.is_finite()||!(0.001..=100.).contains(&n){return Err("preview_scale must be in 0.001..100".into());}}
    }
    if matches!(tool, Apply | Texture | Render | History | Close | Publish) {
        head(args.get("expected").ok_or("expected document head is required")?)?;
    }
    if matches!(tool, Open | Publish) {
        match args.get("alias") {
            Some(Value::Str(alias)) if alias.len() <= 128 => {
                AssetAlias::from_str(alias).map_err(|_| "alias must be a valid asset alias")?;
            }
            None if tool == Open => {}
            _ => return Err("alias must be a valid asset alias".into()),
        }
    }
    match tool {
        Render => {
            identifier(args,"request_id")?;
            if let Some(motion)=args.get("motion"){
                let samples=motion.as_arr().ok_or("motion must be an array")?;
                if samples.len()>4{return Err("motion accepts at most four samples".into());}
                for sample in samples{
                    let kind=need_str(sample,"kind",16)?;
                    match kind.as_str(){
                        "vehicle"=>{check_known(sample,&["kind","steer","suspension"],"vehicle motion")?;
                            for (key,limit) in [("steer",1.2),("suspension",5.)]{let n=sample.get(key).and_then(json_num).ok_or("vehicle motion requires finite steer and suspension")?;if !n.is_finite()||n.abs()>limit{return Err("vehicle motion exceeds angle/travel bounds".into());}}},
                        "soft_body"=>{check_known(sample,&["kind","scenario"],"soft body motion")?;let scenario=need_str(sample,"scenario",16)?;if !["acceleration","landing","wall"].contains(&scenario.as_str()){return Err("unknown soft-body review scenario".into());}},
                        "pose"=>{check_known(sample,&["kind","name"],"pose motion")?;identifier(sample,"name")?;},
                        "clip"=>{check_known(sample,&["kind","name","time"],"clip motion")?;identifier(sample,"name")?;let t=sample.get("time").and_then(json_num).ok_or("clip time is required")?;if !t.is_finite()||!(0.0..=3600.0).contains(&t){return Err("clip time exceeds bounds".into());}},
                        _=>return Err("motion kind must be vehicle, pose, clip or soft_body".into()),
                    }
                }
            }
        },
        Texture => {
            identifier(args, "request_id")?;
            identifier(args, "layer")?;
            need_str(args, "prompt", 3000)?;
            if optional_u64(args,"material")?.is_none_or(|v|v > u32::MAX as u64) { return Err("material must be a u32".into()); }
            for key in ["width", "height"] {
                let size = optional_u64(args,key)?.unwrap_or(256);
                if !(32..=1024).contains(&size) || !size.is_power_of_two() { return Err("texture sizes must be powers of two in 32..1024".into()); }
            }
            let seed = need_str(args,"seed",20)?;
            if seed.parse::<u64>().ok().is_none_or(|s|s.to_string()!=seed) { return Err("seed must be a canonical u64 decimal string".into()); }
            if let Some(channels) = args.get("channels") {
                let channels = channels.as_arr().ok_or("channels must be an array")?;
                let mut seen = std::collections::HashSet::new();
                if channels.is_empty() || channels.len()>6 { return Err("choose 1..6 texture channels".into()); }
                for value in channels {
                    let ch = value.as_str().ok_or("channel must be a string")?;
                    if !["base_color","roughness","metallic","normal","occlusion","emissive"].contains(&ch) || !seen.insert(ch) { return Err("unknown or duplicate texture channel".into()); }
                }
            }
            if let Some(reference) = optional_str(args,"reference")? { if !["current","selected"].contains(&reference) {return Err("reference must be current or selected".into());} }
            if let Some(model) = optional_str(args,"model")? { if model.len()>128 || model.trim().is_empty() {return Err("invalid image model".into());} }
            for key in ["seamless", "derive_maps"] { if args.get(key).is_some_and(|v| !matches!(v,Value::Bool(_))) {return Err(format!("{key} must be boolean"));} }
        }
        Apply => {
            identifier(args, "request_id")?;
            if let Some(mode) = optional_str(args,"result_mode")? {if !["summary","selections"].contains(&mode) {return Err("result_mode must be summary or selections".into());}}
            let ops = args.get("operations").and_then(Value::as_arr).ok_or("operations must be an array")?;
            if ops.is_empty() || ops.len() > 256 || ops.iter().any(|op| !matches!(op, Value::Obj(_))) {
                return Err("operations must contain 1..256 operation objects".into());
            }
        }
        Inspect => {
            if args.get("object").is_some() { identifier(args, "object")?; }
            if let Some(domain) = optional_str(args, "domain")? {
                if !["overview", "objects", "materials", "joints", "clips", "clip_keys", "vertices", "weights", "faces", "corners", "edges", "scene_nodes", "modifiers", "modifier_values", "lights", "sockets", "vehicle_wheels", "colliders", "lods", "surface_materials", "surface_layers", "surface_pixels", "vertex_colors", "rig_rests", "poses", "pose_joints", "constraints", "morphs", "morph_deltas", "clip_options", "clip_events", "clip_event_payload", "morph_keys", "weight_locks", "uv_islands", "uv_island_faces", "uv_pins", "solid", "selections", "selection_vertices", "selection_faces"].contains(&domain) {
                    return Err("unknown model inspection domain".into());
                }
            }
            let offset = optional_u64(args, "offset")?.unwrap_or(0);
            let limit = optional_u64(args, "limit")?.unwrap_or(32);
            if offset > u32::MAX as u64 || !(1..=128).contains(&limit) {
                return Err("inspection offset or limit out of bounds".into());
            }
        }
        History => {
            if !["undo", "redo", "checkpoint"].contains(&need_str(args, "action", 16)?.as_str()) {
                return Err("history action must be undo, redo or checkpoint".into());
            }
        }
        Publish => {
            identifier(args, "title")?;
            if args.get("request_id").is_some() { identifier(args, "request_id")?; }
            let expected = args.get("expected_alias").ok_or("expected_alias is required")?;
            if expected.as_str() != Some("absent") {
                check_known(expected, &["asset_id", "revision"], "expected alias")?;
                AssetId::from_str(&need_str(expected, "asset_id", 128)?).map_err(|_| "invalid expected asset_id")?;
                AssetRevisionId::from_str(&need_str(expected, "revision", 128)?).map_err(|_| "invalid expected revision")?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    #[test]
    fn jobs_wait_is_bounded_and_cancel_cannot_wait() {
        for wait in [None, Some(Value::Int(0)), Some(Value::Int(10_000))] {
            let mut fields = vec![("job", json::s("edit_1"))];
            if let Some(wait) = wait { fields.push(("wait_ms", wait)); }
            assert!(ContentToolCall::parse("model.jobs", &json::obj(fields)).is_ok());
        }
        for wait in [Value::Int(10_001), Value::Int(-1), Value::F64(1.5), json::s("1000"), Value::Bool(true)] {
            assert!(ContentToolCall::parse("model.jobs", &json::obj(vec![("job", json::s("edit_1")), ("wait_ms", wait)])).is_err());
        }
        assert!(ContentToolCall::parse("model.cancel", &json::obj(vec![("job", json::s("edit_1")), ("wait_ms", Value::Int(0))])).is_err());
    }
}

fn schema_bool(description: &str) -> Value {
    json::obj(vec![("type",json::s("boolean")),("description",json::s(description))])
}

fn head_schema() -> Value {
    schema_object(vec![
        ("generation", schema_string_len("document generation as a decimal u64 string", 1, 20)),
        ("content", schema_string_len("document content digest, 64 lowercase hex characters", 64, 64)),
    ], &["generation", "content"], Some(false))
}

pub(super) fn definitions() -> Vec<ToolDef> {
    let doc = || ("document", schema_string_len("local editable document name", 1, 96));
    let expected = || ("expected", head_schema());
    let alias = || ("alias", schema_string_len("published asset alias", 1, 128));
    vec![
        ToolDef {
            name:"model.concepts",api_name:"model_concepts",
            description:"Present 1..3 generated concept images in an in-chat picker before modeling. Starts all requests concurrently through AIHub (default flux1-schnell, the four-step FLUX.1 Fast model), with independent live progress and cancellation. Distinct seeds are assigned. Returns immediately; wait for the user's choice. Choosing an image sends the actual bitmap as Astra's next modeling reference. Ephemeral, no asset publication. Never call model.jobs for a gallery.",
            args_doc:r#"{"request_id":"old-cars","prompts":["1930s weathered saloon, isolated three-quarter view","1950s rusty pickup, isolated three-quarter view","1970s faded station wagon, isolated three-quarter view"],"model":"flux1-schnell","seed":"42"}"#,
            parameters:schema_object(vec![("request_id",schema_ident("retry identity")),("prompts",schema_array_bounded("distinct concept prompts",1,3,schema_string_len("concept description",1,1500))),("model",schema_string_len("AIHub model; default flux1-schnell",1,128)),("seed",schema_string_len("canonical u64 seed; following images use consecutive seeds",1,20))],&["request_id","prompts"],Some(false)),
        },
        ToolDef {
            name: "model.open", api_name: "model_open",
            description: "Create an empty editable polygon document, or reopen a published binary source bundle by alias. Returns its exact head and inventory. At most four documents are open. For original modeling, discover world.api model.workflow once; request deeper contracts only for missing operations. Legacy CSG uses model.fetch/build. Do not invent the returned head.",
            args_doc: r#"{"document":"chair","alias":"gen/models/chair"}"#,
            parameters: schema_object(vec![doc(), alias(), ("max_joints",schema_integer_range("explicit document joint budget; default64; use128 for soft bodies; immutable while open",1,128)), ("preview_scale",schema_number("presentation only; default 1 metre/unit; smaller only for user-requested handheld/XR preview"))], &["document"], Some(false)),
        },
        ToolDef {
            name: "model.apply", api_name: "model_apply",
            description: "Apply an atomic worker batch at the expected head. Sandbox waits up to 10s and returns applied/result.head or failed; query model.jobs only for pending jobs. Chat defaults to summary; request selections only for needed IDs. Start with a small visible silhouette batch, then stages of roughly 8–24 operations/4KiB so the construction cage updates often. Keep dependent operations together; hard limit 256. Same request_id+arguments replay. Use result.head for the next stage and named select/use_selection for whole-object edits. Discover model.workflow once.",
            args_doc: r#"{"document":"chair","request_id":"build-seat","expected":{"generation":"0","content":"0000000000000000000000000000000000000000000000000000000000000000"},"operations":[{"op":"cube","object":"seat","size":[1,0.2,1]}]}"#,
            parameters: schema_object(vec![doc(), ("request_id", schema_string_len("stable transaction request id", 1, 96)), expected(),
                ("result_mode",schema_string_enum("chat default summary; selections returns bounded per-operation IDs",&["summary","selections"])),
                ("operations", schema_array_bounded("typed engine operations", 1, 256, schema_object(vec![], &[], Some(true))))],
                &["document", "request_id", "expected", "operations"], Some(false)),
        },
        ToolDef {
            name:"model.texture", api_name:"model_texture",
            description:"Generate material textures through AIHub on a worker and apply them atomically at the expected head. Returns an accepted model.jobs job; pixels never enter chat. Default generates base color and derives roughness/normal detail; derive_maps:false uses image generation for every channel. reference:current edits the current base texture via an image-edit model. Preview updates before publication. Discover model.texture for map conventions and model selection.",
            args_doc:r#"{"document":"car","request_id":"paint-1","expected":{"generation":"1","content":"0000000000000000000000000000000000000000000000000000000000000000"},"material":1,"layer":"aged-paint","prompt":"worn green automotive paint","seed":"17"}"#,
            parameters:schema_object(vec![doc(),expected(),("request_id",schema_string_len("stable request identity",1,96)),
                ("material",schema_integer_range("existing material",0,u32::MAX as i64)),("layer",schema_string_len("editable layer name",1,96)),
                ("prompt",schema_string_len("material description; no baked lighting",1,3000)),("model",schema_string_len("optional exact AIHub image/edit model",1,128)),
                ("seed",schema_string_len("u64 decimal string",1,20)),("width",schema_integer_range("power of two, default256",32,1024)),("height",schema_integer_range("power of two, default256",32,1024)),
                ("channels",schema_array_bounded("default base_color,roughness,normal",1,6,schema_string_enum("map",&["base_color","roughness","metallic","normal","occlusion","emissive"]))),
                ("reference",schema_string_enum("edit current base texture",&["current","selected"])),("seamless",schema_bool("enforce tile boundary continuity; defaulttrue")),
                ("derive_maps",schema_bool("derive secondary maps from the generated color; defaulttrue"))],
                &["document","request_id","expected","material","layer","prompt","seed"],Some(false)),
        },
        ToolDef {
            name:"model.render",api_name:"model_render",
            description:"Render the exact head: eight rest PBR views (two sheets) plus up to four moving poses. Start with elevated45-degree front AND rear three-quarter views to assess volume and surface joins; cardinal silhouettes alone can hide wrong depth. Cars default to steering/suspension samples; rigs use authored poses/clips. Optional motion overrides samples. Returns an accepted model.jobs job. The completed job attaches actual rendered images to Astra separately from JSON, with objective topology checks. Review and repair defects before publication. Unsupported image delivery/readback fails honestly; tool success alone is not visual approval.",
            args_doc:r#"{"document":"car","expected":{"generation":"1","content":"0000000000000000000000000000000000000000000000000000000000000000"},"request_id":"review-1"}"#,
            parameters:schema_object(vec![doc(),expected(),("request_id",schema_string_len("stable review identity",1,96)),
                ("motion",schema_array_bounded("optional explicit samples; default automatic",0,4,schema_object(vec![
                    ("kind",schema_string_enum("motion sample",&["vehicle","pose","clip","soft_body"])),("steer",schema_number("vehicle angle radians, +/-1.2 max")),
                    ("suspension",schema_number("vehicle wheel displacement model metres, +/-5 max")),("name",schema_string_len("authored pose or clip",1,96)),("time",schema_number("clip seconds")),("scenario",schema_string_enum("real soft-body solver diagnostic",&["acceleration","landing","wall"]))],&["kind"],Some(false))))],&["document","expected","request_id"],Some(false)),
        },
        ToolDef {
            name: "model.inspect", api_name: "model_inspect",
            description: "Inspect document/mesh state and paged scene, surface, rig, UV and selection resources. UV/solid and mesh domains need object; inventories do not. Use world.api model.inspect for paging details. IDs are decimal strings. Follow next_offset: the byte budget may emit fewer rows than the requested limit. This does not publish or place the model.",
            args_doc: r#"{"document":"chair","object":"seat","domain":"faces","offset":0,"limit":32}"#,
            parameters: schema_object(vec![doc(), ("object", schema_string_len("object name", 1, 96)),
                ("domain", schema_string_enum("inspection domain", &["overview", "objects", "materials", "joints", "clips", "clip_keys", "vertices", "weights", "faces", "corners", "edges", "scene_nodes", "modifiers", "modifier_values", "lights", "sockets", "vehicle_wheels", "colliders", "lods", "surface_materials", "surface_layers", "surface_pixels", "vertex_colors", "rig_rests", "poses", "pose_joints", "constraints", "morphs", "morph_deltas", "clip_options", "clip_events", "clip_event_payload", "morph_keys", "weight_locks", "uv_islands", "uv_island_faces", "uv_pins", "solid", "selections", "selection_vertices", "selection_faces"])),
                ("offset", schema_integer_range("first element", 0, u32::MAX as i64)), ("limit", schema_integer_range("page size", 1, 128))], &["document"], Some(false)),
        },
        ToolDef {
            name: "model.history", api_name: "model_history",
            description: "Undo, redo, or checkpoint an editable document at its expected head. The generation stays monotonic even when undo restores old content. Returns the resulting head; this does not change a published alias.",
            args_doc: r#"{"document":"chair","expected":{"generation":"1","content":"0000000000000000000000000000000000000000000000000000000000000000"},"action":"undo"}"#,
            parameters: schema_object(vec![doc(), expected(), ("action", schema_string_enum("history action", &["undo", "redo", "checkpoint"]))], &["document", "expected", "action"], Some(false)),
        },
        ToolDef {
            name: "model.close", api_name: "model_close",
            description: "Close the local editable document at an expected head, releasing its authoring resources. Publish first to retain edits in the shared asset store.",
            args_doc: r#"{"document":"chair","expected":{"generation":"1","content":"0000000000000000000000000000000000000000000000000000000000000000"}}"#,
            parameters: schema_object(vec![doc(), expected()], &["document", "expected"], Some(false)),
        },
        ToolDef {
            name: "model.publish", api_name: "model_publish",
            description: "Compile and publish the exact expected document head as one complete source/GLB/thumbnail bundle. expected_alias is the literal string absent for creation or the exact asset_id/revision previously read. Returns accepted plus job ID; model.jobs must report state published with result.placeable_now:true before world.spawn uses result.alias (form:car for a custom vehicle). Conflicts preserve the local document; creation does not place it.",
            args_doc: r#"{"document":"chair","title":"Chair","alias":"gen/models/chair","expected":{"generation":"1","content":"0000000000000000000000000000000000000000000000000000000000000000"},"expected_alias":"absent","request_id":"publish-chair-1"}"#,
            parameters: schema_object(vec![doc(), ("title", schema_string_len("model title", 1, 96)), alias(), expected(),
                ("request_id", schema_string_len("optional stable publish attempt id", 1, 96)),
                ("expected_alias", json::obj(vec![("oneOf", Value::Arr(vec![schema_string_enum("new alias", &["absent"]),
                    schema_object(vec![("asset_id", schema_string("expected stable asset id")), ("revision", schema_string("expected store revision"))], &["asset_id", "revision"], Some(false))]))]))],
                &["document", "title", "alias", "expected", "expected_alias"], Some(false)),
        },
        ToolDef {
            name: "model.cancel", api_name: "model_cancel",
            description: "Cancel an accepted editable-model job. Queued work is removed; running work receives cooperative cancellation. A store commit already in progress is too late to cancel. Repeating cancellation is idempotent; query model.jobs for the final result. A cancel request alone does not mean an edit was rolled back.",
            args_doc: r#"{"job":"edit_1"}"#,
            parameters: schema_object(vec![("job", schema_string_len("accepted job id", 1, 96))], &["job"], Some(false)),
        },
        ToolDef {
            name: "model.jobs", api_name: "model_jobs",
            description: "Await an accepted editable-model job; use wait_ms:10000 for bounded waiting on the chat worker. Repeat while queued/running/published_installing. Terminal applied contains result.head; published contains result.alias and result.placeable_now. Read error on failed/cancelled/publish_pending/conflict. Acceptance alone is not a completed edit or shared asset; never finish a build order at acceptance.",
            args_doc: r#"{"job":"edit_1"}"#,
            parameters: schema_object(vec![("job", schema_string_len("accepted job id", 1, 96)),("wait_ms",schema_integer_range("wait on the chat worker for a terminal result; default10000, zero returns current state",0,10000))], &["job"], Some(false)),
        },
    ]
}
