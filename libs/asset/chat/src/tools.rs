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
use makepad_asset_data::{AssetAlias, AssetId, AssetRevisionId, ScalePreset};
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

/// The complete reviewed CSG teaching payload. Keep this one block small
/// enough to sit beside the whole game API in a 12k local-model context.
pub const CSG_MODEL_TOOL_DOC: &str = r#"CSG MODELLING (csg.* only inside model.build source). Models and editable source are stored at gen/csg/<title-slug>. asset.search first; reuse with world.place, or model.fetch then edit and model.build with the SAME title to revise that asset (NEW title for a variant). Build immutable SOLIDS, then name them into PARTS. Metres, Y-up, y=0 floor, face +Z. One color per part; differently colored details are separate parts. box/cylinder/extrude stand on y=0 centered x,z; sphere/torus center at origin; lathe uses profile y. Declare at least one part. Budgets: 2000 ops, 32 parts, 150000 triangles, 12000 source bytes, 30s including binding.
csg.box({size}) -> solid; size vec3 metres
csg.sphere({r, seg}) -> solid
csg.cylinder({r, h, r2, seg}) -> solid; r2 tapers top (0 cone); low seg makes prisms
csg.torus({r, tube, seg}) -> solid; flat ring
csg.extrude([vec2(x,z),...], {h, twist, taper, seg}) -> solid; closed CCW outline upward
csg.lathe([vec2(r,y),...], {angle, seg}) -> solid; spin profile around Y
csg.union(a,b,...) -> solid
csg.difference(a,b,...) -> solid; a minus the rest
csg.intersect(a,b,...) -> solid
csg.move(s, vec3) -> solid
csg.rotate(s, {x,y,z}) -> solid; degrees x then y then z, before move
csg.scale(s, n | vec3) -> solid
csg.mirror(s, "x"|"y"|"z") -> solid
csg.implicit(fn, {bounds:[vec3(min),vec3(max)], res, uniforms?}) -> solid; sample pure signed-distance math; res power-of-two 8..128
csg.part(name, solid, {color, parent, pivot}); parent must be an earlier part, pivot defaults AABB center
csg.anim(part, {kind:"swing"|"spin"|"bob", axis:"x"|"y"|"z", degrees, hz, amp}); idle motion through pivot
Defaults: seg 24 (3..64), color #cccccc, angle 360, taper 1, twist 0; anim axis x, degrees 25, hz 2, amp 0.1. Part names match [a-z0-9-]{1,24}.

SMOOTH IMPLICIT BLEND (smin idiom):
let field=|p,c,k| {let a=length(p-c)-0.55 let b=length(p+c)-0.55 let h=clamp(0.5+0.5*(b-a)/k,0,1) mix(b,a,h)-k*h*(1-h)}
let blob=csg.implicit(field,{bounds:[vec3(-1,-0.8,-0.8),vec3(1,0.8,0.8)],res:32,uniforms:[vec3(0.32,0,0),0.22]})
csg.part("blend",blob,{color:#55aadd})

Without joints, animated limbs must be PARTS; csg.anim is rigid node motion.

DOG (rigid parts, +z facing; diagonal legs share phase):
csg.part("body",csg.move(csg.box({size:vec3(0.24,0.22,0.5)}),vec3(0,0.28,0)),{color:#8b5a2b})
let leg=csg.cylinder({r:0.035,h:0.24})
for i in 0..4 {
  let x=if i%2==0 {-0.08} else {0.08}
  let z=if i<2 {0.17} else {-0.17}
  let n=["fl","fr","bl","br"][i]
  csg.part(n,csg.move(leg,vec3(x,0,z)),{color:#5a351d,parent:"body",pivot:vec3(x,0.26,z)})
  csg.anim(n,{kind:"swing",axis:"x",degrees:if i==0||i==3 {30} else {-30},hz:2})
}
csg.part("head",csg.move(csg.sphere({r:0.11}),vec3(0,0.52,0.3)),{color:#8b5a2b,parent:"body"})

WEIGHTED RIG (optional, nonhumanoid names allowed):
csg.joint(name,{pos:vec3,parent?}); MODEL-space rest positions +/-50m, identity rest rotation/scale; parent may be declared later. Local offsets and inverse binds are computed.
csg.bind(part,{joints:[names],radius:0.1}); smooth automatic binding ONLY to selected joints. Each joint owns parent-to-joint segment (root is point). Weight=1/(distanceSquared+radiusSquared); keep top four, normalize; ties use joint declaration order. Radius 0.001..50m. Approximation, not anatomical rigging; provide enough mesh subdivisions to bend.
csg.bind(part,{rigid:"joint"}); rigid shell override, exactly one influence
csg.bind(part,{weights:[{joint:"a",weight:0.25},{joint:"b",weight:0.75}]}); exact whole-part 1..4 weights summing to one. Exact binds override automatic calls regardless of order; last exact wins. One mode per bind call.
csg.clip(name,[{joint:"a",axis:"z",keys:[vec2(0,0),vec2(0.5,45),vec2(1,0)]}]); keys=(seconds,degrees), local-axis rotations with quaternion interpolation. Start 0, strictly increasing through <=60s; degrees +/-180, steps <=180; channels end together. One rotation channel per joint per clip.
Rig limits: 64 joints, 16 clips, 128 keys/channel, 4096 total keys. All parts MUST bind; use rigid for shells. No part parent/pivot/csg.anim in rig documents. Opaque colors preserved in embedded palette. Invalid rigs fail; previews remain rigid until final skin. No joints means unchanged legacy rigid behavior.
For walking characters name clips idle and walk (both required by existing character loader); game.character({model:"gen/csg/title",player:true}) or world.spawn({model:"gen/csg/title",form:"character"}). See libs/csg/csg/examples/spriglet.splash for editable original seedling."#;

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
            args_doc: r#"{"prompt": "motion description", "model": "fasth3-4step", "width": 640, "height": 352, "frames": 39, "steps": 4}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len("full motion / scene description for the video model", 1, 2048),
                    ),
                    (
                        "model",
                        schema_string_len("video model id; omit for fleet affinity (defaults to the fast FastH3 lane, fasth3-4step; minimax-h3 = the 30/50-step base)", 1, 64),
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
            args_doc: r#"{"prompt": "warm neo-soul, Rhodes, intimate male vocal", "lyrics": "[Verse]\nMorning settles on the floor\n\n[Chorus]\nWe can rise into the day", "model": "minimax-music3", "seconds": 180}"#,
            parameters: schema_object(
                vec![
                    (
                        "prompt",
                        schema_string_len(
                            "musical description: genre, mood, tempo, instruments, \
                             vocal type, production. Never put lyric words here",
                            1,
                            2048,
                        ),
                    ),
                    (
                        // The field that makes a song a song. Empty means
                        // instrumental to both music backends, so a vocal
                        // request that leaves it out comes back as a hum.
                        "lyrics",
                        schema_string_len(
                            "the sung words, as a temporal script: section tags such as \
                             [Verse]/[Chorus] alone on their own lines. \
                             \"[Instrumental]\" for instrumental music",
                            1,
                            4096,
                        ),
                    ),
                    (
                        "model",
                        schema_string_len("music model id; omit for minimax-music3", 1, 64),
                    ),
                    (
                        "seconds",
                        schema_integer_range("song length 5..=300; omit for 180", 5, 300),
                    ),
                    (
                        "steps",
                        schema_integer_range("diffusion steps 1..=64; omit for the model's own", 1, 64),
                    ),
                    (
                        "seed",
                        schema_integer_range("reproducibility seed; omit for a fresh one", 0, i64::MAX),
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
            description: "Generate and publish a character through expanded prompt → image → \
                          matte → mesh → rig → motion. Waits for owned jobs and returns \
                          intermediate aliases/revisions and measured skin/clip/playable metadata. \
                          Unavailable stages are reported honestly; creation does not place it.",
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
            description: "Search the asset catalog. Judge hits: a title merely CONTAINING \
                          the word is not the thing (corn-dog is not a dog).",
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

/// Most bytes of SQL text `assets.query` accepts. The executor's own
/// row/step/deadline budgets bound what that SQL may then cost.
pub const MAX_QUERY_SQL_BYTES: usize = 4096;
/// Most placements one `world.place` call may carry (a fence is one call,
/// not one call per segment — tool rounds are budgeted).
pub const MAX_WORLD_PLACEMENTS: usize = 64;

/// The GAME session's tool vocabulary: catalog lookups, one deliberately
/// narrow owned-generation entry point, and the game extension. The richer
/// Asset UI generation controls remain app-local and `llm.consult` remains
/// unavailable here.
pub fn game_definitions() -> Vec<ToolDef> {
    const KEEP: &[&str] = &["asset.search", "asset.inspect", "character.generate"];
    definitions()
        .into_iter()
        .filter(|d| KEEP.contains(&d.name))
        .chain(sandbox_definitions())
        .collect()
}

/// The game-session tool extension: read-only SQL over the live asset
/// catalog (executed by the broker, next to its own catalog file) and the
/// world tools (executed by the connected GAME CLIENT via the parked-turn
/// round trip). NOT part of [`definitions`] — only sessions created with
/// the game profile advertise these.
pub fn sandbox_definitions() -> Vec<ToolDef> {
    let mut defs = vec![
        ToolDef {
            name: "content.generate",
            api_name: "content_generate",
            description: "Run ONE owned asset-generation pipeline after searching the library. \
                          Character: expanded prompt → image → matte → mesh → rig → motion; \
                          prop: image → mesh; sound: audio. Waits for completion and publication, \
                          returning aliases/revisions and measured character metadata. Missing \
                          capabilities return Unavailable. Creation is separate from placement.",
            args_doc: r#"{"kind": "character", "prompt": "...", "dim_height": 1.75}"#,
            parameters: schema_object(
                vec![
                    (
                        "kind",
                        schema_string_enum(
                            "the finished asset the game needs",
                            &["character", "prop", "sound"],
                        ),
                    ),
                    (
                        "prompt",
                        schema_string_len("concise generation brief", 1, 2048),
                    ),
                    (
                        "dim_height",
                        schema_number("intended placement height in metres, 0.01..=100; returned as metadata, does not resize mesh or image pixels"),
                    ),
                ],
                &["kind", "prompt"],
                Some(false),
            ),
        },
        ToolDef {
            name: "assets.query",
            api_name: "query_assets",
            description: "Run ONE read-only SQL SELECT against the live asset catalog \
                          (SQLite). Explore with assets.schema first. Filter live=1. \
                          Results are capped (~200 rows) — narrow with WHERE/LIMIT. \
                          Writes, PRAGMA, ATTACH and multi-statement SQL are refused.",
            args_doc: r#"{"sql": "SELECT canon_alias, kind FROM search_annotations WHERE live=1 AND kind='prop' AND canon_alias LIKE '%fence%' LIMIT 20"}"#,
            parameters: schema_object(
                vec![(
                    "sql",
                    schema_string_len(
                        "a single SELECT statement",
                        1,
                        MAX_QUERY_SQL_BYTES as i64,
                    ),
                )],
                &["sql"],
                Some(false),
            ),
        },
        ToolDef {
            name: "assets.schema",
            api_name: "assets_schema",
            description: "The asset catalog's tables, columns and indexes, with usage \
                          notes. Call this before writing SQL for assets.query.",
            args_doc: r#"{}"#,
            parameters: schema_object(vec![], &[], Some(false)),
        },
        ToolDef {
            name: "model.build",
            api_name: "model_build",
            description: CSG_MODEL_TOOL_DOC,
            args_doc: r#"{"title":"Blue Mug","source":"let outer=csg.cylinder({r:0.045,h:0.09})\nlet bore=csg.move(csg.cylinder({r:0.038,h:0.09}),vec3(0,0.008,0))\ncsg.part(\"mug\",csg.difference(outer,bore),{color:#4477aa})"}"#,
            parameters: schema_object(
                vec![
                    ("title", schema_string_len("model title; also supplies its stable alias slug", 1, MAX_MODEL_TITLE_BYTES as i64)),
                    ("source", schema_string_len("complete bounded CSG Splash program", 1, MAX_MODEL_SOURCE_BYTES as i64)),
                ],
                &["title", "source"],
                Some(false),
            ),
        },
        ToolDef {
            name: "model.fetch",
            api_name: "model_fetch",
            description: "Fetch the editable CSG Splash source stored in a published model-program asset. Use before editing, then pass the complete edited program to model.build with the same title so the same alias and asset receive a new revision.",
            args_doc: r#"{"alias":"gen/csg/copper-mug"}"#,
            parameters: schema_object(
                vec![("alias", schema_string_len("gen/csg/<slug> alias", 9, 128))],
                &["alias"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.place",
            api_name: "world_place",
            description: "Place catalog models into the running game world. Each item \
                          names a model by its canon_alias (from assets.query or \
                          asset.search) plus a position in metres (y up; y=0 is the \
                          ground). Place a whole group (a fence line, a furniture set) \
                          in ONE call. Returns the placement ids and the world's \
                          evaluation result.",
            args_doc: r#"{"items": [{"model": "kenney/props/fence", "pos": [4, 0, 2], "yaw_deg": 90, "scale": 1.0, "tag": "fence"}]}"#,
            parameters: schema_object(
                vec![(
                    "items",
                    schema_array_bounded(
                        "1..=64 placements: required model + pos; optional yaw_deg, scale, tag",
                        1,
                        MAX_WORLD_PLACEMENTS as i64,
                        schema_world_place_item(),
                    ),
                )],
                &["items"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.remove",
            api_name: "world_remove",
            description: "Remove earlier world.place placements: by placement ids, or \
                          every placement carrying a tag. Exactly one of ids/tag.",
            args_doc: r#"{"ids": [3, 4]} or {"tag": "fence"}"#,
            parameters: schema_object(
                vec![
                    (
                        "ids",
                        schema_array_bounded(
                            "placement ids from world.place/world.list",
                            1,
                            MAX_WORLD_PLACEMENTS as i64,
                            schema_integer_range("placement id", 1, 1_000_000_000),
                        ),
                    ),
                    ("tag", schema_ident("remove every placement with this tag")),
                ],
                &[],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.move",
            api_name: "world_move",
            description: "Change one existing placement's position, rotation, or scale. \
                          At least one of pos/yaw_deg/scale.",
            args_doc: r#"{"id": 3, "pos": [6, 0, 2], "yaw_deg": 45}"#,
            parameters: schema_object(
                vec![
                    ("id", schema_integer_range("placement id", 1, 1_000_000_000)),
                    ("pos", schema_pos()),
                    ("yaw_deg", schema_number("rotation about y, degrees")),
                    ("scale", schema_number("uniform scale, 0.001..=1000")),
                ],
                &["id"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.list",
            api_name: "world_list",
            description: "List the current AI placements in the world (id, model, pos, \
                          yaw_deg, scale, tag).",
            args_doc: r#"{}"#,
            parameters: schema_object(vec![], &[], Some(false)),
        },
        ToolDef {
            name: "world.spawn",
            api_name: "world_spawn",
            description: "ADD one thing to the running world, placed on the ground near \
                          the player — THE way to do 'give me / add an X'. No source \
                          read, no rewrite, nothing else in the world changes. The game \
                          picks the right form automatically: vehicles spawn DRIVEABLE, \
                          rigged characters spawn as wanderers (the ambient behaviour \
                          class — a later game.chaser/sentry/follower call retunes \
                          them), everything else is a grounded prop at a sane scale. Use \
                          form: \"follower\" for a character body that follows the player; \
                          rigged and unrigged loadable models both work as bodies. Query the catalog for the \
                          canon_alias first. If no suitable creature asset exists and a \
                          primitive part-built creature is wanted, use world.add_addon \
                          with the worked game-context example instead. Use \
                          world.set_source only for NEW levels or \
                          GAME-LOGIC changes (rules, scoring, objectives) — never for \
                          adding content. A 'small car' is world.spawn with scale: 0.5 \
                          or \"small\" and stays driveable; world.place makes static scenery. \
                          Color tints any model instance; hue rotates textured colors, so ten \
                          differently-colored copies of one asset need no rebuilds.",
            args_doc: r##"{"model": "kenney/car-kit/ambulance", "scale": "small", "color": "#44aaff", "hue": 30}"##,
            parameters: schema_object(
                vec![
                    (
                        "model",
                        schema_string_len(
                            "catalog canon_alias of the thing to add",
                            1,
                            MAX_MODEL_REF_CHARS as i64,
                        ),
                    ),
                    ("pos", schema_pos()),
                    ("scale", schema_spawn_scale()),
                    (
                        "color",
                        schema_string_len("instance tint as #rrggbb or #rrggbbaa", 7, 9),
                    ),
                    ("hue", schema_number("albedo hue rotation in degrees")),
                    (
                        "form",
                        schema_string_len(
                            "optional override: car | character | follower | prop (default: derived \
                             from the asset)",
                            1,
                            16,
                        ),
                    ),
                    ("tag", schema_ident("optional group tag")),
                ],
                &["model"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.add_addon",
            api_name: "world_add_addon",
            description: "ADD MANY things in ONE call: append a small self-contained \
                          splash chunk (loops welcome) to the running world under a \
                          named addon marker — THE way to do 'make me a forest', 'add \
                          a crowd', or a primitive build the library lacks. The chunk \
                          evaluates against the LIVE world: nothing resets, nothing \
                          else changes, and world.remove({tag: name}) undoes it. The same \
                          name replaces its existing marker block on retry. Splash has NO \
                          ternary `?:` — use if/else; loops are `for i in 0..n {}`. The \
                          chunk may not re-declare the world (no game.terrain / \
                          game.sky / game.map / game.player_character) — those are \
                          world.set_source territory. BIG builds: prefer 2-3 \
                          world.add_addon chunks over one giant world.set_source — long \
                          single calls can truncate. For a primitive creature, make ONE \
                          mover body, create owner-local game.part attachments once, add \
                          game.part_swing for gait, and give the body a follower/chaser/ \
                          pacer class. Never reposition parts from game.on_tick. PLACE \
                          spawned creatures NEAR THE PLAYER: `let p = game.player_pos()` \
                          then pos: p + vec3(2, 0.55, 0) for game.mover/game.character \
                          (their y is absolute) — an absolute guess like vec3(0,0,2) \
                          lands 50 m away where nobody sees it. game.model's pos.y is \
                          height ABOVE the ground: use vec3(p.x + 2, 0, p.z) there, \
                          never p + vec3(...) — p.y is an absolute feet height and \
                          buries the model.",
            args_doc: r#"{"name": "forest", "src": "for i in 0..12 {\n  game.model(\"kenney/nature-kit/tree_oak\", {pos: vec3(i * 3, 0, 8), scale: 2})\n}"}"#,
            parameters: schema_object(
                vec![
                    ("name", schema_ident("short addon name (also its remove tag)")),
                    (
                        "src",
                        schema_string_len(
                            "self-contained splash lines to install under this name",
                            1,
                            MAX_ADDON_SRC_BYTES as i64,
                        ),
                    ),
                ],
                &["name", "src"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.tune",
            api_name: "world_tune",
            description: "Adjust a WORLD knob on the running level in ONE cheap call — \
                          nothing else changes, nothing resets. Knobs: time (0-24 local \
                          hours; 0 midnight, 12 noon, 22 night) and car_speed (scale on \
                          EVERY car's speed, 0.2-5, 1 = as authored). THE way to do \
                          'make it night / morning / sunset' and 'make the cars faster / \
                          slower' (0.6 slower, 1.6 faster). Other tuning still uses the \
                          source path.",
            args_doc: r#"{"time": 22} or {"car_speed": 0.6}"#,
            parameters: schema_object(
                vec![
                    ("time", schema_number("local hour 0-24")),
                    (
                        "car_speed",
                        schema_number("speed scale for every car, 0.2-5 (1 = as authored)"),
                    ),
                ],
                &[],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.get_source",
            api_name: "world_get_source",
            description: "Read the running game's current splash source. Call this before \
                          world.set_source so an edit starts from what is actually running.",
            args_doc: r#"{}"#,
            parameters: schema_object(vec![], &[], Some(false)),
        },
        ToolDef {
            name: "world.api",
            api_name: "world_api",
            description: "Read-only discovery of the running engine's actual game verb signatures, docs and examples, plus source/CSG chat tool contracts. Query a verb or topic (game.ui, shader, race, model.build); empty query browses. Follow next_cursor with the SAME query. Documentation does not grant mutation permission or change Guided/Expert policy.",
            args_doc: r#"{"query":"game.ui","limit":8,"cursor":0}"#,
            parameters: schema_object(vec![
                ("query", schema_string_len("verb name or search words; empty browses", 0, MAX_WORLD_API_QUERY_BYTES as i64)),
                ("limit", schema_integer_range("maximum entries per page; default 8", 1, 20)),
                ("cursor", schema_integer_range("next_cursor from the previous page; default 0", 0, 1_000_000)),
            ], &[], Some(false)),
        },
        ToolDef {
            name: "world.get_plan",
            api_name: "world_get_plan",
            description: "Read the running map's PLAN: the normalized world.plan input \
                          (v, seed, biome, terrain, landforms, water, corridors, places, \
                          dressing — every feature with its id), its `revision`, the last \
                          solve's `diagnostics` and the engine's `capabilities` (the kinds \
                          it accepts). THE way to inspect or change a map: call it, edit \
                          the object it returns, send it back with world.set_plan.",
            args_doc: r#"{}"#,
            parameters: schema_object(vec![], &[], Some(false)),
        },
        ToolDef {
            name: "world.set_plan",
            api_name: "world_set_plan",
            description: "Replace the running map's plan with a COMPLETE edited plan object \
                          (start from world.get_plan's `plan`) and re-solve the map. \
                          `revision` MUST be the revision world.get_plan returned — a stale \
                          revision is refused and nothing changes; read again. The engine \
                          writes the world.plan call into the level source itself and keeps \
                          everything after it (player, cars, logic). Every feature needs a \
                          unique `id`; a kind outside `capabilities` is refused by name; \
                          errors refuse the WHOLE plan (nothing changes). Returns the new \
                          `revision`, the resolved plan, `diagnostics` and `committed: true` \
                          only once the world is built and installed. Corridors default to \
                          required: true here; set required: false to permit an explicit \
                          optional omission. Inspect fulfilled separately from installed.",
            args_doc: r#"{"revision": 3, "plan": {"v": 1, "seed": 7, "biome": "alpine", "terrain": {"size": 200, "relief": "hilly"}, "water": [{"id": "brook", "kind": "river", "from": "west", "to": "east", "width": 9}], "corridors": [{"id": "high", "kind": "road", "from": "north", "to": "mill:east"}], "places": [{"id": "mill", "kind": "village", "at": "brook:south_bank", "size": "small"}]}, "note": "removed the railway"}"#,
            parameters: schema_object(
                vec![
                    (
                        "plan",
                        plan_schema(),
                    ),
                    ("revision", schema_integer_range("the `revision` world.get_plan returned (0 for a level with no plan yet)", 0, 1_000_000_000)),
                    ("note", schema_string_len("one line saying what changed", 1, 200)),
                ],
                &["plan", "revision"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.set_source",
            api_name: "world_set_source",
            description: "Replace the running game's splash source with a COMPLETE new \
                          version (level authoring). This ERASES whatever the new text \
                          does not restate — every spawn, addon and runtime thing the \
                          user already has. NEVER for adding content ('add X', 'make me \
                          a forest' = world.spawn / world.add_addon, which add without \
                          erasing); only for NEW levels and GAME-LOGIC edits, built on \
                          world.get_source's text. The game evaluates it and hot-reloads; \
                          on an eval error the previous world keeps running and the error \
                          comes back so you can fix the source and retry. Reference store \
                          content by alias via game.model(\"<canon_alias>\", ...). Keep the \
                          source under 12000 bytes. Splash has NO ternary `?:` — use \
                          if/else; loops are `for i in 0..n {}`. BIG builds: prefer 2-3 \
                          world.add_addon chunks over one giant world.set_source — long \
                          single calls can truncate. Guided/Auto adds default terrain \
                          when no ground is declared (unless `// ground: none`). Expert \
                          preserves exact source: deliberate floorless layouts and custom \
                          assembly need no special comments. In every mode preserve \
                          authored scripts and generated code not targeted by the edit.",
            args_doc: r#"{"source": "game.sky({})\ngame.terrain({size: 120, cells: 65, smooth: true})\n...", "note": "village level v1"}"#,
            parameters: schema_object(
                vec![
                    (
                        "source",
                        schema_string_len(
                            "the complete splash source",
                            1,
                            MAX_WORLD_SOURCE_BYTES as i64,
                        ),
                    ),
                    ("note", schema_string_len("short change description", 1, 200)),
                ],
                &["source"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.new_level",
            api_name: "world_new_level",
            description: "Create a NEW game from this source and switch the player to it — \
                          for a new level/world; the current conversation ends there (the \
                          new game has its own chat). Use world.set_source only for edits of \
                          the CURRENT game. `source` is a COMPLETE splash level (same rules \
                          as world.set_source: game.terrain or game.map first, models by \
                          canon_alias, under 12000 bytes); `title` names the new game. The \
                          game publishes it, switches, and answers with the new game's \
                          asset_id, alias and title — report those and stop.",
            args_doc: r#"{"title": "Quarry Arena", "source": "game.sky({})\ngame.terrain({size: 120, cells: 65, smooth: true})\n...", "note": "first cut"}"#,
            parameters: schema_object(
                vec![
                    (
                        "title",
                        schema_string_len(
                            "the new game's title",
                            1,
                            MAX_NEW_LEVEL_TITLE_BYTES as i64,
                        ),
                    ),
                    (
                        "source",
                        schema_string_len(
                            "the complete splash source of the new level",
                            1,
                            MAX_WORLD_SOURCE_BYTES as i64,
                        ),
                    ),
                    ("note", schema_string_len("short description", 1, 200)),
                ],
                &["title", "source"],
                Some(false),
            ),
        },
        ToolDef {
            name: "world.set_player_model",
            api_name: "world_set_player_model",
            description: "Swap the PLAYER's character to this catalog model IN PLACE — no \
                          source read, no rewrite, nothing else in the world changes. THE \
                          way to do 'let me play as X': query kind='character' for the \
                          alias, then call this. Rigged 'character' aliases only.",
            args_doc: r#"{"model": "kenney/mini-characters/character-male-b"}"#,
            parameters: schema_object(
                vec![(
                    "model",
                    schema_string_len(
                        "catalog canon_alias of a rigged character",
                        1,
                        MAX_MODEL_REF_CHARS as i64,
                    ),
                )],
                &["model"],
                Some(false),
            ),
        },
    ];
    for def in &mut defs {
        if def.name.starts_with("world.") && def.name != "world.new_level" {
            add_optional_sub(&mut def.parameters);
        }
    }
    defs
}

fn add_optional_sub(schema: &mut Value) {
    let Value::Obj(root) = schema else { return };
    let Some((_, Value::Obj(properties))) = root.iter_mut().find(|(key, _)| key == "properties")
    else { return };
    properties.push((
        "sub".into(),
        schema_string_len(
            "named sub-world; omit for the conversation/player default",
            1,
            64,
        ),
    ));
}

/// Most bytes of splash source `world.set_source` accepts (the tool wire
/// caps whole argument objects at 16 KiB; escaping needs headroom).
pub const MAX_WORLD_SOURCE_BYTES: usize = 12_000;

/// `world.new_level` title: a game name, not a paragraph.
pub const MAX_NEW_LEVEL_TITLE_BYTES: usize = 80;

/// An addon is a SMALL self-contained chunk by design — a bulk add is
/// 10-30 lines. Anything larger is a level, and levels go through
/// `world.set_source` with its own cap.
pub const MAX_ADDON_SRC_BYTES: usize = 4_000;
pub const MAX_MODEL_SOURCE_BYTES: usize = 12_000;
pub const MAX_MODEL_TITLE_BYTES: usize = 80;
pub const MAX_WORLD_API_QUERY_BYTES: usize = 160;

fn schema_world_place_item() -> Value {
    schema_object(
        vec![
            (
                "model",
                schema_string_len("catalog canon_alias or library model id", 1, MAX_MODEL_REF_CHARS as i64),
            ),
            ("pos", schema_pos()),
            ("yaw_deg", schema_number("rotation about y, degrees; default 0")),
            ("scale", schema_number("uniform scale 0.001..=1000; default 1")),
            ("tag", schema_ident("optional group tag for world.remove")),
        ],
        &["model", "pos"],
        Some(false),
    )
}

fn schema_pos() -> Value {
    schema_array_bounded(
        "world position [x, y, z] in metres (y up)",
        3,
        3,
        schema_number("coordinate in metres"),
    )
}

fn schema_spawn_scale() -> Value {
    json::obj(vec![
        (
            "description",
            json::s("driveable/character play scale: number 0.2..=3 or dimensions preset"),
        ),
        (
            "anyOf",
            Value::Arr(vec![
                json::obj(vec![
                    ("type", json::s("number")),
                    ("minimum", Value::F64(0.2)),
                    ("maximum", Value::F64(3.0)),
                ]),
                schema_string_enum("dimensions scale preset", &["real", "comic", "small", "handheld"]),
            ]),
        ),
    ])
}

fn schema_number(description: &str) -> Value {
    json::obj(vec![
        ("type", json::s("number")),
        ("description", json::s(description)),
    ])
}

/// Normalize a tool name the model spelled in ANY of its observed ways
/// onto the dotted canonical: the declared api_name (`query_assets`), a
/// mechanical underscoring of the dotted name (`assets_query`,
/// `world_set_source`), or the canonical itself. Unknown names return
/// unchanged and fail closed in the typed parser.
pub fn canonicalize_tool_name(raw: &str) -> String {
    if let Some(canonical) = canonical_from_api_name(raw) {
        return canonical.to_string();
    }
    if raw.contains('_') {
        for def in definitions().into_iter().chain(sandbox_definitions()) {
            if def.name.replace('.', "_") == raw {
                return def.name.to_string();
            }
        }
    }
    raw.to_string()
}

/// Inverse of [`canonical_from_api_name`]: the underscore API (trained
/// template) spelling of a dotted canonical name.
pub fn api_from_canonical(name: &str) -> Option<String> {
    definitions()
        .into_iter()
        .chain(sandbox_definitions())
        .find(|d| d.name == name)
        .map(|d| d.api_name.to_string())
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
        "content_generate" => Some("content.generate"),
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
        "query_assets" => Some("assets.query"),
        "assets_schema" => Some("assets.schema"),
        "world_place" => Some("world.place"),
        "world_remove" => Some("world.remove"),
        "world_move" => Some("world.move"),
        "world_list" => Some("world.list"),
        "world_get_source" => Some("world.get_source"),
        "world_api" => Some("world.api"),
        "world_get_plan" => Some("world.get_plan"),
        "world_set_plan" => Some("world.set_plan"),
        "world_set_source" => Some("world.set_source"),
        "world_new_level" => Some("world.new_level"),
        "world_set_player_model" => Some("world.set_player_model"),
        "world_spawn" => Some("world.spawn"),
        "world_tune" => Some("world.tune"),
        "world_add_addon" => Some("world.add_addon"),
        "model_build" => Some("model.build"),
        "model_fetch" => Some("model.fetch"),
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
        /// The sung words. Separate from `prompt` because both music
        /// backends take them separately — and because an empty lyric field
        /// is exactly what makes MiniMax generate an instrumental.
        lyrics: Option<String>,
        steps: Option<u32>,
        seed: Option<u64>,
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
    ContentGenerate {
        kind: ContentGenerateKind,
        prompt: String,
        dim_height: Option<f64>,
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
    /// One read-only SELECT over the live catalog (sandbox sessions only;
    /// the executor's SQL engine enforces read-only at the AST level).
    AssetsQuery { sql: String },
    /// Catalog table/column summary (sandbox sessions only).
    AssetsSchema,
    /// Build or revise a bounded local exact-polygonal CSG program.
    ModelBuild { title: String, source: String },
    /// Fetch the authoritative CSG source for an existing generated alias.
    ModelFetch { alias: AssetAlias },
    /// Place models into the running game world (sandbox sessions only).
    WorldPlace { items: Vec<WorldPlaceItem> },
    /// Remove placements by id or by tag (exactly one of the two).
    WorldRemove { ids: Vec<u64>, tag: Option<String> },
    /// Re-pose one placement.
    WorldMove { id: u64, pos: Option<[f64; 3]>, yaw_deg: Option<f64>, scale: Option<f64> },
    /// List current placements.
    WorldList,
    /// Read the running game's splash source (sandbox sessions only).
    WorldGetSource,
    /// Read-only live engine vocabulary lookup; never evaluates source.
    WorldApi { query: String, limit: u32, cursor: u32 },
    /// Read the running map's plan (normalized world.plan input, revision,
    /// diagnostics, capabilities) — sandbox sessions only.
    WorldGetPlan,
    /// Replace the running map's plan with a complete edited plan object,
    /// guarded by the revision it was read at (a stale revision is refused).
    /// The plan is a typed JSON object the client validated by shape; the
    /// engine's schema check refuses the rest by name.
    WorldSetPlan { plan: Value, revision: u64, note: Option<String> },
    /// Replace the running game's splash source — the level-authoring
    /// primary path (sandbox sessions only; evaluated with last-good
    /// rollback on the client).
    WorldSetSource { source: String, note: Option<String> },
    /// Publish `source` as a NEW game and switch the player to it (sandbox
    /// sessions only; client-executed). The turn ENDS on the client's
    /// answer — the new game has its own conversation.
    WorldNewLevel { title: String, source: String, note: Option<String> },
    /// Swap the player's character rig in place — no source round-trip,
    /// structurally incapable of resetting the world (the cheap §4.5-style
    /// verb; sandbox sessions only).
    WorldSetPlayerModel { model: String },
    /// ADD one catalog thing to the running world near the player — the
    /// content-add verb (§4.5 addon slice; sandbox sessions only). The
    /// GAME picks position (ground-snapped near the player unless `pos`
    /// overrides), scale and the right form (vehicle/character/prop, or
    /// `form` overrides); no source round-trip, structurally incapable of
    /// resetting the world.
    WorldSpawn {
        model: String,
        pos: Option<[f64; 3]>,
        form: Option<SpawnForm>,
        scale: Option<SpawnScale>,
        color: Option<String>,
        hue: Option<f64>,
        tag: Option<String>,
    },
    /// Adjust a WORLD knob on the running level in one cheap call (§4.5
    /// tune slice; sandbox sessions only). World knobs are idempotent
    /// splash setters, so the tune rides the addon lane — nothing resets.
    /// Knobs: `time` (0-24 local hours) and `car_speed` (0.2-5 around the
    /// authored speed — "make the cars faster/slower" for the whole fleet).
    /// At least one must be present; both may arrive in one call.
    WorldTune { time: Option<f64>, car_speed: Option<f64> },
    /// Append one self-contained splash chunk to the running world under a
    /// named `// @addon:` marker (the §4.5 GENERAL verb; sandbox sessions
    /// only). Bulk adds and primitive builds land as ONE 10-30 line chunk —
    /// no source echo, addon-lane eval, removable by name. The client
    /// refuses chunks that re-declare the world.
    WorldAddAddon { name: String, src: String },
    /// An explicit named source target around any ordinary `world.*` call.
    /// Keeping targeting orthogonal prevents the command variants and their
    /// execution semantics from drifting apart.
    WorldInSub { sub: String, call: Box<ContentToolCall> },
}

/// The three game-facing, publishable pipeline families. This is closed on
/// purpose: the game profile cannot turn arbitrary strings into store jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentGenerateKind {
    Character,
    Prop,
    Sound,
}

impl ContentGenerateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Prop => "prop",
            Self::Sound => "sound",
        }
    }

    pub fn job_kind(self) -> &'static str {
        match self {
            // These are the composite pipeline profiles used by the Asset
            // UI scheduler; audio is already a single publishable stage.
            Self::Character => "character.generate",
            Self::Prop => "mesh_from_image",
            Self::Sound => "audio.generate",
        }
    }
}

/// The spawn form override of [`ContentToolCall::WorldSpawn`]. Absent, the
/// game derives it from the asset (rigged → character, vehicle kit → car,
/// else prop). `Follower` is an explicit character body plus the player-follow
/// class; its model may be rigged or rigid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnForm {
    Car,
    Character,
    Follower,
    Prop,
}

/// A world-spawn play size, preserved as authored until the client maps it to
/// a driveable or character verb. Presets are the shared dimensions-contract
/// vocabulary; exact values use the engine's safe 0.2..=3 band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpawnScale {
    Exact(f64),
    Preset(ScalePreset),
}

impl SpawnForm {
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "car" | "vehicle" => Some(SpawnForm::Car),
            "character" | "npc" => Some(SpawnForm::Character),
            "follower" => Some(SpawnForm::Follower),
            "prop" | "model" => Some(SpawnForm::Prop),
            _ => None,
        }
    }
}

/// Longest model reference `world.place` accepts (canon aliases run long:
/// `doom/doom/worlds/doom1/e1m1`).
pub const MAX_MODEL_REF_CHARS: usize = 160;

/// One placement of `world.place`, bounds-checked at parse: the model
/// reference is plain printable text WITHOUT quotes/backslashes (it is
/// spliced into game source as a string literal — refusing the characters
/// beats escaping them), positions are finite and within the world's
/// numeric range, scale is sane.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldPlaceItem {
    pub model: String,
    pub pos: [f64; 3],
    pub yaw_deg: Option<f64>,
    pub scale: Option<f64>,
    pub tag: Option<String>,
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
            ContentToolCall::ContentGenerate { .. } => "content.generate",
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
            ContentToolCall::AssetsQuery { .. } => "assets.query",
            ContentToolCall::AssetsSchema => "assets.schema",
            ContentToolCall::ModelBuild { .. } => "model.build",
            ContentToolCall::ModelFetch { .. } => "model.fetch",
            ContentToolCall::WorldPlace { .. } => "world.place",
            ContentToolCall::WorldRemove { .. } => "world.remove",
            ContentToolCall::WorldMove { .. } => "world.move",
            ContentToolCall::WorldList => "world.list",
            ContentToolCall::WorldGetSource => "world.get_source",
            ContentToolCall::WorldApi { .. } => "world.api",
            ContentToolCall::WorldGetPlan => "world.get_plan",
            ContentToolCall::WorldSetPlan { .. } => "world.set_plan",
            ContentToolCall::WorldSetSource { .. } => "world.set_source",
            ContentToolCall::WorldNewLevel { .. } => "world.new_level",
            ContentToolCall::WorldSetPlayerModel { .. } => "world.set_player_model",
            ContentToolCall::WorldSpawn { .. } => "world.spawn",
            ContentToolCall::WorldTune { .. } => "world.tune",
            ContentToolCall::WorldAddAddon { .. } => "world.add_addon",
            ContentToolCall::WorldInSub { call, .. } => call.name(),
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
        if name.starts_with("world.")
            && name != "world.generate"
            && name != "world.new_level"
            && args.get("sub").is_some()
        {
            let sub = need_str(args, "sub", 64)?;
            if sub == "main"
                || (!sub.is_empty()
                    && sub != "."
                    && sub != ".."
                    && sub.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')
                    }))
            {
                let Value::Obj(pairs) = args else { unreachable!() };
                let stripped = Value::Obj(
                    pairs.iter().filter(|(key, _)| key != "sub").cloned().collect(),
                );
                return Ok(ContentToolCall::WorldInSub {
                    sub,
                    call: Box::new(ContentToolCall::parse(name, &stripped)?),
                });
            }
            return Err("sub must be 1..64 ASCII letters/digits/dash/underscore/dot".into());
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
                check_known(
                    args,
                    &["prompt", "model", "seconds", "lyrics", "steps", "seed"],
                    "music.generate argument",
                )?;
                Ok(ContentToolCall::MusicGenerate {
                    prompt: need_prompt(args)?,
                    model: optional_str(args, "model")?.map(str::to_string),
                    seconds: optional_u32(args, "seconds", 5, 300)?,
                    lyrics: optional_str(args, "lyrics")?
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string),
                    steps: optional_u32(args, "steps", 1, 64)?,
                    seed: optional_u64(args, "seed")?,
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
            "content.generate" => {
                check_known(
                    args,
                    &["kind", "prompt", "dim_height"],
                    "content.generate argument",
                )?;
                let kind_text = need_str(args, "kind", 16)?;
                let kind = match kind_text.as_str() {
                    "character" => ContentGenerateKind::Character,
                    "prop" => ContentGenerateKind::Prop,
                    "sound" => ContentGenerateKind::Sound,
                    _ => return Err("'kind' must be character, prop, or sound".to_string()),
                };
                let dim_height = optional_bounded_number(args, "dim_height", 0.01, 100.0)?;
                Ok(ContentToolCall::ContentGenerate {
                    kind,
                    prompt: need_prompt(args)?,
                    dim_height,
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
                    Some(slug) => match ProviderKind::from_slug(slug) {
                        Some(kind) if kind.locality() == crate::wire::Locality::Cloud => {
                            Some(kind)
                        }
                        _ => {
                            return Err(
                                "provider must be openai, grok, claude-cli, codex-cli or grok-cli"
                                    .to_string(),
                            );
                        }
                    },
                };
                Ok(ContentToolCall::LlmConsult { task, prompt, provider })
            }
            "assets.query" => {
                check_known(args, &["sql"], "assets.query argument")?;
                let sql = need_str(args, "sql", MAX_QUERY_SQL_BYTES)?;
                Ok(ContentToolCall::AssetsQuery { sql })
            }
            "assets.schema" => {
                check_known(args, &[], "assets.schema argument")?;
                Ok(ContentToolCall::AssetsSchema)
            }
            "world.place" => {
                check_known(args, &["items"], "world.place argument")?;
                let items_v = match args.get("items") {
                    Some(Value::Arr(a)) => a,
                    Some(_) => return Err("'items' must be an array".to_string()),
                    None => return Err("world.place requires items".to_string()),
                };
                if items_v.is_empty() || items_v.len() > MAX_WORLD_PLACEMENTS {
                    return Err(format!("world.place takes 1..={MAX_WORLD_PLACEMENTS} items"));
                }
                let mut items = Vec::with_capacity(items_v.len());
                for iv in items_v {
                    if !matches!(iv, Value::Obj(_)) {
                        return Err("placement item must be an object".to_string());
                    }
                    check_known(
                        iv,
                        &["model", "pos", "yaw_deg", "scale", "tag"],
                        "placement field",
                    )?;
                    let model = need_str(iv, "model", MAX_MODEL_REF_CHARS)?;
                    check_model_ref(&model)?;
                    let pos = need_pos(iv)?;
                    let yaw_deg = optional_angle(iv, "yaw_deg")?;
                    let scale = optional_scale(iv)?;
                    let tag = match optional_str(iv, "tag")? {
                        None => None,
                        Some(t) if ident_ok(t) => Some(t.to_string()),
                        Some(_) => {
                            return Err(
                                "placement tag must be a short lowercase identifier".to_string()
                            )
                        }
                    };
                    items.push(WorldPlaceItem { model, pos, yaw_deg, scale, tag });
                }
                Ok(ContentToolCall::WorldPlace { items })
            }
            "world.remove" => {
                check_known(args, &["ids", "tag"], "world.remove argument")?;
                let tag = match optional_str(args, "tag")? {
                    None => None,
                    Some(t) if ident_ok(t) => Some(t.to_string()),
                    Some(_) => {
                        return Err("tag must be a short lowercase identifier".to_string())
                    }
                };
                let ids = match args.get("ids") {
                    None => Vec::new(),
                    Some(Value::Arr(a)) => {
                        if a.is_empty() || a.len() > MAX_WORLD_PLACEMENTS {
                            return Err(format!(
                                "world.remove takes 1..={MAX_WORLD_PLACEMENTS} ids"
                            ));
                        }
                        let mut out = Vec::with_capacity(a.len());
                        for v in a {
                            out.push(placement_id(v)?);
                        }
                        out
                    }
                    Some(_) => return Err("'ids' must be an array of integers".to_string()),
                };
                if ids.is_empty() == tag.is_none() {
                    return Err("world.remove takes exactly one of ids/tag".to_string());
                }
                Ok(ContentToolCall::WorldRemove { ids, tag })
            }
            "world.move" => {
                check_known(args, &["id", "pos", "yaw_deg", "scale"], "world.move argument")?;
                let id = placement_id(
                    args.get("id").ok_or_else(|| "world.move requires id".to_string())?,
                )?;
                let pos = match args.get("pos") {
                    None => None,
                    Some(_) => Some(need_pos(args)?),
                };
                let yaw_deg = optional_angle(args, "yaw_deg")?;
                let scale = optional_scale(args)?;
                if pos.is_none() && yaw_deg.is_none() && scale.is_none() {
                    return Err("world.move needs at least one of pos/yaw_deg/scale".to_string());
                }
                Ok(ContentToolCall::WorldMove { id, pos, yaw_deg, scale })
            }
            "world.list" => {
                check_known(args, &[], "world.list argument")?;
                Ok(ContentToolCall::WorldList)
            }
            "world.get_source" => {
                check_known(args, &[], "world.get_source argument")?;
                Ok(ContentToolCall::WorldGetSource)
            }
            "world.api" => {
                check_known(args, &["query", "limit", "cursor"], "world.api argument")?;
                let query = optional_str(args, "query")?.unwrap_or("");
                if query.len() > MAX_WORLD_API_QUERY_BYTES {
                    return Err("world.api query exceeds 160 bytes".into());
                }
                Ok(ContentToolCall::WorldApi { query: query.to_string(),
                    limit: optional_u32(args, "limit", 1, 20)?.unwrap_or(8),
                    cursor: optional_u32(args, "cursor", 0, 1_000_000)?.unwrap_or(0) })
            }
            "world.get_plan" => {
                check_known(args, &[], "world.get_plan argument")?;
                Ok(ContentToolCall::WorldGetPlan)
            }
            "world.set_plan" => {
                check_known(args, &["plan", "revision", "note"], "world.set_plan argument")?;
                let mut plan = args
                    .get("plan")
                    .cloned()
                    .ok_or_else(|| "plan is required (start from world.get_plan's `plan`)".to_string())?;
                validate_plan_shape(&plan)?;
                normalize_plan_requirements(&mut plan);
                let revision = match args.get("revision") {
                    Some(Value::Int(n)) if *n >= 0 => *n as u64,
                    Some(Value::F64(f)) if *f >= 0.0 && f.fract() == 0.0 => *f as u64,
                    Some(_) => return Err("revision must be a non-negative integer".to_string()),
                    None => return Err("revision is required — the `revision` world.get_plan returned".to_string()),
                };
                let note = optional_str(args, "note")?
                    .map(|n| {
                        if n.len() > 200 {
                            Err("note too long".to_string())
                        } else {
                            Ok(n.to_string())
                        }
                    })
                    .transpose()?;
                Ok(ContentToolCall::WorldSetPlan { plan, revision, note })
            }
            "model.build" => {
                check_known(args, &["title", "source"], "model.build argument")?;
                let title = need_str(args, "title", MAX_MODEL_TITLE_BYTES)?;
                if title.trim().is_empty() || title.chars().any(char::is_control) {
                    return Err("title must be display text".to_string());
                }
                let source = need_str(args, "source", MAX_MODEL_SOURCE_BYTES)?;
                if source.trim().is_empty() || source.contains('\u{0}') {
                    return Err("source must be non-empty text without NUL".to_string());
                }
                Ok(ContentToolCall::ModelBuild { title, source })
            }
            "model.fetch" => {
                check_known(args, &["alias"], "model.fetch argument")?;
                let text = need_str(args, "alias", 128)?;
                let alias = AssetAlias::from_str(&text)
                    .map_err(|_| "alias must be a valid gen/csg/<slug> alias".to_string())?;
                if !alias.as_str().starts_with("gen/csg/") {
                    return Err("alias must start with gen/csg/".to_string());
                }
                Ok(ContentToolCall::ModelFetch { alias })
            }
            "world.set_source" => {
                check_known(args, &["source", "note"], "world.set_source argument")?;
                let source = need_str(args, "source", MAX_WORLD_SOURCE_BYTES)?;
                if source.contains('\u{0}') {
                    return Err("source must not contain NUL".to_string());
                }
                let note = optional_str(args, "note")?
                    .map(|n| {
                        if n.len() > 200 {
                            Err("note too long".to_string())
                        } else {
                            Ok(n.to_string())
                        }
                    })
                    .transpose()?;
                Ok(ContentToolCall::WorldSetSource { source, note })
            }
            "world.new_level" => {
                check_known(args, &["title", "source", "note"], "world.new_level argument")?;
                let title = need_str(args, "title", MAX_NEW_LEVEL_TITLE_BYTES)?;
                if title.trim().is_empty() || title.chars().any(char::is_control) {
                    return Err("title must be display text".to_string());
                }
                let source = need_str(args, "source", MAX_WORLD_SOURCE_BYTES)?;
                if source.contains('\u{0}') {
                    return Err("source must not contain NUL".to_string());
                }
                let note = optional_str(args, "note")?
                    .map(|n| {
                        if n.len() > 200 {
                            Err("note too long".to_string())
                        } else {
                            Ok(n.to_string())
                        }
                    })
                    .transpose()?;
                Ok(ContentToolCall::WorldNewLevel { title, source, note })
            }
            "world.set_player_model" => {
                check_known(args, &["model"], "world.set_player_model argument")?;
                let model = need_str(args, "model", MAX_MODEL_REF_CHARS)?;
                check_model_ref(&model)?;
                Ok(ContentToolCall::WorldSetPlayerModel { model })
            }
            "world.tune" => {
                check_known(args, &["time", "car_speed"], "world.tune argument")?;
                let knob = |key: &str, lo: f64, hi: f64| -> Result<Option<f64>, String> {
                    match args.get(key) {
                        None => Ok(None),
                        Some(v) => {
                            let n = match v {
                                Value::F64(f) => *f,
                                Value::Int(i) => *i as f64,
                                _ => return Err(format!("{key} must be a number")),
                            };
                            if !(lo..=hi).contains(&n) {
                                return Err(format!("{key} must be {lo}..={hi}"));
                            }
                            Ok(Some(n))
                        }
                    }
                };
                let time = knob("time", 0.0, 24.0)?;
                let car_speed = knob("car_speed", 0.2, 5.0)?;
                if time.is_none() && car_speed.is_none() {
                    return Err(
                        "world.tune needs at least one knob (time, car_speed)".to_string()
                    );
                }
                Ok(ContentToolCall::WorldTune { time, car_speed })
            }
            "world.spawn" => {
                check_known(
                    args,
                    &["model", "pos", "form", "scale", "color", "hue", "tag"],
                    "world.spawn argument",
                )?;
                let model = need_str(args, "model", MAX_MODEL_REF_CHARS)?;
                check_model_ref(&model)?;
                let pos = match args.get("pos") {
                    None => None,
                    Some(_) => Some(need_pos(args)?),
                };
                let form = match optional_str(args, "form")? {
                    None => None,
                    Some(s) => Some(SpawnForm::from_slug(s).ok_or_else(|| {
                        "form must be car, character, follower, or prop".to_string()
                    })?),
                };
                let scale = optional_spawn_scale(args)?;
                let color = match optional_str(args, "color")? {
                    None => None,
                    Some(value) => {
                        let hex = value.strip_prefix('#').unwrap_or(value);
                        if !matches!(hex.len(), 6 | 8)
                            || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                        {
                            return Err("color must be #rrggbb or #rrggbbaa".to_string());
                        }
                        Some(format!("#{hex}"))
                    }
                };
                let hue = optional_bounded_number(args, "hue", -36_000.0, 36_000.0)?;
                let tag = match optional_str(args, "tag")? {
                    None => None,
                    Some(t) if ident_ok(t) => Some(t.to_string()),
                    Some(_) => {
                        return Err("tag must be a short lowercase identifier".to_string())
                    }
                };
                Ok(ContentToolCall::WorldSpawn {
                    model,
                    pos,
                    form,
                    scale,
                    color,
                    hue,
                    tag,
                })
            }
            "world.add_addon" => {
                check_known(args, &["name", "src"], "world.add_addon argument")?;
                let name = need_str(args, "name", 48)?;
                if !ident_ok(&name) {
                    return Err("name must be a short lowercase identifier".to_string());
                }
                let src = need_str(args, "src", MAX_ADDON_SRC_BYTES)?;
                if src.trim().is_empty() {
                    return Err("src must carry the splash lines to append".to_string());
                }
                Ok(ContentToolCall::WorldAddAddon { name, src })
            }
            other => Err(format!("unknown tool '{}'", bounded(other, 32))),
        }
    }
}

/// A model reference is spliced into game source as a quoted literal, so
/// the characters that would need escaping there are refused instead.
fn check_model_ref(model: &str) -> Result<(), String> {
    if model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | ' ' | ':'))
    {
        Ok(())
    } else {
        Err("model must be a plain alias (letters, digits, /_-.: and spaces)".to_string())
    }
}

fn json_num(v: &Value) -> Option<f64> {
    match v {
        Value::Int(i) => Some(*i as f64),
        Value::F64(f) => Some(*f),
        _ => None,
    }
}

/// World coordinates: finite, |v| <= 100 km. Everything a game means fits;
/// NaN/inf (which would poison the world transform) cannot.
const MAX_WORLD_COORD: f64 = 100_000.0;

fn need_pos(v: &Value) -> Result<[f64; 3], String> {
    let arr = match v.get("pos") {
        Some(Value::Arr(a)) => a,
        Some(_) => return Err("'pos' must be an array [x, y, z]".to_string()),
        None => return Err("missing 'pos'".to_string()),
    };
    if arr.len() != 3 {
        return Err("'pos' must have exactly 3 numbers".to_string());
    }
    let mut out = [0.0f64; 3];
    for (i, item) in arr.iter().enumerate() {
        let n = json_num(item).ok_or_else(|| "'pos' values must be numbers".to_string())?;
        if !n.is_finite() || n.abs() > MAX_WORLD_COORD {
            return Err("'pos' value out of range".to_string());
        }
        out[i] = n;
    }
    Ok(out)
}

fn optional_angle(v: &Value, key: &'static str) -> Result<Option<f64>, String> {
    match v.get(key) {
        None => Ok(None),
        Some(n) => {
            let n = json_num(n).ok_or_else(|| format!("'{key}' must be a number"))?;
            if !n.is_finite() || n.abs() > 100_000.0 {
                return Err(format!("'{key}' out of range"));
            }
            Ok(Some(n))
        }
    }
}

fn optional_scale(v: &Value) -> Result<Option<f64>, String> {
    match v.get("scale") {
        None => Ok(None),
        Some(n) => {
            let n = json_num(n).ok_or_else(|| "'scale' must be a number".to_string())?;
            if !n.is_finite() || !(0.001..=1000.0).contains(&n) {
                return Err("'scale' must be 0.001..=1000".to_string());
            }
            Ok(Some(n))
        }
    }
}

fn optional_spawn_scale(v: &Value) -> Result<Option<SpawnScale>, String> {
    match v.get("scale") {
        None => Ok(None),
        Some(Value::Str(name)) => ScalePreset::parse(name)
            .map(SpawnScale::Preset)
            .map(Some)
            .ok_or_else(|| "'scale' must be 0.2..=3 or real/comic/small/handheld".to_string()),
        Some(value) => {
            let n = json_num(value).ok_or_else(|| {
                "'scale' must be 0.2..=3 or real/comic/small/handheld".to_string()
            })?;
            if !n.is_finite() || !(0.2..=3.0).contains(&n) {
                return Err("'scale' must be 0.2..=3 or real/comic/small/handheld".to_string());
            }
            Ok(Some(SpawnScale::Exact(n)))
        }
    }
}

fn optional_bounded_number(
    v: &Value,
    key: &'static str,
    min: f64,
    max: f64,
) -> Result<Option<f64>, String> {
    match v.get(key) {
        None => Ok(None),
        Some(n) => {
            let n = json_num(n).ok_or_else(|| format!("'{key}' must be a number"))?;
            if !n.is_finite() || !(min..=max).contains(&n) {
                return Err(format!("'{key}' must be {min}..={max}"));
            }
            Ok(Some(n))
        }
    }
}

fn placement_id(v: &Value) -> Result<u64, String> {
    match v {
        Value::Int(i) if *i >= 1 && *i <= 1_000_000_000 => Ok(*i as u64),
        _ => Err("placement id must be an integer 1..=1000000000".to_string()),
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

/// The plan object's SHAPE, checked where the model authors it — so a
/// malformed plan never reaches the game: an object of known top-level
/// fields, feature lists of objects that each carry a unique non-empty
/// string `id` and only the fields their category has, bounded in size.
/// Kinds, anchors and ranges are the engine's schema check (it names the
/// capability set); this is the part that stops a typo cold.
pub fn validate_plan_shape(plan: &Value) -> Result<(), String> {
    validate_plan_schema(plan, &plan_schema(), "plan")?;
    const TOP: &[&str] = &["v", "seed", "biome", "biomes", "terrain", "landforms", "water", "corridors", "places", "dressing"];
    const BIOME: &[&str] = &["id", "kind", "at", "pos", "r"];
    const LANDFORM: &[&str] = &["id", "kind", "at", "pos", "r", "height"];
    const WATER: &[&str] = &["id", "kind", "from", "to", "at", "pos", "path", "width", "depth"];
    const CORRIDOR: &[&str] = &["id", "kind", "required", "from", "to", "through", "path", "closed", "size", "radius", "width", "lift_height", "loops", "corkscrews"];
    const PLACE: &[&str] = &["id", "kind", "at", "pos", "size", "density", "class"];
    const TERRAIN: &[&str] = &["size", "relief", "amp", "cells", "base"];
    const DRESSING: &[&str] = &["forest", "models", "biome"];
    const MAX_FEATURES: usize = 64;
    const MAX_POINTS: usize = 600;
    let Value::Obj(pairs) = plan else {
        return Err("plan must be an object".to_string());
    };
    for (key, _) in pairs {
        if !TOP.contains(&key.as_str()) {
            return Err(format!("unknown plan field '{}' (fields: {})", bounded(key, 32), TOP.join(", ")));
        }
    }
    match plan.get("v") {
        None | Some(Value::Null) | Some(Value::Int(1)) => {}
        Some(Value::F64(f)) if *f == 1.0 => {}
        Some(_) => return Err("plan.v must be 1".to_string()),
    }
    for (key, allowed) in [("terrain", TERRAIN), ("dressing", DRESSING)] {
        match plan.get(key) {
            None | Some(Value::Null) => {}
            Some(v) => check_known(v, allowed, &format!("plan.{key} field"))?,
        }
    }
    let mut ids: Vec<&str> = Vec::new();
    let mut points = 0usize;
    for (key, allowed) in [("biomes", BIOME), ("landforms", LANDFORM), ("water", WATER), ("corridors", CORRIDOR), ("places", PLACE)] {
        let items = match plan.get(key) {
            None | Some(Value::Null) => continue,
            Some(Value::Arr(items)) => items,
            Some(_) => return Err(format!("plan.{key} must be a list of objects")),
        };
        for (i, item) in items.iter().enumerate() {
            let Value::Obj(fields) = item else {
                return Err(format!("plan.{key}[{i}] must be an object"));
            };
            for (field, _) in fields {
                if !allowed.contains(&field.as_str()) {
                    return Err(format!(
                        "plan.{key}[{i}] has no field '{}' (fields: {})",
                        bounded(field, 32),
                        allowed.join(", ")
                    ));
                }
            }
            let id = item.get("id").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() {
                return Err(format!("plan.{key}[{i}] needs a non-empty string `id` — anchors and edits name it"));
            }
            if id.len() > 48 || id.contains(':') || id.contains('@') || id.chars().any(char::is_whitespace) {
                return Err(format!("plan.{key}[{i}] id '{}' must be one word without ':' or '@'", bounded(id, 48)));
            }
            if ids.contains(&id) {
                return Err(format!("plan.{key}[{i}] repeats the id '{id}' — every feature needs its own"));
            }
            ids.push(id);
            if let Some(kind) = item.get("kind") {
                if !kind.is_null() && kind.as_str().is_none() {
                    return Err(format!("plan.{key}[{i}].kind must be a string"));
                }
            }
            if let Some(Value::Arr(path)) = item.get("path") {
                points += path.len();
                if path.iter().any(|p| !matches!(p, Value::Arr(xyz) if xyz.len() == 3 && xyz.iter().all(|n| matches!(n, Value::Int(_) | Value::F64(_))))) {
                    return Err(format!("plan.{key}[{i}].path must be a list of [x, y, z] numbers"));
                }
            }
            for field in ["from", "to", "at", "pos"] {
                if let Some(Value::Str(anchor)) = item.get(field) {
                    if !valid_plan_anchor(anchor) { return Err(format!("plan.{key}[{i}].{field}: invalid anchor '{anchor}'")); }
                }
            }
            if let Some(Value::Arr(through)) = item.get("through") {
                points += through.len();
                for a in through {
                    if let Value::Str(anchor) = a {
                        if !valid_plan_anchor(anchor) { return Err(format!("plan.{key}[{i}].through: invalid anchor '{anchor}'")); }
                    }
                }
            }
        }
    }
    if ids.len() > MAX_FEATURES {
        return Err(format!("a plan holds at most {MAX_FEATURES} features"));
    }
    if points > MAX_POINTS {
        return Err(format!("a plan's paths hold at most {MAX_POINTS} points in total"));
    }
    Ok(())
}


fn plan_number(description: &str, min: f64, max: f64) -> Value {
    json::obj(vec![("type", json::s("number")), ("description", json::s(description)),
        ("minimum", Value::F64(min)), ("maximum", Value::F64(max))])
}

fn plan_nullable(schema: Value) -> Value {
    json::obj(vec![("anyOf", Value::Arr(vec![schema, json::obj(vec![("type", json::s("null"))])]))])
}

const PLAN_COMPASS: &[&str] = &["north", "south", "east", "west", "northeast", "northwest", "southeast", "southwest",
    "north_east", "north_west", "south_east", "south_west", "n", "s", "e", "w", "ne", "nw", "se", "sw", "centre", "center", "middle"];
const PLAN_PARTS: &[&str] = &["east_bank", "west_bank", "north_bank", "south_bank", "source", "mouth", "start", "end", "peak", "summit"];

fn valid_plan_anchor(s: &str) -> bool {
    let valid_id = |id: &str| !id.is_empty() && !id.contains(':') && !id.contains('@') && !id.chars().any(char::is_whitespace);
    if let Some((id, t)) = s.split_once('@') {
        return valid_id(id) && t.parse::<f32>().is_ok_and(|t| t.is_finite() && (0.0..=1.0).contains(&t));
    }
    if let Some((id, part)) = s.split_once(':') {
        return valid_id(id) && (PLAN_PARTS.contains(&part) || PLAN_COMPASS.contains(&part));
    }
    PLAN_COMPASS.contains(&s)
}

/// The tool schema and its nested value validator share this definition.
/// Cross-feature anchors, kind-specific constraints and geometry are checked
/// by the engine. Limits describe accepted tool input, before engine assists.
fn plan_schema() -> Value {
    let coord = || schema_array_bounded("position [x, y, z] in metres", 3, 3,
        plan_number("finite coordinate", -3.4e38, 3.4e38));
    let anchor = || {
        let pattern = format!(r"^({}|[^:@\s]+:({}|{})|[^:@\s]+@(0(\.[0-9]+)?|1(\.0+)?|\.[0-9]+))$",
            PLAN_COMPASS.join("|"), PLAN_PARTS.join("|"), PLAN_COMPASS.join("|"));
        json::obj(vec![("anyOf", Value::Arr(vec![
            schema_string_pattern("compass; river:bank/source/mouth; place:compass; landform:peak; corridor:start/end/centre or id@fraction (0..1)", 1, 128, &pattern),
            coord(),
        ]))])
    };
    let path = || schema_array_bounded("authored waypoints; at most 600 path/through points across the plan", 0, 600, coord());
    let kinds = |values: &[&str]| schema_string_enum("supported kind", values);
    let feature = |mut fields: Vec<(&str, Value)>| {
        fields.insert(0, ("id", schema_string_pattern("unique across every category; stable identity for edits", 1, 48, r"^[^:@\s]+$")));
        schema_object(fields.into_iter().map(|(k, v)| (k, if k == "id" { v } else { plan_nullable(v) })).collect(), &["id"], Some(false))
    };
    let biomes = ["temperate", "alpine", "desert", "woodland", "tundra"];
    let required = json::obj(vec![("type", json::s("boolean")),
        ("description", json::s("true refuses the whole plan if this route cannot be built; false permits an explicit omission; legacy source defaults false")),
        ("default", Value::Bool(true))]);
    let categories = vec![
        ("biomes", feature(vec![("kind", kinds(&biomes)), ("at", anchor()), ("pos", anchor()), ("r", plan_number("radius", 4.0, 2000.0))])),
        ("landforms", feature(vec![("kind", kinds(&["mountain", "hill", "ridge", "valley", "crater", "plateau"])), ("at", anchor()), ("pos", anchor()),
            ("r", plan_number("radius", 4.0, 300.0)), ("height", plan_number("height", -3.4e38, 3.4e38))])),
        ("water", feature(vec![("kind", kinds(&["river", "lake", "canal"])), ("from", anchor()), ("to", anchor()), ("at", anchor()), ("pos", anchor()),
            ("path", path()), ("width", plan_number("width", 4.0, 80.0)), ("depth", plan_number("depth", 0.8, 12.0))])),
        ("corridors", feature(vec![("kind", kinds(&["road", "highway", "rail", "monorail", "path", "coaster"])),
            ("required", required), ("from", anchor()), ("to", anchor()), ("through", schema_array_bounded("ordered anchors", 0, 600, anchor())),
            ("path", path()), ("closed", json::obj(vec![("type", json::s("boolean"))])),
            ("size", plan_number("seeded railway loop size; 0 selects default", 0.0, 600.0)),
            ("radius", plan_number("rounding radius", 4.0, 60.0)), ("width", plan_number("width; road/rail minimum 3, footpath minimum 1.5", 1.5, 30.0)),
            ("lift_height", plan_number("coaster lift", 6.0, 60.0)), ("loops", schema_integer_range("coaster loops", 0, 3)),
            ("corkscrews", schema_integer_range("coaster corkscrews", 0, 3))])),
        ("places", feature(vec![("kind", kinds(&["town", "village", "city", "airfield", "airstrip", "helipad"])), ("at", anchor()), ("pos", anchor()),
            ("size", json::obj(vec![("anyOf", Value::Arr(vec![kinds(&["tiny", "small", "medium", "large", "big"]), plan_number("size in metres; airfields derive size from class", 0.0, 2000.0)]))])),
            ("density", plan_number("density; 0 selects default", 0.0, 1.0)), ("class", kinds(&["", "light", "regional"]))])),
    ];
    let mut fields = vec![
        ("v", schema_integer_range("schema version", 1, 1)), ("seed", schema_integer_range("deterministic seed", 0, 1_000_000_000)),
        ("biome", kinds(&biomes)),
        ("terrain", schema_object(vec![("size", plan_number("map side length", 60.0, 600.0)),
            ("relief", kinds(&["", "flat", "rolling", "hilly", "mountain"])), ("amp", plan_number("relief amplitude", 0.0, 60.0)),
            ("cells", plan_number("terrain samples per side", 33.0, 129.0)), ("base", plan_number("ground elevation", -3.4e38, 3.4e38))], &[], Some(false))),
        ("dressing", schema_object(vec![("forest", plan_number("forest density", 0.0, 1.0)),
            ("models", schema_array_bounded("model aliases", 0, 64, schema_string_len("alias", 1, 256))),
            ("biome", schema_string("vegetation biome override, e.g. forest, meadow, conifer"))], &[], Some(false))),
    ];
    for (key, schema) in categories {
        fields.push((key, schema_array_bounded("at most 64 features across all categories; non-corridor features are always required", 0, 64, schema)));
    }
    schema_object(fields.into_iter().map(|(k, v)| (k, plan_nullable(v))).collect(), &[], Some(false))
}

fn plan_numeric(v: &Value) -> Option<f64> {
    match v { Value::Int(n) => Some(*n as f64), Value::F64(n) => Some(*n), _ => None }
}

/// Validate the subset of JSON Schema used above, so schema types, ranges,
/// enums, field lists and required keys cannot drift from tool acceptance.
fn validate_plan_schema(v: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let err = || format!("{path} does not match its plan schema");
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_arr) {
        return if variants.iter().any(|s| validate_plan_schema(v, s, path).is_ok()) { Ok(()) } else { Err(err()) };
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_arr) {
        if !values.contains(v) { return Err(err()); }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("null") if v.is_null() => {}
        Some("boolean") if matches!(v, Value::Bool(_)) => {}
        Some("number" | "integer") => {
            let n = plan_numeric(v).ok_or_else(err)?;
            if !n.is_finite() || (schema.get("type").and_then(Value::as_str) == Some("integer") && n.fract() != 0.0)
                || schema.get("minimum").and_then(plan_numeric).is_some_and(|lo| n < lo)
                || schema.get("maximum").and_then(plan_numeric).is_some_and(|hi| n > hi) { return Err(err()); }
        }
        Some("string") => {
            let s = v.as_str().ok_or_else(err)?;
            if schema.get("minLength").and_then(Value::as_i64).is_some_and(|lo| s.len() < lo as usize)
                || schema.get("maxLength").and_then(Value::as_i64).is_some_and(|hi| s.len() > hi as usize) { return Err(err()); }
            // The two patterns here describe ids and anchors; semantic
            // validation below checks them without a regex dependency.
        }
        Some("array") => {
            let items = v.as_arr().ok_or_else(err)?;
            if schema.get("minItems").and_then(Value::as_i64).is_some_and(|lo| items.len() < lo as usize)
                || schema.get("maxItems").and_then(Value::as_i64).is_some_and(|hi| items.len() > hi as usize) { return Err(err()); }
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() { validate_plan_schema(item, item_schema, &format!("{path}[{i}]"))?; }
            }
        }
        Some("object") => {
            let Value::Obj(fields) = v else { return Err(err()) };
            let props = schema.get("properties").unwrap();
            for (key, value) in fields {
                let Some(prop) = props.get(key) else { return Err(format!("{path} has no field '{key}'")); };
                validate_plan_schema(value, prop, &format!("{path}.{key}"))?;
            }
            if let Some(required) = schema.get("required").and_then(Value::as_arr) {
                for key in required.iter().filter_map(Value::as_str) {
                    if v.get(key).is_none() { return Err(format!("{path} needs {key}")); }
                }
            }
        }
        _ => return Err(err()),
    }
    Ok(())
}

/// New guided requests require routes unless the author explicitly opts out.
/// Normalized legacy plans already carry false and retain their behavior.
pub fn normalize_plan_requirements(plan: &mut Value) {
    if let Value::Obj(top) = plan {
        if let Some((_, Value::Arr(corridors))) = top.iter_mut().find(|(k, _)| k == "corridors") {
            for c in corridors {
                if let Value::Obj(fields) = c {
                    if let Some((_, value)) = fields.iter_mut().find(|(k, _)| k == "required") {
                        if value.is_null() { *value = Value::Bool(true); }
                    } else { fields.push(("required".into(), Value::Bool(true))); }
                }
            }
        }
    }
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
    if let ContentToolCall::WorldInSub { sub, call } = call {
        let mut value = encode_args(call);
        if let Value::Obj(pairs) = &mut value {
            pairs.push(("sub".into(), json::s(sub.clone())));
        }
        return value;
    }
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
        ContentToolCall::MusicGenerate { prompt, model, seconds, lyrics, steps, seed } => {
            let mut pairs = match encode_prompt_model(prompt, model) {
                Value::Obj(p) => p,
                other => return other,
            };
            if let Some(text) = lyrics {
                pairs.push(("lyrics".into(), json::s(text.clone())));
            }
            if let Some(s) = seconds {
                pairs.push(("seconds".into(), Value::Int(*s as i64)));
            }
            if let Some(s) = steps {
                pairs.push(("steps".into(), Value::Int(*s as i64)));
            }
            if let Some(s) = seed {
                pairs.push(("seed".into(), Value::Int(*s as i64)));
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
        ContentToolCall::ContentGenerate { kind, prompt, dim_height } => {
            let mut pairs = vec![
                ("kind", json::s(kind.as_str())),
                ("prompt", json::s(prompt.clone())),
            ];
            if let Some(height) = dim_height {
                pairs.push(("dim_height", Value::F64(*height)));
            }
            json::obj(pairs)
        }
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
        ContentToolCall::AssetsQuery { sql } => json::obj(vec![("sql", json::s(sql.clone()))]),
        ContentToolCall::AssetsSchema => Value::Obj(Vec::new()),
        ContentToolCall::ModelBuild { title, source } => json::obj(vec![
            ("title", json::s(title.clone())),
            ("source", json::s(source.clone())),
        ]),
        ContentToolCall::ModelFetch { alias } => {
            json::obj(vec![("alias", json::s(alias.to_string()))])
        }
        ContentToolCall::WorldPlace { items } => json::obj(vec![(
            "items",
            Value::Arr(
                items
                    .iter()
                    .map(|i| {
                        let mut pairs = vec![
                            ("model", json::s(i.model.clone())),
                            ("pos", encode_pos(&i.pos)),
                        ];
                        if let Some(y) = i.yaw_deg {
                            pairs.push(("yaw_deg", Value::F64(y)));
                        }
                        if let Some(s) = i.scale {
                            pairs.push(("scale", Value::F64(s)));
                        }
                        if let Some(t) = &i.tag {
                            pairs.push(("tag", json::s(t.clone())));
                        }
                        json::obj(pairs)
                    })
                    .collect(),
            ),
        )]),
        ContentToolCall::WorldRemove { ids, tag } => {
            let mut pairs = Vec::new();
            if !ids.is_empty() {
                pairs.push((
                    "ids",
                    Value::Arr(ids.iter().map(|i| Value::Int(*i as i64)).collect()),
                ));
            }
            if let Some(t) = tag {
                pairs.push(("tag", json::s(t.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldMove { id, pos, yaw_deg, scale } => {
            let mut pairs = vec![("id", Value::Int(*id as i64))];
            if let Some(p) = pos {
                pairs.push(("pos", encode_pos(p)));
            }
            if let Some(y) = yaw_deg {
                pairs.push(("yaw_deg", Value::F64(*y)));
            }
            if let Some(s) = scale {
                pairs.push(("scale", Value::F64(*s)));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldList => Value::Obj(Vec::new()),
        ContentToolCall::WorldGetSource => Value::Obj(Vec::new()),
        ContentToolCall::WorldApi { query, limit, cursor } => json::obj(vec![
            ("query", json::s(query.clone())), ("limit", Value::Int(*limit as i64)),
            ("cursor", Value::Int(*cursor as i64)),
        ]),
        ContentToolCall::WorldGetPlan => Value::Obj(Vec::new()),
        ContentToolCall::WorldSetPlan { plan, revision, note } => {
            let mut pairs = vec![("plan", plan.clone()), ("revision", Value::Int(*revision as i64))];
            if let Some(n) = note {
                pairs.push(("note", json::s(n.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldSetSource { source, note } => {
            let mut pairs = vec![("source", json::s(source.clone()))];
            if let Some(n) = note {
                pairs.push(("note", json::s(n.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldNewLevel { title, source, note } => {
            let mut pairs = vec![
                ("title", json::s(title.clone())),
                ("source", json::s(source.clone())),
            ];
            if let Some(n) = note {
                pairs.push(("note", json::s(n.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldSetPlayerModel { model } => {
            json::obj(vec![("model", json::s(model.clone()))])
        }
        ContentToolCall::WorldTune { time, car_speed } => {
            let mut pairs = Vec::new();
            if let Some(t) = time {
                pairs.push(("time", Value::F64(*t)));
            }
            if let Some(s) = car_speed {
                pairs.push(("car_speed", Value::F64(*s)));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldSpawn { model, pos, form, scale, color, hue, tag } => {
            let mut pairs = vec![("model", json::s(model.clone()))];
            if let Some(p) = pos {
                pairs.push(("pos", encode_pos(p)));
            }
            if let Some(f) = form {
                let slug = match f {
                    SpawnForm::Car => "car",
                    SpawnForm::Character => "character",
                    SpawnForm::Follower => "follower",
                    SpawnForm::Prop => "prop",
                };
                pairs.push(("form", json::s(slug)));
            }
            if let Some(scale) = scale {
                pairs.push((
                    "scale",
                    match scale {
                        SpawnScale::Exact(n) => Value::F64(*n),
                        SpawnScale::Preset(preset) => json::s(preset.as_str()),
                    },
                ));
            }
            if let Some(color) = color {
                pairs.push(("color", json::s(color.clone())));
            }
            if let Some(hue) = hue {
                pairs.push(("hue", Value::F64(*hue)));
            }
            if let Some(t) = tag {
                pairs.push(("tag", json::s(t.clone())));
            }
            json::obj(pairs)
        }
        ContentToolCall::WorldAddAddon { name, src } => json::obj(vec![
            ("name", json::s(name.clone())),
            ("src", json::s(src.clone())),
        ]),
        ContentToolCall::WorldInSub { .. } => unreachable!(),
    }
}

fn encode_pos(pos: &[f64; 3]) -> Value {
    Value::Arr(pos.iter().map(|v| Value::F64(*v)).collect())
}
