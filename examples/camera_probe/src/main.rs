//! A device check of camera intake. It runs each camera the platform lists for 10 s, one at a
//! time, at its largest YUV format of at least 24 fps, then a headset's two passthrough cameras
//! together, and logs `camera-probe:` lines: each camera's calibration, then per run the frame
//! rate against the sensor's, the longest gap between frames (a stall), the share of frames that
//! came with their capture result's exposure, the delay from the middle of exposure to arrival on
//! the HAL clock (its clock conversion: a frame that waited for its result arrives one frame
//! late), and for the pair how far apart the two cameras' exposures are. Each run ends with
//! `camera-probe: PASS ...` or `camera-probe: FAIL ...` and its faults; the last line is
//! `camera-probe: done`. It logs no picture.
use makepad_widgets::makepad_platform::makepad_hal::clock_ns;
use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
use makepad_widgets::makepad_platform::video::{CameraExposure, VideoFormat, VideoInputDesc, VideoPixelFormat};
use makepad_widgets::*;
use std::sync::{Arc, Mutex};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(640, 240)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        padding: 24
                        status := Label{ text: "camera probe: asking for the cameras" }
                    }
                }
            }
        }
    }
}

/// How long each run lasts (s).
const RUN_S: f64 = 10.0;
/// The capture's pairing window: a keyframe's cameras' exposures at most this far apart.
const PAIR_NS: u64 = 8_000_000;
/// A frame's delay from the middle of its exposure to its arrival, outside of which the HAL
/// clock conversion is wrong (ns).
const DELAY_MAX_NS: i64 = 500_000_000;

/// What one camera's frames showed during a run.
#[derive(Default)]
struct Intake {
    frames: u64,
    with_exposure: u64,
    with_start: u64,
    backwards: u64,
    last_timestamp: Option<u64>,
    first_at: u64,
    last_at: u64,
    max_gap_ns: u64,
    /// Arrival less the middle of exposure (HAL clock).
    delays_ns: Vec<i64>,
    mids_ns: Vec<u64>,
    frame_durations_ns: Vec<u32>,
    last: CameraExposure,
}

fn median<T: Copy + Ord>(v: &[T]) -> Option<T> {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

impl Intake {
    fn add(&mut self, timestamp_ns: u64, e: CameraExposure, now_ns: u64) {
        if self.frames == 0 {
            self.first_at = now_ns;
        } else {
            self.max_gap_ns = self.max_gap_ns.max(now_ns.saturating_sub(self.last_at));
        }
        self.frames += 1;
        self.last_at = now_ns;
        if self.last_timestamp.is_some_and(|t| timestamp_ns <= t) {
            self.backwards += 1;
        }
        self.last_timestamp = Some(timestamp_ns);
        if e.exposure_ns != 0 {
            self.with_exposure += 1;
            self.frame_durations_ns.push(e.frame_duration_ns);
            self.last = e;
        }
        if let Some(mid) = e.mid_ns() {
            self.with_start += 1;
            self.delays_ns.push(now_ns as i64 - mid as i64);
            self.mids_ns.push(mid);
        }
    }

    /// The run's line and its faults. `format_fps`: the format's rate, for a camera that gives no
    /// frame durations.
    fn verdict(&self, format_fps: Option<f64>) -> (String, Vec<String>) {
        let mut faults = Vec::new();
        if self.frames < 2 {
            faults.push(format!("{} frames", self.frames));
            return (format!("{} frames", self.frames), faults);
        }
        let fps = (self.frames - 1) as f64 * 1e9 / (self.last_at - self.first_at).max(1) as f64;
        let sensor_fps = median(&self.frame_durations_ns).filter(|d| *d > 0).map(|d| 1e9 / d as f64);
        let expected = sensor_fps.or(format_fps).unwrap_or(15.0);
        if fps < 0.9 * expected {
            faults.push(format!("{fps:.1} fps of {expected:.1}"));
        }
        let gap_max = (4e9 / expected).max(200e6) as u64;
        if self.max_gap_ns > gap_max {
            faults.push(format!("a {:.0} ms gap (stall)", self.max_gap_ns as f64 / 1e6));
        }
        if self.backwards != 0 {
            faults.push(format!("{} timestamps not after the last", self.backwards));
        }
        let wild = self.delays_ns.iter().filter(|d| **d <= 0 || **d > DELAY_MAX_NS).count();
        if wild != 0 {
            faults.push(format!("{wild} exposure times off the HAL clock"));
        }
        let pct = |n: u64| 100.0 * n as f64 / self.frames as f64;
        let e = self.last;
        let line = format!(
            "{fps:.1} fps (sensor {}), longest gap {:.1} ms, {:.0}% with exposure, {:.0}% on the HAL clock, delay p50 {} ms max {} ms; exposure {:.2} ms, skew {:.2} ms, ISO {}",
            sensor_fps.map_or("unknown".into(), |f| format!("{f:.1}")),
            self.max_gap_ns as f64 / 1e6,
            pct(self.with_exposure),
            pct(self.with_start),
            median(&self.delays_ns).map_or("-".into(), |d| format!("{:.1}", d as f64 / 1e6)),
            self.delays_ns.iter().max().map_or("-".into(), |d| format!("{:.1}", *d as f64 / 1e6)),
            e.exposure_ns as f64 / 1e6,
            e.rolling_skew_ns as f64 / 1e6,
            e.sensitivity,
        );
        (line, faults)
    }
}

/// For each of `b`'s exposure middles, how far the nearest of `a`'s is (ns): median and max.
fn apart_ns(a: &[u64], b: &[u64]) -> Option<(u64, u64)> {
    let mut a = a.to_vec();
    a.sort_unstable();
    let d: Vec<u64> = b
        .iter()
        .filter_map(|t| {
            let i = a.partition_point(|x| x < t);
            [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|j| a.get(j)).map(|x| x.abs_diff(*t)).min()
        })
        .collect();
    Some((median(&d)?, *d.iter().max()?))
}

/// The format a run reads: the largest YUV of at least 24 fps (or of no stated rate), else the
/// largest YUV.
fn pick_format(desc: &VideoInputDesc) -> Option<VideoFormat> {
    let yuv = desc.formats.iter().filter(|f| f.pixel_format == VideoPixelFormat::YUV420);
    let fast = yuv.clone().filter(|f| f.frame_rate.is_none_or(|r| r >= 24.0)).max_by_key(|f| f.width * f.height);
    fast.or_else(|| yuv.max_by_key(|f| f.width * f.height)).cloned()
}

/// A run: the cameras (desc index each) on slots 0, 1.
struct Run {
    cameras: Vec<usize>,
    formats: Vec<VideoFormat>,
    started: f64,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    intake: Arc<Mutex<[Intake; 2]>>,
    #[rust]
    descs: Vec<VideoInputDesc>,
    #[rust]
    plan: Vec<Vec<usize>>,
    #[rust]
    run: Option<Run>,
    #[rust]
    planned: bool,
    #[rust]
    granted: bool,
    #[rust]
    failed: usize,
    #[rust]
    timer: Timer,
}

impl App {
    fn status(&mut self, cx: &mut Cx, text: &str) {
        log!("camera-probe: {text}");
        self.ui.label(cx, ids!(status)).set_text(cx, text);
    }

    /// The runs, once the cameras are listed and allowed: each camera alone, then a headset's
    /// passthrough pair.
    fn plan(&mut self, cx: &mut Cx) {
        if self.planned || !self.granted || self.descs.is_empty() {
            return;
        }
        self.planned = true;
        let mut pair = Vec::new();
        for (i, d) in self.descs.iter().enumerate() {
            let c = cx.video_input_calibration(d.input_id).unwrap_or_default();
            let format = pick_format(d).map_or("no YUV format".into(), |f| format!("{}x{} at {:?} fps", f.width, f.height, f.frame_rate));
            log!("camera-probe: camera {i} {:?}: orientation {}, {} formats, runs {format}; calibration {c:?}", d.name, d.sensor_orientation, d.formats.len());
            if c.passthrough {
                pair.push((c.position.unwrap_or(u8::MAX), i));
            }
            self.plan.push(vec![i]);
        }
        pair.sort();
        if pair.len() >= 2 {
            self.plan.push(pair.iter().take(2).map(|(_, i)| *i).collect());
        }
        self.plan.reverse();
        self.next_run(cx);
    }

    fn next_run(&mut self, cx: &mut Cx) {
        while let Some(cameras) = self.plan.pop() {
            *self.intake.lock().unwrap() = Default::default();
            let mut formats = Vec::new();
            for (slot, &i) in cameras.iter().enumerate() {
                let opened = pick_format(&self.descs[i])
                    .ok_or_else(|| "no YUV format".to_string())
                    .and_then(|f| cx.video_input_open(slot, self.descs[i].input_id, f.format_id))
                    .and_then(|f| cx.video_input_stream(slot, true).map(|_| f));
                match opened {
                    Ok(f) => formats.push(f),
                    Err(e) => {
                        log!("camera-probe: FAIL camera {i}: {e}");
                        self.failed += 1;
                        break;
                    }
                }
            }
            if formats.len() == cameras.len() {
                self.status(cx, &format!("running camera {cameras:?} for {RUN_S} s"));
                self.run = Some(Run { cameras, formats, started: cx.seconds_since_app_start() });
                return;
            }
            self.close(cx, cameras.len());
        }
        let text = format!("done: {} runs failed", self.failed);
        self.status(cx, &text);
    }

    fn close(&mut self, cx: &mut Cx, slots: usize) {
        for slot in 0..slots {
            let _ = cx.video_input_stream(slot, false);
            cx.video_input_close(slot);
        }
    }

    fn tick(&mut self, cx: &mut Cx) {
        let Some(run) = &self.run else { return };
        if cx.seconds_since_app_start() - run.started < RUN_S {
            return;
        }
        let Some(run) = self.run.take() else { return };
        let intake = std::mem::take(&mut *self.intake.lock().unwrap());
        let mut faults = Vec::new();
        for (slot, &i) in run.cameras.iter().enumerate() {
            let (line, f) = intake[slot].verdict(run.formats[slot].frame_rate);
            log!("camera-probe: camera {i} at {}x{}: {line}", run.formats[slot].width, run.formats[slot].height);
            faults.extend(f.into_iter().map(|f| format!("camera {i}: {f}")));
        }
        if run.cameras.len() == 2 {
            match apart_ns(&intake[0].mids_ns, &intake[1].mids_ns) {
                Some((p50, max)) => {
                    log!("camera-probe: pair {:?}: exposures apart p50 {:.2} ms max {:.2} ms", run.cameras, p50 as f64 / 1e6, max as f64 / 1e6);
                    if p50 > PAIR_NS {
                        faults.push(format!("the pair's exposures {:.1} ms apart (the capture pairs within {} ms)", p50 as f64 / 1e6, PAIR_NS / 1_000_000));
                    }
                }
                None => faults.push("the pair has no exposure times to match".into()),
            }
        }
        if faults.is_empty() {
            log!("camera-probe: PASS cameras {:?}", run.cameras);
        } else {
            self.failed += 1;
            log!("camera-probe: FAIL cameras {:?}: {}", run.cameras, faults.join("; "));
        }
        self.close(cx, run.cameras.len());
        self.next_run(cx);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            Event::Startup => {
                for slot in 0..2 {
                    let intake = self.intake.clone();
                    cx.camera_frame_input(slot, move |frame| intake.lock().unwrap()[slot].add(frame.timestamp_ns, frame.exposure, clock_ns()));
                }
                cx.request_permission(Permission::Camera);
                if cfg!(target_os = "android") {
                    cx.request_permission(Permission::HeadsetCamera);
                }
                self.timer = cx.start_interval(0.5);
            }
            Event::PermissionResult(r) => {
                log!("camera-probe: permission {:?}: {:?}", r.permission, r.status);
                if r.permission == Permission::Camera && r.status == PermissionStatus::Granted {
                    self.granted = true;
                    if let Some(descs) = cx.video_inputs() {
                        self.descs = descs;
                    }
                    self.plan(cx);
                }
            }
            Event::VideoInputs(ev) => {
                if !self.planned {
                    self.descs = ev.descs.clone();
                    self.plan(cx);
                }
            }
            _ => {}
        }
        if self.timer.is_event(event).is_some() {
            self.tick(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exposure(start_ns: u64) -> CameraExposure {
        CameraExposure { start_ns, exposure_ns: 4_000_000, rolling_skew_ns: 10_000_000, frame_duration_ns: 33_333_333, sensitivity: 400 }
    }

    #[test]
    fn camera_probe_passes_a_steady_camera_and_flags_a_stall() {
        let mut steady = Intake::default();
        for i in 0..300u64 {
            let start = 1_000_000_000 + i * 33_333_333;
            steady.add(start + 5_000_000, exposure(start), start + 40_000_000);
        }
        let (line, faults) = steady.verdict(Some(30.0));
        assert!(faults.is_empty(), "{line}: {faults:?}");
        // a camera that stops for a second, then goes on
        let mut stalled = Intake::default();
        for i in (0..300u64).filter(|i| !(100..130).contains(i)) {
            let start = 1_000_000_000 + i * 33_333_333;
            stalled.add(start + 5_000_000, exposure(start), start + 40_000_000);
        }
        let (_, faults) = stalled.verdict(Some(30.0));
        assert!(faults.iter().any(|f| f.contains("stall")), "{faults:?}");
        // frames without results nor HAL times still pass on the format's rate
        let mut bare = Intake::default();
        for i in 0..300u64 {
            bare.add(i * 33_333_333, CameraExposure::default(), 1_000_000_000 + i * 33_333_333);
        }
        let (line, faults) = bare.verdict(Some(30.0));
        assert!(faults.is_empty() && line.contains("0% with exposure"), "{line}: {faults:?}");
        // exposure times on another clock are flagged
        let mut off = Intake::default();
        for i in 0..300u64 {
            let start = 9_000_000_000 + i * 33_333_333;
            off.add(start, exposure(start), 1_000_000_000 + i * 33_333_333);
        }
        let (_, faults) = off.verdict(Some(30.0));
        assert!(faults.iter().any(|f| f.contains("HAL clock")), "{faults:?}");
    }

    #[test]
    fn camera_probe_pairs_the_nearest_exposures() {
        let a: Vec<u64> = (0..30).map(|i| i * 33_000_000).collect();
        let b: Vec<u64> = a.iter().map(|t| t + 2_000_000).collect();
        assert_eq!(apart_ns(&a, &b), Some((2_000_000, 2_000_000)));
        assert_eq!(apart_ns(&a, &[]), None);
    }
}
