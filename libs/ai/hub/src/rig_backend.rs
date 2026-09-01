//! The `rig` backend: RIG domain — mesh GLB -> skinned (rigged) GLB.
//!
//! Reference-tier: drives the box-provisioned SkinTokens auto-rigger
//! (C:\ai\rig_skintokens.py wrapping C:\ai\SkinTokens demo flow, venv_st)
//! through the shared `{in}`/`{out}` subprocess runner with `.glb` temp
//! files. SkinTokens was the motion campaign's PREFERRED rigger: one
//! autoregressive pass emits skeleton + skin weights (26 joints, 33.6s on
//! the 4090) and `--use_transfer` keeps the input texture — vs UniRig's
//! 3-stage ~145s (local/agent_state/motion-123.md P2b).
//!
//! Box provisioning knob (default = the .123 layout):
//!   MAKEPAD_RIG_CMD  command template, `{in}`/`{out}` are GLB paths
//!                    (C:\ai\venv_st\Scripts\python.exe C:\ai\rig_skintokens.py {in} {out})
//!
//! Request: `{model: "skintokens", input_b64: <glb>,
//! input_content_type: "model/gltf-binary"}` -> one `model/gltf-binary`
//! artifact: the same character with `skins` + joint nodes, ready for the
//! motion domain (and for the engine's skin.rs parser).

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::{
    cmd_provisioned, glb_json_chunk, glb_json_value, run_cancellable_req, SubprocError,
    SubprocRequest,
};
use makepad_micro_serde::JsonValue;
use std::path::PathBuf;
use std::time::Duration;

pub const RIG_CMD_ENV: &str = "MAKEPAD_RIG_CMD";
const RIG_CMD_DEFAULT: &str =
    r"C:\ai\venv_st\Scripts\python.exe C:\ai\rig_skintokens.py {in} {out}";

/// Per-job budget: warm rigging is ~34s; cold covers ckpt loads (1.6GB) and
/// the bpy export server spin-up on a slow disk.
const RIG_TIMEOUT: Duration = Duration::from_secs(20 * 60);

fn rig_cmd() -> String {
    std::env::var(RIG_CMD_ENV).unwrap_or_else(|_| RIG_CMD_DEFAULT.to_string())
}

/// True only where the rigging stack is actually provisioned (venv python +
/// script exist). The backend compiles everywhere (pure std subprocess
/// driver); without this probe every box would advertise the rig domain.
pub fn rig_provisioned() -> bool {
    cmd_provisioned(&rig_cmd())
}

/// Pluggable run for tests: takes the input GLB bytes, returns the output
/// GLB bytes (what the subprocess writes to `{out}`).
pub type RigFn = Box<dyn FnMut(&[u8], ProgressSink) -> Result<Vec<u8>, AssetAiError> + Send>;

enum Gen {
    Stub(RigFn),
    Subprocess,
}

pub struct RigBackend {
    model_id: String,
    gen: Gen,
    /// Recorded at ensure_loaded — temp files live under `<cache>/tmp`.
    cache_dir: Option<PathBuf>,
}

impl RigBackend {
    /// Test/CI constructor: the subprocess is the given closure.
    pub fn with_stub(model_id: &str, gen: RigFn) -> Self {
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

/// Validates the subprocess output against the rig contract: a GLB whose
/// header declares a skin (joints + weights present). The engine parser is
/// the deep validator; this catches "script wrote the mesh back unrigged".
pub fn check_rig_output(bytes: &[u8]) -> Result<(), AssetAiError> {
    let Some(root) = glb_json_value(bytes) else {
        return Err(AssetAiError::Backend(
            "rig output is not a structurally valid GLB".to_string(),
        ));
    };
    let has_jointed_skin = matches!(root.key("skins"), Some(JsonValue::Array(skins)) if
        skins.iter().any(|skin| matches!(skin.key("joints"), Some(JsonValue::Array(joints)) if !joints.is_empty()))
    );
    if !has_jointed_skin {
        return Err(AssetAiError::Backend(
            "rig output GLB has no skins with nonempty joints (rigging produced a bare mesh)"
                .to_string(),
        ));
    }
    Ok(())
}

impl ContentBackend for RigBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        // No registry-managed downloads: weights are box-provisioned (like
        // matte/depth/flashworld). Validate the command here so a
        // mis-provisioned box fails with a message naming the knob.
        ctx.ensure_files()?; // no-op for the empty file list; keeps the pattern
        self.cache_dir = Some(ctx.cache_dir.to_path_buf());
        if matches!(self.gen, Gen::Subprocess) && !rig_provisioned() {
            return Err(AssetAiError::Unavailable(format!(
                "rig command not provisioned on this machine: {:?} (set {})",
                rig_cmd(),
                RIG_CMD_ENV
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
                "{} needs an input mesh (input_b64 glb)",
                self.model_id
            )));
        }
        if glb_json_chunk(&params.input_bytes).is_none() {
            return Err(AssetAiError::Params(
                "input_b64 is not a GLB".to_string(),
            ));
        }
        cancel.check()?;
        progress("rig: skintokens", 0.02);

        let bytes = match &mut self.gen {
            Gen::Stub(gen) => gen(&params.input_bytes, progress)?,
            Gen::Subprocess => {
                let tmp_dir = self
                    .cache_dir
                    .clone()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("tmp");
                // Subprocess `@P` phases land inside the 0.05..0.95 band.
                let mut sub_progress = |stage: &str, frac: f64| {
                    progress(&format!("rig: {stage}"), 0.05 + 0.90 * frac);
                };
                let out = run_cancellable_req(
                    &SubprocRequest {
                        cmd_template: &rig_cmd(),
                        tmp_dir: &tmp_dir,
                        tag: "rig",
                        ext: "glb",
                        input: &params.input_bytes,
                        input_sidecar_json: Some(&format!("{{\"seed\":{}}}", params.seed)),
                        timeout: RIG_TIMEOUT,
                    },
                    cancel,
                    &mut sub_progress,
                )
                .map_err(|err| match err {
                    SubprocError::Cancelled => AssetAiError::Cancelled,
                    other => AssetAiError::Backend(format!("rig: {other}")),
                })?;
                out.out_bytes
            }
        };
        cancel.check()?;
        check_rig_output(&bytes)?;
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

    fn rig_params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    fn mesh_glb() -> Vec<u8> {
        fake_glb(r#"{"asset":{"version":"2.0"},"meshes":[{"primitives":[]}]}"#)
    }

    fn rigged_glb() -> Vec<u8> {
        fake_glb(
            r#"{"asset":{"version":"2.0"},"skins":[{"joints":[1,2]}],"meshes":[{"primitives":[]}]}"#,
        )
    }

    #[test]
    fn stub_rig_to_glb_artifact() {
        let expected = rigged_glb();
        let stub_out = expected.clone();
        let mut backend = RigBackend::with_stub(
            "skintokens",
            Box::new(move |input: &[u8], progress: ProgressSink| {
                // The stub sees the exact request bytes.
                assert_eq!(&input[..4], b"glTF");
                progress("skeleton", 0.5);
                Ok(stub_out.clone())
            }),
        );
        let params = rig_params(GenerateRequestJson {
            model: "skintokens".to_string(),
            input_b64: Some(b64(&mesh_glb())),
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
        // Bytes pass through UNCHANGED.
        assert_eq!(artifacts[0].bytes, expected);
    }

    #[test]
    fn missing_or_garbage_input_is_a_params_error() {
        let mut backend = RigBackend::with_stub(
            "skintokens",
            Box::new(|_: &[u8], _p: ProgressSink| unreachable!()),
        );
        let params = rig_params(GenerateRequestJson {
            model: "skintokens".to_string(),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("missing input must be an error");
        match err {
            AssetAiError::Params(msg) => assert!(msg.contains("input_b64")),
            other => panic!("expected Params error, got {other:?}"),
        }
        let params = rig_params(GenerateRequestJson {
            model: "skintokens".to_string(),
            input_b64: Some(b64(b"not a glb")),
            ..GenerateRequestJson::default()
        });
        assert!(matches!(
            backend.generate(&params, &mut sink, &CancelToken::new()),
            Err(AssetAiError::Params(_))
        ));
    }

    #[test]
    fn unrigged_output_is_a_backend_error() {
        // A subprocess writing back a skinless mesh violates the contract.
        let mut backend = RigBackend::with_stub(
            "skintokens",
            Box::new(|_: &[u8], _p: ProgressSink| Ok(mesh_glb())),
        );
        let params = rig_params(GenerateRequestJson {
            model: "skintokens".to_string(),
            input_b64: Some(b64(&mesh_glb())),
            ..GenerateRequestJson::default()
        });
        let mut sink = |_: &str, _: f64| {};
        let err = backend
            .generate(&params, &mut sink, &CancelToken::new())
            .err()
            .expect("skinless output must be an error");
        match err {
            AssetAiError::Backend(msg) => assert!(msg.contains("no skins"), "{msg}"),
            other => panic!("expected Backend error, got {other:?}"),
        }
        // check_rig_output directly.
        assert!(check_rig_output(&rigged_glb()).is_ok());
        assert!(check_rig_output(&mesh_glb()).is_err());
        assert!(check_rig_output(&fake_glb(
            r#"{"asset":{"version":"2.0"},"skins":[]}"#
        ))
        .is_err());
        assert!(check_rig_output(&fake_glb(
            r#"{"asset":{"version":"2.0"},"skins":[{"joints":[]}] }"#
        ))
        .is_err());
        assert!(check_rig_output(&fake_glb(
            r#"{"asset":{"version":"2.0"},"extras":{"skins":[{"joints":[1]}]}}"#
        ))
        .is_err());
        assert!(check_rig_output(b"junk").is_err());
    }

    #[test]
    fn pre_raised_cancel_token_short_circuits() {
        let mut backend = RigBackend::with_stub(
            "skintokens",
            Box::new(|_: &[u8], _p: ProgressSink| {
                panic!("subprocess must not run on a cancelled job")
            }),
        );
        let params = rig_params(GenerateRequestJson {
            model: "skintokens".to_string(),
            input_b64: Some(b64(&mesh_glb())),
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
        // The default command points at the box layout; on a machine without
        // it (and without the env override) the domain must not advertise.
        if std::env::var(RIG_CMD_ENV).is_err() {
            assert!(!rig_provisioned());
        }
    }
}
