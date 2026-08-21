//! Wire types for the HTTP API, shared by the server and the `client` module.
//!
//! Shapes are deliberately flat (state as a string plus optional detail
//! fields, not nested enums) so any client — sandbox toolcalls, curl, a
//! browser — can consume them without a schema.

use makepad_micro_serde::*;

// ---------------------------------------------------------------------------
// GET /health
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct HealthJson {
    pub service: String,
    pub version: String,
    /// GPU name when cheaply obtainable (nvidia-smi), else null.
    pub gpu: Option<String>,
    pub vram_free_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    /// Model ids currently in the "loaded" state.
    pub models_loaded: Vec<String>,
    /// Jobs queued or running right now. `None` on services predating the
    /// field; fleet schedulers treat that as 0 for tiebreaks.
    pub jobs_pending: Option<u64>,
    /// Random per-service-start id, matching the discovery beacon's — lets
    /// clients collapse one box reachable under several addresses. `None` on
    /// services predating discovery.
    pub node_id: Option<u64>,
    /// Durable node identity: 32 lowercase hex chars persisted in the cache
    /// dir (`node-key` file), stable across restarts and redeploys — the
    /// identity a coordinator should key a worker on. `None` on services
    /// predating the field. `node_id` changing while `node_key` stays put is
    /// how an observer detects a service restart.
    pub node_key: Option<String>,
    /// Unix ms when this service process started (restart observability for
    /// supervisors and coordinators).
    pub started_ms: Option<u64>,
    /// Sorted domain names this build + machine can actually serve right now:
    /// only domains with at least one model that is registry-available AND
    /// backend-compiled AND machine-provisioned. The honest capability
    /// snapshot — never lists a domain that would 503 at generate time.
    pub capabilities: Option<Vec<String>>,
    /// VRAM admission safety reserve in MB (see the server residency policy);
    /// a heavy model loads only when fresh free VRAM >= estimate + reserve.
    pub vram_reserve_mb: Option<u64>,
    /// Max queued jobs before POST /generate refuses with 409 "queue full".
    pub queue_limit: Option<u64>,
    /// Partition this process belongs to (`--fleet` / `MAKEPAD_ASSET_AI_FLEET`).
    /// Missing on services predating the field; clients treat that as `default`.
    pub fleet: Option<String>,
    /// Concurrent-decode lane facts for the resident LLM.
    ///
    /// **Absence means one lane, no queue, unknown context ceiling** — which is
    /// exactly how every service predating this field behaves, so a mixed fleet
    /// needs no flag day. Present only when an LLM is actually resident;
    /// a box that has not loaded one has no lanes to describe.
    pub lanes: Option<LanesJson>,
}

/// What a box advertises about its concurrent decode capacity.
///
/// Read through the broker's shared `/health` probe, which caches for ~20 s, so
/// **these numbers are stale by construction and are a PREFERENCE signal, not a
/// reservation**. Whether work actually runs is answered only by the response
/// to the real request: accepted, or honest backpressure. A dispatcher that
/// treats `slots_free` as a guarantee will oversubscribe a box.
///
/// Deliberately absent, and not an oversight: **no tok/s and no predicted
/// latency** (a number that looks authoritative on a health endpoint gets
/// trusted long after it stops being true), and **no speculative draft depth**
/// (internal, changes every step, changes no consumer decision — `lanes_active`
/// already carries the honest contention signal).
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct LanesJson {
    /// Which resident model these facts describe. Lane capacity belongs to the
    /// model, not the box, so a future multi-resident box can report per model.
    pub model: String,
    /// Lanes this service is configured for.
    pub slots_total: u64,
    /// Lanes currently holding a conversation's KV, whether or not that
    /// conversation is generating right now. A claimed-but-idle lane costs no
    /// SPEED — that is the whole dynamic-degradation property — but it does
    /// hold context, which is why this is separate from `lanes_active`.
    pub slots_claimed: u64,
    /// Lanes that could be claimed right now. `slots_total - slots_claimed`.
    pub slots_free: u64,
    /// Lanes generating at this instant. The contention signal: how much a new
    /// conversation would slow the ones already here, and what a scheduler
    /// should prefer the lowest of when choosing between boxes with free lanes.
    pub lanes_active: u64,
    /// Hard per-conversation token ceiling. With N lanes the arena is divided N
    /// ways, so a 4-lane box at 16k **cannot** accept a 32k prompt at all — this
    /// is a constraint to check before dispatching, not a hint.
    pub context_per_slot: u64,
    /// Requests waiting for a lane right now.
    pub queue_depth: u64,
    /// Queue depth beyond which the box refuses rather than queues.
    pub queue_max: u64,
}

// ---------------------------------------------------------------------------
// GET /models
// ---------------------------------------------------------------------------

pub const MODEL_STATE_ABSENT: &str = "absent";
pub const MODEL_STATE_DOWNLOADING: &str = "downloading";
pub const MODEL_STATE_READY: &str = "ready";
pub const MODEL_STATE_LOADED: &str = "loaded";
pub const MODEL_STATE_ERROR: &str = "error";

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ModelInfoJson {
    pub id: String,
    pub domain: String,
    pub backend: String,
    /// False for placeholder entries (flux1-dev, trellis-2) and for models
    /// whose backend is not compiled into this build.
    pub available: bool,
    pub gated: bool,
    pub vram_gb: Option<f64>,
    pub note: Option<String>,
    /// One of the MODEL_STATE_* constants.
    pub state: String,
    /// Download progress in bytes while state == "downloading".
    pub progress_done: Option<u64>,
    pub progress_total: Option<u64>,
    /// Name of the file currently downloading.
    pub downloading_file: Option<String>,
    pub error: Option<String>,
    /// Distinct pinned registry revision(s) of this model's files, comma
    /// separated when several files pin different revisions. `None` when the
    /// registry pins no revision (files tracked on a mutable "main").
    pub revision: Option<String>,
    /// Present exactly when `available == false`: the explicit reason —
    /// backend not compiled into this build, python stack not provisioned on
    /// this machine, or disabled in the registry — so schedulers and UIs
    /// report *why* instead of guessing.
    pub unavailable_reason: Option<String>,
    /// Weight-license identity. Optional so older `/models` payloads still
    /// parse; the Asset UI falls back to the embedded registry by model id.
    pub license_name: Option<String>,
    pub license_url: Option<String>,
    pub license_summary: Option<String>,
    /// `none` / `non-commercial` / `community` / `restricted`.
    pub license_restriction: Option<String>,
    pub license_sha256: Option<String>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ModelsJson {
    pub models: Vec<ModelInfoJson>,
}

// ---------------------------------------------------------------------------
// GET /loras
// ---------------------------------------------------------------------------

/// One adapter file the box operator dropped into `<cache-dir>/loras/`.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct LoraInfoJson {
    /// File stem — exactly what `GenerateRequestJson::loras[].name` takes.
    pub name: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct LorasJson {
    /// Sorted by name.
    pub loras: Vec<LoraInfoJson>,
}

// ---------------------------------------------------------------------------
// POST /generate
// ---------------------------------------------------------------------------

/// One turn of a conversational `/generate` request (`role` + `text`).
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct ChatMessageJson {
    pub role: String,
    pub text: String,
}

/// Empty-think prefill so Qwen3 / Qwen3.6 skip the think block.
pub const CHAT_THINK_PREFILL: &str = "<think>\n\n</think>\n\n";
/// Qwen3.8 official generation prompt: thinking is on by default. Closing
/// `</think>` in the prefill makes 3.8 emit `<|im_end|>` immediately
/// (empty expansion). Open the block and let `clean_expansion` strip it.
pub const CHAT_THINK_PREFILL_OPEN: &str = "<think>\n";
/// Official 3.8 `reasoning_effort=low` system line so an open think
/// block does not eat the expander's token budget on `xhigh`.
pub const QWEN38_LOW_EFFORT: &str = "Reasoning effort is set to low. Keep your thinking brief and focused, moving directly to the conclusion without unnecessary elaboration.";

pub fn model_uses_open_think(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    id.contains("qwen3.8") || id.contains("qwen38")
}

pub fn think_prefill_for_model(model_id: &str) -> &'static str {
    if model_uses_open_think(model_id) {
        CHAT_THINK_PREFILL_OPEN
    } else {
        CHAT_THINK_PREFILL
    }
}

/// Expander jobs need the *answer*, not another think dump. An empty
/// `</think>` prefill makes 3.8 emit `<|im_end|>` immediately; leaving
/// `<think>` open spends the fleet's 220–512 token budget inside the
/// think block. A one-line closed thought starts the model on the prompt.
pub const CHAT_THINK_PREFILL_EXPAND_38: &str =
    "<think>\nWrite the expanded generation prompt next.\n</think>\n\n";

pub fn think_prefill_for_expand(model_id: &str) -> &'static str {
    if model_uses_open_think(model_id) {
        CHAT_THINK_PREFILL_EXPAND_38
    } else {
        CHAT_THINK_PREFILL
    }
}
/// Appended after the generated assistant text so the next chat prompt is
/// a strict string-prefix extension (KV reuse).
pub const CHAT_COMMIT_SUFFIX: &str = "<|im_end|>\n";

/// Prefix-stable ChatML for multi-turn chat. Previous assistant turns are
/// reconstructed with the same think-prefill the generation prompt used, so
/// `prompt_n + reply + CHAT_COMMIT_SUFFIX` is a prefix of `prompt_{n+1}`.
pub fn assemble_chat_prompt(system: &str, messages: &[ChatMessageJson]) -> String {
    assemble_chat_prompt_with_think(system, messages, CHAT_THINK_PREFILL)
}

pub fn assemble_chat_prompt_with_think(
    system: &str,
    messages: &[ChatMessageJson],
    think_prefill: &str,
) -> String {
    let mut out = String::new();
    out.push_str("<|im_start|>system\n");
    let sys = system.trim_end();
    if sys.is_empty() {
        out.push_str("You are Qwen, a helpful assistant.");
    } else {
        out.push_str(sys);
    }
    out.push_str("<|im_end|>\n");
    for m in messages {
        let role = match m.role.as_str() {
            "assistant" => "assistant",
            "system" => "system",
            _ => "user",
        };
        out.push_str("<|im_start|>");
        out.push_str(role);
        out.push('\n');
        if role == "assistant" {
            out.push_str(think_prefill);
        }
        out.push_str(&m.text);
        out.push_str("<|im_end|>\n");
    }
    out.push_str("<|im_start|>assistant\n");
    out.push_str(think_prefill);
    out
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct GenerateRequestJson {
    pub model: String,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub seed: Option<u64>,
    pub steps: Option<u32>,
    pub guidance: Option<f64>,
    /// "queue" (default): wait behind earlier jobs. "reject": fail with
    /// "busy" when any job is queued or running.
    pub queue_policy: Option<String>,

    // -- binary input (cross-stage chaining: image->mesh, image->video i2v,
    //    image->world; also STT/captioning later) --
    /// Base64 input payload, typically a PNG from an earlier pipeline stage
    /// (fetched from another box's /artifact and relayed by the client).
    pub input_b64: Option<String>,
    /// Content type of `input_b64`, e.g. "image/png". Default "image/png".
    pub input_content_type: Option<String>,
    /// Named binary inputs for multi-input models (paint: "mesh" GLB +
    /// "reference_image" PNG). Single-input models keep using `input_b64`;
    /// a model that requires named inputs visibly refuses requests missing
    /// them — it never infers or falls back to `input_b64`.
    pub inputs: Option<Vec<NamedInputJson>>,
    /// Image-to-image denoise strength 0..=1 for image/edit models that
    /// start from `input_b64` instead of pure noise (1.0 = ignore the
    /// input's pixels, only its size; 0.0 = return it unchanged). `None` =
    /// the model's default (instruction editors condition on the reference
    /// tokens and generate fully; plain image models ignore the input).
    /// Extra edit references ride in `inputs` as `reference_1..N`
    /// (`image/png`) next to the primary `input_b64` reference.
    pub strength: Option<f32>,

    // -- video domain (h3 backend) --
    /// Frame count at the model's native fps (H3: 24 fps, frames snapped up
    /// to the video VAE's 17n+5 alignment). Default 124 (~5.2 s).
    pub frames: Option<u32>,
    /// Video codec: "h265"/"hevc" (default, hardware-decodable on Quest-class
    /// devices) or "h264" (compatibility fallback).
    pub codec: Option<String>,
    /// H3 generates video and audio jointly in one denoising pass — the
    /// audio latents cannot be dropped from the DiT's packed sequence
    /// (no such mode exists upstream). Default true = decode the denoised
    /// audio latents through the audio VAE and mux an AAC track. false =
    /// skip both: silent mp4, no audio track (saves the audio VAE decode
    /// and the AAC mux work, not the joint denoise itself).
    pub audio: Option<bool>,
    /// Frame-rate multiplier for the optional RIFE interpolation post-stage:
    /// `2` doubles the frame count (24 -> 48 fps), `4` quadruples it
    /// (24 -> 96 fps). Absent or `1` = off. The clip's DURATION is
    /// unchanged — only the frame cadence gets denser; the audio track is
    /// untouched. Any other value is refused.
    pub interpolate: Option<u32>,
    /// Test hook honored by the testpattern backend only: sleep this long
    /// during generation so queue/reject behavior can be exercised.
    pub delay_ms: Option<u64>,
    /// True = download/verify the model's files and stop before generating —
    /// the fleet's "pull a model to this box" primitive. Rides the normal
    /// job queue, so download progress and cancel come with it.
    pub pull_only: Option<bool>,

    // -- text domain (llm backend: prompt expander) --
    /// Which generation domain the expanded prompt is FOR ("image", "video",
    /// "mesh"); picks the system prompt. Default "image".
    pub target_domain: Option<String>,
    /// Exact subject/character identity the expander must carry verbatim into
    /// its result.  This is separate from descriptive style so an orchestrator
    /// can validate that a terse named intent (for example `yoshi`) was not
    /// creatively replaced while expanding it.
    pub identity_anchor: Option<String>,
    /// Optional style direction woven into the expansion request.
    pub style: Option<String>,
    /// Cap on generated tokens per expansion. Default 512.
    pub max_tokens: Option<u32>,
    /// 0 = greedy/deterministic; > 0 samples. Default 0.7.
    pub temperature: Option<f64>,
    /// Number of alternative expansions (1..=8). When > 1 the job emits a
    /// second application/json artifact with all variants. Default 1.
    pub variants: Option<u32>,
    /// Request domain. `"chat"` selects conversational generation on an
    /// llm backend instead of prompt expansion. Other backends ignore it.
    /// Existing chat clients already send this (and `chat_messages`); the
    /// previous schema dropped the field, so every chat turn was wrapped as
    /// an image-expander job.
    pub domain: Option<String>,
    /// System prompt for conversational generation.
    pub chat_system: Option<String>,
    /// Full transcript for conversational generation. When present and
    /// non-empty the llm backend uses this (not the expander ChatML wrap)
    /// even if `domain` was omitted, so today's FleetQwen text-fallback
    /// body is enough to take the chat path.
    pub chat_messages: Option<Vec<ChatMessageJson>>,

    // -- speech domain (kokoro + indextts backends) --
    /// Text to speak. (`prompt` is accepted as a fallback when empty.)
    pub text: Option<String>,
    /// Voice pack name, e.g. "bm_daniel", "bm_fable", "af_heart" (kokoro),
    /// or a reference-voice wav name from the model's voices/ cache dir
    /// (indextts voice cloning; `input_b64` audio/wav overrides it).
    pub voice: Option<String>,
    /// Speaking-rate multiplier, 0.25..=4.0. Default 1.0.
    pub speed: Option<f64>,
    /// Emotion vector for emotion-controllable TTS (indextts): exactly 8
    /// floats in [0,1.2], order [happy, angry, sad, afraid, disgusted,
    /// melancholic, surprised, calm]. Omitted = neutral (the reference
    /// voice's own affect).
    pub emotion: Option<Vec<f64>>,

    // -- audio domain (sa3 backend: sound effects) --
    /// Requested clip duration in seconds (sa3-sfx: 0.5..=120, default 4).
    /// Also the music domain's song duration (music3: 5..=300, default 60).
    pub seconds: Option<f64>,

    // -- music domain (music3 backend) --
    /// Song lyrics; may carry section tags such as `[Verse]` / `[Chorus]` /
    /// `[Bridge]` / `[Instrumental]` / `[Outro]` on their own lines (the
    /// official Music3 control contract). Empty = instrumental-leaning
    /// generation driven by `prompt` alone. The music description (genre,
    /// BPM, key, vocal and arrangement detail) rides in `prompt`.
    pub lyrics: Option<String>,

    // -- mesh domain (trellis backend) --
    /// FaithC retopo grid resolution (16..=512) applied to the output GLB;
    /// `None` = the raw decode mesh (~10M faces on typical outputs).
    pub remesh_resolution: Option<u32>,
    /// Mesh texturing: run the tex SLAT flow + decode and bake the per-voxel
    /// PBR attrs onto the mesh (UV atlas on retopo'd outputs, COLOR_0 vertex
    /// colors on the raw mesh). Default true; false = untextured (faster).
    pub texture: Option<bool>,
    /// Face target for the retopo'd + decimated output mesh (game-asset
    /// density). Default 80000, clamped 1000..=2000000. Only applies when
    /// texturing a retopo'd mesh (the bake chain decimates).
    pub decimation_target: Option<u32>,
    /// Baked texture atlas size in texels. Default 1024, clamped 256..=4096.
    pub texture_size: Option<u32>,

    // -- splat domain (triposplat backend) --
    /// Target gaussian count for the splat PLY. Default 262144 (the model's
    /// maximum); clamped to 32768..=262144 and rounded to a multiple of 32,
    /// the decoder's gaussians-per-anchor stride. More gaussians = more
    /// detail at linear storage and render cost; the denoiser runs once
    /// regardless, only the decode scales.
    pub gaussians: Option<u32>,

    // -- motion domain (hy-motion backend) --
    /// `"playable"` (default): the fixed playable-character clip set
    /// (idle/walk/jump/run/dance) from the backend's own prompts — the
    /// request prompt is trace metadata. `"prompt"`: ONE finite performance
    /// clip generated from `prompt` (e.g. "A person dances the robot"),
    /// retargeted onto the rig as clip `prompt`; viewers play it as the idle.
    pub motion_mode: Option<String>,

    // -- control domain (control backend: FLUX.1-Depth-dev / FLUX.1-Canny-dev) --
    /// Canny low threshold (flux1-canny-dev only; ignored elsewhere).
    /// `None` = the backend default (50, matching OpenCV's usual default).
    /// Only meaningful together with `canny_high`.
    pub canny_low: Option<f64>,
    /// Canny high threshold. `None` = the backend default (200).
    pub canny_high: Option<f64>,

    // -- image domain (flux backend) --
    /// LoRA adapters to apply, by file name under `<cache-dir>/loras/`
    /// (`GET /loras` lists what this box has). Honored by the `flux`
    /// backend only; any other backend REFUSES a non-empty list rather
    /// than silently ignoring it. Order does not matter — the deltas sum.
    pub loras: Option<Vec<LoraRefJson>>,

    // -- peer-assisted model distribution (all domains; used by pull jobs
    //    and by any generate that must first download model files) --
    /// Coordinator-selected source boxes to try BEFORE Hugging Face: service
    /// base URLs, e.g. ["http://10.0.0.217:8765"]. Peer selection stays
    /// centrally controlled — this field (or the operator's
    /// `MAKEPAD_AI_PEER_SOURCES` env) is the only way peers enter a job.
    pub peer_sources: Option<Vec<String>>,
    /// Coordinator-minted transfer tickets (self-describing scope:
    /// source node/receiver node/artifact digest + expiry). Request-provided
    /// sources always require these tickets. A receiver self-mints only for
    /// operator-configured `MAKEPAD_AI_PEER_SOURCES` when the fleet shares a
    /// transfer secret.
    pub peer_tickets: Option<Vec<String>>,
}

/// One requested LoRA adapter (see `GenerateRequestJson::loras`).
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct LoraRefJson {
    /// File stem or file name under `<cache-dir>/loras/`, e.g. "frosting"
    /// or "frosting.safetensors".
    pub name: String,
    /// Multiplier on the adapter's own `alpha/rank` scale. Default 1.0;
    /// 0.0 is exactly the pristine model.
    pub strength: Option<f64>,
}

/// One named binary input (see `GenerateRequestJson::inputs`).
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct NamedInputJson {
    /// Semantic input name, e.g. "mesh", "reference_image".
    pub name: String,
    /// Exact content type, e.g. "model/gltf-binary", "image/png".
    pub content_type: String,
    pub data_b64: String,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct GenerateResponseJson {
    pub job_id: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /job/<id>
// ---------------------------------------------------------------------------

pub const JOB_STATE_QUEUED: &str = "queued";
pub const JOB_STATE_RUNNING: &str = "running";
/// A live/realtime session (`POST /realtime`) that has started: the worker
/// owns it (one GPU = one job) and loops `live_step` until stopped. See
/// `LiveStatusJson` for the counters surfaced alongside `stage` while in
/// this state, and the "Realtime session wire protocol" doc block below for
/// the websocket contract.
pub const JOB_STATE_LIVE: &str = "live";
pub const JOB_STATE_DONE: &str = "done";
pub const JOB_STATE_ERROR: &str = "error";
/// Cancelled via POST /job/<id>/cancel — either while still queued or
/// mid-run (the worker unwinds at the next step/tile boundary, usually
/// within seconds). Partial artifacts are discarded.
pub const JOB_STATE_CANCELLED: &str = "cancelled";

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ArtifactRefJson {
    pub id: String,
    /// Path on this service, e.g. "/artifact/art-3-0".
    pub url: String,
    pub content_type: String,
    /// SHA-256 of the artifact bytes (lowercase hex), computed when the
    /// artifact was persisted. Fetchers verify the downloaded bytes against
    /// it (see `client::verify_artifact_bytes`); the `/artifact` response
    /// also carries it as an `X-Artifact-Sha256` header. `None` only on
    /// services predating hashed handoff.
    pub sha256: Option<String>,
    /// Exact artifact byte length.
    pub byte_len: Option<u64>,
}

/// `GET /jobs`: every job the service currently holds that is not finished
/// (the running one first, then the queue in order), so a fleet client can
/// SEE other clients' work on a box and cancel it (`POST /job/<id>/cancel`).
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct JobsJson {
    pub jobs: Vec<JobStatusJson>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct JobStatusJson {
    pub job_id: String,
    /// One of the JOB_STATE_* constants.
    pub state: String,
    /// Present while running, e.g. "download", "load", "denoise", "encode".
    pub stage: Option<String>,
    /// 0.0..=1.0 while running.
    pub progress: Option<f64>,
    pub artifacts: Vec<ArtifactRefJson>,
    pub error: Option<String>,
    /// Model id this job targets (observer metadata).
    pub model: Option<String>,
    /// Unix ms timestamps of the job's lifecycle transitions.
    pub queued_ms: Option<u64>,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    /// Bounded recent stage-transition log, oldest first, lines like
    /// "t+3.2s load unet 8.2/23.8GB 12%" (t+ is seconds since the job
    /// started). Phase transitions and slow ticks only — full-rate progress
    /// stays in `stage`/`progress`; this is the durable trail an observer
    /// reads after the fact (VRAM admission/eviction decisions included).
    pub log: Option<Vec<String>>,
    /// Assistant text so far for LLM/chat jobs. Chat clients stream this
    /// instead of fetching the text artifact. Absent on image/audio/mesh
    /// jobs and on services that have not implemented the chat contract.
    pub partial_text: Option<String>,
    /// Present exactly while `state == "live"`: live-session frame counters,
    /// updated at up to 10 Hz by the worker (the same rate the `stats`
    /// websocket message uses for the per-frame timing detail this doesn't
    /// carry).
    pub live: Option<LiveStatusJson>,
}

/// Live-session counters mirrored on `GET /job/<id>` (see
/// `JobStatusJson::live`) — a poller that never opens the websocket still
/// sees the session is alive and roughly how fast it is running.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct LiveStatusJson {
    pub frames_in: u64,
    pub frames_out: u64,
    pub fps: f64,
}

// ---------------------------------------------------------------------------
// POST /realtime, GET /realtime/<id> (websocket) — the live session
// ---------------------------------------------------------------------------
//
// `POST /realtime` admits a live session exactly like `POST /generate`
// admits an ordinary job (same FIFO / `queue_policy=reject` gate in
// `jobs::JobStore`): a live session occupies the box's single GPU worker
// slot until it stops, and every other submitted job stays queued behind it
// — that is intended, the box is dedicated to the live feed while one runs.
// A model whose backend does not implement `ContentBackend::live_step`
// (`backend::backend_live_supported` returns false) is refused with 400
// before a job is even created.
//
// On success the response carries `ws_path`; the client then opens a
// websocket to that path (the in-repo `HttpServer` upgrades any GET whose
// headers carry `Sec-WebSocket-Key`, matched on `path` in server.rs). Any
// number of sockets may be connected to the same session at once — all of
// them receive every output frame and stats message, and any of them may
// send control updates or input frames. A dead/full socket is dropped
// (never blocks the frame loop); see `realtime::RealtimeSession`.
//
// Binary frames (both directions) share one 16-byte little-endian header
// (see `realtime_wire::FrameHeader` / `FRAME_MAGIC`):
//   [ magic:u32 = 0x4C465246 ("FRFL")
//   , kind:u8   (0 = raw RGB8, 1 = PNG, 2 = H.264 Annex-B access unit)
//   , reserved:u8
//   , reserved:u16
//   , width:u16
//   , height:u16
//   , frame_index:u32
//   ] ++ payload
// Client -> server: an input frame (`frame_index` is the client's own input
// counter; not otherwise interpreted). Raw RGB8 is the fast/uncompressed
// path; kind 2 (H.264) is decoded by a per-session `makepad-video`
// `VideoStreamDecoder` (see `realtime::RealtimeSession::handle_binary`) —
// width/height in the header are ignored for kind 2 (the decoder reports
// its own, parsed from the bitstream's SPS); a packet that fails to decode
// is dropped and counted (`stats.codec.dropped_decode`), never treated as a
// protocol error. The session keeps only the LATEST unconsumed DECODED
// input frame — a newer one replaces an older unconsumed one and `dropped`
// increments (see the `stats` message).
// Server -> client: an output frame; `kind` follows the session's
// `output_encoding` (`{"type":"control","output_encoding":"png"}` switches
// it; default is "h264" when the service was built with the `video` cargo
// feature, "raw" otherwise — see `RealtimeRequestJson::output_encoding`).
// `frame_index` is the session's output counter. When a NEW socket
// connects to an H.264-output session, the encoder is asked for a fresh
// keyframe (SPS/PPS + IDR) so the new client can start decoding
// immediately rather than waiting for the next scheduled one.
//
// Text (JSON) frames, client -> server:
//   {"type":"control", ...any subset of: prompt, negative_prompt, strength,
//    steps, guidance, seed, seed_mode, width, height, camera:{dolly,pan_x,
//    pan_y,roll}, loop_mode, input_encoding, output_encoding, max_fps,
//    idle_timeout_s}
//   {"type":"reference", "slot":0, "png_b64":"..."}
//   {"type":"stop"}
// A `control` message only touches the fields it sets (see
// `realtime::apply_control_to_config`) — every other tunable keeps its
// current value.
//
// JSON messages, server -> client:
//   {"type":"stats", "frame_index":N, "fps":.., "frame_ms":..,
//    "stage_ms":{"prep":..,"model":..,"post":..}, "frames_in":..,
//    "frames_out":.., "dropped":..,
//    "codec":{"input":"h264","output":"h264","dropped_decode":N}}
//    (sent every produced frame)
//   {"type":"error", "message":".."}
//   {"type":"stopped", "reason":"stopped"|"cancelled"|"error"}  (once, as
//    the session ends; sockets are then closed from the server side)
//
// IMPORTANT TRANSPORT NOTE: the in-repo `HttpServer`'s websocket write
// thread (platform/network/src/http_server.rs, `handle_web_socket`) always
// frames server -> client pushes with the WebSocket **Binary** opcode —
// there is no server -> client Text-frame path in this transport. So,
// although the messages above are JSON text, they physically arrive as
// Binary websocket frames, indistinguishable BY OPCODE from a pushed output
// frame. Every server -> client push is therefore self-describing instead:
// if the first 4 bytes equal `FRAME_MAGIC` it is a binary frame per the
// layout above; otherwise the whole payload is UTF-8 JSON. Clients MUST
// dispatch on this signature, not the WS opcode (`realtime_wire::
// is_frame_message`). Client -> server text messages are NOT affected —
// the server's read path parses the client's real WS opcode, so JS/browser
// `ws.send(jsonString)` (Text) vs `ws.send(arrayBuffer)` (Binary) both
// arrive correctly split as `HttpServerRequest::TextMessage` /
// `BinaryMessage`.

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct RealtimeRequestJson {
    pub model: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub prompt: Option<String>,
    pub negative_prompt: Option<String>,
    pub strength: Option<f64>,
    pub steps: Option<u32>,
    pub guidance: Option<f64>,
    pub seed: Option<u64>,
    /// "fixed" (default) | "increment" | "random".
    pub seed_mode: Option<String>,
    /// 0 (default) = as fast as possible.
    pub max_fps: Option<f64>,
    /// "feed" (default): wait for client-pushed input frames. "feedback":
    /// the session's own previous output (camera-warped) is the next init.
    pub loop_mode: Option<String>,
    /// "raw" | "png" | "h264" — output frame payload format. Default:
    /// "h264" when this service was built with the `video` cargo feature
    /// (`makepad-video`'s hardware H.264 codec), "raw" otherwise. Requesting
    /// "h264" on a build without that feature is refused (400).
    pub output_encoding: Option<String>,
    /// "raw" | "png" | "h264" — advisory only (the wire is self-describing
    /// per input frame; nothing rejects a client sending a different kind).
    /// Same default rule as `output_encoding`.
    pub input_encoding: Option<String>,
    /// Session ends after this many seconds with zero connected sockets.
    /// Default 30; 0 = never.
    pub idle_timeout_s: Option<u64>,
    /// Same admission policy as `/generate`: "queue" (default) or "reject".
    pub queue_policy: Option<String>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct RealtimeResponseJson {
    pub job_id: Option<String>,
    /// Present on success: `GET`-upgrade this path to a websocket.
    pub ws_path: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /v1/model_inventory
// ---------------------------------------------------------------------------

/// The coordinator-facing digest inventory: every fetchable registry source
/// this box holds in verified form, addressable over the peer blob endpoint.
/// Structured conversions are omitted until receivers can install them.
/// Computed live from the on-disk verification receipts, so a successful
/// install (peer-fed or Hugging Face) registers here immediately.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ModelInventoryJson {
    /// Durable node identity (same value as `/health` `node_key`) — the
    /// ticket source scope a coordinator mints against.
    pub node_key: String,
    /// True when this box will actually serve blobs (a transfer secret is
    /// provisioned and serving is enabled).
    pub peer_serving: bool,
    /// Max bytes per blob response; receivers loop ranged requests.
    pub chunk_bytes: u64,
    pub artifacts: Vec<ModelInventoryArtifactJson>,
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ModelInventoryArtifactJson {
    /// Lowercase-hex SHA-256 — the blob endpoint's only address form.
    pub digest: String,
    pub size: u64,
    /// Canonical cache-relative path (identical on every box).
    pub cache_as: String,
    /// Currently always "source". Reserved for future installable artifact
    /// kinds without changing the wire shape.
    pub kind: String,
    /// Model ids that reference this artifact.
    pub models: Vec<String>,
}

// ---------------------------------------------------------------------------
// Generic error body (non-200 responses)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct ErrorJson {
    pub error: String,
}
