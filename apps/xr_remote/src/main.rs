//! XR Remote: the headset end of remote rendering. It opens the OpenXR session (the HAL's), links
//! to a desktop app's XR server (`XR_REMOTE_TARGET` at build time, default 166's Sandbox) and shows
//! its stereo stream in the XR view: each eye its half of the picture, drawn with the eyes the
//! desktop rendered it with. Its poses, controllers and hands go upstream every display frame.
//! The panel (the window, a quad in the headset) shows the link and its stats; over the stereo
//! view it is hidden until the left thumbstick is clicked. No app state of the remote app lives
//! here.

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_xr_stream::{DrawXrBandDepth, DrawXrStream, XrStreamClient};

app_main!(App);

/// The XR server linked to (host:port), fixed at build time until discovery picks one.
const TARGET: &str = match option_env!("XR_REMOTE_TARGET") {
    Some(t) => t,
    None => "10.0.0.166:8794",
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrRemoteViewBase = #(XrRemoteView::register_widget(vm))
    mod.widgets.XrRemoteView = set_type_default() do mod.widgets.XrRemoteViewBase{
        width: Fill
        height: Fill
        draw_bg +: { color: #x14171c }
        draw_text +: {
            color: #xc8d2dc
            text_style: theme.font_regular{font_size: 14}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 300)
                body +: {
                    remote := mod.widgets.XrRemoteView{}
                }
            }
        }
    }
}

/// The XR view (the stream) and the panel's status line.
#[derive(Script, ScriptHook, Widget)]
pub struct XrRemoteView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_stream: DrawXrStream,
    #[live]
    draw_band_depth: DrawXrBandDepth,
    #[new]
    pass: DrawPass,
    /// The XR pass's own draw list (draws inside a pass need one).
    #[new]
    draw_list: DrawList2d,
    #[rust]
    color: Option<Texture>,
    #[rust]
    presenting: bool,
    #[rust]
    client: Option<XrStreamClient>,
    /// When the debug switches were last read.
    #[rust]
    switches_at: Option<std::time::Instant>,
    #[rust]
    clear_tint: bool,
    /// The XrUpdate of the frame being drawn arrived then; the UI thread's summed times (µs:
    /// update to drawn, draw) and their count.
    #[rust]
    update_at: Option<std::time::Instant>,
    #[rust]
    ui_times: (u64, u64, u32),
    /// A GPU profile runs since then (`debug.xrremote.gpuprof`).
    #[rust]
    gpu_prof_at: Option<std::time::Instant>,
    /// The warp probe last asked for (`debug.xrremote.warpprobe`).
    #[rust]
    probe_asked: u8,
    /// The trace length last asked for (`debug.xrremote.trace`).
    #[rust]
    trace_asked: u8,
    #[rust]
    replay_asked: String,
    /// The refresh rate asked for (Hz; 0 before the first ask).
    #[rust]
    refresh_hz: u8,
    /// An XR frame was asked and not drawn yet; the window's dpi factor (the XR pass draws
    /// with it).
    #[rust]
    xr_due: bool,
    /// The foveation level last asked for (`debug.xrremote.fov`; 255 before the first ask).
    #[rust(255)]
    fov_asked: u8,
    /// The eye buffers' lowest dynamic size last asked for (`debug.xrremote.eyemin`; 0 before
    /// the first ask).
    #[rust]
    eyemin_asked: u8,
    /// Hands and controllers at once last asked (`debug.xrremote.multimodal`; 255 before the
    /// first ask).
    #[rust(255)]
    multimodal_asked: u8,
    /// The environment depth last asked (`debug.xrremote.envdepth`; 255 before the first ask).
    #[rust(255)]
    envdepth_asked: u8,
    #[rust(1.0)]
    dpi: f64,
}

impl XrRemoteView {
    /// The XR pass, drawn on its own (every XR frame is answered, cleared until the first
    /// picture: the headset keeps asking at its display rate and the poses flow at that rate).
    fn draw_xr(&mut self, cx: &mut Cx, event: &DrawEvent) {
        // one XR frame per XR frame asked (any other draw event draws none: an extra frame
        // waits in the HAL's slot and is taken for the next ask)
        if !self.presenting || !std::mem::take(&mut self.xr_due) {
            return;
        }
        let mut draw = CxDraw::new(cx, event);
        let cx = &mut Cx2d::new(&mut draw);
        cx.begin_pass(&self.pass, Some(self.dpi));
        // (the warp's band-depth pass, a child of this one: before its draw list)
        if let Some(client) = self.client.as_mut().filter(|c| c.opaque != 3) {
            client.draw_band(cx, &mut self.draw_band_depth);
        }
        self.draw_list.begin_always(cx);
        cx.begin_unclipped_root_turtle(dvec2(4096.0, 4096.0), Layout::flow_down());
        // (debug `opaque` 3: nothing drawn, the pass only clears: its fixed cost)
        if let Some(client) = self.client.as_mut().filter(|c| c.opaque != 3) {
            client.draw(cx, &mut self.draw_stream);
        }
        cx.end_pass_sized_turtle_no_clip();
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);
    }

    /// Once: the XR pass and the link.
    fn start(&mut self, cx: &mut Cx) {
        if self.presenting || !cx.xr_display() {
            return;
        }
        self.presenting = true;
        let color = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        self.pass.set_pass_name(cx, "xr remote");
        self.pass.set_color_texture(cx, &color, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        // (no depth attachment: the stream's draw tests nothing against it, and clearing and
        // storing a 32-bit depth per eye was half the pass's memory traffic)
        self.pass.set_keep_camera_matrix(cx, true);
        self.pass.set_as_xr_pass(cx);
        cx.xr_start_presenting();
        // mixed reality: the room shows wherever the desktop drew nothing (the stream's coverage)
        cx.xr_set_see_through(option_env!("XR_REMOTE_OPAQUE").is_none());
        self.color = Some(color);
        // the device's `debug.xrremote.target` (adb shell setprop) overrides the built-in target;
        // `none`: no link (the headset's own frame loop alone)
        let target = device_property("debug.xrremote.target").filter(|t| !t.is_empty()).unwrap_or_else(|| TARGET.to_string());
        if target == "none" {
            log!("xr remote: no link (debug.xrremote.target = none)");
            return;
        }
        let (host, port) = match target.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(makepad_xr_stream::DEFAULT_PORT)),
            None => (target.clone(), makepad_xr_stream::DEFAULT_PORT),
        };
        // this headset's fleet identity: its own key, the credential the admin signed for it
        // (`fleet-id sign <name> paired <point>`, pushed to the app's external files folder)
        #[cfg(target_os = "android")]
        {
            use makepad_widgets::makepad_platform::makepad_network::{fleet::id, tls::to_hex};
            // the app's internal files (never the shared external folder, never the cache)
            let files = cx.get_data_dir().unwrap_or_else(|| "/data/user/0/dev.makepad.xrremote/files".to_string());
            std::env::set_var("MAKEPAD_FLEET_DIR", std::path::Path::new(&files).join("fleet"));
            let pushed = std::path::PathBuf::from("/sdcard/Android/data/dev.makepad.xrremote/files/fleet");
            // made by the app, so what adb pushes into it stays readable to the app
            let _ = std::fs::create_dir_all(&pushed);
            match id::enrol_device(Some(&pushed)) {
                Ok((point, installed)) => log!("xr remote: fleet point {} (installed {installed:?})", to_hex(&point)),
                Err(e) => log!("xr remote: no fleet identity: {e}"),
            }
        }
        // the rate the display is asked to run at (`debug.xrremote.hz`, default 90): the server
        // encodes for it (its pictures are paced by the poses, one per display frame)
        let refresh = device_property("debug.xrremote.hz").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(90.0);
        log!("xr remote: linking to {host}:{port} at {refresh} Hz");
        // `streamscale` N (percent, read at the link; default 120, the user's pick: 100 looked
        // half-res blurry, 140 is past what 166 encodes at 90/s): the eye picture the desktop
        // renders and encodes, N% of the headset's 1680x1760
        let percent = device_property("debug.xrremote.streamscale").and_then(|v| v.trim().parse::<u32>().ok()).filter(|p| (50..=160).contains(p)).unwrap_or(120);
        // (even sizes, 16-aligned widths: the encoder's blocks)
        let eye = [((1680 * percent / 100 + 15) / 16) * 16, ((1760 * percent / 100 + 7) / 8) * 8];
        log!("xr remote: stream eye {}x{} ({percent}%)", eye[0], eye[1]);
        let client = XrStreamClient::connect(host, port, eye, refresh);
        // the desktop app's game audio, played here (AAudio, low latency) in step with the
        // pictures: the player's buffer, concealment and drift correction run in the callback
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut player = client.audio_player();
            cx.audio_output(0, move |info, out| {
                let (frames, channels) = (out.frame_count(), out.channel_count());
                player.render_planar(info.sample_rate, frames, channels, &mut out.data);
            });
        }
        self.client = Some(client);
    }
}

impl Widget for XrRemoteView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(client) = self.client.as_mut() {
            client.handle_event(cx, event);
        }
        if self.presenting {
            // the debug switches (`adb shell setprop debug.xrremote.<name> 0|1`), read once a
            // second: `occlusion` (default 1) the room's depth hides the picture (2: the room's
            // depth shown instead of the picture as 10 cm contour bands), `opaque`
            // (default 0) 1 the picture covers the room whatever its coverage, 2 a flat magenta
            // instead (the draw reaches the view), `clear` (default 0) 1 a green tint under it
            // (the XR pass reaches the view), `owneyes` (default 0) 1 the frame shows with the
            // headset's own eyes instead of the picture's
            if self.switches_at.map_or(true, |t| t.elapsed().as_secs() >= 1) {
                self.switches_at = Some(std::time::Instant::now());
                let switch = |name: &str, default: u8| device_property(name).and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(default);
                let occlusion_switch = switch("debug.xrremote.occlusion", 1);
                let (occlusion, opaque, clear) = (occlusion_switch != 0, switch("debug.xrremote.opaque", 0), switch("debug.xrremote.clear", 0) != 0);
                let was = self.client.as_ref().map_or((true, 0), |c| (c.occlusion, c.opaque));
                if (occlusion, opaque, clear) != (was.0, was.1, self.clear_tint) {
                    log!("xr remote: occlusion {occlusion}, opaque {opaque}, clear tint {clear}");
                    if clear != self.clear_tint {
                        self.clear_tint = clear;
                        let c = if clear { vec4(0.0, 0.25, 0.0, 0.5) } else { vec4(0.0, 0.0, 0.0, 0.0) };
                        self.pass.set_window_clear_color(cx, c);
                    }
                }
                // `hz` (default 90): the display's refresh rate (the supported one nearest)
                let hz = switch("debug.xrremote.hz", 90);
                if hz != self.refresh_hz {
                    self.refresh_hz = hz;
                    log!("xr remote: refresh rate {hz} Hz asked");
                    cx.xr_set_refresh_rate(hz as f32);
                }
                // `gpuprof` 1: the GPU's time per pass over the next 4 s, logged (then set it to 0)
                let prof = switch("debug.xrremote.gpuprof", 0) != 0;
                if prof && self.gpu_prof_at.is_none() {
                    use makepad_widgets::makepad_platform::makepad_hal::gpu_prof;
                    gpu_prof::start(gpu_prof::StartOptions { split_every: 0, ..Default::default() });
                    self.gpu_prof_at = Some(std::time::Instant::now());
                    log!("xr remote: gpu profile started");
                }
                if self.gpu_prof_at.is_some_and(|t| t.elapsed().as_secs() >= 4) {
                    gpu_prof_report();
                }
                if !prof {
                    self.gpu_prof_at = None;
                }
                // `trace` N: a per-display-frame trace of the next N seconds into the app's
                // external files folder (adb pull; set it back to 0 for the next one)
                let trace = switch("debug.xrremote.trace", 0);
                if trace != self.trace_asked {
                    self.trace_asked = trace;
                    if let (true, Some(client)) = (trace > 0, self.client.as_mut()) {
                        let path = format!("/sdcard/Android/data/dev.makepad.xrremote/files/trace-{}.csv", makepad_widgets::makepad_platform::makepad_hal::clock_ns() / 1_000_000_000);
                        log!("xr remote: tracing {trace} s into {path}");
                        client.start_trace(trace as f64, path);
                    }
                }
                // `replay` NAME: the tracking log NAME (a trace's `track-<n>`) plays instead of
                // the headset's own poses (the whole stream runs that motion with the headset on
                // the desk); 0 stops
                let replay = device_property("debug.xrremote.replay").map(|v| v.trim().to_string()).unwrap_or_default();
                if replay != self.replay_asked {
                    self.replay_asked = replay.clone();
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = makepad_xr_stream::replay_track_log("/sdcard/Android/data/dev.makepad.xrremote/files", &replay);
                }
                // `fov` N (0 none .. 3 high, live; default 2, the user's pick: the edges look the
                // same, the pass ~20% cheaper): the headset's fixed foveation of the view
                let fov = switch("debug.xrremote.fov", 2);
                if fov != self.fov_asked {
                    self.fov_asked = fov;
                    log!("xr remote: foveation level {fov}");
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_foveation(fov as u32);
                }
                // `eyemin` N (default 70; 100: fixed): the eye buffers step down to N% of their
                // start size when the view's GPU work nears the display period (any picture
                // keeps 90/s; back up with headroom)
                let eyemin = switch("debug.xrremote.eyemin", 70);
                if eyemin != self.eyemin_asked {
                    self.eyemin_asked = eyemin;
                    log!("xr remote: eye buffers {eyemin}..100%");
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_eye_scale(eyemin as u32, 100);
                }
                // `multimodal` (default 1, the user's pick): hands and controllers tracked at once
                // (the cameras shared: the hand tracker at 25 Hz instead of its high-frequency
                // rate; the uplink's hands are interpolated between its samples)
                let multimodal = switch("debug.xrremote.multimodal", 1);
                if multimodal != self.multimodal_asked {
                    self.multimodal_asked = multimodal;
                    log!("xr remote: hands and controllers at once {}", multimodal != 0);
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_hands_and_controllers(multimodal != 0);
                }
                // `envdepth` (default 1): the headset's environment depth runs (0 stops it: no
                // occlusion, no room field; its GPU cost A/B)
                let envdepth = switch("debug.xrremote.envdepth", 1);
                if envdepth != self.envdepth_asked {
                    self.envdepth_asked = envdepth;
                    log!("xr remote: environment depth {}", envdepth != 0);
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_environment_depth(envdepth != 0);
                }
                if let Some(client) = self.client.as_mut() {
                    client.occlusion = occlusion;
                    // `roomfield` 0 / 1: the room field off or on whatever the server says (its
                    // CPU A/B); unset or 2: as the server's app asks
                    client.room_field_override = match switch("debug.xrremote.roomfield", 2) { 0 => Some(false), 1 => Some(true), _ => None };
                    // `ahead` N (default 4): the poses at least N display frames past the
                    // headset's newest (more when the measured sample to decoded needs it; 0:
                    // that alone)
                    client.pin_ahead = switch("debug.xrremote.ahead", 4) as u32;
                    client.opaque = opaque;
                    let own = switch("debug.xrremote.owneyes", 0) != 0;
                    if own != client.own_eyes { log!("xr remote: own eyes {own}"); }
                    client.own_eyes = own;
                    client.room_view = occlusion_switch == 2;
                    client.env_map = switch("debug.xrremote.envmap", 0);
                    // `depthlayer` (default 1): the frame carries the picture's depth to the
                    // compositor (positional correction)
                    client.depth_layer = switch("debug.xrremote.depthlayer", 1) != 0;
                    // `freeze` 1: the last picture stays, shown with its own eyes
                    client.freeze = switch("debug.xrremote.freeze", 0) != 0;
                    // `warp` (default 0 until it is verified on the headset): the picture
                    // reprojected by its depth to the display eyes
                    client.warp = switch("debug.xrremote.warp", 0) != 0;
                    // `warpprobe` N: a new N runs the warp's desk check once
                    let probe = switch("debug.xrremote.warpprobe", 0);
                    if probe != self.probe_asked {
                        self.probe_asked = probe;
                        if probe > 0 {
                            #[cfg(target_os = "android")]
                            client.warp_probe(cx);
                        }
                    }
                    // `focuscm` N: the depth layer at N cm instead of the picture's depth
                    client.focus_override_m = switch("debug.xrremote.focuscm", 0) as f32 / 100.0;
                }
            }
        }
        if matches!(event, Event::XrUpdate(_)) && self.update_at.is_none() {
            self.update_at = Some(std::time::Instant::now());
        }
        // every XR frame is answered (with or without a link): the XR pass alone is drawn
        // (`draw_xr`); the panel (hidden over the stereo view) is redrawn once a second for its
        // status (redrawn per XR frame it cost a 1920x1200 pass and its scanout copy, 1.8 ms
        // of GPU every frame)
        // (only an XR frame draws the XR pass: drawn on a decoded picture's signal too, the
        // extra frame waited in the HAL's slot and was taken for the NEXT ask, shown with that
        // ask's eyes: a frame of head rotation off, and twice the GPU work)
        // (the panel is not redrawn while the stereo view shows: drawn once a second, its
        // draw event drew an XR frame too, outside the asks: a far one-frame jump each second)
        if self.presenting && matches!(event, Event::XrUpdate(_)) {
            self.xr_due = true;
            self.draw_list.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let draw_started = std::time::Instant::now();
        self.start(cx.cx);
        let rect = cx.walk_turtle(walk);
        self.dpi = cx.current_dpi_factor();
        self.draw_bg.draw_abs(cx, rect);
        let status = match &self.client {
            Some(c) => c.status(),
            None => "no headset: XR Remote shows a remote XR view on a headset".to_string(),
        };
        self.draw_text.draw_abs(cx, rect.pos + dvec2(24.0, 24.0), &status);
        // the UI thread's part of a frame: its XrUpdate to the end of its draw, and the draw
        if let Some(at) = self.update_at.take() {
            let t = &mut self.ui_times;
            t.0 += at.elapsed().as_micros() as u64;
            t.1 += draw_started.elapsed().as_micros() as u64;
            t.2 += 1;
            if t.2 >= 450 {
                log!("xr remote: ui thread per frame ms: update to drawn {:.2}, draw {:.2}", t.0 as f64 / 1000.0 / t.2 as f64, t.1 as f64 / 1000.0 / t.2 as f64);
                *t = (0, 0, 0);
            }
        }
        DrawStep::done()
    }
}

/// Stops the GPU profile and logs each pass's GPU time (mean, max) with its size and
/// attachments, and the frames' GPU spans.
fn gpu_prof_report() {
    use makepad_widgets::makepad_platform::makepad_hal::gpu_prof;
    if gpu_prof::stop().is_none() {
        return;
    }
    let Some(rec) = gpu_prof::recording_of(None) else { return };
    let mut by: std::collections::BTreeMap<String, (u64, u64, u32, u32, String, u64, u32)> = Default::default();
    let mut spans = Vec::new();
    for f in rec.frames.iter().filter(|f| !f.special()) {
        let timed: Vec<_> = f.passes.iter().filter(|p| p.gpu_ok()).collect();
        if let (Some(a), Some(b)) = (timed.iter().map(|p| p.start).min(), timed.iter().map(|p| p.end).max()) {
            spans.push(b - a);
        }
        for p in timed {
            let e = by.entry(rec.s(p.name).to_string()).or_insert((0, 0, p.w, p.h, rec.s(p.attach).to_string(), p.bytes, 0));
            let d = p.end - p.start;
            e.0 += d;
            e.1 = e.1.max(d);
            e.6 += 1;
        }
    }
    spans.sort_unstable();
    let n = spans.len().max(1);
    log!("xr remote: gpu profile: {} frames, frame GPU span ms p50 {:.2} p90 {:.2}", spans.len(), spans.get(n / 2).copied().unwrap_or(0) as f64 / 1e6, spans.get(n * 9 / 10).copied().unwrap_or(0) as f64 / 1e6);
    for (name, (sum, max, w, h, attach, bytes, count)) in by {
        log!("xr remote: gpu pass {name}: {count}x mean {:.3} max {:.3} ms, {w}x{h}, {} KB, {attach}", sum as f64 / count.max(1) as f64 / 1e6, max as f64 / 1e6, bytes / 1024);
    }
}

/// An Android system property (`None` elsewhere or when unset).
fn device_property(name: &str) -> Option<String> {
    #[cfg(target_os = "android")]
    {
        extern "C" {
            fn __system_property_get(name: *const std::ffi::c_char, value: *mut std::ffi::c_char) -> i32;
        }
        let name = std::ffi::CString::new(name).ok()?;
        let mut value = [0 as std::ffi::c_char; 92];
        let n = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
        if n <= 0 {
            return None;
        }
        let bytes: Vec<u8> = value[..n as usize].iter().map(|c| *c as u8).collect();
        return String::from_utf8(bytes).ok();
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = name;
        None
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr_stream::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        // the stream's game audio plays on the headset's default output
        if let Event::AudioDevices(devices) = event {
            cx.use_audio_outputs(&devices.default_output());
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // the XR pass, apart from the window's draw
        if let Event::Draw(e) = event {
            if let Some(mut view) = self.ui.widget(cx, ids!(remote)).borrow_mut::<XrRemoteView>() {
                view.draw_xr(cx, e);
            }
        }
    }
}
