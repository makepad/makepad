//! Native SAM 3D Body backend: RGB image -> structured pose JSON.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, LiveFrameIn,
    LiveFrameOut, ProgressSink,
};
use crate::error::AssetAiError;
use crate::subproc_img::png_header;
#[cfg(feature = "body-native")]
use makepad_ai_body::model::BodyModel;
#[cfg(feature = "body-native")]
use makepad_ai_common::DiffusionError;
#[cfg(feature = "body-native")]
use std::path::PathBuf;
use std::time::Instant;

/// Pluggable inference for CPU-only backend tests.
pub type BodyFn = Box<
    dyn FnMut(&[u8], u32, u32, Option<[f32; 4]>) -> Result<String, AssetAiError> + Send,
>;

enum Gen {
    Stub(BodyFn),
    #[cfg(feature = "body-native")]
    Native,
}

pub struct BodyNativeBackend {
    model_id: String,
    gen: Gen,
    #[cfg(feature = "body-native")]
    model_path: Option<PathBuf>,
    #[cfg(feature = "body-native")]
    model: Option<BodyModel>,
}

impl BodyNativeBackend {
    pub fn with_stub(model_id: &str, gen: BodyFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Stub(gen),
            #[cfg(feature = "body-native")]
            model_path: None,
            #[cfg(feature = "body-native")]
            model: None,
        }
    }

    #[cfg(feature = "body-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            model_path: None,
            model: None,
        }
    }

    fn infer_rgb(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: Option<[f32; 4]>,
    ) -> Result<String, AssetAiError> {
        let packet = match &mut self.gen {
            Gen::Stub(gen) => gen(rgb, width, height, bbox)?,
            #[cfg(feature = "body-native")]
            Gen::Native => {
                let model = self.model.as_mut().ok_or_else(|| {
                    AssetAiError::Backend(
                        "native body used before ensure_loaded".to_string(),
                    )
                })?;
                let start = Instant::now();
                let mut packet = model
                    .infer(rgb, width, height, bbox)
                    .map_err(diffusion_err)?;
                packet.ms = start.elapsed().as_secs_f32() * 1000.0;
                packet.to_json()
            }
        };
        crate::body_backend::validate_pose_packet(&packet)?;
        Ok(packet)
    }
}

#[cfg(feature = "body-native")]
fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("body: {other}")),
    }
}

impl ContentBackend for BodyNativeBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "body-native")]
            Gen::Native => {
                let path = ctx.path_by_role("native-body")?;
                if self.model.is_some() && self.model_path.as_ref() == Some(&path) {
                    return Ok(());
                }
                self.model = None;
                let model = BodyModel::load(&path).map_err(diffusion_err)?;
                self.model_path = Some(path);
                self.model = Some(model);
                Ok(())
            }
        }
    }

    fn is_resident(&self) -> bool {
        #[cfg(feature = "body-native")]
        {
            return self.model.is_some();
        }
        #[cfg(not(feature = "body-native"))]
        false
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(feature = "body-native")]
        {
            self.model = None;
            self.model_path = None;
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
                "{} needs an input image (input_b64 png)",
                self.model_id
            )));
        }
        if png_header(&params.input_bytes).is_none() {
            return Err(AssetAiError::Params(
                "sam3dbody input_b64 is not a png".to_string(),
            ));
        }
        cancel.check()?;
        progress("body: infer", 0.05);
        let (rgb, width, height) = crate::testpattern::decode_png_rgb8(&params.input_bytes)?;
        let packet = self.infer_rgb(&rgb, width, height, None)?;
        cancel.check()?;
        progress("done", 1.0);
        Ok(vec![ArtifactData {
            content_type: "application/json",
            ext: "json",
            bytes: packet.into_bytes(),
        }])
    }

    fn live_supported(&self) -> bool {
        true
    }

    fn live_step(
        &mut self,
        frame: LiveFrameIn<'_>,
        cancel: &CancelToken,
    ) -> Result<LiveFrameOut, AssetAiError> {
        cancel.check()?;
        let start = Instant::now();
        let init = frame.init.ok_or_else(|| {
            AssetAiError::Params("sam3dbody live step requires an input frame".to_string())
        })?;
        let packet = self.infer_rgb(&init.data, init.width, init.height, None)?;
        cancel.check()?;
        Ok(LiveFrameOut {
            image: init.clone(),
            aux_json: Some(packet),
            model_ms: start.elapsed().as_secs_f64() * 1000.0,
            text_encode_ms: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{LiveConfig, RgbImage};
    use crate::protocol::GenerateRequestJson;

    fn params(request: GenerateRequestJson) -> GenerateParams {
        GenerateParams::from_request(&request).unwrap()
    }

    fn b64(bytes: &[u8]) -> String {
        String::from_utf8(makepad_base64::base64_encode(
            bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap()
    }

    fn input_png() -> Vec<u8> {
        crate::testpattern::encode_png_rgb8(&vec![128u8; 8 * 4 * 3], 8, 4).unwrap()
    }

    #[test]
    fn reports_live_support_and_echoes_the_frame() {
        let packet = r#"{"n_people":0,"people":[],"ms":0.0}"#.to_string();
        let mut backend = BodyNativeBackend::with_stub(
            "sam3dbody",
            Box::new(move |rgb, width, height, bbox| {
                assert_eq!((width, height), (3, 2));
                assert_eq!(rgb, &[17u8; 18]);
                assert_eq!(bbox, None);
                Ok(packet.clone())
            }),
        );
        assert!(backend.live_supported());
        let init = RgbImage {
            width: 3,
            height: 2,
            data: vec![17u8; 18],
        };
        let config = LiveConfig::default();
        let out = backend
            .live_step(
                LiveFrameIn {
                    init: Some(&init),
                    anchor: None,
                    frame_index: 0,
                    config: &config,
                },
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(out.image, init);
        assert_eq!(
            out.aux_json.as_deref(),
            Some(r#"{"n_people":0,"people":[],"ms":0.0}"#)
        );
    }

    #[cfg(feature = "body-native")]
    #[test]
    fn packet_round_trips_through_generate_and_strict_json() {
        use makepad_ai_body::packet::{BodyPacket, BodyPerson};
        use makepad_strict_json::Value;

        let mut mhr = [0.0; 204];
        mhr[0] = 1.23456;
        let packet = BodyPacket {
            people: vec![BodyPerson {
                mhr,
                global_rot: [0.1, 0.2, 0.3],
                cam_t: [1.0, 2.0, 3.0],
                shape: [0.0; 45],
                expr: [0.0; 72],
                focal: 900.12345,
                bbox: [0.0, 0.0, 8.0, 4.0],
                kp3d: vec![0.0; 70 * 3],
                kp2d: vec![0.0; 70 * 2],
                joints: None,
                rots: None,
            }],
            ms: 4.56789,
        };
        let expected = packet.to_json();
        let mut backend = BodyNativeBackend::with_stub(
            "sam3dbody",
            Box::new(move |rgb, width, height, bbox| {
                assert_eq!((width, height), (8, 4));
                assert_eq!(rgb.len(), 8 * 4 * 3);
                assert_eq!(bbox, None);
                Ok(expected.clone())
            }),
        );
        let request = GenerateRequestJson {
            model: "sam3dbody".to_string(),
            input_b64: Some(b64(&input_png())),
            ..GenerateRequestJson::default()
        };
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params(request), &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "application/json");
        let json = std::str::from_utf8(&artifacts[0].bytes).unwrap();
        let Value::Obj(root) = makepad_strict_json::parse(json.as_bytes()).unwrap() else {
            panic!("body packet root is not an object");
        };
        assert_eq!(
            root.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            ["n_people", "people", "ms"]
        );
        assert_eq!(root[0].1.as_u64(), Some(1));
        let people = root[1].1.as_arr().unwrap();
        let Value::Obj(person) = &people[0] else {
            panic!("body person is not an object");
        };
        assert_eq!(
            person.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            [
                "mhr",
                "global_rot",
                "cam_t",
                "shape",
                "expr",
                "focal",
                "bbox",
                "kp3d",
                "kp2d",
            ]
        );
        assert_eq!(person[0].1.as_arr().unwrap()[0], Value::F64(1.2346));
    }

    #[cfg(feature = "body-native")]
    #[test]
    fn missing_native_model_artifact_is_a_clean_error() {
        use crate::backend::BackendCtx;
        use crate::download::Downloader;
        use crate::registry::{Domain, FileSpec, ModelSpec};

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let cache_dir = std::env::current_dir().unwrap().join(format!(
            "target/body-native-missing-test-{}-{nonce}",
            std::process::id()
        ));
        let spec = ModelSpec {
            id: "sam3dbody".to_string(),
            domain: Domain::Body,
            backend: "body-native".to_string(),
            available: true,
            gated: false,
            vram_gb: Some(4.5),
            min_vram_gb: None,
            min_compute_cap: None,
            note: None,
            license: None,
            files: vec![FileSpec {
                role: Some("native-body".to_string()),
                repo: String::new(),
                path: String::new(),
                revision: None,
                cache_as: "body/sam3dbody/missing.safetensors".to_string(),
                size: None,
                sha256: None,
                local: true,
                optional: true,
                converts_to: None,
                conversion: None,
            }],
        };
        let downloader = Downloader::new("http://127.0.0.1:1", None).unwrap();
        let cancel = CancelToken::new();
        let mut download_progress = |_| {};
        let mut progress = |_: &str, _: f64| {};
        let mut ctx = BackendCtx {
            spec: &spec,
            cache_dir: &cache_dir,
            downloader: &downloader,
            download_progress: &mut download_progress,
            cancel: &cancel,
            progress: &mut progress,
        };
        let mut backend = BodyNativeBackend::new_native("sam3dbody");
        let err = backend
            .ensure_loaded(&mut ctx)
            .expect_err("missing body model must fail");
        assert!(matches!(err, AssetAiError::Backend(_)), "{err:?}");
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
}
