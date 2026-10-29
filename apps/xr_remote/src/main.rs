//! XR Remote: the headset end of remote rendering. It opens the OpenXR session (the HAL's), links
//! to a desktop app's XR server (found by its PIN: `makepad_desk_link::xr_pair`; the PIN shows
//! while nothing is linked, the user types it in the app's XR menu on the computer) and shows
//! its stereo stream in the XR view: each eye its half of the picture, drawn with the eyes the
//! desktop rendered it with. Its poses, controllers and hands go upstream every display frame.
//! The panel (the window, a quad in the headset) shows the link and its stats; over the stereo
//! view it is hidden until the left thumbstick is clicked. No app state of the remote app lives
//! here.

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_xr_stream::{DrawXrBandDepth, DrawXrStream, XrStreamClient, XrTarget};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // The PIN card: a rounded box with a soft edge glow and a hairline border; transparent
    // outside it (the panel's layer blends with its alpha: the room shows around the card).
    mod.draw.DrawPinCard = mod.std.set_type_default() do #(DrawPinCard::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let p = self.pos * self.rect_size
            let m = 28.0
            let sdf = Sdf2d.viewport(p)
            sdf.box(m - 10.0, m - 6.0, self.rect_size.x - 2.0 * m + 20.0, self.rect_size.y - 2.0 * m + 20.0, 40.0)
            sdf.fill(vec4(0.0, 0.0, 0.0, 0.25))
            let glow = sdf.result
            let card = Sdf2d.viewport(p)
            card.box(m, m, self.rect_size.x - 2.0 * m, self.rect_size.y - 2.0 * m, 32.0)
            card.fill_keep(vec4(0.075, 0.085, 0.105, 0.96))
            card.stroke(vec4(1.0, 1.0, 1.0, 0.16), 2.0)
            return card.result + glow * (1.0 - card.result.w)
        }
    }

    mod.widgets.XrRemoteViewBase = #(XrRemoteView::register_widget(vm))
    mod.widgets.XrRemoteView = set_type_default() do mod.widgets.XrRemoteViewBase{
        width: Fill
        height: Fill
        draw_bg +: { color: #x14171c }
        draw_text +: {
            color: #xc8d2dc
            text_style: theme.font_regular{font_size: 14}
        }
        draw_pin +: {
            color: #xffffff
            text_style: theme.font_bold{font_size: 160}
        }
        draw_pin_title +: {
            color: #xe8eef4
            text_style: theme.font_regular{font_size: 30}
        }
        draw_pin_hint +: {
            color: #x9aa7b4
            text_style: theme.font_regular{font_size: 18}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 300)
                body +: {
                    remote := mod.widgets.XrRemoteView{}
                }
                // (transparent where nothing is drawn: the PIN card's layer shows the room
                // around it)
                pass +: { clear_color: #x00000000 }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPinCard {
    #[deref]
    pub draw_super: DrawQuad,
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
    /// The PIN while nothing is linked (the only thing the headset shows of its own).
    #[live]
    draw_pin: DrawText,
    #[live]
    draw_pin_title: DrawText,
    #[live]
    draw_pin_hint: DrawText,
    #[live]
    draw_pin_card: DrawPinCard,
    /// The PIN the panel shows (None: linked; the panel is the stream's own).
    #[rust]
    pin_shown: Option<String>,
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
    /// The housekeeping timer (5 a second: the room field's bricks and the report go on while
    /// the stream is off and no XR frame comes), the default audio outputs, and whether the
    /// stream's sound plays now (off with the stream: the device stops).
    #[rust]
    idle_timer: Timer,
    #[rust]
    audio_outputs: Vec<AudioDeviceId>,
    #[rust]
    audio_on: Option<bool>,
    /// The threads' CPU time at the last per-thread line (tid -> ticks) and when.
    #[rust]
    thread_cpu: (std::collections::HashMap<u32, u64>, Option<std::time::Instant>),
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
    /// The server's foveation level last applied (255 before the first).
    #[rust(255)]
    fov_asked: u8,
    /// The eye buffers' range last applied (percent; 0 before the first).
    #[rust]
    eyes_asked: (u8, u8),
    /// Hands and controllers at once last applied.
    #[rust]
    multimodal_asked: Option<bool>,
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
        // the device's `debug.xrremote.target` (adb shell setprop) host:port links there instead
        // of pairing by PIN; `none`: no link (the headset's own frame loop alone)
        let fixed = device_property("debug.xrremote.target").filter(|t| !t.is_empty());
        if fixed.as_deref() == Some("none") {
            log!("xr remote: no link (debug.xrremote.target = none)");
            return;
        }
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
        // where the pairing is kept: the app's internal files (kept across reboots and updates)
        let files = cx.get_data_dir().unwrap_or_else(|| "/data/user/0/dev.makepad.xrremote/files".to_string());
        let target = match fixed.as_deref().and_then(|t| t.rsplit_once(':')).and_then(|(h, p)| Some((h.to_string(), p.parse::<u16>().ok()?))) {
            Some((host, port)) => XrTarget::Address { host, port },
            None => XrTarget::Pair { pairing: std::path::Path::new(&files).join("xr-pairing"), name: "Quest".into() },
        };
        // the rate the display runs at until the server asks for its own (`XrServerMsg::Refresh`):
        // the server encodes for it (its pictures are paced by the poses, one per display frame)
        let refresh = 90.0;
        log!("xr remote: linking ({target:?}) at {refresh} Hz");
        // `streamscale` N (percent, read at the link; default 120, the user's pick: 100 looked
        // half-res blurry, 140 is past what 166 encodes at 90/s): the eye picture the desktop
        // renders and encodes, N% of the headset's 1680x1760
        let percent = device_property("debug.xrremote.streamscale").and_then(|v| v.trim().parse::<u32>().ok()).filter(|p| (50..=160).contains(p)).unwrap_or(120);
        // (even sizes, 16-aligned widths: the encoder's blocks)
        let eye = [((1680 * percent / 100 + 15) / 16) * 16, ((1760 * percent / 100 + 7) / 8) * 8];
        log!("xr remote: stream eye {}x{} ({percent}%)", eye[0], eye[1]);
        let client = XrStreamClient::connect(target, eye, refresh);
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
        self.idle_timer = cx.start_interval(0.2);
    }
}

impl Widget for XrRemoteView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(client) = self.client.as_mut() {
            client.handle_event(cx, event);
            if self.idle_timer.is_event(event).is_some() {
                client.idle_tick(cx);
            }
            // the stream's sound on the headset's default output while the stream is on (off:
            // the device stops, its callback no longer runs)
            if let Event::AudioDevices(devices) = event {
                self.audio_outputs = devices.default_output();
                self.audio_on = None;
            }
            let on = !client.stream_off();
            if self.audio_on != Some(on) && !self.audio_outputs.is_empty() {
                self.audio_on = Some(on);
                cx.use_audio_outputs(if on { &self.audio_outputs } else { &[] });
            }
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
                // not linked: the PIN card is the panel, always shown, in front of the person
                // (following the head lazily, 1.5 m away, 1 m wide); linked: the panel as before
                // (hidden over the stream until a thumbstick click)
                let pin = self.client.as_ref().and_then(|c| c.pin());
                if pin != self.pin_shown {
                    use makepad_widgets::makepad_platform::makepad_hal::hal_xr_panel;
                    match &pin {
                        Some(_) => hal_xr_panel(1, 1.5, 1.0, true),
                        None => hal_xr_panel(0, 2.0, 2.0, false),
                    }
                    self.pin_shown = pin;
                    self.draw_bg.redraw(cx);
                }
                // the server decides the policy (`XrServerMsg::Headset`: foveation, the eye
                // buffers' range, hands and controllers at once, occlusion, the poses' frames
                // ahead); this headset executes it. Until it says: its defaults.
                let host = self.client.as_ref().and_then(|c| c.headset_settings()).unwrap_or(makepad_xr_stream::HeadsetSettings { foveation: 2, eye_min_pct: 70, eye_max_pct: 100, multimodal: true, occlusion: true, ahead: 4 });
                // (debug: `roomview` 1 shows the room's depth as 10 cm contour bands)
                let room_view = switch("debug.xrremote.roomview", 0) != 0;
                let (occlusion, opaque, clear) = (host.occlusion || room_view, switch("debug.xrremote.opaque", 0), switch("debug.xrremote.clear", 0) != 0);
                let was = self.client.as_ref().map_or((true, 0), |c| (c.occlusion, c.opaque));
                if (occlusion, opaque, clear) != (was.0, was.1, self.clear_tint) {
                    log!("xr remote: occlusion {occlusion}, opaque {opaque}, clear tint {clear}");
                    if clear != self.clear_tint {
                        self.clear_tint = clear;
                        let c = if clear { vec4(0.0, 0.25, 0.0, 0.5) } else { vec4(0.0, 0.0, 0.0, 0.0) };
                        self.pass.set_window_clear_color(cx, c);
                    }
                }
                // the display's refresh rate: the link's (the server's app picks it, the
                // Sandbox gear's stream fps; the supported one nearest)
                let hz = self.client.as_ref().map_or(90, |c| c.refresh_hz().round() as u8);
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
                if host.foveation != self.fov_asked {
                    self.fov_asked = host.foveation;
                    log!("xr remote: foveation level {} (the server's)", host.foveation);
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_foveation(host.foveation as u32);
                }
                if (host.eye_min_pct, host.eye_max_pct) != self.eyes_asked {
                    self.eyes_asked = (host.eye_min_pct, host.eye_max_pct);
                    log!("xr remote: eye buffers {}..{}% (the server's)", host.eye_min_pct, host.eye_max_pct);
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_eye_scale(host.eye_min_pct as u32, host.eye_max_pct as u32);
                }
                if Some(host.multimodal) != self.multimodal_asked {
                    self.multimodal_asked = Some(host.multimodal);
                    log!("xr remote: hands and controllers at once {} (the server's)", host.multimodal);
                    makepad_widgets::makepad_platform::makepad_hal::hal_xr_hands_and_controllers(host.multimodal);
                }
                if let Some(client) = self.client.as_mut() {
                    client.occlusion = occlusion;
                    // the poses at least `ahead` display frames past the headset's newest (more
                    // when the measured sample to decoded needs it; 0: that alone)
                    client.pin_ahead = host.ahead as u32;
                    client.opaque = opaque;
                    let own = switch("debug.xrremote.owneyes", 0) != 0;
                    if own != client.own_eyes { log!("xr remote: own eyes {own}"); }
                    client.own_eyes = own;
                    client.room_view = room_view;
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
        // (idle: no server linked or the stream off: no XR frames, the PIN card or the room alone)
        if self.presenting && matches!(event, Event::XrUpdate(_)) && !self.client.as_ref().is_some_and(|c| c.idle()) {
            self.xr_due = true;
            self.draw_list.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let draw_started = std::time::Instant::now();
        self.start(cx.cx);
        let rect = cx.walk_turtle(walk);
        self.dpi = cx.current_dpi_factor();
        let status = match &self.client {
            Some(c) => c.status(),
            None => "no headset: XR Remote shows a remote XR view on a headset".to_string(),
        };
        // unlinked: the PIN card (big digits; type them in the app on the computer)
        if let Some(pin) = self.pin_shown.clone() {
            let card = Rect { pos: rect.pos + (rect.size - dvec2(1040.0, 560.0)) * 0.5, size: dvec2(1040.0, 560.0) };
            self.draw_pin_card.draw_abs(cx, card);
            let centred = |text: &mut DrawText, cx: &mut Cx2d, y: f64, s: &str| {
                let w = text.prepare_single_line_run(cx, s).map(|r| r.width_in_lpxs as f64).unwrap_or(0.0);
                text.draw_abs(cx, dvec2(card.pos.x + (card.size.x - w) * 0.5, card.pos.y + y), s);
            };
            centred(&mut self.draw_pin_title, cx, 70.0, "Type this PIN on your computer");
            let spaced: String = pin.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
            centred(&mut self.draw_pin, cx, 150.0, &spaced);
            // one short status line, nothing else (the stats and the link's details stay in the log)
            centred(&mut self.draw_pin_hint, cx, 440.0, "Looking for computers…");
        } else {
            self.draw_bg.draw_abs(cx, rect);
            self.draw_text.draw_abs(cx, rect.pos + dvec2(24.0, 24.0), &status);
        }
        // the UI thread's part of a frame: its XrUpdate to the end of its draw, and the draw
        if let Some(at) = self.update_at.take() {
            let t = &mut self.ui_times;
            t.0 += at.elapsed().as_micros() as u64;
            t.1 += draw_started.elapsed().as_micros() as u64;
            t.2 += 1;
            if t.2 >= 450 {
                log!("xr remote: ui thread per frame ms: update to drawn {:.2}, draw {:.2}", t.0 as f64 / 1000.0 / t.2 as f64, t.1 as f64 / 1000.0 / t.2 as f64);
                *t = (0, 0, 0);
                self.log_thread_cpu();
            }
        }
        DrawStep::done()
    }
}

impl XrRemoteView {
    /// The app's threads by CPU since the last call (% of one core; the busiest, with their
    /// names and ids: this thread's id is marked), for the load hunt.
    fn log_thread_cpu(&mut self) {
        let Ok(dir) = std::fs::read_dir("/proc/self/task") else { return };
        let mut now = std::collections::HashMap::new();
        let mut names = std::collections::HashMap::new();
        for e in dir.flatten() {
            let Some(tid) = e.file_name().to_str().and_then(|t| t.parse::<u32>().ok()) else { continue };
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
            // (pid (comm) state ... utime stime: fields 14 and 15, counted after the comm)
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let rest: Vec<&str> = stat[close + 2..].split(' ').collect();
            let ticks = rest.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + rest.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            now.insert(tid, ticks);
            names.insert(tid, stat[open + 1..close].to_string());
        }
        let (before, at) = std::mem::replace(&mut self.thread_cpu, (now.clone(), Some(std::time::Instant::now())));
        let Some(at) = at else { return };
        let secs = at.elapsed().as_secs_f64().max(0.1);
        // (ticks are 1/100 s)
        let mut rows: Vec<(f64, u32)> = now.iter().map(|(tid, t)| ((t.saturating_sub(*before.get(tid).unwrap_or(t))) as f64 / secs, *tid)).collect();
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let me = unsafe { libc_gettid() } as u32;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let me = 0u32; // no /proc/self/task here: the read above already returned
        let total: f64 = rows.iter().map(|r| r.0).sum();
        let top: Vec<String> = rows.iter().take(12).filter(|r| r.0 > 0.0).map(|(pct, tid)| format!("{} {tid}{} {pct:.1}", names.get(tid).map_or("?", |n| n.as_str()), if *tid == me { "(ui)" } else { "" })).collect();
        log!("xr remote: threads % of a core: total {total:.0} · {}", top.join(" · "));
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
extern "C" {
    #[link_name = "gettid"]
    fn libc_gettid() -> i32;
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
        // (the stream's game audio: the view picks the output, on while the stream is)
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // the XR pass, apart from the window's draw
        if let Event::Draw(e) = event {
            if let Some(mut view) = self.ui.widget(cx, ids!(remote)).borrow_mut::<XrRemoteView>() {
                view.draw_xr(cx, e);
            }
        }
    }
}
