//! The `multiview-native` backend: MapAnything behind the generic job API.
//!
//! Request, model `mapanything` or `mapanything-apache`:
//! - View 0: `input_b64`, `image/png`. Views 1..N-1: named inputs `view_1` ..
//!   `view_<N-1>`, `image/png`, contiguous (a `view_<i>` without `view_<i-1>`
//!   is refused, naming both). Every view decodes with
//!   `crate::trellis_backend::decode_png_rgba8`, alpha dropped, to exactly
//!   518 x 518, else refused with its size. 1 <= N <= `MULTIVIEW_MAX_VIEWS`
//!   = 32, else refused.
//! - Optional named `views`, `application/json`:
//!   `{"metric": true, "views": [{"k": [fx, fy, cx, cy], "pose":
//!   {"quat": [x, y, z, w], "trans": [x, y, z]}}, ...]}`.
//!   `metric` is optional (default true); `views` must have exactly N entries;
//!   `k` and `pose` are each optional (JSON null or absent). `k` uses OpenCV
//!   pixels: pixel (x, y)'s centre is at (x, y). Pose is camera-to-world,
//!   OpenCV axes (x right, y down, z forward), metres. A pose on any view
//!   requires one on view 0, else refused.
//! - Optional named `depth_<i>` (i = 0..N-1), `application/octet-stream`:
//!   518*518 little-endian f32 z-depth in metres, 0 = unknown. Refused when
//!   the byte length is wrong, when view i has no `k`, or when i >= N.
//! - Other named inputs are ignored. A right name with a wrong content type
//!   is refused. JSON arrays must have lengths k 4, quat 4, trans 3.
//!
//! Response: exactly two artifacts, in order:
//! 1. `application/json`, ext `json`:
//!    `{"width": 518, "height": 518, "metric_scale": s, "frame": "input" | "view0",
//!    "views": [{"k": [fx, fy, cx, cy], "pose": {"quat": [...], "trans": [...]},
//!    "depth_offset": o, "confidence_offset": o, "mask_offset": o}]}`.
//! 2. `application/octet-stream`, ext `bin`: per view in order, row-major
//!    518x518: depth_z f32 LE (metres), confidence f32 LE, mask u8
//!    (1 = unambiguous, 0 = not). Sidecar offsets are bytes into this artifact.
//!
//! The model returns poses in view 0's frame. If the request gave view 0 a
//! pose (q0, t0), output poses map back into the caller's world (`frame: input`):
//! quat = q0 * q (Hamilton product, xyzw, then normalised), trans = R(q0)t + t0.
//! Otherwise poses stay in view 0's frame (`frame: view0`). Mapping uses f64
//! and the rotation convention of `mapanything_prep::quaternion_to_rotation`.

use crate::backend::{
    ArtifactData, BackendCtx, CancelToken, ContentBackend, GenerateParams, NamedInput, ProgressSink,
};
use crate::error::AssetAiError;
use makepad_micro_serde::*;
use std::collections::BTreeMap;

#[cfg(feature = "multiview-native")]
use makepad_ai_common::DiffusionError;
#[cfg(feature = "multiview-native")]
use makepad_ai_vision::{
    mapanything::{MapAnything, MapAnythingPrecision, MapAnythingView},
    mapanything_prep::{
        depth_z_to_along_ray, normalize_rgb8, pinhole_from_rays, rays_from_pinhole,
    },
};
#[cfg(feature = "multiview-native")]
use std::path::PathBuf;

/// MapAnything's input side in pixels.
pub const MULTIVIEW_SIZE: usize = 518;
/// Maximum number of views in one request.
pub const MULTIVIEW_MAX_VIEWS: usize = 32;
const PIXELS: usize = MULTIVIEW_SIZE * MULTIVIEW_SIZE;
#[cfg(feature = "multiview-native")]
const _: () = assert!(MULTIVIEW_SIZE == makepad_ai_vision::mapanything::MAPANYTHING_SIZE);

/// One decoded input view, with optional camera geometry.
pub struct MultiviewView {
    /// Interleaved RGB, exactly 518*518*3 bytes.
    pub rgb: Vec<u8>,
    /// OpenCV pixel intrinsics: fx, fy, cx, cy.
    pub k: Option<[f32; 4]>,
    /// Camera-to-world xyzw quaternion and translation in metres.
    pub pose: Option<([f32; 4], [f32; 3])>,
    /// Row-major z-depth in metres; zero means unknown.
    pub depth_z: Option<Vec<f32>>,
}

/// Decoded views and the geometry's metric flag.
pub struct MultiviewRequest {
    /// Views in input order, starting with `input_b64`.
    pub views: Vec<MultiviewView>,
    /// Whether supplied geometry is metric; defaults to true.
    pub metric: bool,
}

/// One predicted view's maps and camera geometry.
pub struct MultiviewViewResult {
    /// Row-major z-depth in metres.
    pub depth_z: Vec<f32>,
    /// Row-major confidence values.
    pub confidence: Vec<f32>,
    /// Row-major ambiguity mask: 1 is unambiguous, 0 is not.
    pub mask: Vec<u8>,
    /// OpenCV pixel intrinsics: fx, fy, cx, cy.
    pub k: [f32; 4],
    /// Camera-to-world xyzw quaternion and translation in view 0's frame.
    pub pose: ([f32; 4], [f32; 3]),
}

/// Predictions in input order with their shared metric scale.
pub struct MultiviewResult {
    /// One prediction per requested view.
    pub views: Vec<MultiviewViewResult>,
    /// Scale already applied by the model to its predictions.
    pub metric_scale: f32,
}

#[derive(DeJson, SerJson)]
struct PoseJson {
    quat: Vec<f32>,
    trans: Vec<f32>,
}

#[derive(DeJson, SerJson)]
struct ViewJson {
    k: Option<Vec<f32>>,
    pose: Option<PoseJson>,
}

#[derive(DeJson, SerJson)]
struct ViewsJson {
    metric: Option<bool>,
    views: Vec<ViewJson>,
}

#[derive(DeJson, SerJson)]
struct ViewResultJson {
    k: Vec<f32>,
    pose: PoseJson,
    depth_offset: usize,
    confidence_offset: usize,
    mask_offset: usize,
}

#[derive(DeJson, SerJson)]
struct ResultJson {
    width: usize,
    height: usize,
    metric_scale: f32,
    frame: String,
    views: Vec<ViewResultJson>,
}

fn check_type(name: &str, actual: &str, expected: &str) -> Result<(), AssetAiError> {
    if actual != expected {
        return Err(AssetAiError::Params(format!(
            "{name}: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn input_index(name: &str, prefix: &str) -> Result<Option<usize>, AssetAiError> {
    let Some(suffix) = name.strip_prefix(prefix) else {
        return Ok(None);
    };
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    // Only canonical index names belong to the wire contract.
    if suffix.len() > 1 && suffix.starts_with('0') {
        return Ok(None);
    }
    suffix
        .parse()
        .map(Some)
        .map_err(|_| AssetAiError::Params(format!("{name}: view index is too large")))
}

fn array<const N: usize>(values: Vec<f32>, name: &str) -> Result<[f32; N], AssetAiError> {
    let len = values.len();
    values
        .try_into()
        .map_err(|_| AssetAiError::Params(format!("{name}: expected {N} numbers, got {len}")))
}

fn decode_view(name: &str, bytes: &[u8]) -> Result<MultiviewView, AssetAiError> {
    let (rgba, width, height) = crate::trellis_backend::decode_png_rgba8(bytes)
        .map_err(|err| AssetAiError::Params(format!("{name}: {err}")))?;
    if width != MULTIVIEW_SIZE || height != MULTIVIEW_SIZE {
        return Err(AssetAiError::Params(format!(
            "{name}: got {width}x{height}, expected {MULTIVIEW_SIZE}x{MULTIVIEW_SIZE}"
        )));
    }
    let mut rgb = Vec::with_capacity(PIXELS * 3);
    for pixel in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    Ok(MultiviewView {
        rgb,
        k: None,
        pose: None,
        depth_z: None,
    })
}

/// Parses the wire request. Every refusal is `Params` and names the input.
pub fn parse_request(params: &GenerateParams) -> Result<MultiviewRequest, AssetAiError> {
    if params.input_bytes.is_empty() {
        return Err(AssetAiError::Params(
            "input_b64: a PNG view is required".into(),
        ));
    }
    check_type("input_b64", &params.input_content_type, "image/png")?;
    let mut images = BTreeMap::<usize, &NamedInput>::new();
    let mut depths = BTreeMap::<usize, &NamedInput>::new();
    let mut metadata = None;
    for input in &params.inputs {
        if input.name == "views" {
            check_type(&input.name, &input.content_type, "application/json")?;
            if metadata.replace(input).is_some() {
                return Err(AssetAiError::Params("views: duplicate input".into()));
            }
        } else if let Some(i) = input_index(&input.name, "view_")? {
            if i == 0 {
                continue;
            }
            check_type(&input.name, &input.content_type, "image/png")?;
            if i >= MULTIVIEW_MAX_VIEWS {
                return Err(AssetAiError::Params(format!(
                    "{}: at most {MULTIVIEW_MAX_VIEWS} views are supported",
                    input.name
                )));
            }
            if images.insert(i, input).is_some() {
                return Err(AssetAiError::Params(format!(
                    "{}: duplicate input",
                    input.name
                )));
            }
        } else if let Some(i) = input_index(&input.name, "depth_")? {
            check_type(&input.name, &input.content_type, "application/octet-stream")?;
            if depths.insert(i, input).is_some() {
                return Err(AssetAiError::Params(format!(
                    "{}: duplicate input",
                    input.name
                )));
            }
        }
    }
    for &i in images.keys() {
        if i > 1 && !images.contains_key(&(i - 1)) {
            return Err(AssetAiError::Params(format!(
                "view_{i}: missing preceding view_{}",
                i - 1
            )));
        }
    }
    let mut views = Vec::with_capacity(images.len() + 1);
    views.push(decode_view("input_b64", &params.input_bytes)?);
    for input in images.values() {
        views.push(decode_view(&input.name, &input.bytes)?);
    }
    let mut metric = true;
    if let Some(input) = metadata {
        let json = std::str::from_utf8(&input.bytes)
            .map_err(|err| AssetAiError::Params(format!("views: {err}")))?;
        let meta = ViewsJson::deserialize_json(json)
            .map_err(|err| AssetAiError::Params(format!("views: {err:?}")))?;
        if meta.views.len() != views.len() {
            return Err(AssetAiError::Params(format!(
                "views: expected {} entries, got {}",
                views.len(),
                meta.views.len()
            )));
        }
        metric = meta.metric.unwrap_or(true);
        for (i, (view, meta)) in views.iter_mut().zip(meta.views).enumerate() {
            view.k = meta
                .k
                .map(|k| array(k, &format!("views[{i}].k")))
                .transpose()?;
            view.pose = meta
                .pose
                .map(|pose| {
                    Ok::<_, AssetAiError>((
                        array(pose.quat, &format!("views[{i}].pose.quat"))?,
                        array(pose.trans, &format!("views[{i}].pose.trans"))?,
                    ))
                })
                .transpose()?;
        }
        if views[0].pose.is_none() {
            if let Some(i) = views.iter().position(|view| view.pose.is_some()) {
                return Err(AssetAiError::Params(format!(
                    "views[{i}].pose requires views[0].pose"
                )));
            }
        }
    }
    for (i, input) in depths {
        let view = views.get_mut(i).ok_or_else(|| {
            AssetAiError::Params(format!("{}: no corresponding view", input.name))
        })?;
        if view.k.is_none() {
            return Err(AssetAiError::Params(format!(
                "{}: view_{i} needs k in views",
                input.name
            )));
        }
        if input.bytes.len() != PIXELS * 4 {
            return Err(AssetAiError::Params(format!(
                "{}: expected {} bytes, got {}",
                input.name,
                PIXELS * 4,
                input.bytes.len()
            )));
        }
        view.depth_z = Some(
            input
                .bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        );
    }
    Ok(MultiviewRequest { views, metric })
}

fn hamilton(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [x1, y1, z1, w1] = a;
    let [x2, y2, z2, w2] = b;
    [
        w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
        w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
        w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
        w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
    ]
}

fn rotation(q: [f64; 4]) -> [[f64; 3]; 3] {
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    let [x, y, z, w] = q.map(|v| v / norm);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Produces JSON then binary maps, mapping poses into the input world if supplied.
/// Returns `Backend` when view counts or any map lengths disagree.
pub fn encode_result(
    request: &MultiviewRequest,
    result: &MultiviewResult,
) -> Result<Vec<ArtifactData>, AssetAiError> {
    if request.views.len() != result.views.len() {
        return Err(AssetAiError::Backend(format!(
            "multiview: expected {} result views, got {}",
            request.views.len(),
            result.views.len()
        )));
    }
    for (i, view) in result.views.iter().enumerate() {
        for (name, len) in [
            ("depth_z", view.depth_z.len()),
            ("confidence", view.confidence.len()),
            ("mask", view.mask.len()),
        ] {
            if len != PIXELS {
                return Err(AssetAiError::Backend(format!(
                    "multiview view_{i} {name}: expected {PIXELS} values, got {len}"
                )));
            }
        }
    }
    let origin = request.views.first().and_then(|v| v.pose);
    let mut bytes = Vec::with_capacity(result.views.len() * PIXELS * 9);
    let mut views = Vec::with_capacity(result.views.len());
    for view in &result.views {
        let (mut quat, mut trans) = view.pose;
        if let Some((q0, t0)) = origin {
            let q0 = q0.map(f64::from);
            let q = hamilton(q0, quat.map(f64::from));
            let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
            quat = q.map(|v| (v / norm) as f32);
            let t = trans.map(f64::from);
            let r = rotation(q0);
            trans = std::array::from_fn(|i| {
                (r[i][0] * t[0] + r[i][1] * t[1] + r[i][2] * t[2] + f64::from(t0[i])) as f32
            });
        }
        let depth_offset = bytes.len();
        for value in &view.depth_z {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let confidence_offset = bytes.len();
        for value in &view.confidence {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let mask_offset = bytes.len();
        bytes.extend_from_slice(&view.mask);
        views.push(ViewResultJson {
            k: view.k.to_vec(),
            pose: PoseJson {
                quat: quat.to_vec(),
                trans: trans.to_vec(),
            },
            depth_offset,
            confidence_offset,
            mask_offset,
        });
    }
    let meta = ResultJson {
        width: MULTIVIEW_SIZE,
        height: MULTIVIEW_SIZE,
        metric_scale: result.metric_scale,
        frame: if origin.is_some() { "input" } else { "view0" }.into(),
        views,
    };
    Ok(vec![
        ArtifactData {
            content_type: "application/json",
            ext: "json",
            bytes: meta.serialize_json().into_bytes(),
        },
        ArtifactData {
            content_type: "application/octet-stream",
            ext: "bin",
            bytes,
        },
    ])
}

/// Which frame the result's poses are in (the sidecar's `frame`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MultiviewFrame {
    /// Poses are in the caller's input world.
    Input,
    /// Poses are relative to the first view.
    View0,
}

/// Parses the two artifacts `encode_result` writes back into the result and its frame.
/// Returns `Params` naming the field for malformed metadata, wrong array lengths,
/// dimensions other than 518, or any offset/map extending past the binary artifact.
pub fn decode_result(
    json: &[u8],
    bin: &[u8],
) -> Result<(MultiviewResult, MultiviewFrame), AssetAiError> {
    fn bad(field: &str, reason: &str) -> AssetAiError {
        AssetAiError::Params(format!("{field}: {reason}"))
    }
    fn field<'a>(
        value: &'a JsonValue,
        key: &str,
        path: &str,
    ) -> Result<&'a JsonValue, AssetAiError> {
        value.get(key).ok_or_else(|| bad(path, "missing field"))
    }
    fn integer(value: &JsonValue, path: &str) -> Result<usize, AssetAiError> {
        value
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| bad(path, "expected an unsigned byte offset or dimension"))
    }
    fn number(value: &JsonValue, path: &str) -> Result<f32, AssetAiError> {
        value
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .ok_or_else(|| bad(path, "expected a finite f32"))
    }
    fn numbers<const N: usize>(value: &JsonValue, path: &str) -> Result<[f32; N], AssetAiError> {
        let values = value
            .as_array()
            .ok_or_else(|| bad(path, "expected an array"))?;
        array(
            values
                .iter()
                .enumerate()
                .map(|(i, v)| number(v, &format!("{path}[{i}]")))
                .collect::<Result<Vec<_>, _>>()?,
            path,
        )
    }
    fn map<'a>(
        bin: &'a [u8],
        view: &JsonValue,
        key: &str,
        path: &str,
        len: usize,
    ) -> Result<&'a [u8], AssetAiError> {
        let offset = integer(field(view, key, path)?, path)?;
        offset
            .checked_add(len)
            .and_then(|end| bin.get(offset..end))
            .ok_or_else(|| bad(path, "offset or map runs past the binary artifact"))
    }
    fn floats(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    let text = std::str::from_utf8(json).map_err(|e| bad("result JSON", &e.to_string()))?;
    let meta =
        JsonValue::deserialize_json(text).map_err(|e| bad("result JSON", &format!("{e:?}")))?;
    for key in ["width", "height"] {
        if integer(field(&meta, key, key)?, key)? != MULTIVIEW_SIZE {
            return Err(bad(key, "expected 518"));
        }
    }
    let metric_scale = number(
        field(&meta, "metric_scale", "metric_scale")?,
        "metric_scale",
    )?;
    let frame = match field(&meta, "frame", "frame")?.as_str() {
        Some("input") => MultiviewFrame::Input,
        Some("view0") => MultiviewFrame::View0,
        _ => return Err(bad("frame", "expected input or view0")),
    };
    let metadata = field(&meta, "views", "views")?
        .as_array()
        .ok_or_else(|| bad("views", "expected an array"))?;
    let mut views = Vec::with_capacity(metadata.len());
    for (i, view) in metadata.iter().enumerate() {
        let path = format!("views[{i}]");
        let k = numbers(
            field(view, "k", &format!("{path}.k"))?,
            &format!("{path}.k"),
        )?;
        let pose = field(view, "pose", &format!("{path}.pose"))?;
        let quat = numbers(
            field(pose, "quat", &format!("{path}.pose.quat"))?,
            &format!("{path}.pose.quat"),
        )?;
        let trans = numbers(
            field(pose, "trans", &format!("{path}.pose.trans"))?,
            &format!("{path}.pose.trans"),
        )?;
        // Validate every range before allocating this view's maps.
        let depth = map(
            bin,
            view,
            "depth_offset",
            &format!("{path}.depth_offset"),
            PIXELS * 4,
        )?;
        let confidence = map(
            bin,
            view,
            "confidence_offset",
            &format!("{path}.confidence_offset"),
            PIXELS * 4,
        )?;
        let mask = map(
            bin,
            view,
            "mask_offset",
            &format!("{path}.mask_offset"),
            PIXELS,
        )?;
        views.push(MultiviewViewResult {
            depth_z: floats(depth),
            confidence: floats(confidence),
            mask: mask.to_vec(),
            k,
            pose: (quat, trans),
        });
    }
    Ok((
        MultiviewResult {
            views,
            metric_scale,
        },
        frame,
    ))
}

/// Pluggable inference for stub tests or another in-process executor.
pub type MultiviewFn =
    Box<dyn FnMut(&MultiviewRequest, ProgressSink) -> Result<MultiviewResult, AssetAiError> + Send>;

enum Gen {
    Stub(MultiviewFn),
    #[cfg(feature = "multiview-native")]
    Native,
}

/// Multi-view geometry backend with optional resident MapAnything inference.
pub struct MultiviewBackend {
    model_id: String,
    gen: Gen,
    #[cfg(feature = "multiview-native")]
    model_path: Option<PathBuf>,
    #[cfg(feature = "multiview-native")]
    model: Option<MapAnything>,
}

impl MultiviewBackend {
    /// Constructs a backend with a supplied inference closure.
    pub fn with_stub(model_id: &str, run: MultiviewFn) -> Self {
        Self {
            model_id: model_id.into(),
            gen: Gen::Stub(run),
            #[cfg(feature = "multiview-native")]
            model_path: None,
            #[cfg(feature = "multiview-native")]
            model: None,
        }
    }

    /// Constructs a native backend; weights load in `ensure_loaded`.
    #[cfg(feature = "multiview-native")]
    pub fn new_native(model_id: &str) -> Self {
        Self {
            model_id: model_id.into(),
            gen: Gen::Native,
            model_path: None,
            model: None,
        }
    }
}

#[cfg(feature = "multiview-native")]
fn diffusion_err(err: DiffusionError) -> AssetAiError {
    match err {
        DiffusionError::Cancelled => AssetAiError::Cancelled,
        other => AssetAiError::Backend(format!("multiview: {other}")),
    }
}

#[cfg(feature = "multiview-native")]
fn run_native(
    model: &MapAnything,
    request: &MultiviewRequest,
    progress: ProgressSink,
    cancel: &CancelToken,
) -> Result<MultiviewResult, AssetAiError> {
    let bad = |err| AssetAiError::Backend(format!("multiview: {err}"));
    let mut prepared = Vec::with_capacity(request.views.len());
    for view in &request.views {
        cancel.check()?;
        let image = normalize_rgb8(&view.rgb, MULTIVIEW_SIZE, MULTIVIEW_SIZE).map_err(bad)?;
        let rays = view.k.map(|[fx, fy, cx, cy]| {
            rays_from_pinhole(fx, fy, cx, cy, MULTIVIEW_SIZE, MULTIVIEW_SIZE)
        });
        let depth = match (&view.depth_z, &rays) {
            (Some(depth), Some(rays)) => Some(depth_z_to_along_ray(depth, rays).map_err(bad)?),
            (Some(_), None) => return Err(bad("depth requires camera rays".into())),
            _ => None,
        };
        prepared.push((image, rays, depth));
    }
    let views: Vec<_> = prepared
        .iter()
        .zip(&request.views)
        .map(|((image, rays, depth), view)| MapAnythingView {
            image,
            rays: rays.as_deref(),
            depth_along_ray: depth.as_deref(),
            pose: view.pose,
            metric: request.metric,
        })
        .collect();
    let mut hook = |stage: &str, fraction: f64| {
        if cancel.is_cancelled() {
            return Err(DiffusionError::Cancelled);
        }
        progress(&format!("multiview: {stage}"), 0.02 + 0.96 * fraction);
        Ok(())
    };
    let output = model
        .forward(&views, Some(&mut hook), None)
        .map_err(diffusion_err)?;
    let mut views = Vec::with_capacity(output.views.len());
    for output in output.views {
        if output.rays.len() != PIXELS * 3
            || output.depth_along_ray.len() != PIXELS
            || output.confidence.len() != PIXELS
            || output.mask_logits.len() != PIXELS
        {
            return Err(bad("native output map lengths disagree".into()));
        }
        let k = pinhole_from_rays(&output.rays, MULTIVIEW_SIZE, MULTIVIEW_SIZE).map_err(bad)?;
        views.push(MultiviewViewResult {
            depth_z: output
                .depth_along_ray
                .iter()
                .zip(output.rays.chunks_exact(3))
                .map(|(d, r)| d * r[2])
                .collect(),
            confidence: output.confidence,
            mask: output
                .mask_logits
                .iter()
                .map(|&v| (v > 0.0) as u8)
                .collect(),
            k,
            pose: (output.quat, output.trans),
        });
    }
    Ok(MultiviewResult {
        views,
        metric_scale: output.metric_scale,
    })
}

impl ContentBackend for MultiviewBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        ctx.ensure_files()?;
        match &self.gen {
            Gen::Stub(_) => Ok(()),
            #[cfg(feature = "multiview-native")]
            Gen::Native => {
                let path = ctx.path_by_role("native-multiview")?;
                if self.model.is_some() && self.model_path.as_ref() == Some(&path) {
                    return Ok(());
                }
                self.model = None;
                let mut progress = |stage: &str, fraction: f64| {
                    if ctx.cancel.is_cancelled() {
                        return Err(DiffusionError::Cancelled);
                    }
                    (ctx.progress)(stage, fraction);
                    Ok(())
                };
                let model = MapAnything::load(
                    &path,
                    &self.model_id,
                    MapAnythingPrecision::Bf16,
                    Some(&mut progress),
                )
                .map_err(diffusion_err)?;
                self.model_path = Some(path);
                self.model = Some(model);
                Ok(())
            }
        }
    }

    fn is_resident(&self) -> bool {
        #[cfg(feature = "multiview-native")]
        {
            self.model.is_some()
        }
        #[cfg(not(feature = "multiview-native"))]
        {
            false
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        #[cfg(feature = "multiview-native")]
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
        cancel.check()?;
        let request = parse_request(params)?;
        cancel.check()?;
        progress("multiview: prepare", 0.02);
        let result = match &mut self.gen {
            Gen::Stub(run) => run(&request, progress)?,
            #[cfg(feature = "multiview-native")]
            Gen::Native => {
                let model = self.model.as_ref().ok_or_else(|| {
                    AssetAiError::Backend("native multiview used before ensure_loaded".into())
                })?;
                run_native(model, &request, progress, cancel)?
            }
        };
        cancel.check()?;
        let artifacts = encode_result(&request, &result)?;
        cancel.check()?;
        progress("done", 1.0);
        Ok(artifacts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::GenerateRequestJson;
    use crate::testpattern::encode_png_rgba;

    const K: [f32; 4] = [500.0, 510.0, 258.0, 257.0];
    const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn png(width: usize) -> Vec<u8> {
        encode_png_rgba(
            &[17, 43, 91, 123].repeat(width * MULTIVIEW_SIZE),
            width,
            MULTIVIEW_SIZE,
        )
        .expect("test PNG")
    }

    fn named(name: &str, content_type: &str, bytes: Vec<u8>) -> NamedInput {
        NamedInput {
            name: name.into(),
            content_type: content_type.into(),
            bytes,
        }
    }

    fn params(count: usize) -> GenerateParams {
        let png = png(MULTIVIEW_SIZE);
        let input_b64 = String::from_utf8(makepad_base64::base64_encode(
            &png,
            &makepad_base64::BASE64_STANDARD,
        ))
        .expect("base64 ASCII");
        let mut params = GenerateParams::from_request(&GenerateRequestJson {
            model: "mapanything".into(),
            input_b64: Some(input_b64),
            ..Default::default()
        })
        .expect("test request");
        params.inputs = (1..count)
            .map(|i| named(&format!("view_{i}"), "image/png", png.clone()))
            .collect();
        params
    }

    fn metadata(count: usize, poses: bool) -> NamedInput {
        let meta = ViewsJson {
            metric: Some(false),
            views: (0..count)
                .map(|i| ViewJson {
                    k: Some(K.to_vec()),
                    pose: poses.then(|| PoseJson {
                        quat: IDENTITY.to_vec(),
                        trans: vec![i as f32, 0.25, -0.5],
                    }),
                })
                .collect(),
        };
        named(
            "views",
            "application/json",
            meta.serialize_json().into_bytes(),
        )
    }

    fn result(count: usize) -> MultiviewResult {
        MultiviewResult {
            metric_scale: 1.25,
            views: (0..count)
                .map(|v| MultiviewViewResult {
                    depth_z: (0..PIXELS).map(|i| v as f32 + i as f32 * 0.125).collect(),
                    confidence: (0..PIXELS)
                        .map(|i| v as f32 + 1.0 + i as f32 * 0.25)
                        .collect(),
                    mask: (0..PIXELS).map(|i| ((i + v) % 2) as u8).collect(),
                    k: K,
                    pose: (IDENTITY, [v as f32, 0.0, 0.0]),
                })
                .collect(),
        }
    }

    fn sidecar(artifacts: &[ArtifactData]) -> ResultJson {
        ResultJson::deserialize_json(std::str::from_utf8(&artifacts[0].bytes).expect("JSON UTF-8"))
            .expect("result JSON")
    }

    fn refuses(params: &GenerateParams, names: &[&str]) {
        match parse_request(params) {
            Err(AssetAiError::Params(message)) => {
                for name in names {
                    assert!(message.contains(name), "{name} missing from {message}");
                }
            }
            Err(other) => panic!("expected Params, got {other}"),
            Ok(_) => panic!("expected refusal naming {names:?}"),
        }
    }

    #[test]
    fn multiview_parse_preserves_rgb_geometry_depth_and_metric_defaults() {
        let mut params = params(3);
        params.inputs.push(metadata(3, true));
        let depth: Vec<f32> = (0..PIXELS).map(|i| i as f32 * 0.5).collect();
        params.inputs.push(named(
            "depth_1",
            "application/octet-stream",
            depth.iter().flat_map(|v| v.to_le_bytes()).collect(),
        ));
        let request = parse_request(&params).expect("three views with geometry");
        assert_eq!(request.views.len(), 3);
        assert!(!request.metric);
        for (i, view) in request.views.iter().enumerate() {
            assert_eq!(
                view.rgb,
                [17, 43, 91].repeat(PIXELS),
                "alpha dropped for view {i}"
            );
            assert_eq!(view.k, Some(K));
            assert_eq!(view.pose, Some((IDENTITY, [i as f32, 0.25, -0.5])));
            assert_eq!(view.depth_z.as_ref(), (i == 1).then_some(&depth));
        }
        params
            .inputs
            .retain(|input| input.name != "views" && input.name != "depth_1");
        let request = parse_request(&params).expect("uncalibrated views");
        assert!(request.metric);
        assert!(request
            .views
            .iter()
            .all(|v| v.k.is_none() && v.pose.is_none() && v.depth_z.is_none()));
        params.inputs.push(named(
            "views",
            "application/json",
            br#"{"views":[{}, {"k":null,"pose":null}, {}]}"#.to_vec(),
        ));
        let request = parse_request(&params).expect("null and absent optional geometry");
        assert!(request.metric);
        assert!(request
            .views
            .iter()
            .all(|v| v.k.is_none() && v.pose.is_none()));
    }

    #[test]
    fn multiview_refusals_name_invalid_or_missing_inputs() {
        let mut params = params(3);
        let input = std::mem::take(&mut params.input_bytes);
        refuses(&params, &["input_b64"]);
        params.input_bytes = input;
        let first = params.inputs.remove(0);
        refuses(&params, &["view_2", "view_1"]);
        params.inputs.insert(0, first);
        let original = std::mem::replace(&mut params.inputs[1].bytes, png(517));
        refuses(&params, &["view_2", "517x518"]);
        params.inputs[1].bytes = original;
        // Full 33-view request; rejected before decoding its images.
        for i in 3..33 {
            params.inputs.push(named(
                &format!("view_{i}"),
                "image/png",
                params.input_bytes.clone(),
            ));
        }
        refuses(&params, &["view_32", "32"]);
        params.inputs.truncate(2);
        params.inputs.push(metadata(4, false));
        refuses(&params, &["views"]);
        params.inputs.pop();
        params.inputs.push(named(
            "views",
            "application/json",
            br#"{"views":[{}, {"pose":{"quat":[0,0,0,1],"trans":[0,0,0]}}, {}]}"#.to_vec(),
        ));
        refuses(&params, &["views[1].pose", "views[0].pose"]);
        params.inputs.pop();
        params.inputs.push(named(
            "depth_0",
            "application/octet-stream",
            vec![0; PIXELS * 4],
        ));
        refuses(&params, &["depth_0", "k"]);
        params.inputs.pop();
        params.inputs.push(metadata(3, false));
        params.inputs.push(named(
            "depth_1",
            "application/octet-stream",
            vec![0; PIXELS * 4 - 4],
        ));
        refuses(&params, &["depth_1"]);
        params.inputs.pop();
        params.inputs.push(named(
            "depth_3",
            "application/octet-stream",
            vec![0; PIXELS * 4],
        ));
        refuses(&params, &["depth_3"]);
        params.inputs.truncate(2);
        for (json, field) in [
            (r#"{"views":[{"k":[1,2,3]}, {}, {}]}"#, "k"),
            (
                r#"{"views":[{"pose":{"quat":[0,0,1],"trans":[0,0,0]}}, {}, {}]}"#,
                "quat",
            ),
            (
                r#"{"views":[{"pose":{"quat":[0,0,0,1],"trans":[0,0]}}, {}, {}]}"#,
                "trans",
            ),
        ] {
            params
                .inputs
                .push(named("views", "application/json", json.as_bytes().to_vec()));
            refuses(&params, &["views", field]);
            params.inputs.pop();
        }
        params.inputs.push(named("views", "image/png", vec![]));
        refuses(&params, &["views"]);
        params.inputs.pop();
        params.input_content_type = "image/jpeg".into();
        refuses(&params, &["input_b64"]);
        params.input_content_type = "image/png".into();
        params.inputs[0].content_type = "application/json".into();
        refuses(&params, &["view_1"]);
        params.inputs[0].content_type = "image/png".into();
        params.inputs.push(named("depth_0", "image/png", vec![]));
        refuses(&params, &["depth_0"]);
    }

    #[test]
    fn multiview_unknown_named_input_is_ignored() {
        let mut params = params(1);
        params
            .inputs
            .push(named("notes", "text/plain", b"ignored".to_vec()));
        let request = parse_request(&params).expect("notes is not a geometry input");
        assert_eq!(request.views.len(), 1);
        assert!(request.metric);
    }

    #[test]
    fn multiview_encode_preserves_view0_poses_and_exact_map_bytes() {
        let request = parse_request(&params(2)).expect("two views");
        let mut result = result(2);
        result.views[1].pose.0 = [0.0, 0.0, 0.6, 0.8];
        let artifacts = encode_result(&request, &result).expect("encode maps");
        assert_eq!(artifacts.len(), 2);
        let meta = sidecar(&artifacts);
        assert_eq!((meta.width, meta.height), (MULTIVIEW_SIZE, MULTIVIEW_SIZE));
        assert_eq!(meta.metric_scale, 1.25);
        assert_eq!(meta.frame, "view0");
        assert_eq!(meta.views.len(), 2);
        assert_eq!(artifacts[1].bytes.len(), 2 * PIXELS * 9);
        for (v, (meta, expected)) in meta.views.iter().zip(&result.views).enumerate() {
            assert_eq!(meta.k, expected.k);
            assert_eq!(meta.pose.quat, expected.pose.0);
            assert_eq!(meta.pose.trans, expected.pose.1);
            assert_eq!(meta.depth_offset, v * PIXELS * 9);
            assert_eq!(meta.confidence_offset, v * PIXELS * 9 + PIXELS * 4);
            assert_eq!(meta.mask_offset, v * PIXELS * 9 + PIXELS * 8);
            for (offset, values) in [
                (meta.depth_offset, &expected.depth_z),
                (meta.confidence_offset, &expected.confidence),
            ] {
                let bytes: Vec<_> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
                assert_eq!(
                    &artifacts[1].bytes[offset..offset + PIXELS * 4],
                    bytes,
                    "view {v} map at {offset}"
                );
            }
            assert_eq!(
                &artifacts[1].bytes[meta.mask_offset..meta.mask_offset + PIXELS],
                expected.mask
            );
        }
    }

    #[test]
    fn multiview_decode_round_trips_maps_geometry_frames_and_refuses_truncation() {
        let mut request = parse_request(&params(2)).expect("two views");
        let mut prediction = result(2);
        prediction.views[1].pose.0 = [0.5, -0.5, 0.5, 0.5];
        prediction.views[0].depth_z[0] = -0.0;
        prediction.views[1].confidence[1] = f32::from_bits(0x7fc01234);
        for frame in [MultiviewFrame::View0, MultiviewFrame::Input] {
            request.views[0].pose =
                (frame == MultiviewFrame::Input).then_some((IDENTITY, [0.25, -0.5, 1.0]));
            let artifacts = encode_result(&request, &prediction).expect("encode stub");
            let (decoded, actual_frame) =
                decode_result(&artifacts[0].bytes, &artifacts[1].bytes).expect("decode stub");
            assert_eq!(actual_frame, frame);
            assert_eq!(
                decoded.metric_scale.to_bits(),
                prediction.metric_scale.to_bits()
            );
            assert_eq!(decoded.views.len(), prediction.views.len());
            for (actual, expected) in decoded.views.iter().zip(&prediction.views) {
                for (actual, expected) in actual
                    .depth_z
                    .iter()
                    .zip(&expected.depth_z)
                    .chain(actual.confidence.iter().zip(&expected.confidence))
                {
                    assert_eq!(actual.to_bits(), expected.to_bits(), "map bits");
                }
                assert_eq!(actual.depth_z.len(), expected.depth_z.len());
                assert_eq!(actual.confidence.len(), expected.confidence.len());
                assert_eq!(actual.mask, expected.mask);
                assert_eq!(actual.k.map(f32::to_bits), expected.k.map(f32::to_bits));
                assert_eq!(
                    actual.pose.0.map(f32::to_bits),
                    expected.pose.0.map(f32::to_bits)
                );
                let trans = if frame == MultiviewFrame::Input {
                    std::array::from_fn(|i| expected.pose.1[i] + [0.25, -0.5, 1.0][i])
                } else {
                    expected.pose.1
                };
                assert_eq!(actual.pose.1.map(f32::to_bits), trans.map(f32::to_bits));
            }
            let short = &artifacts[1].bytes[..artifacts[1].bytes.len() - 1];
            assert!(matches!(decode_result(&artifacts[0].bytes, short),
                Err(AssetAiError::Params(message)) if message.contains("views[1].mask_offset")));
            // The decoder's error must locate malformed fields, not just reject them.
            for (field, replacement) in [
                ("width", "517"),
                ("height", "0"),
                ("frame", "null"),
                ("metric_scale", "null"),
                ("views", "null"),
            ] {
                let mut meta = JsonValue::deserialize_json(
                    std::str::from_utf8(&artifacts[0].bytes).expect("UTF-8"),
                )
                .expect("JSON");
                *meta.get_mut(field).expect("field") =
                    JsonValue::deserialize_json(replacement).expect("replacement");
                assert!(
                    matches!(decode_result(meta.serialize_json().as_bytes(), &artifacts[1].bytes),
                    Err(AssetAiError::Params(message)) if message.contains(field)),
                    "{field}"
                );
            }
            for field in [
                "k",
                "quat",
                "trans",
                "depth_offset",
                "confidence_offset",
                "mask_offset",
            ] {
                let mut meta = sidecar(&artifacts);
                match field {
                    "k" => {
                        meta.views[0].k.pop();
                    }
                    "quat" => {
                        meta.views[0].pose.quat.pop();
                    }
                    "trans" => {
                        meta.views[0].pose.trans.pop();
                    }
                    "depth_offset" => meta.views[0].depth_offset = usize::MAX,
                    "confidence_offset" => {
                        meta.views[0].confidence_offset = artifacts[1].bytes.len()
                    }
                    _ => meta.views[0].mask_offset = artifacts[1].bytes.len(),
                }
                assert!(
                    matches!(decode_result(meta.serialize_json().as_bytes(), &artifacts[1].bytes),
                    Err(AssetAiError::Params(message)) if message.contains(field)),
                    "{field}"
                );
            }
        }
    }

    #[test]
    fn multiview_encode_maps_poses_back_into_input_world() {
        let mut request = parse_request(&params(2)).expect("two views");
        let q0 = [
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        let t0 = [0.1, -0.2, 0.3];
        request.views[0].pose = Some((q0, t0));
        let meta = sidecar(&encode_result(&request, &result(2)).expect("world poses"));
        assert_eq!(meta.frame, "input");
        for (i, view) in meta.views.iter().enumerate() {
            for (actual, expected) in view.pose.quat.iter().zip(q0) {
                assert!((actual - expected).abs() < 1e-6, "view {i} quaternion");
            }
            let expected = if i == 0 { t0 } else { [0.1, -0.2, -0.7] };
            for (actual, expected) in view.pose.trans.iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-6, "view {i} translation");
            }
        }
        // A noncommuting rotation also checks Hamilton product order and normalization.
        let mut prediction = result(2);
        prediction.views[1].pose.0 = [2.0, 0.0, 0.0, 2.0];
        let meta = sidecar(&encode_result(&request, &prediction).expect("composed rotation"));
        for (actual, expected) in meta.views[1].pose.quat.iter().zip([0.5, 0.5, -0.5, 0.5]) {
            assert!((actual - expected).abs() < 1e-6, "normalized q0 * q");
        }
    }

    #[test]
    fn multiview_generate_orders_artifacts_finishes_progress_and_honors_cancellation() {
        let mut backend = MultiviewBackend::with_stub(
            "mapanything-apache",
            Box::new(|request, progress| {
                progress("stub inference", 0.5);
                Ok(result(request.views.len()))
            }),
        );
        assert_eq!(backend.model_id(), "mapanything-apache");
        let params = params(2);
        let mut fractions = Vec::new();
        let artifacts = backend
            .generate(&params, &mut |_, f| fractions.push(f), &CancelToken::new())
            .expect("stub generation");
        assert_eq!(artifacts.len(), 2);
        assert_eq!(
            (artifacts[0].content_type, artifacts[0].ext),
            ("application/json", "json")
        );
        assert_eq!(
            (artifacts[1].content_type, artifacts[1].ext),
            ("application/octet-stream", "bin")
        );
        assert_eq!(fractions.last(), Some(&1.0));
        let token = CancelToken::new();
        token.cancel();
        let mut backend = MultiviewBackend::with_stub(
            "mapanything",
            Box::new(|_, _| panic!("cancelled job must not run")),
        );
        assert!(matches!(
            backend.generate(&params, &mut |_, _| {}, &token),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn multiview_rejects_stub_view_count_and_map_length_mismatches() {
        let params = params(2);
        let mut backend =
            MultiviewBackend::with_stub("mapanything", Box::new(|_, _| Ok(result(1))));
        assert!(matches!(
            backend.generate(&params, &mut |_, _| {}, &CancelToken::new()),
            Err(AssetAiError::Backend(_))
        ));
        let request = parse_request(&params).expect("two views");
        let mut result = result(2);
        result.views[1].depth_z.pop();
        assert!(
            matches!(
                encode_result(&request, &result),
                Err(AssetAiError::Backend(_))
            ),
            "depth length"
        );
        result.views[1].depth_z.push(0.0);
        result.views[1].confidence.pop();
        assert!(
            matches!(
                encode_result(&request, &result),
                Err(AssetAiError::Backend(_))
            ),
            "confidence length"
        );
        result.views[1].confidence.push(0.0);
        result.views[1].mask.pop();
        assert!(
            matches!(
                encode_result(&request, &result),
                Err(AssetAiError::Backend(_))
            ),
            "mask length"
        );
    }
}
