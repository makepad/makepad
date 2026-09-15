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
use makepad_ai_body::packet::BodyPacket;
#[cfg(feature = "body-native")]
use makepad_ai_common::DiffusionError;
#[cfg(all(feature = "body-native", feature = "segment-native"))]
use makepad_ai_vision::sam3::{Sam3, Sam3Image, Sam3Weights};
#[cfg(feature = "body-native")]
use std::path::PathBuf;
use std::time::Instant;

/// Per-request options of the body domain, parsed from the request's
/// free-text prompt (`prompt` on `/generate`, `LiveConfig.prompt` on a
/// realtime session): whitespace- or comma-separated words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyOptions {
    /// Run the hand crops, the hand decoder and the wrist fusion.
    pub hands: bool,
    /// Find persons with SAM 3.1 (`person:N`) and run one pass per person
    /// with its box and mask; otherwise the frame (or the request's box)
    /// is one person.
    pub detect: bool,
    /// Upper bound on detected persons.
    pub persons: usize,
}

impl Default for BodyOptions {
    fn default() -> Self {
        Self {
            hands: false,
            detect: false,
            persons: 1,
        }
    }
}

impl BodyOptions {
    pub const MAX_PERSONS: usize = 8;

    pub fn parse(prompt: &str) -> Result<Self, AssetAiError> {
        let mut options = Self::default();
        for word in prompt
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|word| !word.is_empty())
        {
            match word {
                "hands" => options.hands = true,
                "detect" => options.detect = true,
                _ if word.starts_with("persons=") => {
                    let count = word["persons=".len()..].parse::<usize>().ok();
                    match count {
                        Some(count) if (1..=Self::MAX_PERSONS).contains(&count) => {
                            options.persons = count
                        }
                        _ => {
                            return Err(AssetAiError::Params(format!(
                                "sam3dbody: persons must be 1..={} (got {word:?})",
                                Self::MAX_PERSONS
                            )))
                        }
                    }
                }
                other => {
                    return Err(AssetAiError::Params(format!(
                        "sam3dbody: unknown option {other:?} (hands, detect, persons=N)"
                    )))
                }
            }
        }
        Ok(options)
    }
}

/// The packet contract every body result must meet before it leaves the
/// backend: a JSON object with a top-level `n_people`.
pub fn validate_pose_packet(line: &str) -> Result<(), AssetAiError> {
    let value = makepad_strict_json::parse(line.as_bytes()).map_err(|error| {
        AssetAiError::Backend(format!("sam3dbody returned invalid json: {error}"))
    })?;
    if !matches!(&value, makepad_strict_json::Value::Obj(_)) || value.get("n_people").is_none() {
        return Err(AssetAiError::Backend(
            "sam3dbody json is missing top-level n_people".to_string(),
        ));
    }
    Ok(())
}

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
    /// The optional detector weights (role `native-segment`), when the
    /// registry could fetch them; loaded on the first `detect` request.
    #[cfg(all(feature = "body-native", feature = "segment-native"))]
    segment_path: Option<PathBuf>,
    #[cfg(all(feature = "body-native", feature = "segment-native"))]
    segment: Option<Sam3>,
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
            #[cfg(all(feature = "body-native", feature = "segment-native"))]
            segment_path: None,
            #[cfg(all(feature = "body-native", feature = "segment-native"))]
            segment: None,
        }
    }

    #[cfg(feature = "body-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            gen: Gen::Native,
            model_path: None,
            model: None,
            #[cfg(feature = "segment-native")]
            segment_path: None,
            #[cfg(feature = "segment-native")]
            segment: None,
        }
    }

    fn infer_rgb(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        bbox: Option<[f32; 4]>,
        options: BodyOptions,
    ) -> Result<String, AssetAiError> {
        let _ = options.detect;
        let packet = match &mut self.gen {
            Gen::Stub(gen) => gen(rgb, width, height, bbox)?,
            #[cfg(feature = "body-native")]
            Gen::Native => {
                let start = Instant::now();
                let mut packet = if options.detect {
                    self.infer_detected(rgb, width, height, options)?
                } else {
                    let model = self.model.as_mut().ok_or_else(|| {
                        AssetAiError::Backend(
                            "native body used before ensure_loaded".to_string(),
                        )
                    })?;
                    if options.hands {
                        model
                            .infer_full(rgb, width, height, bbox, None)
                            .map_err(diffusion_err)?
                            .into_packet(0.0)
                    } else {
                        model.infer(rgb, width, height, bbox).map_err(diffusion_err)?
                    }
                };
                packet.ms = start.elapsed().as_secs_f32() * 1000.0;
                packet.to_json()
            }
        };
        validate_pose_packet(&packet)?;
        Ok(packet)
    }

    /// `detect`: SAM 3.1 finds up to `options.persons` persons, then every
    /// person gets a body pass on its own box with its mask (spec 13.3).
    #[cfg(all(feature = "body-native", feature = "segment-native"))]
    fn infer_detected(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        options: BodyOptions,
    ) -> Result<BodyPacket, AssetAiError> {
        if self.segment.is_none() {
            let path = self.segment_path.clone().ok_or_else(|| {
                AssetAiError::Unavailable(
                    "sam3dbody detect: the person detector weights (optional role native-segment) are not available on this node"
                        .to_string(),
                )
            })?;
            let weights = Sam3Weights::load(&path).map_err(diffusion_err)?;
            self.segment = Some(Sam3::prepare(&weights).map_err(diffusion_err)?);
        }
        let segment = self.segment.as_ref().unwrap();
        let model = self.model.as_mut().ok_or_else(|| {
            AssetAiError::Backend("native body used before ensure_loaded".to_string())
        })?;
        let (w, h) = (width as usize, height as usize);
        let image = Sam3Image::rgb8(rgb, w, h).map_err(diffusion_err)?;
        let found = segment
            .segment(image, &format!("person:{}", options.persons), None)
            .map_err(diffusion_err)?;
        let mut people = Vec::new();
        for (bbox, &score) in found
            .boxes_xyxy
            .iter()
            .zip(&found.scores)
            .take(options.persons)
        {
            // The detector returns one instance-union alpha: the person's
            // mask is that alpha inside the person's box, at the 0.5 level.
            let mut mask = vec![0u8; w * h];
            let x0 = bbox[0].floor().max(0.0) as usize;
            let y0 = bbox[1].floor().max(0.0) as usize;
            let x1 = (bbox[2].ceil().max(0.0) as usize).min(w);
            let y1 = (bbox[3].ceil().max(0.0) as usize).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    if found.alpha[y * w + x] >= 0.5 {
                        mask[y * w + x] = 1;
                    }
                }
            }
            let bbox = [bbox[0], bbox[1], bbox[2], bbox[3]];
            let mut person_packet = if options.hands {
                model
                    .infer_full(rgb, width, height, Some(bbox), Some((&mask, score)))
                    .map_err(diffusion_err)?
                    .into_packet(0.0)
            } else {
                model
                    .infer_masked(rgb, width, height, bbox, Some((&mask, score)))
                    .map_err(diffusion_err)?
            };
            people.append(&mut person_packet.people);
        }
        Ok(BodyPacket { people, ms: 0.0 })
    }

    #[cfg(all(feature = "body-native", not(feature = "segment-native")))]
    fn infer_detected(
        &mut self,
        _rgb: &[u8],
        _width: u32,
        _height: u32,
        _options: BodyOptions,
    ) -> Result<BodyPacket, AssetAiError> {
        Err(AssetAiError::Unavailable(
            "sam3dbody detect needs a build with the 'segment-native' cargo feature".to_string(),
        ))
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
                #[cfg(feature = "segment-native")]
                {
                    self.segment = None;
                    self.segment_path = ctx.path_by_role("native-segment").ok();
                }
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
        #[cfg(all(feature = "body-native", feature = "segment-native"))]
        {
            self.segment = None;
            self.segment_path = None;
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
        let options = BodyOptions::parse(&params.prompt)?;
        let packet = self.infer_rgb(&rgb, width, height, None, options)?;
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
        let options = BodyOptions::parse(&frame.config.prompt)?;
        let packet = self.infer_rgb(&init.data, init.width, init.height, None, options)?;
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
    fn options_parse_the_prompt_words() {
        assert_eq!(BodyOptions::parse("").unwrap(), BodyOptions::default());
        assert_eq!(
            BodyOptions::parse("hands, detect persons=3").unwrap(),
            BodyOptions {
                hands: true,
                detect: true,
                persons: 3
            }
        );
        assert!(matches!(
            BodyOptions::parse("persons=0"),
            Err(AssetAiError::Params(_))
        ));
        assert!(matches!(
            BodyOptions::parse("feet"),
            Err(AssetAiError::Params(_))
        ));
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
                hands: None,
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
