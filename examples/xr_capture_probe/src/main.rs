//! A desktop XR server that asks the linked headset for its camera capture and checks what
//! arrives, as `xr-capture-probe:` lines: each camera's calibration (intrinsics inside the array,
//! distortion, the lens pose), then every 5 s the keyframes so far: their rate and bytes, the two
//! views' exposures apart, keyframe times in order, head poses (unit rotations, the asked step
//! between keyframes), exposure times, environment depth (share and age) and every plane decoded
//! to its size. Once 20 keyframes came, `xr-capture-probe: PASS ...` or `... FAIL ...` with its
//! faults. The pictures are decoded and dropped: none is kept or logged.
//!
//! Run with the local network allowed and the stream on (`MAKEPAD_LAN=1`, `--xr-stream=on`);
//! `--pin NNNN` pairs the headset showing that PIN.
use makepad_desk_link::xr_capture::{decode_plane, CaptureAsk, CaptureCamera, CaptureKeyframe, CaptureRecord, CapturePlane};
use makepad_widgets::*;
use makepad_xr_stream::XrServer;
use std::sync::{Arc, Mutex};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(720, 240)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        padding: 24
                        status := Label{ text: "xr capture probe: starting" }
                    }
                }
            }
        }
    }
}

/// What the probe asks: a keyframe each 5 degrees or 2 cm.
const ASK: CaptureAsk = CaptureAsk { step_deg: 5.0, step_m: 0.02, preview_hz: 0 };
/// The capture's pairing window (ns).
const PAIR_NS: u64 = 8_000_000;
/// Keyframes before the verdict.
const VERDICT_AFTER: u64 = 20;

fn norm<const N: usize>(v: [f32; N]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn angle_deg(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
    2.0 * dot.acos().to_degrees()
}

/// `a - b` in ns, signed, saturating (the times are a peer's: any u64).
fn signed_ns(a: u64, b: u64) -> i64 {
    if a >= b { i64::try_from(a - b).unwrap_or(i64::MAX) } else { i64::try_from(b - a).map_or(i64::MIN, |d| -d) }
}

fn median(v: &[i64]) -> Option<i64> {
    let mut v = v.to_vec();
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

/// What arrived so far, and its faults (each kind counted once per keyframe).
#[derive(Default)]
struct Check {
    cameras: Vec<CaptureCamera>,
    keyframes: u64,
    first_ns: Option<u64>,
    last: Option<(u32, u64, [f32; 4], [f32; 3])>,
    bytes: u64,
    raw_bytes: u64,
    apart_ns: Vec<i64>,
    with_depth: u64,
    depth_age_ns: Vec<i64>,
    previews: u64,
    faults: Vec<String>,
    status: Option<(u8, String)>,
    verdict: bool,
}

impl Check {
    fn fault(&mut self, f: String) {
        if self.faults.len() < 32 {
            self.faults.push(f);
        }
    }

    fn cameras(&mut self, cams: Vec<CaptureCamera>) {
        let mut positions: Vec<u8> = cams.iter().map(|c| c.position).collect();
        positions.sort_unstable();
        positions.dedup();
        if cams.len() != 2 || positions.len() != 2 {
            self.fault(format!("{} cameras at positions {positions:?}", cams.len()));
        }
        for c in &cams {
            let p = c.position;
            match c.intrinsics {
                Some([fx, fy, cx, cy, _]) if fx > 0.0 && fy > 0.0 && cx > 0.0 && cy > 0.0 && cx < c.array[2] as f32 && cy < c.array[3] as f32 => {}
                k => self.fault(format!("camera {p}: intrinsics {k:?} outside its array {:?}", c.array)),
            }
            if c.distortion.is_none() {
                self.fault(format!("camera {p}: no distortion"));
            }
            match &c.lens_pose {
                Some(l) if (norm(l.rotation) - 1.0).abs() < 0.01 && norm(l.translation) < 0.3 => {}
                l => self.fault(format!("camera {p}: lens pose {l:?}")),
            }
            if c.size[0] == 0 || c.size[1] == 0 {
                self.fault(format!("camera {p}: size {:?}", c.size));
            }
        }
        self.cameras = cams;
    }

    fn plane(&mut self, what: &str, p: &CapturePlane, size: [u32; 2], bps: u8) {
        self.bytes += p.data.len() as u64;
        self.raw_bytes += p.width as u64 * p.height as u64 * p.bytes_per_sample as u64;
        if [p.width, p.height] != size || p.bytes_per_sample != bps {
            self.fault(format!("{what}: {}x{} x{} of {size:?} x{bps}", p.width, p.height, p.bytes_per_sample));
        }
        // (decoded to its size, then dropped)
        if let Err(e) = decode_plane(p) {
            self.fault(format!("{what}: {e}"));
        }
    }

    fn keyframe(&mut self, k: CaptureKeyframe) {
        self.keyframes += 1;
        let i = k.index;
        if k.views.len() != self.cameras.len() || k.views.is_empty() {
            self.fault(format!("keyframe {i}: {} views of {} cameras", k.views.len(), self.cameras.len()));
            return;
        }
        let times: Vec<u64> = k.views.iter().map(|v| v.time_ns).collect();
        let (lo, hi) = (*times.iter().min().unwrap_or(&0), *times.iter().max().unwrap_or(&0));
        self.apart_ns.push(signed_ns(hi, lo));
        if hi - lo > PAIR_NS {
            self.fault(format!("keyframe {i}: views {:.1} ms apart", (hi - lo) as f64 / 1e6));
        }
        if self.first_ns.is_none() {
            self.first_ns = Some(lo);
        }
        let head = k.views[0].head;
        if let Some((li, lt, lq, lp)) = self.last {
            if li.checked_add(1) != Some(i) || lo <= lt {
                self.fault(format!("keyframe {i} after {li}: {:.1} ms on", (lo as f64 - lt as f64) / 1e6));
            }
            let d = [head.position[0] - lp[0], head.position[1] - lp[1], head.position[2] - lp[2]];
            if angle_deg(head.orientation, lq) < 0.9 * ASK.step_deg && norm(d) < 0.9 * ASK.step_m {
                self.fault(format!("keyframe {i}: {:.1} deg, {:.1} mm from the last (asked {} deg or {} mm)", angle_deg(head.orientation, lq), norm(d) * 1e3, ASK.step_deg, ASK.step_m * 1e3));
            }
        }
        self.last = Some((i, lo, head.orientation, head.position));
        for v in &k.views {
            let c = v.camera;
            if (norm(v.head.orientation) - 1.0).abs() > 0.01 {
                self.fault(format!("keyframe {i} camera {c}: head rotation {:?}", v.head.orientation));
            }
            if v.exposure_ns == 0 {
                self.fault(format!("keyframe {i} camera {c}: no exposure time"));
            }
            let Some(size) = self.cameras.iter().find(|cam| cam.position == c).map(|cam| cam.size) else {
                self.fault(format!("keyframe {i}: camera {c} not described"));
                continue;
            };
            if v.planes.len() != 3 {
                self.fault(format!("keyframe {i} camera {c}: {} planes", v.planes.len()));
                continue;
            }
            let half = [size[0].div_ceil(2), size[1].div_ceil(2)];
            for (n, p) in v.planes.iter().enumerate() {
                self.plane(&format!("keyframe {i} camera {c} plane {n}"), p, if n == 0 { size } else { half }, 1);
            }
        }
        if let Some(d) = &k.depth {
            self.with_depth += 1;
            self.depth_age_ns.push(signed_ns(lo, d.time_ns));
            let t = &d.texels;
            if d.layers == 0 || t.height % d.layers != 0 || !(d.near > 0.0 && d.far > d.near) {
                self.fault(format!("keyframe {i}: depth of {} layers in {}x{}, range {}..{}", d.layers, t.width, t.height, d.near, d.far));
            }
            self.plane(&format!("keyframe {i} depth"), t, [t.width, t.height], 2);
        }
    }

    fn add(&mut self, record: CaptureRecord) {
        match record {
            CaptureRecord::Cameras(cams) => {
                for c in &cams {
                    log!("xr-capture-probe: camera {} at {:?}: intrinsics {:?} array {:?} active {:?} distortion {:?} lens {:?}", c.position, c.size, c.intrinsics, c.array, c.active, c.distortion, c.lens_pose);
                }
                self.cameras(cams);
            }
            CaptureRecord::Keyframe(k) => self.keyframe(k),
            CaptureRecord::Preview { .. } => self.previews += 1,
            CaptureRecord::Status { state, text } => {
                if self.status.as_ref() != Some(&(state, text.clone())) {
                    log!("xr-capture-probe: headset: state {state}: {text}");
                }
                self.status = Some((state, text));
            }
        }
    }

    /// The summary line, and once enough keyframes came, the verdict (once).
    fn report(&mut self) -> (String, Option<String>) {
        let span_s = match (self.first_ns, self.last) {
            (Some(a), Some((_, b, _, _))) if b > a => (b - a) as f64 / 1e9,
            _ => 0.0,
        };
        let ms = |v: Option<i64>| v.map_or("-".into(), |v| format!("{:.1}", v as f64 / 1e6));
        let line = format!(
            "{} keyframes in {span_s:.0} s, {:.1} MB ({:.0}% of raw), views apart p50 {} ms max {} ms, depth with {}% (age p50 {} ms), {} previews, {} faults",
            self.keyframes,
            self.bytes as f64 / 1e6,
            100.0 * self.bytes as f64 / self.raw_bytes.max(1) as f64,
            ms(median(&self.apart_ns)),
            ms(self.apart_ns.iter().max().copied()),
            100 * self.with_depth / self.keyframes.max(1),
            ms(median(&self.depth_age_ns)),
            self.previews,
            self.faults.len(),
        );
        if self.verdict || self.keyframes < VERDICT_AFTER {
            return (line, None);
        }
        self.verdict = true;
        let verdict = if self.faults.is_empty() && self.with_depth > 0 { format!("PASS {line}") } else {
            let depth = if self.with_depth == 0 { "; no keyframe had environment depth" } else { "" };
            format!("FAIL {}{depth}", self.faults.join("; "))
        };
        (line, Some(verdict))
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    server: Option<XrServer>,
    #[rust]
    check: Arc<Mutex<Check>>,
    #[rust]
    timer: Timer,
    /// The session asked (the linked headset's), and the ticks since the last report.
    #[rust]
    asked: Option<u64>,
    #[rust]
    ticks: u32,
}

impl App {
    fn status(&mut self, cx: &mut Cx, text: &str) {
        log!("xr-capture-probe: {text}");
        self.ui.label(cx, ids!(status)).set_text(cx, text);
    }

    fn start(&mut self, cx: &mut Cx) {
        let Some(server) = XrServer::install(cx, "xr-capture-probe") else {
            self.status(cx, "no XR server here (the local network needs MAKEPAD_LAN=1)");
            return;
        };
        let args: Vec<String> = std::env::args().collect();
        if let Some(pin) = args.windows(2).find(|w| w[0] == "--pin").map(|w| w[1].clone()) {
            server.set_pin(&pin);
        }
        let check = self.check.clone();
        server.set_capture_sink(Some(Arc::new(move |_session: u64, record: CaptureRecord| check.lock().unwrap().add(record))));
        self.server = Some(server);
        self.status(cx, &format!("waiting for a headset (asking {} deg / {} mm a keyframe)", ASK.step_deg, ASK.step_m * 1e3));
        self.timer = cx.start_interval(1.0);
    }

    fn tick(&mut self, cx: &mut Cx) {
        let Some(server) = self.server.clone() else { return };
        // the capture is asked of the linked headset's own session (a new link is asked anew)
        let session = server.live_headset().map(|h| h.session);
        if session != self.asked {
            self.asked = session;
            server.set_capture(session.map(|s| (s, ASK)));
            let text = match session {
                None => "no headset linked".to_string(),
                Some(_) if server.capture_available() => "a headset linked: capture asked".into(),
                Some(_) => "a headset linked: it does not capture (an older client)".into(),
            };
            self.status(cx, &text);
        }
        self.ticks += 1;
        if self.ticks % 5 != 0 {
            return;
        }
        let (line, verdict) = self.check.lock().unwrap().report();
        self.status(cx, &line);
        if let Some(v) = verdict {
            log!("xr-capture-probe: {v}");
        }
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
        if let Event::Startup = event {
            self.start(cx);
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
    use makepad_desk_link::xr_capture::{encode_plane, CaptureDepth, CaptureLensPose, CapturePose, CaptureView};

    fn camera(position: u8) -> CaptureCamera {
        CaptureCamera {
            position,
            size: [8, 6],
            intrinsics: Some([5.0, 5.0, 4.0, 3.0, 0.0]),
            array: [0, 0, 8, 6],
            active: [0, 0, 8, 6],
            distortion: Some([0.0; 5]),
            lens_pose: Some(CaptureLensPose { rotation: [0.0, 0.0, 0.0, 1.0], translation: [0.03 * position as f32, 0.0, 0.0], reference: 2 }),
        }
    }

    fn keyframe(index: u32, time_ns: u64, apart_ns: u64, yaw_deg: f32) -> CaptureKeyframe {
        let half = (yaw_deg.to_radians() / 2.0).sin_cos();
        let head = CapturePose { orientation: [0.0, half.0, 0.0, half.1], position: [0.0; 3] };
        let view = |camera: u8, t: u64| CaptureView {
            camera,
            time_ns: t,
            head,
            velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
            exposure_ns: 4_000_000,
            rolling_skew_ns: 10_000_000,
            sensitivity: 400,
            planes: vec![encode_plane(8, 6, 1, &[7; 48]), encode_plane(4, 3, 1, &[1; 12]), encode_plane(4, 3, 1, &[2; 12])],
        };
        let depth = CaptureDepth { time_ns: time_ns + 3_000_000, near: 0.1, far: 5.0, layers: 2, texels: encode_plane(4, 4, 2, &[9; 32]), ..Default::default() };
        CaptureKeyframe { index, views: vec![view(0, time_ns), view(1, time_ns + apart_ns)], depth: Some(depth) }
    }

    #[test]
    fn capture_probe_passes_a_good_capture_and_names_faults() {
        let mut good = Check::default();
        good.add(CaptureRecord::Cameras(vec![camera(0), camera(1)]));
        for i in 0..VERDICT_AFTER as u32 {
            good.add(CaptureRecord::Keyframe(keyframe(i, 1_000_000_000 + i as u64 * 100_000_000, 1_000_000, i as f32 * 6.0)));
        }
        let (line, verdict) = good.report();
        assert!(verdict.as_deref().is_some_and(|v| v.starts_with("PASS")), "{line}: {verdict:?} {:?}", good.faults);
        assert_eq!(good.report().1, None, "the verdict comes once");

        let mut bad = Check::default();
        let mut cam = camera(1);
        cam.intrinsics = Some([5.0, 5.0, 40.0, 3.0, 0.0]);
        bad.add(CaptureRecord::Cameras(vec![camera(0), cam]));
        for i in 0..VERDICT_AFTER as u32 {
            // views 12 ms apart, and too little head turn between keyframes
            bad.add(CaptureRecord::Keyframe(keyframe(i, 1_000_000_000 + i as u64 * 100_000_000, 12_000_000, i as f32 * 1.0)));
        }
        let (_, verdict) = bad.report();
        let v = verdict.unwrap_or_default();
        assert!(v.starts_with("FAIL") && v.contains("outside its array") && v.contains("ms apart") && v.contains("from the last"), "{v}");

        // a peer's numbers at their extremes: faults, never an overflow
        let mut odd = Check::default();
        odd.add(CaptureRecord::Cameras(vec![camera(0), camera(1)]));
        odd.add(CaptureRecord::Keyframe(keyframe(u32::MAX, 1_000_000_000, 1_000_000, 0.0)));
        let mut late = keyframe(0, 2_000_000_000, 1_000_000, 30.0);
        late.depth.as_mut().unwrap().time_ns = u64::MAX;
        odd.add(CaptureRecord::Keyframe(late));
        let mut far = keyframe(1, 3_000_000_000, 1_000_000, 60.0);
        far.views[1].time_ns = u64::MAX;
        odd.add(CaptureRecord::Keyframe(far));
        assert!(odd.faults.iter().any(|f| f.contains("keyframe 0 after 4294967295")), "{:?}", odd.faults);
        assert!(odd.faults.iter().any(|f| f.contains("ms apart")), "{:?}", odd.faults);
        assert_eq!(odd.depth_age_ns[1], i64::MIN, "a depth far after its keyframe");
        assert_eq!(signed_ns(5, 7), -2);
        assert_eq!(signed_ns(u64::MAX, 0), i64::MAX);
        assert_eq!(signed_ns(0, u64::MAX), i64::MIN);
    }
}
