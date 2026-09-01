//! The explicit `motion-oracle` backend: reference MOTION runtime for parity
//! and performance comparisons. The canonical `hy-motion` model uses the
//! native worker; this subprocess is never an automatic fallback.
//!
//! Reference-tier: drives the box-provisioned HY-Motion 1.0 text-to-motion
//! stack (C:\ai\motion_hymotion.py: HY-Motion 1.0 FULL generation in
//! venv_hymotion, then the motion campaign's direction-based retarget in
//! venv_unirig bpy — NEVER the global-delta transfer, which double-applies
//! the rest pose; see local/agent_state/motion-123.md P4) through the shared
//! subprocess runner with `.glb` temp files.
//!
//! This oracle preserves the original three-clip parity contract: its params
//! sidecar requests exactly `["idle", "walk", "jump"]`. The canonical native
//! backend emits the four playable clips `idle`, `walk`, `run`, and `jump`,
//! plus the finite `dance` performance clip (looped by the VJ layer, not
//! natively seamless). In both cases the output GLB must answer the
//! engine's substring resolver (skin.rs GAIT_*_CLIPS / clip_index_any) with
//! those names. Clips are generated IN PLACE (horizontal pelvis drift
//! stripped at retarget) so a game host drives movement from its own
//! transform — the standard locomotion approach.
//!
//! Box provisioning knob (default = the .123 layout):
//!   MAKEPAD_MOTION_CMD  command template, `{in}`/`{out}` are GLB paths
//!                       (C:\ai\venv_hymotion\Scripts\python.exe
//!                        C:\ai\motion_hymotion.py {in} {out});
//!                       the free-text params (prompt, clip list, seed)
//!                       arrive in the `{in}.json` sidecar.
//!
//! Request: `{model: "hy-motion-oracle", prompt: <style hint>, input_b64: <rigged
//! glb>, input_content_type: "model/gltf-binary"}` -> one
//! `model/gltf-binary` artifact: the character with animations.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::{
    cmd_provisioned, glb_json_chunk, glb_json_value, run_cancellable_req, SubprocError,
    SubprocRequest,
};
use makepad_micro_serde::*;
use std::path::PathBuf;
use std::time::Duration;

pub const MOTION_CMD_ENV: &str = "MAKEPAD_MOTION_CMD";
const MOTION_CMD_DEFAULT: &str =
    r"C:\ai\venv_hymotion\Scripts\python.exe C:\ai\motion_hymotion.py {in} {out}";

/// Per-job budget: warm generation was 38s for 3 clips on the 4090, but the
/// cold path loads the Qwen3-8B text encoder (~16GB) plus the bpy retarget
/// venv — minutes on a slow disk.
const MOTION_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The deterministic clip-name contract between the motion domain and every
/// game host: these names, exactly, in the output GLB. skin.rs resolves them
/// by case-insensitive substring (GAIT_IDLE_CLIPS/GAIT_WALK_CLIPS + the
/// AnimState jump candidates), so a locomotion state machine can map
/// input -> clip without inspecting the artifact.
pub const MOTION_CLIP_NAMES: [&str; 3] = ["idle", "walk", "jump"];

fn motion_cmd() -> String {
    std::env::var(MOTION_CMD_ENV).unwrap_or_else(|_| MOTION_CMD_DEFAULT.to_string())
}

/// True only where the motion stack is actually provisioned (venv python +
/// script exist). The backend compiles everywhere (pure std subprocess
/// driver); without this probe every box would advertise the motion domain.
pub fn motion_provisioned() -> bool {
    cmd_provisioned(&motion_cmd())
}

/// The `{in}.json` params sidecar the box script reads (prompts and clip
/// names must not fight the whitespace-split command template).
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct MotionParamsJson {
    /// Style hint prepended to the per-clip motion prompts ("a brave
    /// knight" -> "a brave knight walks forward naturally...").
    pub prompt: String,
    /// Requested clip names, also the NLA track names in the output.
    pub clips: Vec<String>,
    pub seed: u64,
    pub fps: u32,
    /// Strip horizontal pelvis drift at retarget so clips play in place.
    pub in_place: bool,
}

/// Pluggable run for tests: takes (input GLB bytes, params sidecar json),
/// returns the output GLB bytes (what the subprocess writes to `{out}`).
pub type MotionFn =
    Box<dyn FnMut(&[u8], &str, ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(MotionFn),
    Subprocess,
}

pub struct MotionBackend {
    model_id: String,
    gen: Gen,
    /// Recorded at ensure_loaded — temp files live under `<cache>/tmp`.
    cache_dir: Option<PathBuf>,
}

impl MotionBackend {
    /// Test/CI constructor: the subprocess is the given closure.
    pub fn with_stub(model_id: &str, gen: MotionFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            cache_dir: None,
        }
    }

    /// Real constructor used by `create_backend`.
    pub fn new_subprocess(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Subprocess,
            cache_dir: None,
        }
    }
}

/// Validates the subprocess output against the motion contract: a GLB that
/// still carries the skin AND now carries animations. The engine parser is
/// the deep validator; this catches "retarget wrote the rig back unchanged".
/// Clip name of a prompt-mode (`motion_mode: "prompt"`) take: ONE finite
/// performance generated from the request prompt. Viewers without a
/// matching locomotion name fall back to clip 0, so it plays as the idle.
pub const MOTION_PROMPT_CLIP_NAME: &str = "prompt";

/// The playable contract: every locomotion clip present and well-formed.
pub fn check_motion_output(bytes: &[u8]) -> Result<(), AssetAiError> {
    check_motion_output_clips(bytes, &MOTION_CLIP_NAMES)
}

/// Structural check of a motion GLB against an explicit required clip set.
pub fn check_motion_output_clips(bytes: &[u8], required_clips: &[&str]) -> Result<(), AssetAiError> {
    let Some(root) = glb_json_value(bytes) else {
        return Err(AssetAiError::Backend(
            "motion output is not a structurally valid GLB".to_string(),
        ));
    };
    let has_jointed_skin = matches!(root.key("skins"), Some(JsonValue::Array(skins)) if
        skins.iter().any(|skin| matches!(skin.key("joints"), Some(JsonValue::Array(joints)) if !joints.is_empty()))
    );
    if !has_jointed_skin {
        return Err(AssetAiError::Backend(
            "motion output GLB lost its skin".to_string(),
        ));
    }
    let Some(JsonValue::Array(animations)) = root.key("animations") else {
        return Err(AssetAiError::Backend(
            "motion output GLB has no animations".to_string(),
        ));
    };
    for required in required_clips.iter().copied() {
        let Some(animation) = animations.iter().find(|animation| {
            matches!(animation.key("name"), Some(JsonValue::String(name)) if name.eq_ignore_ascii_case(required))
        }) else {
            return Err(AssetAiError::Backend(format!(
                "motion output GLB is missing required '{required}' clip"
            )));
        };
        check_animation_contract(&root, animation, required)?;
    }
    Ok(())
}

fn json_usize(value: Option<&JsonValue>) -> Option<usize> {
    match value? {
        JsonValue::U64(value) => usize::try_from(*value).ok(),
        JsonValue::U128(value) => usize::try_from(*value).ok(),
        JsonValue::I64(value) if *value >= 0 => usize::try_from(*value).ok(),
        JsonValue::I128(value) if *value >= 0 => usize::try_from(*value).ok(),
        JsonValue::F64(value) if value.is_finite() && *value >= 0.0 && value.fract() == 0.0 => {
            Some(*value as usize)
        }
        _ => None,
    }
}

fn json_string(value: Option<&JsonValue>) -> Option<&str> {
    match value? {
        JsonValue::String(value) => Some(value),
        _ => None,
    }
}

/// Validate the glTF animation graph instead of accepting a top-level name
/// alone. Every required clip must carry channels whose sampler/accessor and
/// target node/path contracts are internally consistent. This catches empty
/// named bait, dangling accessors, and a surprisingly common exporter bug
/// where key times and values have different counts.
fn check_animation_contract(
    root: &JsonValue,
    animation: &JsonValue,
    clip_name: &str,
) -> Result<(), AssetAiError> {
    let node_count = match root.key("nodes") {
        Some(JsonValue::Array(nodes)) => nodes.len(),
        _ => 0,
    };
    let accessors = match root.key("accessors") {
        Some(JsonValue::Array(accessors)) => accessors,
        _ => {
            return Err(AssetAiError::Backend(format!(
                "motion clip '{clip_name}' has no accessor table"
            )))
        }
    };
    let samplers = match animation.key("samplers") {
        Some(JsonValue::Array(samplers)) if !samplers.is_empty() => samplers,
        _ => {
            return Err(AssetAiError::Backend(format!(
                "motion clip '{clip_name}' has no samplers"
            )))
        }
    };
    let channels = match animation.key("channels") {
        Some(JsonValue::Array(channels)) if !channels.is_empty() => channels,
        _ => {
            return Err(AssetAiError::Backend(format!(
                "motion clip '{clip_name}' has no channels"
            )))
        }
    };
    for channel in channels {
        let sampler_index = json_usize(channel.key("sampler")).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' has a non-integer channel sampler"
            ))
        })?;
        let sampler = samplers.get(sampler_index).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' channel sampler {sampler_index} is out of range"
            ))
        })?;
        let target = channel.key("target").ok_or_else(|| {
            AssetAiError::Backend(format!("motion clip '{clip_name}' channel has no target"))
        })?;
        let node = json_usize(target.key("node")).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' channel target node is not an integer"
            ))
        })?;
        if node >= node_count {
            return Err(AssetAiError::Backend(format!(
                "motion clip '{clip_name}' targets node {node} outside {node_count} nodes"
            )));
        }
        let path = json_string(target.key("path")).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' channel target has no path"
            ))
        })?;
        let output_type = match path {
            "translation" | "scale" => "VEC3",
            "rotation" => "VEC4",
            other => {
                return Err(AssetAiError::Backend(format!(
                    "motion clip '{clip_name}' has unsupported target path '{other}'"
                )))
            }
        };
        let input_index = json_usize(sampler.key("input")).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' sampler input is not an integer"
            ))
        })?;
        let output_index = json_usize(sampler.key("output")).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' sampler output is not an integer"
            ))
        })?;
        let input = accessors.get(input_index).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' input accessor {input_index} is out of range"
            ))
        })?;
        let output = accessors.get(output_index).ok_or_else(|| {
            AssetAiError::Backend(format!(
                "motion clip '{clip_name}' output accessor {output_index} is out of range"
            ))
        })?;
        let input_count = json_usize(input.key("count")).unwrap_or(0);
        let output_count = json_usize(output.key("count")).unwrap_or(0);
        if input_count == 0 || input_count != output_count {
            return Err(AssetAiError::Backend(format!(
                "motion clip '{clip_name}' sampler key counts disagree ({input_count} vs {output_count})"
            )));
        }
        if json_usize(input.key("componentType")) != Some(5126)
            || json_string(input.key("type")) != Some("SCALAR")
            || json_usize(output.key("componentType")) != Some(5126)
            || json_string(output.key("type")) != Some(output_type)
        {
            return Err(AssetAiError::Backend(format!(
                "motion clip '{clip_name}' sampler accessor types do not match {path}"
            )));
        }
    }
    Ok(())
}

impl ContentBackend for MotionBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        // No registry-managed downloads: weights are box-provisioned (like
        // matte/depth/flashworld). Validate the command here so a
        // mis-provisioned box fails with a message naming the knob.
        ctx.ensure_files()?; // no-op for the empty file list; keeps the pattern
        self.cache_dir = Some(ctx.cache_dir.to_path_buf());
        if matches!(self.gen, Gen::Subprocess) && !motion_provisioned() {
            return Err(AssetAiError::Unavailable(format!(
                "motion command not provisioned on this machine: {:?} (set {})",
                motion_cmd(),
                MOTION_CMD_ENV
            )));
        }
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        if params.input_bytes.is_empty() {
            return Err(AssetAiError::Params(format!(
                "{} needs an input rigged mesh (input_b64 glb)",
                self.model_id
            )));
        }
        if glb_json_chunk(&params.input_bytes).is_none() {
            return Err(AssetAiError::Params(
                "input_b64 is not a GLB".to_string(),
            ));
        }
        let input_is_rigged = glb_json_value(&params.input_bytes)
            .and_then(|root| match root.key("skins") {
                Some(JsonValue::Array(skins)) => Some(skins.iter().any(|skin| {
                    matches!(skin.key("joints"), Some(JsonValue::Array(joints)) if !joints.is_empty())
                })),
                _ => None,
            })
            .unwrap_or(false);
        if !input_is_rigged {
            return Err(AssetAiError::Params(
                "input GLB is not rigged (no skins) — run the rig domain first".to_string(),
            ));
        }
        cancel.check()?;
        progress("motion: hy-motion", 0.02);

        let sidecar = MotionParamsJson {
            prompt: params.prompt.clone(),
            clips: MOTION_CLIP_NAMES.iter().map(|s| s.to_string()).collect(),
            seed: params.seed,
            fps: 30,
            in_place: true,
        }
        .serialize_json();

        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, &sidecar, progress)?,
            Gen::Subprocess => {
                let tmp_dir = self
                    .cache_dir
                    .clone()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("tmp");
                // Subprocess `@P` phases land inside the 0.05..0.95 band.
                let mut sub_progress = |stage: &str, frac: f64| {
                    progress(&format!("motion: {stage}"), 0.05 + 0.90 * frac);
                };
                let out = run_cancellable_req(
                    &SubprocRequest {
                        cmd_template: &motion_cmd(),
                        tmp_dir: &tmp_dir,
                        tag: "motion",
                        ext: "glb",
                        input: &params.input_bytes,
                        input_sidecar_json: Some(&sidecar),
                        timeout: MOTION_TIMEOUT,
                    },
                    cancel,
                    &mut sub_progress,
                )
                .map_err(|err| match err {
                    SubprocError::Cancelled => AssetAiError::Cancelled,
                    other => AssetAiError::Backend(format!("motion: {other}")),
                })?;
                out.out_bytes
            }
        };
        cancel.check()?;
        check_motion_output(&bytes)?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "model/gltf-binary",
            ext: "glb",
            bytes,
        }])
    }
}

// ---------------------------------------------------------------------------
// Tests (stubbed subprocess — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::subproc_img::fake_glb;

    fn motion_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    fn rigged_glb() -> Vec<u8> {
        fake_glb(
            r#"{"asset":{"version":"2.0"},"skins":[{"joints":[1,2]}],"meshes":[{"primitives":[]}]}"#,
        )
    }

    fn animated_glb() -> Vec<u8> {
        animated_glb_named(["walk", "idle", "jump"])
    }

    fn animated_glb_named(names: [&str; 3]) -> Vec<u8> {
        let animation = |name: &str| {
            format!(
                r#"{{"name":"{name}","samplers":[{{"input":0,"output":1,"interpolation":"LINEAR"}}],"channels":[{{"sampler":0,"target":{{"node":1,"path":"rotation"}}}}]}}"#
            )
        };
        fake_glb(&format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{{}},{{}},{{}}],"skins":[{{"joints":[1,2]}}],"accessors":[{{"componentType":5126,"count":2,"type":"SCALAR"}},{{"componentType":5126,"count":2,"type":"VEC4"}}],"animations":[{},{},{}]}}"#,
            animation(names[0]),
            animation(names[1]),
            animation(names[2]),
        ))
    }

    #[test]
    fn stub_motion_to_animated_glb_with_clip_contract() {
        let expected = animated_glb();
        let stub_out = expected.clone();
        let mut backend = MotionBackend::with_stub(
            "hy-motion",
            Box::new(move |input: &[u8], sidecar: &str, progress: ProgressSink| {
                assert_eq!(&input[..4], b"glTF");
                // The params sidecar carries the prompt AND the deterministic
                // clip-name contract the play mode depends on.
                let parsed = MotionParamsJson::deserialize_json(sidecar).unwrap();
                assert_eq!(parsed.prompt, "a brave knight");
                assert_eq!(parsed.clips, vec!["idle", "walk", "jump"]);
                assert_eq!(parsed.fps, 30);
                assert!(parsed.in_place);
                assert_eq!(parsed.seed, 42);
                progress("clips 1/3", 0.4);
                Ok(stub_out.clone())
            }),
        );
        let params = motion_params(GenerateRequestJson {
            model: "hy-motion".to_string(),
            prompt: Some("a brave knight".to_string()),
            seed: Some(42),
            input_b64: Some(b64(&rigged_glb())),
            input_content_type: Some("model/gltf-binary".to_string()),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "model/gltf-binary");
        assert_eq!(artifacts[0].ext, "glb");
        assert_eq!(artifacts[0].bytes, expected);
    }

    #[test]
    fn unrigged_input_is_a_params_error() {
        // Motion needs a RIGGED mesh; a bare Trellis mesh must be refused
        // with a message pointing at the rig domain.
        let mut backend = MotionBackend::with_stub(
            "hy-motion",
            Box::new(|_: &[u8], _: &str, _p: ProgressSink| unreachable!()),
        );
        let bare = fake_glb(r#"{"asset":{"version":"2.0"},"meshes":[{"primitives":[]}]}"#);
        let params = motion_params(GenerateRequestJson {
            model: "hy-motion".to_string(),
            input_b64: Some(b64(&bare)),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("unrigged input must be an error");
        match err {
            AssetAiError::Params(msg) => assert!(msg.contains("rig domain"), "{msg}"),
            other => panic!("expected Params error, got {other:?}"),
        }
        // Missing and garbage inputs too.
        let params = motion_params(GenerateRequestJson {
            model: "hy-motion".to_string(),
            ..GenerateRequestJson::default()
        });
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Params(_))
        ));
        let params = motion_params(GenerateRequestJson {
            model: "hy-motion".to_string(),
            input_b64: Some(b64(b"junk")),
            ..GenerateRequestJson::default()
        });
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Params(_))
        ));
    }

    #[test]
    fn animationless_output_is_a_backend_error() {
        let mut backend = MotionBackend::with_stub(
            "hy-motion",
            Box::new(|input: &[u8], _: &str, _p: ProgressSink| Ok(input.to_vec())),
        );
        let params = motion_params(GenerateRequestJson {
            model: "hy-motion".to_string(),
            input_b64: Some(b64(&rigged_glb())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("clipless output must be an error");
        match err {
            AssetAiError::Backend(msg) => assert!(msg.contains("no animations"), "{msg}"),
            other => panic!("expected Backend error, got {other:?}"),
        }
        // check_motion_output directly.
        assert!(check_motion_output(&animated_glb()).is_ok());
        assert!(check_motion_output(&rigged_glb()).is_err());
        assert!(check_motion_output(&fake_glb(
            r#"{"asset":{"version":"2.0"},"skins":[{"joints":[1]}],"animations":[{"name":"idle"},{"name":"walk"}]}"#
        ))
        .is_err());
        assert!(check_motion_output(&animated_glb_named(["Idle", "WALK", "Jump"])).is_ok());
        // Names alone are not a valid animation contract.
        assert!(check_motion_output(&fake_glb(
            r#"{"asset":{"version":"2.0"},"skins":[{"joints":[1]}],"animations":[{"name":"idle"},{"name":"walk"},{"name":"jump"}]}"#
        ))
        .is_err());
        assert!(check_motion_output(b"junk").is_err());
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = MotionBackend::with_stub(
            "hy-motion",
            Box::new(|_: &[u8], _: &str, _p: ProgressSink| {
                panic!("subprocess must not run on a cancelled job")
            }),
        );
        let params = motion_params(GenerateRequestJson {
            model: "hy-motion".to_string(),
            input_b64: Some(b64(&rigged_glb())),
            ..GenerateRequestJson::default()
        });
        let token = CancelToken::new();
        token.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params, &mut sink, &token),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn not_provisioned_on_dev_machines() {
        if std::env::var(MOTION_CMD_ENV).is_err() {
            assert!(!motion_provisioned());
        }
    }
}
