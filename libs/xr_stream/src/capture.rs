//! The headset's cameras for the desktop app that asks (`XrServerMsg::Capture`): its
//! passthrough cameras' calibration once, then keyframes: both cameras' pictures, lossless, each
//! with the head pose at the middle of its exposure, and the newest environment depth image.
//! A keyframe is taken each time the head turned or moved the asked step since the last one,
//! from frames sharp enough (the head's turn during the exposure smears less than a pixel).
//! Everything goes up as `crate::xr_capture` records in the link's gaps; the next keyframe waits
//! until the link has taken most of the last.
//!
//! The pictures are the wearer's: they go to the app that asked and nowhere else (never to a
//! log).
//!
//! Both ends use the plane codec (`makepad_desk_link::xr_capture`'s `encode_plane`, `decode_plane`).

use makepad_desk_link::xr::link::XrLink;
use makepad_desk_link::xr_capture::*;
use makepad_widgets::makepad_platform::makepad_hal as hal;
use makepad_widgets::makepad_platform::permission::{Permission, PermissionResult, PermissionStatus};
use makepad_widgets::makepad_platform::video::{CameraCalibration, CameraFrameLayout, CameraFrameRef, VideoInputId, VideoPixelFormat, MAX_VIDEO_DEVICE_INDEX};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The camera slots the capture reads: the platform's last two.
const SLOTS: [usize; 2] = [MAX_VIDEO_DEVICE_INDEX - 2, MAX_VIDEO_DEVICE_INDEX - 1];
/// A keyframe's pictures: the cameras' middles of exposure at most this far apart, so a camera
/// and the other's next frame at 30 Hz, in step or not. Each picture has its own head pose.
const PAIR_NS: u64 = 34_000_000;
/// How long after the grant the platform's camera list may still be the one read before it
/// (it reads the cameras once a second).
const LIST_SETTLES: Duration = Duration::from_secs(5);
/// A frame the head's turn smears by more than this during its exposure (pixels) is not taken.
const BLUR_MAX_PX: f32 = 1.0;
/// The next keyframe is taken once the link holds less than this of the last one.
const QUEUED_MAX: usize = 4 << 20;
/// What the wearer allows before the passthrough cameras list, asked one after the other: the device's cameras,
/// then the headset's own.
const ALLOWED: [(Permission, &str); 2] = [(Permission::Camera, "camera"), (Permission::HeadsetCamera, "headset camera")];
/// How often the capture says how it goes.
const STATUS_EVERY: Duration = Duration::from_secs(2);

// ------------------------------------------------------------------ the headset's side

/// A camera's picture of a keyframe, taken from its frame (planes still raw).
struct Taken {
    camera: u8,
    time_ns: u64,
    head: hal::XrHeadAt,
    exposure_ns: u32,
    rolling_skew_ns: u32,
    sensitivity: u32,
    /// Y, U, V: (width, height, samples)
    planes: Vec<(u32, u32, Vec<u8>)>,
}

enum Job {
    Record(CaptureRecord),
    Keyframe { index: u32, views: Vec<Taken>, depth: Option<makepad_widgets::makepad_platform::os::XrDepthTexels> },
}

impl Job {
    /// What the sender sends of this job: anything while the capture is on; once it is off,
    /// only a status (the pictures queued before the off are dropped, not sent).
    fn admit(self, on: bool) -> Option<Job> {
        (on || matches!(self, Job::Record(CaptureRecord::Status { .. }))).then_some(self)
    }
}

/// What the frame callbacks share: whether a keyframe is due, the one being put together.
struct Selector {
    jobs: Option<Sender<Job>>,
    ask: CaptureAsk,
    cameras: usize,
    /// Each camera's focal length in its frames' pixels (the blur gate).
    focal_px: [f32; 2],
    /// The head at the last keyframe.
    last: Option<hal::XrHeadAt>,
    pending: [Option<Taken>; 2],
    index: u32,
    in_flight: Arc<AtomicUsize>,
    /// Since the last status: frames seen, held for blur, without an exposure time on the HAL
    /// clock, without a tracked head then.
    frames: u64,
    blurred: u64,
    untimed: u64,
    untracked: u64,
}

fn angle_deg(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    2.0 * dot.acos().to_degrees()
}

impl Selector {
    /// The capture is off: no more keyframes, and the pictures and pose it held let go.
    fn off(&mut self) {
        self.jobs = None;
        self.pending = [None, None];
        self.last = None;
    }

    fn due(&self, head: &hal::XrHeadAt) -> bool {
        let Some(last) = &self.last else { return true };
        let d = [head.position[0] - last.position[0], head.position[1] - last.position[1], head.position[2] - last.position[2]];
        angle_deg(head.orientation, last.orientation) >= self.ask.step_deg || (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() >= self.ask.step_m
    }

    fn on_frame(&mut self, camera: usize, frame: CameraFrameRef<'_>) {
        if self.jobs.is_none() || camera >= self.cameras || frame.layout != CameraFrameLayout::I420 || frame.plane_count < 3 {
            return;
        }
        self.frames += 1;
        let Some(time_ns) = frame.exposure.mid_ns() else {
            self.untimed += 1;
            return;
        };
        let Some(head) = hal::hal_xr_locate_head(time_ns) else {
            self.untracked += 1;
            return;
        };
        // (the link still holds the last keyframe: the next waits)
        if self.in_flight.load(Ordering::Acquire) > 0 || !self.due(&head) {
            return;
        }
        let w = head.angular_velocity;
        let blur = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt() * frame.exposure.exposure_ns as f32 * 1e-9 * self.focal_px[camera];
        if blur > BLUR_MAX_PX {
            self.blurred += 1;
            return;
        }
        let planes = (0..3)
            .map(|i| {
                let (pw, ph) = if i == 0 { (frame.width, frame.height) } else { (frame.width.div_ceil(2), frame.height.div_ceil(2)) };
                let p = frame.planes[i];
                let mut out = vec![0u8; pw * ph];
                for (r, line) in out.chunks_exact_mut(pw.max(1)).enumerate() {
                    let at = r * p.row_stride;
                    if p.pixel_stride == 1 {
                        if let Some(src) = p.bytes.get(at..at + pw) {
                            line.copy_from_slice(src);
                        }
                    } else {
                        for (c, v) in line.iter_mut().enumerate() {
                            *v = p.bytes.get(at + c * p.pixel_stride).copied().unwrap_or(0);
                        }
                    }
                }
                (pw as u32, ph as u32, out)
            })
            .collect();
        let e = frame.exposure;
        self.pending[camera] = Some(Taken { camera: camera as u8, time_ns, head, exposure_ns: e.exposure_ns, rolling_skew_ns: e.rolling_skew_ns, sensitivity: e.sensitivity, planes });
        // a camera's picture of another moment goes (the pair starts again from this one)
        for p in &mut self.pending {
            if p.as_ref().is_some_and(|t| t.time_ns.abs_diff(time_ns) > PAIR_NS) {
                *p = None;
            }
        }
        if self.pending[..self.cameras].iter().all(|p| p.is_some()) {
            let views: Vec<Taken> = self.pending.iter_mut().filter_map(|p| p.take()).collect();
            self.last = Some(views[0].head);
            let depth = Cx::xr_newest_depth_texels();
            self.in_flight.fetch_add(1, Ordering::AcqRel);
            if let Some(jobs) = &self.jobs {
                let _ = jobs.send(Job::Keyframe { index: self.index, views, depth });
            }
            self.index += 1;
        }
    }
}

/// A capture the desktop app asked for, running while it asks and the link lasts.
pub(crate) struct HeadsetCapture {
    link: Arc<XrLink>,
    jobs: Sender<Job>,
    /// On until stopped: the sender drops what was queued before the off.
    on: Arc<AtomicBool>,
    selector: Arc<Mutex<Selector>>,
    /// The cameras read (input, format size), by slot; empty until listed and opened.
    opened: Vec<(VideoInputId, [usize; 2])>,
    /// How many of [`ALLOWED`] the wearer granted so far, in their order.
    granted: usize,
    /// The next of them to ask for, at the next tick.
    to_ask: Option<Permission>,
    /// When the last of them came.
    granted_at: Option<Instant>,
    state: u8,
    status_at: Instant,
}

fn lens(c: &CameraCalibration) -> Option<CaptureLensPose> {
    Some(CaptureLensPose { rotation: c.lens_rotation?, translation: c.lens_translation?, reference: c.lens_reference.unwrap_or(2) })
}

/// What an answer to a permission request means to a capture that has `granted` of [`ALLOWED`].
#[derive(Debug, PartialEq)]
enum Answer {
    /// Not the one waited for.
    Other,
    /// Granted, and this one is asked for next.
    Next(Permission),
    /// Granted, the last of them.
    All,
    /// Refused: the capture's status.
    Refused(String),
}

fn answer(granted: usize, result: &PermissionResult) -> Answer {
    let Some(&(permission, name)) = ALLOWED.get(granted) else { return Answer::Other };
    if result.permission != permission {
        return Answer::Other;
    }
    match result.status {
        PermissionStatus::Granted => ALLOWED.get(granted + 1).map_or(Answer::All, |next| Answer::Next(next.0)),
        PermissionStatus::DeniedCanRetry | PermissionStatus::DeniedPermanent => {
            Answer::Refused(format!("the {name} permission was refused: allow it for this app in the headset's settings"))
        }
        _ => Answer::Other,
    }
}

/// A picture of this camera can be placed in the world.
fn calibrated(c: &CameraCalibration) -> bool {
    c.intrinsics.is_some() && lens(c).is_some()
}

impl HeadsetCapture {
    fn start(cx: &mut Cx, link: Arc<XrLink>, ask: CaptureAsk) -> HeadsetCapture {
        log!("xr capture: on ({:.0} deg / {:.0} mm a keyframe)", ask.step_deg, ask.step_m * 1000.0);
        let (jobs, rx) = channel();
        let in_flight = Arc::new(AtomicUsize::new(0));
        let on = Arc::new(AtomicBool::new(true));
        let spawned = {
            let (link, in_flight, on) = (link.clone(), in_flight.clone(), on.clone());
            std::thread::Builder::new().name("xr-capture".into()).spawn(move || send_jobs(link, rx, in_flight, on))
        };
        let selector = Selector { jobs: Some(jobs.clone()), ask, cameras: 0, focal_px: [0.0; 2], last: None, pending: [None, None], index: 0, in_flight, frames: 0, blurred: 0, untimed: 0, untracked: 0 };
        let selector = Arc::new(Mutex::new(selector));
        let mut c = HeadsetCapture { link, jobs, on, selector: selector.clone(), opened: Vec::new(), granted: 0, to_ask: None, granted_at: None, state: capture_state::OFF, status_at: Instant::now() };
        if let Err(e) = spawned {
            // (no sender thread: the status goes on the link from here, nothing else does)
            log!("xr capture: no sender thread: {e}");
            c.state = capture_state::FAILED;
            c.on.store(false, Ordering::Release);
            selector.lock().unwrap().off();
            let failed = CaptureRecord::Status { state: capture_state::FAILED, text: format!("the headset could not start the capture: {e}") };
            for chunk in capture_chunks(0, &failed.encode()) {
                c.link.send_bulk(&chunk);
            }
            return c;
        }
        // (each asked once, the next when the one before is granted: granted already, it is answered at once)
        cx.request_permission(ALLOWED[0].0);
        cx.xr_set_depth_tap(true);
        // the slots' frames to the selector (camera i reads slot i; the first callback also has
        // the platform list its cameras)
        for (i, slot) in SLOTS.iter().enumerate() {
            let selector = selector.clone();
            cx.camera_frame_input(*slot, move |frame| selector.lock().unwrap().on_frame(i, frame));
        }
        c.status(capture_state::STARTING, "opening the cameras".into());
        c
    }

    fn status(&mut self, state: u8, text: String) {
        if state != self.state {
            log!("xr capture: {text}");
        }
        self.state = state;
        let _ = self.jobs.send(Job::Record(CaptureRecord::Status { state, text }));
    }

    /// The passthrough cameras opened at their largest size once the platform lists them.
    fn open(&mut self, cx: &mut Cx) {
        // (without the permission the list hides what the capture is for: a camera's intrinsics
        // and lens pose, perhaps the passthrough cameras themselves)
        let Some(granted_at) = self.granted_at else { return };
        if !self.opened.is_empty() || self.state == capture_state::FAILED {
            return;
        }
        let Some(descs) = cx.video_inputs() else { return };
        let mut cams: Vec<_> = descs.iter().filter_map(|d| cx.video_input_calibration(d.input_id).filter(|c| c.passthrough).map(|c| (d.clone(), c))).collect();
        cams.sort_by_key(|(_, c)| c.position.unwrap_or(u8::MAX));
        cams.truncate(SLOTS.len());
        if cams.is_empty() || !cams.iter().all(|(_, c)| calibrated(c)) {
            if granted_at.elapsed() >= LIST_SETTLES {
                let why = if cams.is_empty() { "this headset lists no passthrough cameras" } else { "the passthrough cameras give no intrinsics or lens pose" };
                self.status(capture_state::FAILED, why.into());
            }
            return;
        }
        let mut records = Vec::new();
        for (i, (desc, calibration)) in cams.iter().enumerate() {
            let Some(format) = desc.formats.iter().filter(|f| f.pixel_format == VideoPixelFormat::YUV420).max_by_key(|f| f.width * f.height) else {
                self.status(capture_state::FAILED, format!("{} has no YUV format", desc.name));
                return;
            };
            if let Err(e) = cx.video_input_open(SLOTS[i], desc.input_id, format.format_id).and_then(|_| cx.video_input_stream(SLOTS[i], true)) {
                self.status(capture_state::FAILED, format!("camera {i}: {e}"));
                return;
            }
            // the intrinsics count in the pre-correction array's pixels; the frames show the
            // active array scaled to their size
            let array = calibration.pre_correction.unwrap_or([0, 0, format.width as i32, format.height as i32]);
            let active = calibration.active.unwrap_or(array);
            let focal = calibration.intrinsics.map_or(0.0, |k| k[0] * format.width as f32 / active[2].max(1) as f32);
            self.selector.lock().unwrap().focal_px[i] = focal;
            records.push(CaptureCamera {
                position: calibration.position.unwrap_or(i as u8),
                size: [format.width as u32, format.height as u32],
                intrinsics: calibration.intrinsics,
                array,
                active,
                distortion: calibration.distortion,
                lens_pose: lens(calibration),
            });
            self.opened.push((desc.input_id, [format.width, format.height]));
            log!("xr capture: camera {i} ({}) at {}x{}", desc.name, format.width, format.height);
        }
        self.selector.lock().unwrap().cameras = self.opened.len();
        let _ = self.jobs.send(Job::Record(CaptureRecord::Cameras(records)));
    }

    fn stop(mut self, cx: &mut Cx) {
        for slot in SLOTS.iter().take(self.opened.len()) {
            let _ = cx.video_input_stream(*slot, false);
            cx.video_input_close(*slot);
        }
        // (the callbacks stay registered but see no frames; this ends the worker's channel, and
        // the sender drops the pictures still queued)
        self.on.store(false, Ordering::Release);
        self.selector.lock().unwrap().off();
        cx.xr_set_depth_tap(false);
        if self.link.closed().is_none() {
            self.status(capture_state::OFF, "capture off".into());
        }
        log!("xr capture: off");
    }

    fn tick(&mut self, cx: &mut Cx) {
        if let Some(permission) = self.to_ask.take() {
            cx.request_permission(permission);
        }
        self.open(cx);
        if self.state == capture_state::FAILED || self.status_at.elapsed() < STATUS_EVERY {
            return;
        }
        self.status_at = Instant::now();
        let (frames, blurred, untimed, untracked, index) = {
            let mut s = self.selector.lock().unwrap();
            let v = (s.frames, s.blurred, s.untimed, s.untracked, s.index);
            (s.frames, s.blurred, s.untimed, s.untracked) = (0, 0, 0, 0);
            v
        };
        if frames == 0 {
            let waiting = match ALLOWED.get(self.granted) {
                Some((_, name)) => format!("waiting for the wearer to allow the {name}"),
                None if self.opened.is_empty() => "waiting for the camera list".into(),
                None => "waiting for the cameras' first frames".into(),
            };
            self.status(capture_state::STARTING, waiting);
        } else if untimed == frames {
            self.status(capture_state::FAILED, "the cameras give no exposure times on the headset's clock: no head poses for them".into());
        } else {
            self.status(capture_state::CAPTURING, format!("{index} keyframes; last 2 s: {frames} frames, {blurred} too blurred (turn slower), {untracked} while the head was not tracked"));
        }
    }

    pub(crate) fn permission(&mut self, result: &PermissionResult) {
        match answer(self.granted, result) {
            Answer::Other => {}
            Answer::Next(permission) => {
                self.granted += 1;
                self.to_ask = Some(permission);
            }
            Answer::All => {
                self.granted += 1;
                self.granted_at = Some(Instant::now());
            }
            Answer::Refused(why) => self.status(capture_state::FAILED, why),
        }
    }

    /// Runs the capture as `ask` says on `link` (from the client's housekeeping: every XR frame,
    /// or its idle timer).
    pub(crate) fn drive(capture: &mut Option<HeadsetCapture>, cx: &mut Cx, link: Option<&Arc<XrLink>>, ask: Option<CaptureAsk>) {
        let same = |c: &HeadsetCapture| link.is_some_and(|l| Arc::ptr_eq(l, &c.link)) && ask.is_some();
        if capture.as_ref().is_some_and(|c| !same(c)) {
            capture.take().unwrap().stop(cx);
        }
        match (capture.as_mut(), link, ask) {
            (None, Some(link), Some(ask)) => *capture = Some(HeadsetCapture::start(cx, link.clone(), ask)),
            (Some(c), _, Some(ask)) => {
                c.selector.lock().unwrap().ask = ask;
                c.tick(cx);
            }
            _ => {}
        }
    }
}

/// The capture's sender thread: each record encoded (a keyframe's planes compressed here, off
/// the UI thread), cut into chunks and queued in the link's gaps, in order.
fn send_jobs(link: Arc<XrLink>, jobs: Receiver<Job>, in_flight: Arc<AtomicUsize>, on: Arc<AtomicBool>) {
    let mut seq = 0u32;
    while let Ok(job) = jobs.recv() {
        if link.closed().is_some() {
            return;
        }
        let is_keyframe = matches!(job, Job::Keyframe { .. });
        let record = match job.admit(on.load(Ordering::Acquire)) {
            Some(Job::Record(r)) => Some(r),
            Some(Job::Keyframe { index, views, depth }) => Some(CaptureRecord::Keyframe(keyframe(index, views, depth))),
            None => None,
        };
        // (off during the planes' compression: that keyframe is dropped too)
        if let Some(record) = record.filter(|r| matches!(r, CaptureRecord::Status { .. }) || on.load(Ordering::Acquire)) {
            for chunk in capture_chunks(seq, &record.encode()) {
                link.send_bulk(&chunk);
            }
            seq = seq.wrapping_add(1);
            if is_keyframe {
                link.wait_bulk_below(QUEUED_MAX);
            }
        }
        if is_keyframe {
            in_flight.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

fn keyframe(index: u32, views: Vec<Taken>, depth: Option<makepad_widgets::makepad_platform::os::XrDepthTexels>) -> CaptureKeyframe {
    let pose = |o: [f32; 4], p: [f32; 3]| CapturePose { orientation: o, position: p };
    let views = views
        .into_iter()
        .map(|t| CaptureView {
            camera: t.camera,
            time_ns: t.time_ns,
            head: pose(t.head.orientation, t.head.position),
            velocity: t.head.linear_velocity,
            angular_velocity: t.head.angular_velocity,
            exposure_ns: t.exposure_ns,
            rolling_skew_ns: t.rolling_skew_ns,
            sensitivity: t.sensitivity,
            planes: t.planes.iter().map(|(w, h, s)| encode_plane(*w, *h, 1, s)).collect(),
        })
        .collect();
    let depth = depth.map(|d| {
        let h = d.header;
        let bytes: Vec<u8> = d.texels.iter().flat_map(|v| v.to_le_bytes()).collect();
        let eye = |e: &hal::XrEye| CaptureEye { pose: pose(e.pose.orientation, e.pose.position), fov: e.fov };
        CaptureDepth { time_ns: d.display_ns, near: h.near, far: h.far, views: [eye(&h.views[0]), eye(&h.views[1])], layers: h.layers.max(1), texels: encode_plane(h.width, h.height * h.layers.max(1), 2, &bytes) }
    });
    CaptureKeyframe { index, views, depth }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_without_intrinsics_or_lens_pose_is_not_calibrated() {
        // what the list says of a camera before the permission
        let hidden = CameraCalibration { passthrough: true, ..Default::default() };
        assert!(!calibrated(&hidden));
        let intrinsics = Some([870.0, 870.0, 640.0, 640.0, 0.0]);
        assert!(!calibrated(&CameraCalibration { intrinsics, ..hidden }));
        let (lens_rotation, lens_translation) = (Some([0.0, 0.0, 0.0, 1.0]), Some([0.03, 0.0, 0.0]));
        assert!(!calibrated(&CameraCalibration { lens_rotation, lens_translation, ..hidden }));
        assert!(calibrated(&CameraCalibration { intrinsics, lens_rotation, lens_translation, ..hidden }));
    }

    #[test]
    fn both_camera_permissions_are_asked_in_turn_and_a_refusal_names_its_own() {
        let result = |permission, status| PermissionResult { permission, request_id: 0, status };
        let granted = PermissionStatus::Granted;
        assert_eq!(answer(0, &result(Permission::Camera, granted)), Answer::Next(Permission::HeadsetCamera), "the headset's cameras are asked for after the device's");
        assert_eq!(answer(1, &result(Permission::HeadsetCamera, granted)), Answer::All, "both granted starts the capture");
        // an answer for another permission, or one out of turn, changes nothing
        assert_eq!(answer(0, &result(Permission::HeadsetCamera, granted)), Answer::Other);
        assert_eq!(answer(1, &result(Permission::AudioInput, PermissionStatus::DeniedPermanent)), Answer::Other);
        assert_eq!(answer(2, &result(Permission::Camera, granted)), Answer::Other);
        for (at, permission, name) in [(0, Permission::Camera, "the camera permission"), (1, Permission::HeadsetCamera, "the headset camera permission")] {
            for status in [PermissionStatus::DeniedCanRetry, PermissionStatus::DeniedPermanent] {
                let Answer::Refused(why) = answer(at, &result(permission, status)) else { panic!("a refusal fails the capture") };
                assert!(why.starts_with(name), "the status names the refused permission: {why}");
            }
        }
    }

    #[test]
    fn capture_off_lets_go_of_pictures() {
        let taken = || Taken { camera: 0, time_ns: 1, head: hal::XrHeadAt::default(), exposure_ns: 1, rolling_skew_ns: 0, sensitivity: 0, planes: vec![(2, 2, vec![9; 4])] };
        let (jobs, _rx) = channel();
        let mut s = Selector { jobs: Some(jobs), ask: CaptureAsk { step_deg: 10.0, step_m: 0.05, preview_hz: 0 }, cameras: 2, focal_px: [500.0; 2], last: Some(hal::XrHeadAt::default()), pending: [Some(taken()), None], index: 3, in_flight: Default::default(), frames: 0, blurred: 0, untimed: 0, untracked: 0 };
        // off: the picture held for a pair, the last pose and the way to the sender all go
        s.off();
        assert!(s.jobs.is_none() && s.pending.iter().all(|p| p.is_none()) && s.last.is_none());
        // the sender: anything while on; once off, only a status (no queued picture goes)
        let keyframe = || Job::Keyframe { index: 0, views: vec![taken()], depth: None };
        let status = || Job::Record(CaptureRecord::Status { state: capture_state::OFF, text: "capture off".into() });
        let cameras = || Job::Record(CaptureRecord::Cameras(Vec::new()));
        assert!(keyframe().admit(true).is_some() && cameras().admit(true).is_some());
        assert!(keyframe().admit(false).is_none() && cameras().admit(false).is_none());
        assert!(matches!(status().admit(false), Some(Job::Record(CaptureRecord::Status { .. }))));
    }

    #[test]
    fn capture_keyframe_steps() {
        let head = |deg: f32, x: f32| {
            let h = (deg.to_radians() * 0.5).sin_cos();
            hal::XrHeadAt { orientation: [0.0, h.0, 0.0, h.1], position: [x, 1.5, 0.0], ..Default::default() }
        };
        let s = Selector { jobs: None, ask: CaptureAsk { step_deg: 10.0, step_m: 0.05, preview_hz: 0 }, cameras: 2, focal_px: [500.0; 2], last: Some(head(0.0, 0.0)), pending: [None, None], index: 0, in_flight: Default::default(), frames: 0, blurred: 0, untimed: 0, untracked: 0 };
        assert!(!s.due(&head(9.0, 0.0)));
        assert!(s.due(&head(10.5, 0.0)));
        assert!(!s.due(&head(0.0, 0.04)));
        assert!(s.due(&head(0.0, 0.051)));
        assert!((angle_deg(head(0.0, 0.0).orientation, head(30.0, 0.0).orientation) - 30.0).abs() < 1e-3);
    }
}
