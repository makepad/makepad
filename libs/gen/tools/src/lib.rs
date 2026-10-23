//! The `gen.*` tools' text and shape: one definition and one guide, byte
//! for byte the same in every app that offers generation (Sandbox's game
//! chat, Stage's studio API), their argument parser and the JSON they
//! answer with. Dependency-free (JSON only), so a light registry such as
//! an MCP bridge can list them; running them is `makepad-gen`'s.

use makepad_strict_json::Value as Json;

/// One tool as every registry declares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenToolDef {
    /// The dotted canonical name.
    pub name: &'static str,
    /// The underscore spelling for function-calling APIs.
    pub api_name: &'static str,
    pub description: &'static str,
    /// The arguments' JSON Schema.
    pub input: &'static str,
}

pub const PRESETS: GenToolDef = GenToolDef {
    name: "gen.presets",
    api_name: "gen_presets",
    description: "The generation presets the AI Hub fleet runs, the same list in every app: each preset's id, kind (image, video, audio, mesh, character, world), what it takes (a prompt, input assets, params with their defaults), which models it pins and which the LAN has ready now, and a rough ETA. Call it before gen.run to pick a preset and see its params.",
    input: r#"{"type": "object", "properties": {"kind": {"type": "string", "enum": ["image", "video", "audio", "mesh", "character", "world"], "description": "only presets that make this"}}}"#,
};

pub const RUN: GenToolDef = GenToolDef {
    name: "gen.run",
    api_name: "gen_run",
    description: "Run a preset on the AI Hub fleet. Everything it makes is published into this app's library and comes back as an asset id and revision (a character also says whether it is skinned and playable, and its clips). With `wait` (the default) the call returns when the job ends, or with the job id when it outlasts the wait budget: follow it with gen.job. A given `seed` makes the run repeatable: the same preset, prompt, inputs, params and seed return the stored result again without running a job.",
    input: r#"{"type": "object", "properties": {"preset": {"type": "string", "minLength": 1, "description": "a preset id from gen.presets"}, "prompt": {"type": "string", "description": "what to make (a line to speak, for speech)"}, "inputs": {"type": "array", "items": {"type": "string", "description": "an asset id or alias from the library"}, "description": "the preset's input assets, in the order gen.presets lists them"}, "params": {"type": "object", "additionalProperties": true, "properties": {}, "description": "preset params by name, e.g. {\"seconds\": 8} or {\"lyrics\": \"...\"}; unknown names are refused"}, "seed": {"type": "integer", "minimum": 0, "description": "a fixed seed: repeatable, and reused when the same run is asked again"}, "wait": {"type": "boolean", "description": "wait for the result (default true)"}}, "required": ["preset"]}"#,
};

pub const JOB: GenToolDef = GenToolDef {
    name: "gen.job",
    api_name: "gen_job",
    description: "A generation job's state: phase (running, choosing, done, failed, cancelled), progress and stage, the assets it published, and a choice waiting for gen.choose. With `wait` it waits for the job to end or stop at a choice.",
    input: r#"{"type": "object", "properties": {"id": {"type": "string", "minLength": 1, "description": "the job id gen.run returned"}, "wait": {"type": "boolean", "description": "wait for the job to end (default false)"}}, "required": ["id"]}"#,
};

pub const CHOOSE: GenToolDef = GenToolDef {
    name: "gen.choose",
    api_name: "gen_choose",
    description: "Answer a job waiting at a choice (best of N) with candidate `pick`.",
    input: r#"{"type": "object", "properties": {"id": {"type": "string", "minLength": 1, "description": "the job id"}, "pick": {"type": "integer", "minimum": 0, "description": "the chosen candidate"}}, "required": ["id", "pick"]}"#,
};

pub const CANCEL: GenToolDef = GenToolDef {
    name: "gen.cancel",
    api_name: "gen_cancel",
    description: "Cancel a generation job: it stops on the fleet and publishes nothing more.",
    input: r#"{"type": "object", "properties": {"id": {"type": "string", "minLength": 1, "description": "the job id"}}, "required": ["id"]}"#,
};

pub const ESTIMATE: GenToolDef = GenToolDef {
    name: "gen.estimate",
    api_name: "gen_estimate",
    description: "How long a preset would take on the fleet now: per generator its model, the node that would run it and the ETA, and which generators no node can run.",
    input: r#"{"type": "object", "properties": {"preset": {"type": "string", "minLength": 1, "description": "a preset id"}, "params": {"type": "object", "additionalProperties": true, "properties": {}, "description": "params as gen.run takes them"}}, "required": ["preset"]}"#,
};

pub const TOOLS: [GenToolDef; 6] = [PRESETS, RUN, JOB, CHOOSE, CANCEL, ESTIMATE];

/// The generation chapter of every app's guide; each app appends its own
/// placement addendum.
pub const GUIDE: &str = "\nGENERATION: gen.presets lists what the AI Hub fleet makes, the same in every app: image (pictures, posters, infographics, menu screens), image.edit, image.upscale, image.matte (cut out), image.depth, video (a clip from a prompt: cutscenes, menu plates), video.from_image, video.enhance, sound (a sound effect to length), music (instrumental, to length), song (sung lyrics), speech (a spoken line), mesh (from a picture), mesh.paint, prop (a 3D object from a prompt), character (a rigged, animated, playable character at full quality) and world (a walkable splat scene). gen.run {preset, prompt, inputs, params, seed} runs one and publishes every result into the library, answering with asset ids and revisions to place. Long jobs (video, song, character, world take minutes): say the ETA first (gen.estimate), and when gen.run returns a job id instead of a result, follow it with gen.job {id, wait: true}. Reuse ids you already have instead of generating the same thing twice; a fixed seed makes a run repeatable and a repeated run returns the stored result. A job that stops at a choice waits for gen.choose; gen.cancel stops one.\n";

/// What a job has done so far, as both apps report it.
#[derive(Clone, Debug, Default)]
pub struct JobView {
    pub id: String,
    pub preset: String,
    pub phase: Phase,
    /// 0..1, of the job's planned work.
    pub progress: f32,
    pub stage: String,
    pub seconds: f64,
    pub seed: Option<u64>,
    pub assets: Vec<AssetResult>,
    /// A character's facts (skinned, animated, clips, playable).
    pub character: Option<Json>,
    /// Text results, by output.
    pub text: Vec<(String, String)>,
    /// The result was already in the library: no job ran.
    pub reused: bool,
    pub error: Option<String>,
    pub choice: Option<Choice>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Running,
    Choosing,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Running => "running",
            Phase::Choosing => "choosing",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }

    pub fn ended(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::Cancelled)
    }
}

/// One published product.
#[derive(Clone, Debug, Default)]
pub struct AssetResult {
    /// The recipe node that published it (`published` is the primary one).
    pub node: String,
    pub asset_id: String,
    pub revision: String,
    pub alias: Option<String>,
    pub kind: String,
    pub title: String,
}

#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub node: String,
    pub question: String,
    pub candidates: usize,
}

fn s(text: impl Into<String>) -> Json {
    Json::Str(text.into())
}

/// A job as the tools answer it.
pub fn job_json(job: &JobView) -> Json {
    let mut fields = vec![
        ("id".to_string(), s(&job.id)),
        ("preset".to_string(), s(&job.preset)),
        ("phase".to_string(), s(job.phase.as_str())),
    ];
    if !job.phase.ended() {
        fields.push(("progress".into(), Json::F64((job.progress.clamp(0.0, 1.0) * 100.0).round() as f64 / 100.0)));
        if !job.stage.is_empty() {
            fields.push(("stage".into(), s(&job.stage)));
        }
    }
    fields.push(("seconds".into(), Json::F64((job.seconds * 10.0).round() / 10.0)));
    if let Some(seed) = job.seed {
        fields.push(("seed".into(), Json::Int(seed.min(i64::MAX as u64) as i64)));
    }
    if !job.assets.is_empty() {
        fields.push(("assets".into(), Json::Arr(job.assets.iter().map(|asset| {
            let mut entry = vec![
                ("asset_id".to_string(), s(&asset.asset_id)),
                ("revision".to_string(), s(&asset.revision)),
                ("kind".to_string(), s(&asset.kind)),
                ("stage".to_string(), s(&asset.node)),
            ];
            if let Some(alias) = &asset.alias {
                entry.push(("alias".into(), s(alias)));
            }
            if !asset.title.is_empty() {
                entry.push(("title".into(), s(&asset.title)));
            }
            Json::Obj(entry)
        }).collect())));
    }
    if let Some(character) = &job.character {
        fields.push(("character".into(), character.clone()));
    }
    if !job.text.is_empty() {
        fields.push(("text".into(), Json::Obj(job.text.iter().map(|(name, text)| (name.clone(), s(text))).collect())));
    }
    if job.reused {
        fields.push(("reused".into(), Json::Bool(true)));
    }
    if let Some(error) = &job.error {
        fields.push(("error".into(), s(error)));
    }
    if let Some(choice) = &job.choice {
        fields.push(("choice".into(), Json::Obj(vec![
            ("node".to_string(), s(&choice.node)),
            ("question".to_string(), s(&choice.question)),
            ("candidates".to_string(), Json::Int(choice.candidates as i64)),
        ])));
    }
    Json::Obj(fields)
}

/// One tool call, parsed.
#[derive(Clone, Debug, PartialEq)]
pub enum GenCall {
    Presets { kind: Option<String> },
    Run { request: RunRequest, wait: bool },
    Job { id: String, wait: bool },
    Choose { id: String, pick: usize },
    Cancel { id: String },
    Estimate { preset: String, params: Vec<(String, Json)> },
}

/// What `gen.run` asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunRequest {
    pub preset: String,
    pub prompt: Option<String>,
    pub inputs: Vec<String>,
    pub params: Vec<(String, Json)>,
    pub seed: Option<u64>,
}

fn fields<'a>(args: &'a Json, allowed: &[&str]) -> Result<&'a [(String, Json)], String> {
    let Json::Obj(fields) = args else { return Err("arguments must be an object".into()) };
    if let Some((name, _)) = fields.iter().find(|(name, _)| !allowed.contains(&name.as_str())) {
        return Err(format!("unknown argument `{}`", name.chars().take(48).collect::<String>()));
    }
    Ok(fields)
}

fn get<'a>(fields: &'a [(String, Json)], name: &str) -> Option<&'a Json> {
    fields.iter().find(|(key, _)| key == name).map(|(_, value)| value).filter(|value| !matches!(value, Json::Null))
}

fn text(fields: &[(String, Json)], name: &str, max: usize) -> Result<Option<String>, String> {
    match get(fields, name) {
        None => Ok(None),
        Some(Json::Str(text)) if text.len() <= max => Ok(Some(text.clone())),
        Some(Json::Str(_)) => Err(format!("`{name}` is longer than {max} bytes")),
        Some(_) => Err(format!("`{name}` must be a string")),
    }
}

fn required(fields: &[(String, Json)], name: &str, max: usize) -> Result<String, String> {
    text(fields, name, max)?.filter(|text| !text.trim().is_empty()).ok_or_else(|| format!("`{name}` is required"))
}

fn flag(fields: &[(String, Json)], name: &str, default: bool) -> Result<bool, String> {
    match get(fields, name) {
        None => Ok(default),
        Some(Json::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("`{name}` must be true or false")),
    }
}

fn count(fields: &[(String, Json)], name: &str) -> Result<Option<u64>, String> {
    match get(fields, name) {
        None => Ok(None),
        Some(Json::Int(value)) if *value >= 0 => Ok(Some(*value as u64)),
        Some(Json::F64(value)) if value.fract() == 0.0 && *value >= 0.0 && *value < 9.0e15 => Ok(Some(*value as u64)),
        Some(_) => Err(format!("`{name}` must be a non-negative integer")),
    }
}

fn params(fields: &[(String, Json)]) -> Result<Vec<(String, Json)>, String> {
    match get(fields, "params") {
        None => Ok(Vec::new()),
        Some(Json::Obj(params)) if params.len() <= 32 => Ok(params.clone()),
        Some(Json::Obj(_)) => Err("at most 32 params".into()),
        Some(_) => Err("`params` must be an object".into()),
    }
}

/// Parse one call by its dotted or underscore name. The parser is the
/// arguments' boundary: unknown fields and wrong types are refused.
pub fn parse(name: &str, args: &Json) -> Result<GenCall, String> {
    let name = TOOLS.iter().find(|tool| tool.name == name || tool.api_name == name).map(|tool| tool.name)
        .ok_or_else(|| format!("unknown generation tool `{}`", name.chars().take(48).collect::<String>()))?;
    Ok(match name {
        "gen.presets" => {
            let fields = fields(args, &["kind"])?;
            GenCall::Presets { kind: text(fields, "kind", 32)? }
        }
        "gen.run" => {
            let fields = fields(args, &["preset", "prompt", "inputs", "params", "seed", "wait"])?;
            let inputs = match get(fields, "inputs") {
                None => Vec::new(),
                Some(Json::Arr(items)) if items.len() <= 8 => items.iter().map(|item| match item {
                    Json::Str(id) if !id.trim().is_empty() && id.len() <= 256 => Ok(id.trim().to_string()),
                    _ => Err("`inputs` holds asset ids or aliases".to_string()),
                }).collect::<Result<_, _>>()?,
                Some(_) => Err("`inputs` must be a list of at most 8 asset ids".to_string())?,
            };
            GenCall::Run {
                request: RunRequest {
                    preset: required(fields, "preset", 64)?,
                    prompt: text(fields, "prompt", 8_000)?,
                    inputs,
                    params: params(fields)?,
                    seed: count(fields, "seed")?,
                },
                wait: flag(fields, "wait", true)?,
            }
        }
        "gen.job" => {
            let fields = fields(args, &["id", "wait"])?;
            GenCall::Job { id: required(fields, "id", 64)?, wait: flag(fields, "wait", false)? }
        }
        "gen.choose" => {
            let fields = fields(args, &["id", "pick"])?;
            GenCall::Choose {
                id: required(fields, "id", 64)?,
                pick: count(fields, "pick")?.ok_or("`pick` is required")? as usize,
            }
        }
        "gen.cancel" => {
            let fields = fields(args, &["id"])?;
            GenCall::Cancel { id: required(fields, "id", 64)? }
        }
        _ => {
            let fields = fields(args, &["preset", "params"])?;
            GenCall::Estimate { preset: required(fields, "preset", 64)?, params: params(fields)? }
        }
    })
}
