//! A Bilibili-style mobile video player example.
//!
//! Features:
//!  - Fullscreen toggle (button in the control bar).
//!  - Vertical swipe on the RIGHT half of the video adjusts VOLUME.
//!  - Vertical swipe on the LEFT half of the video adjusts BRIGHTNESS (simulated with a dim overlay,
//!    since there is no device-backlight API in the platform layer).
//!  - Horizontal swipe anywhere scrubs the playback position (fast-forward / rewind); the target
//!    time is previewed in the HUD while dragging and applied on release.
//!  - Single tap on the center area toggles play/pause.
//!  - A control bar at the bottom with play/pause, a draggable progress bar, a time readout, a
//!    playback-speed selector, and fullscreen.
//!  - `play_media(cx, video, source)` — a small "plugin" helper that auto-detects whether a
//!    string is a local path or a network URL, and which container it is (.m3u8/.mpd vs .mp4/etc),
//!    and points the Video widget at it accordingly.
//!
//! Note on orientation: makepad exposes fullscreen (which hides the status bar / home indicator on
//! iOS) but has no Rust API to force landscape orientation — that must be configured in the native
//! app manifest / Info.plist and is therefore out of scope for this example.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

// A public HLS (m3u8) adaptive-bitrate stream. The Video widget's backend plays HLS natively
// (AVPlayer on Apple, ExoPlayer on Android), so this is a real streaming source with multiple
// quality renditions (240p–1080p) rather than a single progressive MP4.
const VIDEO_URL: &str = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8";

// The available playback speeds, cycled by the speed button.
const SPEEDS: &[f64] = &[0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // Register our custom VideoPlayer widget with the script VM.
    let VideoPlayerBase = #(VideoPlayer::register_widget(vm))

    let VideoPlayer = set_type_default() do VideoPlayerBase{
        width: Fill
        height: Fill
        flow: Overlay
        show_bg: true
        draw_bg.color: #x000000

        // The hardware video surface. Its own controls are disabled so our custom gestures own
        // all touch input; we drive playback through the VideoRef API instead.
        // Source + playback are driven at runtime by the `play_media` plugin helper (see below),
        // which auto-detects local-vs-network and HLS-vs-progressive from the string it is given.
        video := Video{
            width: Fill
            height: Fill
            is_looping: true
            show_controls: false
        }

        // Brightness dimming layer. Its alpha (dim) is driven at runtime from the left-half swipe.
        dim_overlay := View{
            width: Fill
            height: Fill
            show_bg: true
            draw_bg +: {
                dim: instance(0.0)
                pixel: fn() {
                    return vec4(0.0, 0.0, 0.0, self.dim)
                }
            }
        }

        // Transparent full-size layer that OWNS all playback gestures (tap / swipe). Drawn above
        // the video but below the controls, it is the area we hit-test — so it captures the finger
        // before the inner Video widget's own (dormant) gesture handler can, and the control bar
        // (drawn last, so hit-tested first) still gets its clicks.
        gesture_layer := View{
            width: Fill
            height: Fill
            capture_overload: true
        }

        // Centered HUD that shows the value being adjusted during a gesture (volume / brightness /
        // seek target). Hidden by default; shown on gesture and auto-hidden shortly after.
        hud := View{
            width: Fill
            height: Fill
            align: Align{x: 0.5, y: 0.5}
            visible: false
            hud_panel := View{
                width: 220
                height: 84
                flow: Down
                align: Align{x: 0.5, y: 0.5}
                padding: 10
                spacing: 6
                show_bg: true
                draw_bg +: {
                    pixel: fn() {
                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                        sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 10.0)
                        sdf.fill(vec4(0.0, 0.0, 0.0, 0.6))
                        return sdf.result
                    }
                }
                hud_label := Label{
                    width: Fill
                    align: Align{x: 0.5, y: 0.5}
                    draw_text.color: #fff
                    draw_text.text_style.font_size: 17
                    draw_text.align: Align{x: 0.5, y: 0.5}
                    text: ""
                }
            }
        }

        // Bottom control bar.
        controls := View{
            width: Fill
            height: Fill
            flow: Down
            align: Align{x: 0.5, y: 1.0}
            control_bar := View{
                width: Fill
                height: 56
                flow: Right
                spacing: 12
                align: Align{x: 0.0, y: 0.5}
                padding: Inset{left: 16, right: 16, top: 8, bottom: 8}
                show_bg: true
                draw_bg.color: #x00000099

                play_button := Button{
                    width: 64
                    height: Fit
                    text: "Pause"
                    show_bg: true
                    draw_bg.color: #x00000099
                }

                // The track. The fill child's WIDTH is resized at runtime to reflect progress,
                // which is more robust than a shader uniform (it survives relayout and is visible
                // immediately). The whole track is a hit target for tap/drag-to-seek.
                // The track is a solid dark bar (clearly visible at all times). The played portion
                // is the `progress_fill` child whose WIDTH is resized at runtime to the progress
                // fraction — width-driven, so it's visible immediately and doesn't depend on the
                // stream having reported a duration to the shader. A round knob sits at the fill
                // edge (grows while dragging). The whole track is a hit target for tap/drag-to-seek.
                progress_bar := View{
                    width: Fill
                    height: 6
                    flow: Overlay
                    align: Align{x: 0.0, y: 0.5}
                    show_bg: true
                    draw_bg +: {
                        pixel: fn() {
                            // Solid dark "groove" for the unplayed portion — opaque enough to read
                            // as a distinct bar against the video, so the pink fill clearly contrasts.
                            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                            let h = self.rect_size.y
                            sdf.box(0.0, 0.0, self.rect_size.x, h, h * 0.5)
                            sdf.fill(vec4(1.0, 1.0, 1.0, 0.35))
                            return sdf.result
                        }
                    }
                    // Played portion: solid Bilibili pink, width set at runtime (0 -> track width).
                    progress_fill := View{
                        width: 0
                        height: Fill
                        show_bg: true
                        draw_bg +: {
                            dragging: instance(0.0)
                            pixel: fn() {
                                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                let h = self.rect_size.y
                                // Bilibili pink #fb7299 for the played portion.
                                let pink = vec4(0.984, 0.447, 0.6, 1.0)
                                sdf.box(0.0, 0.0, self.rect_size.x, h, h * 0.5)
                                sdf.fill(pink)
                                return sdf.result
                            }
                        }
                    }
                    // A round knob pinned to the right edge of the fill (i.e. current position).
                    // It's always visible (small) and grows while dragging for an obvious handle.
                    progress_knob := View{
                        width: Fill
                        height: Fill
                        draw_bg +: {
                            edge: instance(0.0)
                            dragging: instance(0.0)
                            pixel: fn() {
                                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                let h = self.rect_size.y
                                let pink = vec4(0.984, 0.447, 0.6, 1.0)
                                let r = mix(h * 0.9, h * 1.8, self.dragging)
                                sdf.circle(self.edge * self.rect_size.x, h * 0.5, r)
                                sdf.fill(pink)
                                return sdf.result
                            }
                        }
                        show_bg: true
                    }
                }

                time_label := Label{
                    width: Fit
                    height: Fit
                    draw_text.color: #fff
                    draw_text.text_style.font_size: 11
                    text: "00:00 / 00:00"
                }

                speed_button := Button{
                    width: 52
                    height: Fit
                    text: "1.0x"
                    show_bg: true
                    draw_bg.color: #x00000099
                }

                fullscreen_button := Button{
                    width: 56
                    height: Fit
                    text: "Full"
                    show_bg: true
                    draw_bg.color: #x00000099
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Video Player"
                // Portrait phone proportions for desktop verification; Fill on device.
                window.inner_size: vec2(1024, 576)
                pass.clear_color: #x000000
                body +: {
                    player := VideoPlayer{}
                }
            }
        }
    }
}

/// Which gesture the current drag has locked onto.
#[derive(Clone, Copy, PartialEq)]
enum Gesture {
    Undecided,
    Volume,
    Brightness,
    Seek,
}

#[derive(Script, ScriptHook, Widget)]
pub struct VideoPlayer {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    // Gesture tracking.
    #[rust(Gesture::Undecided)]
    gesture: Gesture,
    #[rust]
    start_abs: DVec2,
    #[rust(1.0)]
    volume: f64,
    #[rust(1.0)]
    start_volume: f64,
    #[rust(1.0)]
    brightness: f64,
    #[rust(1.0)]
    start_brightness: f64,
    #[rust]
    start_position_ms: u128,
    #[rust]
    seek_preview_ms: u128,
    #[rust(false)]
    dragging_bar: bool,

    // Playback speed (index into SPEEDS).
    #[rust(2usize)]
    speed_index: usize,

    // Intended play/pause state (source of truth for the button label; the video autostarts).
    #[rust(false)]
    paused: bool,

    // HUD auto-hide + progress refresh.
    #[rust]
    hud_timer: Timer,
    // Immersive mode: hide the control bar after 5s of no interaction; any gesture re-shows it.
    #[rust]
    idle_timer: Timer,
    #[rust]
    tick_timer: Timer,
    #[rust(false)]
    tick_started: bool,
    #[rust(false)]
    media_started: bool,
}

impl Widget for VideoPlayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Start a periodic tick to refresh the progress bar and time readout during playback.
        if !self.tick_started {
            self.tick_timer = cx.start_interval(0.25);
            self.tick_started = true;
        }
        // Kick off playback via the plugin helper on the first event (once the widget exists).
        if !self.media_started {
            let video = self.video(cx, ids!(video));
            play_media(cx, &video, VIDEO_URL);
            self.media_started = true;
            // NOTE: we intentionally do NOT arm the immersive countdown here. Controls stay
            // visible until the first user interaction; once the user touches anything, the 5s
            // "no gesture -> hide" rule takes over (armed from show_controls_bar / gesture paths).
        }
        if self.tick_timer.is_event(event).is_some() {
            self.refresh_progress(cx);
        }
        if self.hud_timer.is_event(event).is_some() {
            self.hide_hud(cx);
        }
        if self.idle_timer.is_event(event).is_some() {
            self.hide_controls_bar(cx);
        }

        // IMPORTANT ordering: we hit-test our own areas BEFORE handing the event to the inner
        // widget tree. The inner `Video` widget calls `event.hits()` on its own full-screen area
        // unconditionally (even with `show_controls:false`) and would capture the finger on
        // MouseDown, so if we ran `self.view.handle_event` first, our gesture layer would never
        // see a FingerDown/FingerMove. By capturing first, our layer wins the digit and the inner
        // Video's `event.hits` returns Nothing (it doesn't use capture_overload).
        //
        // To keep the control-bar buttons clickable we EXCLUDE the bottom control-bar band from
        // the gesture layer's capture: a down inside that band falls through to the buttons.
        let bar_area = self.view(cx, ids!(progress_bar)).area();
        let controls_rect = self.view(cx, ids!(control_bar)).area().rect(cx);
        match event.hits(cx, bar_area) {
            Hit::FingerDown(fe) => {
                self.dragging_bar = true;
                self.set_dragging(cx, true);
                self.show_controls_bar(cx);
                let rect = bar_area.rect(cx);
                let frac = (fe.abs.x - rect.pos.x) / rect.size.x;
                self.seek_to_track_fraction(cx, frac);
            }
            Hit::FingerMove(fe) => {
                if self.dragging_bar {
                    let rect = bar_area.rect(cx);
                    let frac = (fe.abs.x - rect.pos.x) / rect.size.x;
                    self.seek_to_track_fraction(cx, frac);
                }
            }
            Hit::FingerUp(_) => {
                self.dragging_bar = false;
                self.set_dragging(cx, false);
                self.arm_idle_timeout(cx);
            }
            _ => {}
        }

        // Custom gestures over our dedicated transparent gesture layer (NOT the video's own area,
        // which would fight the inner Video widget's dormant gesture handler for finger capture).
        // Hit-test the full-screen gesture layer, but EXCLUDE the bottom control-bar band via a
        // custom hit test so downs on the buttons/progress bar fall through to those widgets
        // (handled later in `self.view.handle_event`). Everywhere else, this layer owns the finger.
        let area = self.view(cx, ids!(gesture_layer)).area();
        let ctrl_top = if controls_rect.size.x > 0.0 { controls_rect.pos.y } else { f64::INFINITY };
        match event.hits_with_options_and_test(
            cx,
            area,
            HitOptions::new().with_capture_overload(true),
            |abs, rect, margin| {
                Inset::rect_contains_with_inset(abs, rect, margin) && abs.y < ctrl_top
            },
        ) {
            Hit::FingerDown(fe) => {
                self.gesture = Gesture::Undecided;
                self.start_abs = fe.abs;
                self.start_volume = self.volume;
                self.start_brightness = self.brightness;
                self.start_position_ms = self.video(cx, ids!(video)).current_position_ms();
                // Any center-screen interaction leaves immersive mode and re-arms the countdown.
                self.show_controls_bar(cx);
            }
            Hit::FingerMove(fe) => {
                let rect = area.rect(cx);
                let d = fe.abs - self.start_abs;

                // Lock the gesture direction once the finger has moved past a small threshold.
                if self.gesture == Gesture::Undecided && d.length() > 10.0 {
                    if d.x.abs() > d.y.abs() {
                        self.gesture = Gesture::Seek;
                        self.set_dragging(cx, true);
                        log!("gesture: locked Seek (horizontal drag)");
                    } else {
                        let mid_x = rect.pos.x + rect.size.x * 0.5;
                        self.gesture = if self.start_abs.x < mid_x {
                            Gesture::Brightness
                        } else {
                            Gesture::Volume
                        };
                    }
                }

                match self.gesture {
                    Gesture::Volume => {
                        // Drag up to raise, down to lower. Full range over ~60% of the height.
                        let v = (self.start_volume - d.y / (rect.size.y * 0.6)).clamp(0.0, 1.0);
                        self.volume = v;
                        self.video(cx, ids!(video)).set_volume(cx, v);
                        self.show_hud(cx, &format!("VOL {}%", (v * 100.0).round() as i32));
                    }
                    Gesture::Brightness => {
                        let b = (self.start_brightness - d.y / (rect.size.y * 0.6)).clamp(0.05, 1.0);
                        self.brightness = b;
                        self.apply_brightness(cx);
                        self.show_hud(cx, &format!("BRI {}%", (b * 100.0).round() as i32));
                    }
                    Gesture::Seek => {
                        let total = self.video(cx, ids!(video)).total_duration_ms();
                        if total > 0 {
                            self.seek_preview_ms =
                                scrub_target_ms(self.start_position_ms, d.x, rect.size.x, total);
                            let target = self.seek_preview_ms as f64;

                            // Drag the progress bar live to the preview position.
                            let frac = (target / total as f64) as f32;
                            self.set_progress_fill(cx, frac);
                            self.label(cx, ids!(time_label)).set_text(
                                cx,
                                &format!("{} / {}", fmt_time(self.seek_preview_ms), fmt_time(total)),
                            );

                            // Centered HUD: direction arrow + signed delta + absolute position.
                            let signed = self.seek_preview_ms as i128 - self.start_position_ms as i128;
                            let arrow = if signed >= 0 { "⏩" } else { "⏪" };
                            self.show_hud(
                                cx,
                                &format!(
                                    "{} {}{}\n{} / {}",
                                    arrow,
                                    if signed >= 0 { "+" } else { "-" },
                                    fmt_time(signed.unsigned_abs()),
                                    fmt_time(self.seek_preview_ms),
                                    fmt_time(total),
                                ),
                            );
                        }
                    }
                    Gesture::Undecided => {}
                }
            }
            Hit::FingerUp(fe) => {
                match self.gesture {
                    Gesture::Seek => {
                        self.video(cx, ids!(video)).seek_to(cx, self.seek_preview_ms as u64);
                    }
                    Gesture::Undecided => {
                        // No direction locked -> treat as a tap on the center area: play/pause.
                        if fe.was_tap() {
                            self.toggle_play_pause(cx);
                        }
                    }
                    _ => {}
                }
                self.gesture = Gesture::Undecided;
                self.set_dragging(cx, false);
                self.arm_hud_timeout(cx);
                // Re-arm the immersive countdown from the end of the gesture.
                self.arm_idle_timeout(cx);
            }
            _ => {}
        }

        // Finally hand the event to the inner widget tree (buttons, labels, and the dormant inner
        // Video). Downs over the center were already captured above, so the inner Video won't steal
        // them; downs over the control bar were excluded from our layer and reach the buttons here.
        self.view.handle_event(cx, event, scope);
    }
}

impl VideoPlayer {
    fn toggle_play_pause(&mut self, cx: &mut Cx) {
        // We track the intended play/pause state ourselves rather than querying the backend's
        // `is_playing()`, which reflects buffering/decoding reality (a network stream may report
        // "not playing" while it buffers) and would make the button label flicker or stick. The
        // user's intent is the source of truth for the label; we push it to the backend.
        let video = self.video(cx, ids!(video));
        if self.paused {
            // Currently paused -> resume/start playing.
            if video.is_paused() {
                video.resume_playback(cx);
            } else {
                video.begin_playback(cx);
            }
            self.paused = false;
            self.button(cx, ids!(play_button)).set_text(cx, "Pause");
        } else {
            video.pause_playback(cx);
            self.paused = true;
            self.button(cx, ids!(play_button)).set_text(cx, "Play");
        }
    }

    /// Advances to the next playback speed and applies it to the video.
    fn cycle_speed(&mut self, cx: &mut Cx) {
        self.speed_index = (self.speed_index + 1) % SPEEDS.len();
        let rate = SPEEDS[self.speed_index];
        self.video(cx, ids!(video)).set_playback_rate(cx, rate);
        self.button(cx, ids!(speed_button)).set_text(cx, &fmt_speed(rate));
    }

    /// Seeks to the position corresponding to a horizontal fraction of the progress track.
    fn seek_to_track_fraction(&mut self, cx: &mut Cx, fraction: f64) {
        let total = self.video(cx, ids!(video)).total_duration_ms();
        if total == 0 {
            return;
        }
        let target = (fraction.clamp(0.0, 1.0) * total as f64) as u64;
        self.video(cx, ids!(video)).seek_to(cx, target);
        self.set_progress_fill(cx, fraction.clamp(0.0, 1.0) as f32);
        self.label(cx, ids!(time_label)).set_text(
            cx,
            &format!("{} / {}", fmt_time(target as u128), fmt_time(total)),
        );
    }

    fn apply_brightness(&mut self, cx: &mut Cx) {
        // dim == how much black to overlay. brightness 1.0 -> dim 0.0.
        let dim = (1.0 - self.brightness) as f32;
        let dim_ref = self.view(cx, ids!(dim_overlay));
        {
            let mut v = dim_ref.borrow_mut();
            if let Some(v) = v.as_mut() {
                v.draw_bg.draw_vars.set_uniform(cx, live_id!(dim), &[dim]);
                v.redraw(cx);
            }
        }
    }

    /// Drives the progress-bar fill to `fraction` (0..1) of the track width. The played portion is
    /// a child view whose WIDTH we resize to `fraction * track_width` (visible immediately, no
    /// dependence on the shader/stream), and a knob is pinned to the fill edge via the `edge` uniform.
    fn set_progress_fill(&mut self, cx: &mut Cx, fraction: f32) {
        let f = fraction.clamp(0.0, 1.0) as f64;
        let track_w = self.view(cx, ids!(progress_bar)).area().rect(cx).size.x;
        let fill = self.view(cx, ids!(progress_fill));
        if let Some(mut inner) = fill.borrow_mut() {
            inner.walk.width = Size::Fixed(track_w * f);
        }
        fill.redraw(cx);
        // Pin the knob to the fill edge (fraction across the full track width).
        let knob = self.view(cx, ids!(progress_knob));
        knob.set_uniform(cx, live_id!(edge), &[f as f32]);
        knob.redraw(cx);
    }

    /// Toggles the progress-bar drag effect (larger knob) via the `dragging` shader uniform.
    fn set_dragging(&mut self, cx: &mut Cx, dragging: bool) {
        let knob = self.view(cx, ids!(progress_knob));
        knob.set_uniform(cx, live_id!(dragging), &[if dragging { 1.0 } else { 0.0 }]);
        knob.redraw(cx);
    }

    /// Reveals the control bar and (re)starts the 5s immersive-mode countdown. Called on any user
    /// interaction (gesture / bar drag / button click).
    fn show_controls_bar(&mut self, cx: &mut Cx) {
        self.view(cx, ids!(controls)).set_visible(cx, true);
        self.redraw(cx);
        self.arm_idle_timeout(cx);
    }

    /// Hides the control bar for an immersive view; playback keeps going.
    fn hide_controls_bar(&mut self, cx: &mut Cx) {
        self.view(cx, ids!(controls)).set_visible(cx, false);
        self.redraw(cx);
    }

    /// Restarts the 5-second no-interaction countdown that triggers immersive mode.
    fn arm_idle_timeout(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.idle_timer);
        self.idle_timer = cx.start_timeout(5.0);
    }

    fn refresh_progress(&mut self, cx: &mut Cx) {
        // While the user is scrubbing (swipe-seek OR dragging the bar), the gesture owns the bar +
        // time label; don't let the periodic tick fight the drag by snapping to the real position.
        if self.gesture == Gesture::Seek || self.dragging_bar {
            return;
        }
        let video = self.video(cx, ids!(video));
        let pos = video.current_position_ms();
        let total = video.total_duration_ms();
        let fill = if total > 0 { (pos as f64 / total as f64) as f32 } else { 0.0 };

        self.set_progress_fill(cx, fill);
        self.label(cx, ids!(time_label))
            .set_text(cx, &format!("{} / {}", fmt_time(pos), fmt_time(total)));
    }

    fn show_hud(&mut self, cx: &mut Cx, text: &str) {
        self.label(cx, ids!(hud_label)).set_text(cx, text);
        self.view(cx, ids!(hud)).set_visible(cx, true);
        self.redraw(cx);
    }

    fn arm_hud_timeout(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.hud_timer);
        self.hud_timer = cx.start_timeout(0.6);
    }

    fn hide_hud(&mut self, cx: &mut Cx) {
        self.view(cx, ids!(hud)).set_visible(cx, false);
        self.redraw(cx);
    }
}

/// Points a [`Video`] widget at a media source given as a plain string, auto-detecting:
///  - **local vs network**: an `http(s)://` (or `file://`) prefix means network; otherwise it is
///    treated as a filesystem path.
///  - **container**: `.m3u8` / `.mpd` are adaptive-streaming manifests (played natively as HLS/
///    DASH); everything else (`.mp4`, `.mov`, …) is a progressive container.
///
/// This is the "plugin" entry point: callers just pass a path or URL and the right
/// [`VideoDataSource`] is chosen and applied.
fn play_media(cx: &mut Cx, video: &VideoRef, src: &str) {
    let lower_path = src.split(['?', '#']).next().unwrap_or(src).to_ascii_lowercase();
    let is_manifest = lower_path.ends_with(".m3u8") || lower_path.ends_with(".mpd");

    let is_network = src.starts_with("http://")
        || src.starts_with("https://")
        || src.starts_with("file://");

    // Both HLS/DASH manifests and progressive files go through Network when given a URL; a local
    // path uses Filesystem. (Local .m3u8 is unusual and not handled specially here.)
    let source = if is_network {
        VideoDataSource::Network { url: src.to_string() }
    } else {
        VideoDataSource::Filesystem { path: src.to_string() }
    };

    let kind = if is_manifest { "HLS/DASH manifest" } else { "progressive file" };
    let origin = if is_network { "network" } else { "local" };
    log!("play_media: {} {} -> {}", origin, kind, src);

    video.set_source(source);
    video.begin_playback(cx);
}

/// Computes the target scrub position for a horizontal seek drag.
///
/// Bilibili-style sensitivity: a full-width drag scrubs at most ~120s (or the whole clip, if
/// shorter), so short videos map the full width to the whole timeline while long ones stay
/// fine-grained and draggy. The result is clamped to `[0, total_ms]`.
fn scrub_target_ms(start_ms: u128, dx: f64, track_w: f64, total_ms: u128) -> u128 {
    if total_ms == 0 || track_w <= 0.0 {
        return start_ms.min(total_ms);
    }
    let span_ms = (total_ms as f64).min(120_000.0);
    let delta_ms = (dx / track_w) * span_ms;
    (start_ms as f64 + delta_ms).clamp(0.0, total_ms as f64) as u128
}

/// Formats a playback rate as a speed label, e.g. `0.5x`, `1.0x`, `1.25x`, `2.0x`.
/// A plain `{}` would render `1.0` as `1`, so we keep at least one decimal and trim only the
/// trailing zeros beyond it (so `1.25` stays `1.25` but `1.50` collapses to `1.5`).
fn fmt_speed(rate: f64) -> String {
    let mut s = format!("{:.2}", rate); // e.g. "1.00", "1.25", "0.75"
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    format!("{}x", s)
}

/// Formats a millisecond duration as `MM:SS`.
fn fmt_time(ms: u128) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{:02}:{:02}", mins, secs)
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust(false)]
    is_fullscreen: bool,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(play_button)).clicked(actions) {
            if let Some(mut vp) = self.ui.widget(cx, ids!(player)).borrow_mut::<VideoPlayer>() {
                vp.toggle_play_pause(cx);
                vp.show_controls_bar(cx);
            }
        }
        if self.ui.button(cx, ids!(speed_button)).clicked(actions) {
            if let Some(mut vp) = self.ui.widget(cx, ids!(player)).borrow_mut::<VideoPlayer>() {
                vp.cycle_speed(cx);
                vp.show_controls_bar(cx);
            }
        }
        if self.ui.button(cx, ids!(fullscreen_button)).clicked(actions) {
            self.is_fullscreen = !self.is_fullscreen;
            let window = self.ui.window(cx, ids!(main_window));
            if self.is_fullscreen {
                window.fullscreen(cx);
            } else {
                window.disable_fullscreen(cx);
            }
            let label = if self.is_fullscreen { "Window" } else { "Full" };
            self.ui.button(cx, ids!(fullscreen_button)).set_text(cx, label);
            if let Some(mut vp) = self.ui.widget(cx, ids!(player)).borrow_mut::<VideoPlayer>() {
                vp.show_controls_bar(cx);
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_time_formats_mm_ss() {
        assert_eq!(fmt_time(0), "00:00");
        assert_eq!(fmt_time(1_000), "00:01");
        assert_eq!(fmt_time(59_000), "00:59");
        assert_eq!(fmt_time(60_000), "01:00");
        assert_eq!(fmt_time(9 * 60_000 + 56_000), "09:56");
    }

    #[test]
    fn fmt_speed_keeps_one_decimal_and_trims() {
        // Every entry in the selector renders exactly as the UI shows it.
        assert_eq!(fmt_speed(0.5), "0.5x");
        assert_eq!(fmt_speed(0.75), "0.75x");
        assert_eq!(fmt_speed(1.0), "1.0x");
        assert_eq!(fmt_speed(1.25), "1.25x");
        assert_eq!(fmt_speed(1.5), "1.5x");
        assert_eq!(fmt_speed(1.75), "1.75x");
        assert_eq!(fmt_speed(2.0), "2.0x");
    }

    #[test]
    fn scrub_target_maps_full_width_to_whole_short_clip() {
        // A 30s clip is under the 120s cap, so a full-width right-drag reaches the end and a
        // full-width left-drag reaches the start (both clamped).
        let total = 30_000;
        let w = 400.0;
        assert_eq!(scrub_target_ms(0, w, w, total), total); // full drag right from 0 -> end
        assert_eq!(scrub_target_ms(total, -w, w, total), 0); // full drag left from end -> start
        assert_eq!(scrub_target_ms(15_000, 0.0, w, total), 15_000); // no drag -> unchanged
    }

    #[test]
    fn scrub_target_is_fine_grained_for_long_clips() {
        // A 20-minute clip: the 120s cap means a full-width drag moves only ~120s, not the whole
        // timeline — so scrubbing stays precise. Half a width from the middle -> +60s.
        let total = 20 * 60_000; // 1_200_000 ms
        let w = 400.0;
        let start = 10 * 60_000; // 600_000 ms
        let target = scrub_target_ms(start, w * 0.5, w, total);
        assert_eq!(target, start + 60_000);
    }

    #[test]
    fn scrub_target_clamps_within_bounds() {
        let total = 60_000;
        let w = 400.0;
        // Dragging far right past the end clamps to total.
        assert_eq!(scrub_target_ms(50_000, w * 10.0, w, total), total);
        // Dragging far left past the start clamps to 0.
        assert_eq!(scrub_target_ms(10_000, -w * 10.0, w, total), 0);
        // Zero duration is a no-op.
        assert_eq!(scrub_target_ms(5_000, w, w, 0), 0);
    }
}