use crate::{
    body_view::{map_measurements_to_vertices, BodyPoseMapping},
    camera::CameraMailbox,
};
use makepad_ai_body::model::BodyModel;
use makepad_fabric_measure::{measure, BodyMesh, MeasureOptions, Measured};
use makepad_widgets::image_cache::ImageBuffer;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const PHOTO_CROP_SIZE: usize = 512;
const LIVE_CROP_SIZE: usize = 384;
const SHAPE_ALPHA: f32 = 0.35;
const POSE_ALPHA: f32 = 0.6;
const SHAPE_RESET_GAP: Duration = Duration::from_secs(1);
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);
const FRAME_POLL: Duration = Duration::from_millis(5);

pub enum PipelineMessage {
    Stage(String),
    LiveFrame {
        fps: f32,
        model_ms: f32,
        pose_ms: f32,
        person: bool,
        bbox: Option<[f32; 4]>,
    },
    Done {
        measured: Box<Measured>,
        mesh: Arc<BodyMesh>,
        posed: Option<Arc<Vec<[f32; 3]>>>,
        pose_mapping: BodyPoseMapping,
        reset_pose: bool,
    },
    Failed(String),
}

struct RunRequest {
    photo: PathBuf,
    weights: PathBuf,
    height_cm: Option<f32>,
}

struct LiveRequest {
    weights: PathBuf,
    height_cm: Option<f32>,
    mailbox: CameraMailbox,
}

enum WorkerRequest {
    Photo(RunRequest),
    Live(LiveRequest),
}

pub struct Pipeline {
    request_tx: Sender<WorkerRequest>,
    message_rx: Receiver<PipelineMessage>,
    live_stop: Arc<AtomicBool>,
}

impl Pipeline {
    pub fn new() -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (message_tx, message_rx) = mpsc::channel();
        let live_stop = Arc::new(AtomicBool::new(true));
        let worker_stop = live_stop.clone();
        thread::Builder::new()
            .name("fabric-body-pipeline".to_string())
            .spawn(move || worker(request_rx, message_tx, worker_stop))
            .expect("spawn fabric body worker");
        Self {
            request_tx,
            message_rx,
            live_stop,
        }
    }

    pub fn run(
        &self,
        photo: PathBuf,
        weights: PathBuf,
        height_cm: Option<f32>,
    ) -> Result<(), String> {
        self.request_tx
            .send(WorkerRequest::Photo(RunRequest {
                photo,
                weights,
                height_cm,
            }))
            .map_err(|_| "the body model worker stopped".to_string())
    }

    pub fn start_live(
        &self,
        weights: PathBuf,
        height_cm: Option<f32>,
        mailbox: CameraMailbox,
    ) -> Result<(), String> {
        self.live_stop.store(false, Ordering::Release);
        if self
            .request_tx
            .send(WorkerRequest::Live(LiveRequest {
                weights,
                height_cm,
                mailbox,
            }))
            .is_err()
        {
            self.live_stop.store(true, Ordering::Release);
            return Err("the body model worker stopped".to_string());
        }
        Ok(())
    }

    pub fn stop_live(&self) {
        self.live_stop.store(true, Ordering::Release);
    }

    pub fn poll(&self) -> Vec<PipelineMessage> {
        self.message_rx.try_iter().collect()
    }
}

fn emit(sender: &Sender<PipelineMessage>, message: PipelineMessage) -> bool {
    if sender.send(message).is_err() {
        return false;
    }
    SignalToUI::set_ui_signal();
    true
}

fn worker(
    requests: Receiver<WorkerRequest>,
    messages: Sender<PipelineMessage>,
    live_stop: Arc<AtomicBool>,
) {
    let mut loaded: Option<(PathBuf, BodyModel)> = None;
    while let Ok(request) = requests.recv() {
        let result = match request {
            WorkerRequest::Photo(request) => run_one(&request, &messages, &mut loaded),
            WorkerRequest::Live(request) => {
                run_live(&request, &messages, &mut loaded, &live_stop)
            }
        };
        if let Err(error) = result {
            if !emit(&messages, PipelineMessage::Failed(error)) {
                return;
            }
        }
    }
}

fn run_one(
    request: &RunRequest,
    messages: &Sender<PipelineMessage>,
    loaded: &mut Option<(PathBuf, BodyModel)>,
) -> Result<(), String> {
    emit(messages, PipelineMessage::Stage("decoding photo…".to_string()));
    let (rgb, width, height) = decode_rgb(&request.photo)?;

    ensure_model(&request.weights, messages, loaded)?;

    emit(messages, PipelineMessage::Stage("inferring…".to_string()));
    let model = &mut loaded.as_mut().expect("model was loaded above").1;
    model
        .set_crop_size(PHOTO_CROP_SIZE)
        .map_err(|error| format!("could not configure the body model: {error}"))?;
    let packet = model
        .infer(&rgb, width, height, None)
        .map_err(|error| format!("body inference failed: {error}"))?;
    let person = packet
        .people
        .first()
        .ok_or_else(|| "no person found".to_string())?;
    let posed = Arc::new(posed_vertices(
        model,
        &person.shape,
        &person.expr,
        person.mhr,
        person.global_rot,
    )?);
    let vertices = model.rig().rest_vertices(&person.shape, &person.expr);
    let face_indices = model
        .weights
        .i64_shaped("head_pose.faces", &[36_874, 3])
        .map_err(|error| format!("could not read the body mesh faces: {error}"))?;
    let mesh = Arc::new(body_mesh_from_flat(&vertices, &face_indices)?);

    emit(messages, PipelineMessage::Stage("measuring…".to_string()));
    let measured = measure(
        &mesh,
        &MeasureOptions {
            height_cm: request.height_cm,
        },
    )
    .map_err(|error| error.to_string())?;
    let pose_mapping = map_measurements_to_vertices(&mesh, &measured);
    emit(
        messages,
        PipelineMessage::Done {
            measured: Box::new(measured),
            mesh,
            posed: Some(posed),
            pose_mapping,
            reset_pose: true,
        },
    );
    Ok(())
}

fn ensure_model(
    weights: &Path,
    messages: &Sender<PipelineMessage>,
    loaded: &mut Option<(PathBuf, BodyModel)>,
) -> Result<(), String> {
    if loaded
        .as_ref()
        .map(|(path, _)| path != weights)
        .unwrap_or(true)
    {
        emit(
            messages,
            PipelineMessage::Stage("loading model 2.8 GB…".to_string()),
        );
        let model = BodyModel::load(weights)
            .map_err(|error| format!("could not load the body model: {error}"))?;
        *loaded = Some((weights.to_path_buf(), model));
    }
    Ok(())
}

fn run_live(
    request: &LiveRequest,
    messages: &Sender<PipelineMessage>,
    loaded: &mut Option<(PathBuf, BodyModel)>,
    stop: &AtomicBool,
) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    ensure_model(&request.weights, messages, loaded)?;
    let model = &mut loaded.as_mut().expect("model was loaded above").1;
    model
        .set_crop_size(LIVE_CROP_SIZE)
        .map_err(|error| format!("could not configure live body inference: {error}"))?;
    let face_indices = model
        .weights
        .i64_shaped("head_pose.faces", &[36_874, 3])
        .map_err(|error| format!("could not read the body mesh faces: {error}"))?;

    let started = Instant::now();
    let mut previous_bbox = None;
    let mut smoother = ShapeSmoother::default();
    let mut pose_smoother = PoseSmoother::default();
    let mut fps = FpsCounter::default();
    let mut reset_pose = true;

    while !stop.load(Ordering::Acquire) {
        request.mailbox.request();
        let wait_started = Instant::now();
        let frame = loop {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Some(frame) = request.mailbox.take() {
                break Some(frame);
            }
            if wait_started.elapsed() >= FRAME_TIMEOUT {
                break None;
            }
            thread::sleep(FRAME_POLL);
        };
        let Some(frame) = frame else {
            if !emit(
                messages,
                PipelineMessage::Stage("no camera frames".to_string()),
            ) {
                return Ok(());
            }
            continue;
        };

        let model_started = Instant::now();
        let packet = model
            .infer(&frame.rgb, frame.width, frame.height, previous_bbox)
            .map_err(|error| format!("live body inference failed: {error}"))?;
        let model_ms = model_started.elapsed().as_secs_f32() * 1000.0;
        let now = started.elapsed();
        let person = packet.people.first();
        let bbox = person.map(|person| person.bbox);
        let mut pose_ms = 0.0;
        let mut posed = None;
        let smoothed_shape = if let Some(person) = person {
            previous_bbox = Some(expand_bbox(
                person.bbox,
                frame.width,
                frame.height,
                0.15,
            ));
            let (mhr, global_rot) = pose_smoother.observe(person.mhr, person.global_rot);
            let pose_started = Instant::now();
            posed = Some(Arc::new(posed_vertices(
                model,
                &person.shape,
                &person.expr,
                mhr,
                global_rot,
            )?));
            pose_ms = pose_started.elapsed().as_secs_f32() * 1000.0;
            Some(smoother.observe_person(person.shape, now))
        } else {
            previous_bbox = None;
            smoother.observe_miss(now);
            pose_smoother.reset();
            None
        };
        let current_fps = fps.tick(now);
        if !emit(
            messages,
            PipelineMessage::LiveFrame {
                fps: current_fps,
                model_ms,
                pose_ms,
                person: person.is_some(),
                bbox,
            },
        ) {
            return Ok(());
        }
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }

        let Some(shape) = smoothed_shape else {
            continue;
        };
        let expression = [0.0f32; 72];
        let vertices = model.rig().rest_vertices(&shape, &expression);
        let mesh = Arc::new(body_mesh_from_flat(&vertices, &face_indices)?);
        let measured = measure(
            &mesh,
            &MeasureOptions {
                height_cm: request.height_cm,
            },
        )
        .map_err(|error| error.to_string())?;
        let pose_mapping = map_measurements_to_vertices(&mesh, &measured);
        if !emit(
            messages,
            PipelineMessage::Done {
                measured: Box::new(measured),
                mesh,
                posed,
                pose_mapping,
                reset_pose,
            },
        ) {
            return Ok(());
        }
        reset_pose = false;
    }
    Ok(())
}

#[derive(Default)]
struct PoseSmoother {
    mhr: Option<[f32; 204]>,
    global_rot: Option<[f32; 3]>,
}

impl PoseSmoother {
    fn observe(
        &mut self,
        mhr: [f32; 204],
        global_rot: [f32; 3],
    ) -> ([f32; 204], [f32; 3]) {
        let global_rot = match self.global_rot {
            Some(previous) => std::array::from_fn(|index| {
                previous[index] + POSE_ALPHA * (global_rot[index] - previous[index])
            }),
            None => global_rot,
        };
        let mut mhr = match self.mhr {
            Some(previous) => std::array::from_fn(|index| {
                previous[index] + POSE_ALPHA * (mhr[index] - previous[index])
            }),
            None => mhr,
        };
        // The packet's 204 values are exactly the rig's [pose 136 | scales 68]
        // input. Global rotation is pose slots 3..6; keep the separately
        // smoothed copy authoritative before MhrRig::forward pads to 249.
        mhr[3..6].copy_from_slice(&global_rot);
        self.mhr = Some(mhr);
        self.global_rot = Some(global_rot);
        (mhr, global_rot)
    }

    fn reset(&mut self) {
        self.mhr = None;
        self.global_rot = None;
    }
}

#[derive(Default)]
struct ShapeSmoother {
    shape: Option<[f32; 45]>,
    last_person: Option<Duration>,
}

impl ShapeSmoother {
    fn observe_person(&mut self, shape: [f32; 45], now: Duration) -> [f32; 45] {
        if self
            .last_person
            .is_some_and(|last| now.saturating_sub(last) > SHAPE_RESET_GAP)
        {
            self.shape = None;
        }
        self.last_person = Some(now);
        let smoothed = match self.shape {
            Some(previous) => std::array::from_fn(|index| {
                previous[index] + SHAPE_ALPHA * (shape[index] - previous[index])
            }),
            None => shape,
        };
        self.shape = Some(smoothed);
        smoothed
    }

    fn observe_miss(&mut self, now: Duration) {
        if self
            .last_person
            .is_some_and(|last| now.saturating_sub(last) > SHAPE_RESET_GAP)
        {
            self.shape = None;
            self.last_person = None;
        }
    }
}

#[derive(Default)]
struct FpsCounter {
    samples: VecDeque<Duration>,
}

impl FpsCounter {
    fn tick(&mut self, now: Duration) -> f32 {
        self.samples.push_back(now);
        while self.samples.len() > 10 {
            self.samples.pop_front();
        }
        let Some(first) = self.samples.front().copied() else {
            return 0.0;
        };
        let seconds = now.saturating_sub(first).as_secs_f32();
        if seconds <= f32::EPSILON {
            0.0
        } else {
            (self.samples.len().saturating_sub(1)) as f32 / seconds
        }
    }
}

pub(crate) fn expand_bbox(
    bbox: [f32; 4],
    width: u32,
    height: u32,
    amount: f32,
) -> [f32; 4] {
    let box_width = (bbox[2] - bbox[0]).max(0.0);
    let box_height = (bbox[3] - bbox[1]).max(0.0);
    [
        (bbox[0] - box_width * amount).clamp(0.0, width as f32),
        (bbox[1] - box_height * amount).clamp(0.0, height as f32),
        (bbox[2] + box_width * amount).clamp(0.0, width as f32),
        (bbox[3] + box_height * amount).clamp(0.0, height as f32),
    ]
}

fn posed_vertices(
    model: &BodyModel,
    shape: &[f32; 45],
    expression: &[f32; 72],
    mut mhr: [f32; 204],
    global_rot: [f32; 3],
) -> Result<Vec<[f32; 3]>, String> {
    // BodyPerson::mhr is already model_params(): pose 0..136 followed by
    // scales 136..204. The root translation remains zero and the model's
    // global rotation occupies 3..6. forward() pads the 45 identity slots
    // to the rig's 249-wide internal vector, then returns rig-space cm.
    mhr[3..6].copy_from_slice(&global_rot);
    let output = model.rig().forward(shape, &mhr, expression, true);
    vertices_from_flat(&output.verts)
}

fn decode_rgb(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let image = match extension.as_str() {
        "jpg" | "jpeg" => ImageBuffer::from_jpg(&bytes),
        "png" => ImageBuffer::from_png(&bytes),
        _ => return Err("choose a JPG or PNG photo".to_string()),
    }
    .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    let pixel_count = image
        .width
        .checked_mul(image.height)
        .ok_or_else(|| "photo dimensions are too large".to_string())?;
    if image.data.len() < pixel_count {
        return Err("decoded photo has too few pixels".to_string());
    }
    let width = u32::try_from(image.width).map_err(|_| "photo is too wide".to_string())?;
    let height = u32::try_from(image.height).map_err(|_| "photo is too tall".to_string())?;
    Ok((argb_to_rgb(&image.data[..pixel_count]), width, height))
}

pub(crate) fn argb_to_rgb(pixels: &[u32]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        rgb.push((pixel >> 16) as u8);
        rgb.push((pixel >> 8) as u8);
        rgb.push(*pixel as u8);
    }
    rgb
}

pub(crate) fn faces_i64_to_u32(
    values: &[i64],
    vertex_count: usize,
) -> Result<Vec<[u32; 3]>, String> {
    if values.len() % 3 != 0 {
        return Err("body face index buffer is not made of triangles".to_string());
    }
    values
        .chunks_exact(3)
        .enumerate()
        .map(|(face_index, triangle)| {
            let mut face = [0; 3];
            for corner in 0..3 {
                let index = usize::try_from(triangle[corner]).map_err(|_| {
                    format!("body face {face_index} contains a negative vertex index")
                })?;
                if index >= vertex_count {
                    return Err(format!(
                        "body face {face_index} references vertex {index}, but there are {vertex_count} vertices"
                    ));
                }
                face[corner] = u32::try_from(index)
                    .map_err(|_| format!("body vertex index {index} exceeds u32"))?;
            }
            Ok(face)
        })
        .collect()
}

pub(crate) fn body_mesh_from_flat(
    vertices: &[f32],
    face_indices: &[i64],
) -> Result<BodyMesh, String> {
    let vertices = vertices_from_flat(vertices)?;
    let faces = faces_i64_to_u32(face_indices, vertices.len())?;
    Ok(BodyMesh {
        vertices,
        faces,
        landmarks: None,
    })
}

fn vertices_from_flat(vertices: &[f32]) -> Result<Vec<[f32; 3]>, String> {
    if vertices.is_empty() || vertices.len() % 3 != 0 {
        return Err("body vertex buffer is empty or malformed".to_string());
    }
    Ok(vertices
        .chunks_exact(3)
        .map(|point| [point[0], point[1], point[2]])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_aarrggbb_to_rgb() {
        assert_eq!(
            argb_to_rgb(&[0xff_12_34_56, 0x00_ab_cd_ef]),
            vec![0x12, 0x34, 0x56, 0xab, 0xcd, 0xef]
        );
    }

    #[test]
    fn builds_a_tiny_checked_mesh() {
        let mesh = body_mesh_from_flat(
            &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            &[0, 1, 2],
        )
        .unwrap();
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.faces, vec![[0, 1, 2]]);
        assert!(faces_i64_to_u32(&[0, 1, 3], 3).is_err());
        assert!(faces_i64_to_u32(&[-1, 1, 2], 3).is_err());
    }

    #[test]
    fn live_shape_ema_converges_and_resets_after_a_gap() {
        let mut smoother = ShapeSmoother::default();
        assert_eq!(
            smoother.observe_person([0.0; 45], Duration::ZERO),
            [0.0; 45]
        );
        let first = smoother.observe_person([10.0; 45], Duration::from_millis(250));
        assert!((first[0] - 3.5).abs() < 0.0001);
        let second = smoother.observe_person([10.0; 45], Duration::from_millis(500));
        assert!((second[0] - 5.775).abs() < 0.0001);

        smoother.observe_miss(Duration::from_millis(1_501));
        let reset = smoother.observe_person([8.0; 45], Duration::from_millis(1_750));
        assert_eq!(reset, [8.0; 45]);
    }

    #[test]
    fn live_pose_ema_smooths_parameters_and_keeps_global_slots_in_sync() {
        let mut smoother = PoseSmoother::default();
        let mut initial_mhr = [0.0; 204];
        initial_mhr[20] = 2.0;
        let (initial, initial_rot) = smoother.observe(initial_mhr, [1.0, 2.0, 3.0]);
        assert_eq!(initial[3..6], initial_rot);

        let mut next_mhr = [10.0; 204];
        next_mhr[20] = 12.0;
        let (smoothed, smoothed_rot) = smoother.observe(next_mhr, [3.0, 4.0, 5.0]);
        assert!((smoothed[20] - 8.0).abs() < 0.0001);
        assert_eq!(smoothed_rot, [2.2, 3.2, 4.2]);
        assert_eq!(smoothed[3..6], smoothed_rot);

        smoother.reset();
        let (reset, _) = smoother.observe([7.0; 204], [0.5, 1.0, 1.5]);
        assert_eq!(reset[20], 7.0);
    }

    #[test]
    fn bbox_expansion_is_fifteen_percent_and_clamped() {
        let expanded = expand_bbox([100.0, 50.0, 300.0, 250.0], 640, 360, 0.15);
        for (actual, expected) in expanded.into_iter().zip([70.0, 20.0, 330.0, 280.0]) {
            assert!((actual - expected).abs() < 0.0001, "{expanded:?}");
        }
        assert_eq!(
            expand_bbox([5.0, 10.0, 635.0, 350.0], 640, 360, 0.15),
            [0.0, 0.0, 640.0, 360.0]
        );
    }

    #[test]
    fn fps_counter_tracks_recent_frame_cadence() {
        let mut counter = FpsCounter::default();
        assert_eq!(counter.tick(Duration::ZERO), 0.0);
        for quarter in 1..=4 {
            let fps = counter.tick(Duration::from_millis(quarter * 250));
            assert!((fps - 4.0).abs() < 0.0001, "{fps}");
        }
    }
}
