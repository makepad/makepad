//! The content-only tool allowlist: typed calls the LLM may make, parsed
//! fail-closed from the raw `(name, args)` a provider emitted. This is the
//! ENTIRE surface — there is no way to express "enqueue a raw job",
//! "publish", "move an alias", "read a path", or "run a command", so those
//! threats are unrepresentable rather than filtered. Asset mutation happens
//! ONLY through typed, server-validated operations whose publication policy
//! is validated at creation and executed by the server-side finalizer.
//!
//! Availability is a separate, honest layer: `operation.create` names a
//! registered operation kind, and the dispatcher answers
//! `ToolOutcome::Unavailable` when the server reports no live worker for
//! it. Parsing here only enforces shape and bounds.

use crate::wire::{ident_ok, ProviderKind, MAX_MESSAGE_BYTES, MAX_TOOL_JSON_BYTES, MAX_TRANSFORM_INPUTS};
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::OperationId;
use makepad_asset_data::{AssetAlias, AssetId, AssetRevisionId};
use std::str::FromStr;

/// Prompt-facing description of one tool (rendered into the provider's
/// system text by `toolcall::render_system` / [`render_native_system`]).
#[derive(Clone, Debug)]
pub struct ToolDef {
    /// Dotted canonical name used by the typed parser and the chat wire.
    pub name: &'static str,
    /// Underscore API name sent to native Responses function calling.
    pub api_name: &'static str,
    pub description: &'static str,
    /// Human/LLM-readable argument sketch, e.g. `{"query": "...", "limit": 5}`.
    pub args_doc: &'static str,
    /// JSON Schema for the native function. `strict` stays false on the
    /// wrapper because `operation.create` params are dynamic; this crate's
    /// typed parser is the security boundary.
    pub parameters: Value,
}

/// The allowlist, in the order it is documented to the model.
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "image.generate",
            api_name: "image_generate",
            description: "Generate a new image from a text prompt on the GPU fleet. \
                          Extract a complete visual description from the user's words. \
                          Omitted model/size/steps use the session defaults. \
                          Optional then= follows the image with mesh, video, world, \
                          character, matte, or depth.",
            args_doc: r#"{"prompt": "visual description", "model": "flux1-schnell", "width": 1024, "height": 1024, "steps": 4, "then": "mesh"}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("full visual description for the image model", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("image model id; omit to use defaults.image_model", 1, 64),
                    ),
                    (
                        "width",
                        schema_integer_range("pixels; omit to use defaults.width", 64, 2048),
                    ),
                    (
                        "height",
                        schema_integer_range("pixels; omit to use defaults.height", 64, 2048),
                    ),
                    (
                        "steps",
                        schema_integer_range("diffusion steps; omit to use defaults.steps", 1, 50),
                    ),
                    (
                        "then",
                        schema_string_enum(
                            "optional follow-on after the image",
                            GenerateThen::SLUGS,
                        ),
                    ),
                ],
                &["prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "video.generate",
            api_name: "video_generate",
            description: "Generate a new video from a text prompt (text-to-video). \
                          Use this when the user asks for a clip, animation, or movie. \
                          For image-to-video, call image.generate with then=video instead.",
            args_doc: r#"{"prompt": "motion description", "model": "minimax-h3", "width": 640, "height": 352, "frames": 39, "steps": 30}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("full motion / scene description for the video model", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("video model id; omit for fleet affinity", 1, 64),
                    ),
                    ("width", schema_integer_range("pixels; omit for default 640", 64, 1920)),
                    ("height", schema_integer_range("pixels; omit for default 352", 64, 1080)),
                    (
                        "frames",
                        schema_integer_range("frame count at 16 fps; omit for default 39", 1, 256),
                    ),
                    (
                        "steps",
                        schema_integer_range("diffusion steps; omit for default 30", 1, 100),
                    ),
                ],
                &["prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "audio.generate",
            api_name: "audio_generate",
            description: "Generate a sound effect from a text prompt. \
                          Use for SFX, whooshes, impacts, ambience — not songs or speech.",
            args_doc: r#"{"prompt": "sword clash, metallic", "model": "sa3-sfx"}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("sound-effect description", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("audio model id: sa3-sfx, moss-sfx, or woosh-sfx", 1, 64),
                    ),
                ],
                &["prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "speech.generate",
            api_name: "speech_generate",
            description: "Speak the given text with a TTS voice. \
                          The prompt is the exact words to say, not a description of a voice.",
            args_doc: r#"{"prompt": "exact words to speak", "model": "kokoro", "voice": "bm_daniel"}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("exact words to speak", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("speech model id: kokoro or indextts-2.5", 1, 64),
                    ),
                    (
                        "voice",
                        schema_string_len("kokoro voice pack id, e.g. bm_daniel", 1, 64),
                    ),
                ],
                &["prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "music.generate",
            api_name: "music_generate",
            description: "Generate a full song from a text prompt. \
                          Use for music, score, or a track — not short SFX.",
            args_doc: r#"{"prompt": "lofi rain loop, warm keys", "model": "minimax-music3", "seconds": 180}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("musical description", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("music model id; omit for minimax-music3", 1, 64),
                    ),
                    (
                        "seconds",
                        schema_integer_range("song length 5..=300; omit for 180", 5, 300),
                    ),
                ],
                &["prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "mesh.generate",
            api_name: "mesh_generate",
            description: "Generate a 3D mesh from a text prompt (image then TRELLIS). \
                          Use when the user asks for a model, GLB, or 3D object.",
            args_doc: r#"{"prompt": "low-poly sci-fi crate", "model": "flux1-schnell", "width": 1024, "height": 1024, "steps": 4}"#,
            parameters: schema_image_prompt(),
        },
        ToolDef {
            name: "world.generate",
            api_name: "world_generate",
            description: "Generate a 3D Gaussian splat world from a text prompt \
                          (image then FlashWorld). Use for a scene, environment, or splat.",
            args_doc: r#"{"prompt": "misty harbor at dawn", "model": "flux1-schnell"}"#,
            parameters: schema_image_prompt(),
        },
        ToolDef {
            name: "character.generate",
            api_name: "character_generate",
            description: "Generate a playable character: expanded prompt → image → \
                          matte → mesh → rig → motion. Use when the user asks for a \
                          character, avatar, or playable figure.",
            args_doc: r#"{"prompt": "armored fox ranger, standing idle"}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("character description", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("optional image-stage model pin", 1, 64),
                    ),
                ],
                &["prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "defaults.get",
            api_name: "defaults_get",
            description: "Read the mutable generation defaults (image model, width, height, steps, then).",
            args_doc: r#"{}"#,
            parameters: schema_object(vec![], &[], Some(false)),
        },
        ToolDef {
            name: "defaults.set",
            api_name: "defaults_set",
            description: "Change generation defaults for later image.generate calls. \
                          Only provided fields change. Use this when the user says \
                          switch model, change resolution, or set default steps.",
            args_doc: r#"{"image_model": "flux1-dev", "width": 1024, "height": 1024, "steps": 20, "then": "mesh"}"#,
            parameters: schema_object(
                vec![
                    (
                        "image_model",
                        schema_string_len("default image model id", 1, 64),
                    ),
                    ("width", schema_integer_range("default width", 64, 2048)),
                    ("height", schema_integer_range("default height", 64, 2048)),
                    ("steps", schema_integer_range("default steps", 1, 50)),
                    (
                        "then",
                        schema_string_enum("default follow-on", GenerateThen::SLUGS),
                    ),
                ],
                &[],
                Some(false),
            ),
        },
        ToolDef {
            name: "fleet.introspect",
            api_name: "fleet_introspect",
            description: "Query live backends, available models, and legal generation options \
                          (resolutions, steps). Call this before defaults.set or any generate \
                          tool when the user asks what models or sizes exist.",
            args_doc: r#"{"domain": "image"}"#,
            parameters: schema_object(
                vec![(
                    "domain",
                    schema_string_len(
                        "optional domain filter: image, video, audio, speech, music, mesh, world, text",
                        1,
                        32,
                    ),
                )],
                &[],
                Some(false),
            ),
        },
        ToolDef {
            name: "asset.search",
            api_name: "asset_search",
            description: "Search the asset catalog.",
            args_doc: r#"{"query": "text", "limit": 10}"#,
            parameters: schema_object(
                vec![
                    ("query", schema_string_len("search text", 1, 1024)),
                    ("limit", schema_integer_range("1..=25, default 10 if omitted", 1, 25)),
                ],
                &["query"],
                Some(false),
            ),
        },
        ToolDef {
            name: "asset.inspect",
            api_name: "asset_inspect",
            description: "Inspect an asset (by id), an alias, or an exact revision.",
            args_doc: r#"{"asset": "ast_..."} or {"alias": "ns/name"} or {"revision": "arev_..."}"#,
            parameters: schema_inspect(),
        },
        ToolDef {
            name: "operation.capabilities",
            api_name: "operation_capabilities",
            description: "List the registered operation types with their live availability, \
                          input slots, parameters and outputs.",
            args_doc: r#"{}"#,
            parameters: schema_object(vec![], &[], Some(false)),
        },
        ToolDef {
            name: "operation.create",
            api_name: "operation_create",
            description: "Create a typed asset operation (e.g. mesh.from_image.v1) over exact \
                          input revisions bound to this session. The server pins the exact \
                          input file, validates rights, executes on a worker, and publishes \
                          the immutable output with full lineage. Identical replays join the \
                          same operation.",
            args_doc: r#"{"kind": "mesh.from_image.v1", "inputs": [{"slot": "image", "asset": "ast_...", "revision": "arev_...", "role": "texture"}], "params": {"seed": 0}}"#,
            parameters: schema_object(
                vec![
                    (
                        "kind",
                        schema_string_pattern(
                            "dotted lowercase identifier",
                            1,
                            64,
                            r"^[a-z0-9_.]+$",
                        ),
                    ),
                    (
                        "inputs",
                        schema_array_bounded(
                            "1..=4 objects: required asset, revision, role; optional slot (default image), tier, lod, media. Unknown fields refused.",
                            1,
                            MAX_TRANSFORM_INPUTS as i64,
                            schema_operation_input(),
                        ),
                    ),
                    (
                        "params",
                        schema_free_object("operation-specific parameters (dynamic object)"),
                    ),
                    (
                        "publication",
                        schema_object(
                            vec![
                                (
                                    "mode",
                                    schema_string_enum(
                                        "publish or publish_and_alias",
                                        &["publish", "publish_and_alias"],
                                    ),
                                ),
                                ("alias", schema_string("required for publish_and_alias")),
                                (
                                    "expect",
                                    schema_string_enum(
                                        "any/absent/head; omitted means absent",
                                        &["any", "absent", "head"],
                                    ),
                                ),
                                ("expect_head", schema_string_len("required when expect=head", 1, 80)),
                            ],
                            &["mode"],
                            Some(false),
                        ),
                    ),
                    (
                        "idempotency_key",
                        schema_string_pattern("1..=128 printable ASCII", 1, 128, r"^[\x21-\x7E]{1,128}$"),
                    ),
                ],
                &["kind", "inputs"],
                Some(false),
            ),
        },
        ToolDef {
            name: "operation.get",
            api_name: "operation_get",
            description: "One status snapshot of an operation (state, progress, result).",
            args_doc: r#"{"operation": "op_..."}"#,
            parameters: schema_object(
                vec![("operation", schema_string_len("op_...", 1, 64))],
                &["operation"],
                Some(false),
            ),
        },
        ToolDef {
            name: "operation.wait",
            api_name: "operation_wait",
            description: "Wait for an operation to finish, streaming its progress and \
                          returning its new events.",
            args_doc: r#"{"operation": "op_...", "timeout_ms": 60000}"#,
            parameters: schema_object(
                vec![
                    ("operation", schema_string_len("op_...", 1, 64)),
                    (
                        "timeout_ms",
                        schema_integer_range("1..=120000, default 60000", 1, 120_000),
                    ),
                    ("after", schema_integer_range("event cursor, default 0", 0, i64::MAX)),
                ],
                &["operation"],
                Some(false),
            ),
        },
        ToolDef {
            name: "operation.cancel",
            api_name: "operation_cancel",
            description: "Cancel an operation (and its executor job).",
            args_doc: r#"{"operation": "op_..."}"#,
            parameters: schema_object(
                vec![("operation", schema_string_len("op_...", 1, 64))],
                &["operation"],
                Some(false),
            ),
        },
        ToolDef {
            name: "operation.retry",
            api_name: "operation_retry",
            description: "Retry a failed or cancelled operation as a fresh round.",
            args_doc: r#"{"operation": "op_..."}"#,
            parameters: schema_object(
                vec![("operation", schema_string_len("op_...", 1, 64))],
                &["operation"],
                Some(false),
            ),
        },
        ToolDef {
            name: "llm.consult",
            api_name: "llm_consult",
            description: "Ask the configured external generative LLM (OpenAI or Grok) to \
                          draft code, a level, or a design. Use this from a local (fleet \
                          Qwen) session when a stronger generator is needed. The consult \
                          cannot run tools or open nested sessions. Unavailable when the \
                          session is already on an external provider or no key is configured.",
            args_doc: r#"{"task": "code"|"level"|"design", "prompt": "...", "provider": "openai"|"grok"}"#,
            parameters: schema_object(
                vec![
                    (
                        "task",
                        schema_string_enum(
                            "what to generate",
                            &["code", "level", "design"],
                        ),
                    ),
                    (
                        "prompt",
                        schema_string_len("generation brief", 1, MAX_MESSAGE_BYTES as i64),
                    ),
                    (
                        "provider",
                        schema_string_enum(
                            "openai or grok; omitted picks the first available external",
                            &["openai", "grok"],
                        ),
                    ),
                ],
                &["task", "prompt"],
                Some(false),
            ),
        },
    ]
}

/// Map a native underscore API name onto the dotted canonical tool.
/// Unknown names (including dotted names sent to a native provider) fail
/// closed.
pub fn canonical_from_api_name(api_name: &str) -> Option<&'static str> {
    match api_name {
        "image_generate" => Some("image.generate"),
        "video_generate" => Some("video.generate"),
        "audio_generate" => Some("audio.generate"),
        "speech_generate" => Some("speech.generate"),
        "music_generate" => Some("music.generate"),
        "mesh_generate" => Some("mesh.generate"),
        "world_generate" => Some("world.generate"),
        "character_generate" => Some("character.generate"),
        "defaults_get" => Some("defaults.get"),
        "defaults_set" => Some("defaults.set"),
        "fleet_introspect" => Some("fleet.introspect"),
        "asset_search" => Some("asset.search"),
        "asset_inspect" => Some("asset.inspect"),
        "operation_capabilities" => Some("operation.capabilities"),
        "operation_create" => Some("operation.create"),
        "operation_get" => Some("operation.get"),
        "operation_wait" => Some("operation.wait"),
        "operation_cancel" => Some("operation.cancel"),
        "operation_retry" => Some("operation.retry"),
        "llm_consult" => Some("llm.consult"),
        _ => None,
    }
}

/// Native Responses tool list: `type=function`, `strict=false`,
/// `parallel_tool_calls` is set on the request (false), not here.
pub fn native_tools_payload() -> Value {
    Value::Arr(
        definitions()
            .into_iter()
            .map(|d| {
                json::obj(vec![
                    ("type", json::s("function")),
                    ("name", json::s(d.api_name)),
                    ("description", json::s(d.description)),
                    ("parameters", d.parameters),
                    ("strict", Value::Bool(false)),
                ])
            })
            .collect(),
    )
}

/// System prompt for native function-calling providers. Must not instruct
/// the model to emit a textual `<<tool>>` marker.
pub fn render_native_system(defs: &[ToolDef], capabilities: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "You can operate the asset catalog and generation backend through the \
         provided functions.\n\
         Call at most one function at a time, then wait for the function result.\n\
         Never invent asset or revision ids: use only ids from bound inputs, \
         tool results, or catalog search.\n\nFunctions:\n",
    );
    for d in defs {
        out.push_str("- ");
        out.push_str(d.api_name);
        out.push_str(" (");
        out.push_str(d.name);
        out.push_str("): ");
        out.push_str(d.description);
        out.push_str(" args: ");
        out.push_str(d.args_doc);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(capabilities);
    out
}

fn schema_image_prompt() -> Value {
    schema_object(
        vec![
            (
                "prompt",
                schema_string_len("full visual description", 1, 2048),
            ),
            (
                "model",
                schema_string_len("image model id; omit to use defaults.image_model", 1, 64),
            ),
            (
                "width",
                schema_integer_range("pixels; omit to use defaults.width", 64, 2048),
            ),
            (
                "height",
                schema_integer_range("pixels; omit to use defaults.height", 64, 2048),
            ),
            (
                "steps",
                schema_integer_range("diffusion steps; omit to use defaults.steps", 1, 50),
            ),
        ],
        &["prompt"],
        Some(false),
    )
}

fn schema_object(
    properties: Vec<(&str, Value)>,
    required: &[&str],
    additional: Option<bool>,
) -> Value {
    let mut pairs = vec![
        ("type", json::s("object")),
        ("properties", json::obj(properties)),
    ];
    if !required.is_empty() {
        pairs.push((
            "required",
            Value::Arr(required.iter().map(|s| json::s(*s)).collect()),
        ));
    }
    if let Some(flag) = additional {
        pairs.push(("additionalProperties", Value::Bool(flag)));
    }
    json::obj(pairs)
}

fn schema_string(description: &str) -> Value {
    json::obj(vec![
        ("type", json::s("string")),
        ("description", json::s(description)),
    ])
}

fn schema_string_len(description: &str, min: i64, max: i64) -> Value {
    json::obj(vec![
        ("type", json::s("string")),
        ("description", json::s(description)),
        ("minLength", Value::Int(min)),
        ("maxLength", Value::Int(max)),
    ])
}

fn schema_string_pattern(description: &str, min: i64, max: i64, pattern: &str) -> Value {
    json::obj(vec![
        ("type", json::s("string")),
        ("description", json::s(description)),
        ("minLength", Value::Int(min)),
        ("maxLength", Value::Int(max)),
        ("pattern", json::s(pattern)),
    ])
}

fn schema_integer_range(description: &str, min: i64, max: i64) -> Value {
    json::obj(vec![
        ("type", json::s("integer")),
        ("description", json::s(description)),
        ("minimum", Value::Int(min)),
        ("maximum", Value::Int(max)),
    ])
}

fn schema_array_bounded(description: &str, min_items: i64, max_items: i64, items: Value) -> Value {
    json::obj(vec![
        ("type", json::s("array")),
        ("description", json::s(description)),
        ("minItems", Value::Int(min_items)),
        ("maxItems", Value::Int(max_items)),
        ("items", items),
    ])
}

fn schema_inspect() -> Value {
    let mut pairs = match schema_object(
        vec![
            ("asset", schema_string("exactly one of asset/alias/revision")),
            ("alias", schema_string("exactly one of asset/alias/revision")),
            ("revision", schema_string("exactly one of asset/alias/revision")),
        ],
        &[],
        Some(false),
    ) {
        Value::Obj(p) => p,
        other => return other,
    };
    pairs.push(("minProperties".to_string(), Value::Int(1)));
    pairs.push(("maxProperties".to_string(), Value::Int(1)));
    Value::Obj(pairs)
}

fn schema_ident(description: &str) -> Value {
    schema_string_pattern(description, 1, 32, r"^[a-z0-9_-]{1,32}$")
}

fn schema_string_enum(description: &str, values: &[&str]) -> Value {
    json::obj(vec![
        ("type", json::s("string")),
        ("description", json::s(description)),
        ("enum", Value::Arr(values.iter().map(|s| json::s(*s)).collect())),
    ])
}

fn schema_operation_input() -> Value {
    schema_object(
        vec![
            ("slot", schema_ident("lowercase identifier, default image")),
            ("asset", schema_string_len("asset id", 1, 64)),
            ("revision", schema_string_len("revision id", 1, 80)),
            ("role", schema_ident("lowercase identifier")),
            ("tier", schema_ident("lowercase identifier")),
            ("lod", schema_integer_range("0..=255", 0, 255)),
            ("media", schema_ident("lowercase identifier")),
        ],
        &["asset", "revision", "role"],
        Some(false),
    )
}

fn schema_free_object(description: &str) -> Value {
    json::obj(vec![
        ("type", json::s("object")),
        ("description", json::s(description)),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectTarget {
    Asset(AssetId),
    Alias(AssetAlias),
    Revision(AssetRevisionId),
}

/// One exact compound input selector of `operation.create`: slot + asset +
/// revision + role, optionally narrowed by tier/lod and guarded by the
/// expected media. Role/tier/media stay validated identifier STRINGS here;
/// the dispatcher maps them onto the typed client vocabulary fail-closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationInputArg {
    pub slot: String,
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub role: String,
    pub tier: Option<String>,
    pub lod: Option<u8>,
    pub media: Option<String>,
}

/// Publication policy of `operation.create`, validated at creation and
/// executed only by the server finalizer — never a separate alias tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicationArg {
    Publish,
    PublishAndAlias { alias: AssetAlias, expect: AliasExpectArg },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AliasExpectArg {
    Any,
    Absent,
    Head(AssetRevisionId),
}

/// Optional follow-on after `image.generate`. `None` on the call means
/// "use session defaults"; `Some(GenerateThen::None)` means image only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerateThen {
    None,
    Mesh,
    Video,
    World,
    Character,
    Matte,
    Depth,
}

impl GenerateThen {
    pub const SLUGS: &'static [&'static str] =
        &["mesh", "video", "world", "character", "matte", "depth", "none"];

    pub fn slug(self) -> &'static str {
        match self {
            GenerateThen::None => "none",
            GenerateThen::Mesh => "mesh",
            GenerateThen::Video => "video",
            GenerateThen::World => "world",
            GenerateThen::Character => "character",
            GenerateThen::Matte => "matte",
            GenerateThen::Depth => "depth",
        }
    }

    pub fn from_slug(s: &str) -> Option<GenerateThen> {
        match s {
            "none" => Some(GenerateThen::None),
            "mesh" => Some(GenerateThen::Mesh),
            "video" => Some(GenerateThen::Video),
            "world" => Some(GenerateThen::World),
            "character" => Some(GenerateThen::Character),
            "matte" => Some(GenerateThen::Matte),
            "depth" => Some(GenerateThen::Depth),
            _ => None,
        }
    }

    pub fn is_follow_on(self) -> bool {
        !matches!(self, GenerateThen::None)
    }
}

/// A parsed, bounds-checked tool call. Everything the dispatcher executes
/// starts as one of these; raw `(name, args)` that does not parse is a
/// typed refusal fed back to the model.
#[derive(Clone, Debug, PartialEq)]
pub enum ContentToolCall {
    ImageGenerate {
        prompt: String,
        then: Option<GenerateThen>,
        model: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
    },
    VideoGenerate {
        prompt: String,
        model: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        frames: Option<u32>,
        steps: Option<u32>,
    },
    AudioGenerate {
        prompt: String,
        model: Option<String>,
    },
    SpeechGenerate {
        prompt: String,
        model: Option<String>,
        voice: Option<String>,
    },
    MusicGenerate {
        prompt: String,
        model: Option<String>,
        seconds: Option<u32>,
    },
    MeshGenerate {
        prompt: String,
        model: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
    },
    WorldGenerate {
        prompt: String,
        model: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
    },
    CharacterGenerate {
        prompt: String,
        model: Option<String>,
    },
    DefaultsGet,
    DefaultsSet {
        image_model: Option<String>,
        width: Option<u32>,
        height: Option<u32>,
        steps: Option<u32>,
        then: Option<GenerateThen>,
    },
    FleetIntrospect { domain: Option<String> },
    AssetSearch { query: String, limit: u32 },
    AssetInspect { target: InspectTarget },
    OperationCapabilities,
    OperationCreate {
        kind: String,
        inputs: Vec<OperationInputArg>,
        params: Value,
        publication: PublicationArg,
        idempotency_key: Option<String>,
    },
    OperationGet { operation: OperationId },
    OperationWait { operation: OperationId, after: u64, timeout_ms: u64 },
    OperationCancel { operation: OperationId },
    OperationRetry { operation: OperationId },
    /// Local session delegates a text-only generation to OpenAI or Grok.
    LlmConsult { task: ConsultTask, prompt: String, provider: Option<ProviderKind> },
}

/// What `llm.consult` is asked to generate. Text only — never a nested
/// tool-capable chat session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsultTask {
    Code,
    Level,
    Design,
}

impl ConsultTask {
    pub fn slug(self) -> &'static str {
        match self {
            ConsultTask::Code => "code",
            ConsultTask::Level => "level",
            ConsultTask::Design => "design",
        }
    }

    pub fn from_slug(s: &str) -> Option<ConsultTask> {
        match s {
            "code" => Some(ConsultTask::Code),
            "level" => Some(ConsultTask::Level),
            "design" => Some(ConsultTask::Design),
            _ => None,
        }
    }
}

impl ContentToolCall {
    pub fn name(&self) -> &'static str {
        match self {
            ContentToolCall::ImageGenerate { .. } => "image.generate",
            ContentToolCall::VideoGenerate { .. } => "video.generate",
            ContentToolCall::AudioGenerate { .. } => "audio.generate",
            ContentToolCall::SpeechGenerate { .. } => "speech.generate",
            ContentToolCall::MusicGenerate { .. } => "music.generate",
            ContentToolCall::MeshGenerate { .. } => "mesh.generate",
            ContentToolCall::WorldGenerate { .. } => "world.generate",
            ContentToolCall::CharacterGenerate { .. } => "character.generate",
            ContentToolCall::DefaultsGet => "defaults.get",
            ContentToolCall::DefaultsSet { .. } => "defaults.set",
            ContentToolCall::FleetIntrospect { .. } => "fleet.introspect",
            ContentToolCall::AssetSearch { .. } => "asset.search",
            ContentToolCall::AssetInspect { .. } => "asset.inspect",
            ContentToolCall::OperationCapabilities => "operation.capabilities",
            ContentToolCall::OperationCreate { .. } => "operation.create",
            ContentToolCall::OperationGet { .. } => "operation.get",
            ContentToolCall::OperationWait { .. } => "operation.wait",
            ContentToolCall::OperationCancel { .. } => "operation.cancel",
            ContentToolCall::OperationRetry { .. } => "operation.retry",
            ContentToolCall::LlmConsult { .. } => "llm.consult",
        }
    }

    /// Parse `(name, args)` fail-closed. The typed parser is the security
    /// boundary for `strict:false` native tools: every unknown field and
    /// every present wrong type is a refusal. The error string is safe to
    /// show to the model; it never echoes oversized input back.
    pub fn parse(name: &str, args: &Value) -> Result<ContentToolCall, String> {
        if !matches!(args, Value::Obj(_)) {
            return Err("tool arguments must be an object".to_string());
        }
        if args.to_json().len() > MAX_TOOL_JSON_BYTES {
            return Err("tool arguments too large".to_string());
        }
        match name {
            "image.generate" => {
                let img = parse_image_prompt(args, &["prompt", "then", "model", "width", "height", "steps"], "image.generate argument")?;
                Ok(ContentToolCall::ImageGenerate {
                    prompt: img.prompt,
                    then: parse_then(optional_str(args, "then")?)?,
                    model: img.model,
                    width: img.width,
                    height: img.height,
                    steps: img.steps,
                })
            }
            "video.generate" => {
                check_known(
                    args,
                    &["prompt", "model", "width", "height", "frames", "steps"],
                    "video.generate argument",
                )?;
                Ok(ContentToolCall::VideoGenerate {
                    prompt: need_prompt(args)?,
                    model: optional_str(args, "model")?.map(str::to_string),
                    width: optional_u32(args, "width", 64, 1920)?,
                    height: optional_u32(args, "height", 64, 1080)?,
                    frames: optional_u32(args, "frames", 1, 256)?,
                    steps: optional_u32(args, "steps", 1, 100)?,
                })
            }
            "audio.generate" => {
                check_known(args, &["prompt", "model"], "audio.generate argument")?;
                Ok(ContentToolCall::AudioGenerate {
                    prompt: need_prompt(args)?,
                    model: optional_str(args, "model")?.map(str::to_string),
                })
            }
            "speech.generate" => {
                check_known(args, &["prompt", "model", "voice"], "speech.generate argument")?;
                Ok(ContentToolCall::SpeechGenerate {
                    prompt: need_prompt(args)?,
                    model: optional_str(args, "model")?.map(str::to_string),
                    voice: optional_str(args, "voice")?.map(str::to_string),
                })
            }
            "music.generate" => {
                check_known(args, &["prompt", "model", "seconds"], "music.generate argument")?;
                Ok(ContentToolCall::MusicGenerate {
                    prompt: need_prompt(args)?,
                    model: optional_str(args, "model")?.map(str::to_string),
                    seconds: optional_u32(args, "seconds", 5, 300)?,
                })
            }
            "mesh.generate" => {
                let img = parse_image_prompt(args, &["prompt", "model", "width", "height", "steps"], "mesh.generate argument")?;
                Ok(ContentToolCall::MeshGenerate {
                    prompt: img.prompt,
                    model: img.model,
                    width: img.width,
                    height: img.height,
                    steps: img.steps,
                })
            }
            "world.generate" => {
                let img = parse_image_prompt(args, &["prompt", "model", "width", "height", "steps"], "world.generate argument")?;
                Ok(ContentToolCall::WorldGenerate {
                    prompt: img.prompt,
                    model: img.model,
                    width: img.width,
                    height: img.height,
                    steps: img.steps,
                })
            }
            "character.generate" => {
                check_known(args, &["prompt", "model"], "character.generate argument")?;
                Ok(ContentToolCall::CharacterGenerate {
                    prompt: need_prompt(args)?,
                    model: optional_str(args, "model")?.map(str::to_string),
                })
            }
            "defaults.get" => {
                check_known(args, &[], "defaults.get argument")?;
                Ok(ContentToolCall::DefaultsGet)
            }
            "defaults.set" => {
                check_known(
                    args,
                    &["image_model", "width", "height", "steps", "then"],
                    "defaults.set argument",
                )?;
                let empty = match args {
                    Value::Obj(pairs) => pairs.is_empty(),
                    _ => true,
                };
                if empty {
                    return Err("defaults.set needs at least one field".to_string());
                }
                Ok(ContentToolCall::DefaultsSet {
                    image_model: optional_str(args, "image_model")?.map(str::to_string),
                    width: optional_u32(args, "width", 64, 2048)?,
                    height: optional_u32(args, "height", 64, 2048)?,
                    steps: optional_u32(args, "steps", 1, 50)?,
                    then: parse_then(optional_str(args, "then")?)?,
                })
            }
            "fleet.introspect" => {
                check_known(args, &["domain"], "fleet.introspect argument")?;
                Ok(ContentToolCall::FleetIntrospect {
                    domain: optional_str(args, "domain")?.map(str::to_string),
                })
            }
            "asset.search" => {
                check_known(args, &["query", "limit"], "asset.search argument")?;
                let query = need_str(args, "query", 1024)?;
                let limit = optional_u64(args, "limit")?.unwrap_or(10);
                if limit == 0 || limit > 25 {
                    return Err("limit must be 1..=25".to_string());
                }
                Ok(ContentToolCall::AssetSearch { query, limit: limit as u32 })
            }
            "asset.inspect" => {
                check_known(args, &["asset", "alias", "revision"], "asset.inspect argument")?;
                let asset = optional_str(args, "asset")?;
                let alias = optional_str(args, "alias")?;
                let revision = optional_str(args, "revision")?;
                let n = u8::from(asset.is_some())
                    + u8::from(alias.is_some())
                    + u8::from(revision.is_some());
                let target = match (n, asset, alias, revision) {
                    (1, Some(a), None, None) => InspectTarget::Asset(parse_asset(a)?),
                    (1, None, Some(a), None) => InspectTarget::Alias(parse_alias(a)?),
                    (1, None, None, Some(r)) => InspectTarget::Revision(parse_revision(r)?),
                    _ => {
                        return Err(
                            "asset.inspect takes exactly one of asset/alias/revision".to_string()
                        )
                    }
                };
                Ok(ContentToolCall::AssetInspect { target })
            }
            "operation.capabilities" => {
                check_known(args, &[], "operation.capabilities argument")?;
                Ok(ContentToolCall::OperationCapabilities)
            }
            "operation.create" => parse_operation_create(args),
            "operation.get" => {
                check_known(args, &["operation"], "operation.get argument")?;
                Ok(ContentToolCall::OperationGet { operation: need_op(args)? })
            }
            "operation.wait" => {
                check_known(
                    args,
                    &["operation", "timeout_ms", "after"],
                    "operation.wait argument",
                )?;
                let timeout_ms = optional_u64(args, "timeout_ms")?.unwrap_or(60_000);
                if timeout_ms == 0 || timeout_ms > 120_000 {
                    return Err("timeout_ms must be 1..=120000".to_string());
                }
                let after = optional_u64(args, "after")?.unwrap_or(0);
                Ok(ContentToolCall::OperationWait { operation: need_op(args)?, after, timeout_ms })
            }
            "operation.cancel" => {
                check_known(args, &["operation"], "operation.cancel argument")?;
                Ok(ContentToolCall::OperationCancel { operation: need_op(args)? })
            }
            "operation.retry" => {
                check_known(args, &["operation"], "operation.retry argument")?;
                Ok(ContentToolCall::OperationRetry { operation: need_op(args)? })
            }
            "llm.consult" => {
                check_known(args, &["task", "prompt", "provider"], "llm.consult argument")?;
                let task = ConsultTask::from_slug(need_str(args, "task", 16)?.as_str())
                    .ok_or_else(|| "task must be code, level, or design".to_string())?;
                let prompt = need_str(args, "prompt", MAX_MESSAGE_BYTES)?;
                if prompt.is_empty() {
                    return Err("prompt must not be empty".to_string());
                }
                let provider = match optional_str(args, "provider")? {
                    None => None,
                    Some("openai") => Some(ProviderKind::OpenAi),
                    Some("grok") => Some(ProviderKind::Grok),
                    Some(_) => {
                        return Err("provider must be openai or grok".to_string());
                    }
                };
                Ok(ContentToolCall::LlmConsult { task, prompt, provider })
            }
            other => Err(format!("unknown tool '{}'", bounded(other, 32))),
        }
    }
}

/// `operation.create` is the mutating surface, so its parse is STRICT:
/// unknown argument or input fields refuse instead of being ignored —
/// a model mistake must never become a silently different operation.
fn parse_operation_create(args: &Value) -> Result<ContentToolCall, String> {
    check_known(
        args,
        &["kind", "inputs", "params", "publication", "idempotency_key"],
        "operation.create argument",
    )?;
    let kind = need_str(args, "kind", 64)?;
    if !kind
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
    {
        return Err("'kind' must be a dotted lowercase identifier".to_string());
    }
    let inputs_v = match args.get("inputs") {
        None => return Err("operation.create requires inputs".to_string()),
        Some(Value::Arr(a)) => a,
        Some(_) => return Err("'inputs' must be an array".to_string()),
    };
    if inputs_v.is_empty() || inputs_v.len() > MAX_TRANSFORM_INPUTS {
        return Err(format!("operation.create takes 1..={MAX_TRANSFORM_INPUTS} inputs"));
    }
    let mut inputs = Vec::with_capacity(inputs_v.len());
    for iv in inputs_v {
        if !matches!(iv, Value::Obj(_)) {
            return Err("operation input must be an object".to_string());
        }
        check_known(
            iv,
            &["slot", "asset", "revision", "role", "tier", "lod", "media"],
            "operation input field",
        )?;
        let slot = optional_str(iv, "slot")?.unwrap_or("image").to_string();
        if !ident_ok(&slot) {
            return Err("input slot must be a short lowercase identifier".to_string());
        }
        let asset = parse_asset(need_str(iv, "asset", 64)?.as_str())?;
        let revision = parse_revision(need_str(iv, "revision", 80)?.as_str())?;
        let role = need_str(iv, "role", 32)?;
        if !ident_ok(&role) {
            return Err("input role must be a short lowercase identifier".to_string());
        }
        let tier = match optional_str(iv, "tier")? {
            None => None,
            Some(t) if ident_ok(t) => Some(t.to_string()),
            Some(_) => return Err("input tier must be a short lowercase identifier".to_string()),
        };
        let lod = match optional_u64(iv, "lod")? {
            None => None,
            Some(l) if l <= u8::MAX as u64 => Some(l as u8),
            Some(_) => return Err("input lod out of range".to_string()),
        };
        let media = match optional_str(iv, "media")? {
            None => None,
            Some(m) if ident_ok(m) => Some(m.to_string()),
            Some(_) => return Err("input media must be a short lowercase identifier".to_string()),
        };
        inputs.push(OperationInputArg { slot, asset, revision, role, tier, lod, media });
    }
    let params = match args.get("params") {
        None => Value::Obj(Vec::new()),
        Some(p @ Value::Obj(_)) => p.clone(),
        Some(_) => return Err("'params' must be an object".to_string()),
    };
    let publication = match args.get("publication") {
        None => PublicationArg::Publish,
        Some(p @ Value::Obj(_)) => {
            check_known(p, &["mode", "alias", "expect", "expect_head"], "publication field")?;
            match optional_str(p, "mode")? {
                None => return Err("publication mode missing".to_string()),
                Some("publish") => {
                    if p.get("alias").is_some()
                        || p.get("expect").is_some()
                        || p.get("expect_head").is_some()
                    {
                        return Err(
                            "publication mode publish cannot include alias/expect/expect_head"
                                .to_string(),
                        );
                    }
                    PublicationArg::Publish
                }
                Some("publish_and_alias") => {
                    let alias = parse_alias(
                        optional_str(p, "alias")?
                            .ok_or_else(|| "publication alias missing".to_string())?,
                    )?;
                    let expect_head = optional_str(p, "expect_head")?;
                    let expect = match optional_str(p, "expect")? {
                        // Omitted defaults to the SAFE expectation: the
                        // alias must not exist yet. Overwriting an existing
                        // head is a deliberate act the model must spell out
                        // ("any", or a compare-and-set "head") — a silent
                        // default must never clobber a curated alias.
                        None | Some("absent") => {
                            if expect_head.is_some() {
                                return Err("expect_head is only valid when expect is head".into());
                            }
                            AliasExpectArg::Absent
                        }
                        Some("any") => {
                            if expect_head.is_some() {
                                return Err("expect_head is only valid when expect is head".into());
                            }
                            AliasExpectArg::Any
                        }
                        Some("head") => AliasExpectArg::Head(parse_revision(
                            expect_head.ok_or_else(|| "expect_head missing".to_string())?,
                        )?),
                        Some(_) => return Err("publication expect must be any/absent/head".into()),
                    };
                    PublicationArg::PublishAndAlias { alias, expect }
                }
                Some(_) => return Err("publication mode must be publish/publish_and_alias".into()),
            }
        }
        Some(_) => return Err("publication must be an object".to_string()),
    };
    let idempotency_key = match optional_str(args, "idempotency_key")? {
        None => None,
        Some(k) => {
            if k.is_empty() || k.len() > 128 || !k.bytes().all(|b| b.is_ascii_graphic()) {
                return Err("idempotency_key must be 1..=128 printable ASCII".to_string());
            }
            Some(k.to_string())
        }
    };
    Ok(ContentToolCall::OperationCreate { kind, inputs, params, publication, idempotency_key })
}

fn check_known(v: &Value, allowed: &[&str], what: &str) -> Result<(), String> {
    let Value::Obj(pairs) = v else {
        return Err(format!("{what} must be an object"));
    };
    for (key, _) in pairs {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown {what} '{}'", bounded(key, 32)));
        }
    }
    Ok(())
}

fn bounded(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn optional_str<'a>(args: &'a Value, key: &'static str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Str(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("'{key}' must be a string")),
    }
}

fn optional_u64(args: &Value, key: &'static str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Int(i)) if *i >= 0 => Ok(Some(*i as u64)),
        Some(_) => Err(format!("'{key}' must be an integer")),
    }
}

fn optional_u32(args: &Value, key: &'static str, min: u32, max: u32) -> Result<Option<u32>, String> {
    match optional_u64(args, key)? {
        None => Ok(None),
        Some(n) if n >= min as u64 && n <= max as u64 => Ok(Some(n as u32)),
        Some(_) => Err(format!("'{key}' must be {min}..={max}")),
    }
}

struct ImagePromptArgs {
    prompt: String,
    model: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    steps: Option<u32>,
}

fn parse_image_prompt(args: &Value, allowed: &[&str], what: &str) -> Result<ImagePromptArgs, String> {
    check_known(args, allowed, what)?;
    Ok(ImagePromptArgs {
        prompt: need_prompt(args)?,
        model: optional_str(args, "model")?.map(str::to_string),
        width: optional_u32(args, "width", 64, 2048)?,
        height: optional_u32(args, "height", 64, 2048)?,
        steps: optional_u32(args, "steps", 1, 50)?,
    })
}

fn need_prompt(args: &Value) -> Result<String, String> {
    let prompt = need_str(args, "prompt", 2048)?;
    if prompt.is_empty() {
        return Err("prompt must not be empty".to_string());
    }
    Ok(prompt)
}

fn parse_then(v: Option<&str>) -> Result<Option<GenerateThen>, String> {
    match v {
        None => Ok(None),
        Some(s) => GenerateThen::from_slug(s).map(Some).ok_or_else(|| {
            "then must be mesh, video, world, character, matte, depth, or none".to_string()
        }),
    }
}

fn need_str(args: &Value, key: &'static str, max: usize) -> Result<String, String> {
    match optional_str(args, key)? {
        None => Err(format!("missing '{key}'")),
        Some(s) if s.is_empty() || s.len() > max => Err(format!("'{key}' out of bounds")),
        Some(s) => Ok(s.to_string()),
    }
}

fn need_op(args: &Value) -> Result<OperationId, String> {
    let s = need_str(args, "operation", 64)?;
    OperationId::parse(&s).ok_or_else(|| "malformed operation id".to_string())
}

fn parse_asset(s: &str) -> Result<AssetId, String> {
    AssetId::from_str(s).map_err(|_| "malformed asset id".to_string())
}

fn parse_alias(s: &str) -> Result<AssetAlias, String> {
    AssetAlias::from_str(s).map_err(|_| "malformed alias".to_string())
}

fn parse_revision(s: &str) -> Result<AssetRevisionId, String> {
    AssetRevisionId::from_str(s).map_err(|_| "malformed revision id".to_string())
}

/// Recursively sort object keys so semantically identical JSON hashes equal.
pub fn canonicalize_json(v: &Value) -> Value {
    match v {
        Value::Obj(pairs) => {
            let mut items: Vec<(String, Value)> = pairs
                .iter()
                .map(|(k, val)| (k.clone(), canonicalize_json(val)))
                .collect();
            items.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Obj(items)
        }
        Value::Arr(items) => Value::Arr(items.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

fn encode_image_prompt(
    prompt: &str,
    model: &Option<String>,
    width: &Option<u32>,
    height: &Option<u32>,
    steps: &Option<u32>,
    then: Option<GenerateThen>,
) -> Value {
    let mut pairs = vec![("prompt", json::s(prompt.to_string()))];
    if let Some(m) = model {
        pairs.push(("model", json::s(m.clone())));
    }
    if let Some(w) = width {
        pairs.push(("width", Value::Int(*w as i64)));
    }
    if let Some(h) = height {
        pairs.push(("height", Value::Int(*h as i64)));
    }
    if let Some(s) = steps {
        pairs.push(("steps", Value::Int(*s as i64)));
    }
    if let Some(then) = then {
        if then.is_follow_on() {
            pairs.push(("then", json::s(then.slug())));
        }
    }
    json::obj(pairs)
}

fn encode_prompt_model(prompt: &str, model: &Option<String>) -> Value {
    let mut pairs = vec![("prompt", json::s(prompt.to_string()))];
    if let Some(m) = model {
        pairs.push(("model", json::s(m.clone())));
    }
    json::obj(pairs)
}

/// Encode a typed call back to its wire argument object (used when
/// re-rendering history and in tests).
pub fn encode_args(call: &ContentToolCall) -> Value {
    match call {
        ContentToolCall::ImageGenerate { prompt, then, model, width, height, steps } => {
            encode_image_prompt(prompt, model, width, height, steps, *then)
        }
        ContentToolCall::VideoGenerate { prompt, model, width, height, frames, steps } => {
            let mut pairs = vec![("prompt", json::s(prompt.clone()))];
            if let Some(m) = model {
                pairs.push(("model", json::s(m.clone())));
            }
            if let Some(w) = width {
                pairs.push(("width", Value::Int(*w as i64)));
            }
            if let Some(h) = height {
                pairs.push(("height", Value::Int(*h as i64)));
            }
            if let Some(f) = frames {
                pairs.push(("frames", Value::Int(*f as i64)));
            }
            if let Some(s) = steps {
                pairs.push(("steps", Value::Int(*s as i64)));
            }
            json::obj(pairs)
        }
        ContentToolCall::AudioGenerate { prompt, model } => encode_prompt_model(prompt, model),
        ContentToolCall::SpeechGenerate { prompt, model, voice } => {
            let mut pairs = match encode_prompt_model(prompt, model) {
                Value::Obj(p) => p,
                other => return other,
            };
            if let Some(v) = voice {
                pairs.push(("voice".into(), json::s(v.clone())));
            }
            Value::Obj(pairs)
        }
        ContentToolCall::MusicGenerate { prompt, model, seconds } => {
            let mut pairs = match encode_prompt_model(prompt, model) {
                Value::Obj(p) => p,
                other => return other,
            };
            if let Some(s) = seconds {
                pairs.push(("seconds".into(), Value::Int(*s as i64)));
            }
            Value::Obj(pairs)
        }
        ContentToolCall::MeshGenerate { prompt, model, width, height, steps } => {
            encode_image_prompt(prompt, model, width, height, steps, None)
        }
        ContentToolCall::WorldGenerate { prompt, model, width, height, steps } => {
            encode_image_prompt(prompt, model, width, height, steps, None)
        }
        ContentToolCall::CharacterGenerate { prompt, model } => encode_prompt_model(prompt, model),
        ContentToolCall::DefaultsGet => Value::Obj(Vec::new()),
        ContentToolCall::DefaultsSet { image_model, width, height, steps, then } => {
            let mut pairs = Vec::new();
            if let Some(m) = image_model {
                pairs.push(("image_model", json::s(m.clone())));
            }
            if let Some(w) = width {
                pairs.push(("width", Value::Int(*w as i64)));
            }
            if let Some(h) = height {
                pairs.push(("height", Value::Int(*h as i64)));
            }
            if let Some(s) = steps {
                pairs.push(("steps", Value::Int(*s as i64)));
            }
            if let Some(then) = then {
                pairs.push(("then", json::s(then.slug())));
            }
            json::obj(pairs)
        }
        ContentToolCall::FleetIntrospect { domain } => {
            let mut pairs = Vec::new();
            if let Some(d) = domain {
                pairs.push(("domain", json::s(d.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::AssetSearch { query, limit } => json::obj(vec![
            ("query", json::s(query.clone())),
            ("limit", Value::Int(*limit as i64)),
        ]),
        ContentToolCall::AssetInspect { target } => match target {
            InspectTarget::Asset(a) => json::obj(vec![("asset", json::s(a.to_string()))]),
            InspectTarget::Alias(a) => json::obj(vec![("alias", json::s(a.to_string()))]),
            InspectTarget::Revision(r) => json::obj(vec![("revision", json::s(r.to_string()))]),
        },
        ContentToolCall::OperationCapabilities => Value::Obj(Vec::new()),
        ContentToolCall::OperationCreate { kind, inputs, params, publication, idempotency_key } => {
            let inputs_v = Value::Arr(
                inputs
                    .iter()
                    .map(|i| {
                        let mut pairs = vec![
                            ("slot", json::s(i.slot.clone())),
                            ("asset", json::s(i.asset.to_string())),
                            ("revision", json::s(i.revision.to_string())),
                            ("role", json::s(i.role.clone())),
                        ];
                        if let Some(t) = &i.tier {
                            pairs.push(("tier", json::s(t.clone())));
                        }
                        if let Some(l) = i.lod {
                            pairs.push(("lod", Value::Int(l as i64)));
                        }
                        if let Some(m) = &i.media {
                            pairs.push(("media", json::s(m.clone())));
                        }
                        json::obj(pairs)
                    })
                    .collect(),
            );
            let mut pairs = vec![
                ("kind", json::s(kind.clone())),
                ("inputs", inputs_v),
                ("params", params.clone()),
            ];
            match publication {
                PublicationArg::Publish => {}
                PublicationArg::PublishAndAlias { alias, expect } => {
                    let mut pub_pairs = vec![
                        ("mode", json::s("publish_and_alias")),
                        ("alias", json::s(alias.to_string())),
                    ];
                    match expect {
                        AliasExpectArg::Any => pub_pairs.push(("expect", json::s("any"))),
                        AliasExpectArg::Absent => pub_pairs.push(("expect", json::s("absent"))),
                        AliasExpectArg::Head(rev) => {
                            pub_pairs.push(("expect", json::s("head")));
                            pub_pairs.push(("expect_head", json::s(rev.to_string())));
                        }
                    }
                    pairs.push(("publication", json::obj(pub_pairs)));
                }
            }
            if let Some(k) = idempotency_key {
                pairs.push(("idempotency_key", json::s(k.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::OperationGet { operation } => {
            json::obj(vec![("operation", json::s(operation.to_string()))])
        }
        ContentToolCall::OperationWait { operation, after, timeout_ms } => json::obj(vec![
            ("operation", json::s(operation.to_string())),
            ("after", Value::Int((*after).min(i64::MAX as u64) as i64)),
            ("timeout_ms", Value::Int(*timeout_ms as i64)),
        ]),
        ContentToolCall::OperationCancel { operation } => {
            json::obj(vec![("operation", json::s(operation.to_string()))])
        }
        ContentToolCall::OperationRetry { operation } => {
            json::obj(vec![("operation", json::s(operation.to_string()))])
        }
        ContentToolCall::LlmConsult { task, prompt, provider } => {
            let mut pairs = vec![
                ("task", json::s(task.slug())),
                ("prompt", json::s(prompt.clone())),
            ];
            if let Some(kind) = provider {
                pairs.push(("provider", json::s(kind.slug())));
            }
            json::obj(pairs)
        }
    }
}
