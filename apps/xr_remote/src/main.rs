//! XR Remote: the headset end of remote rendering. It opens the OpenXR session (the HAL's), links
//! to a desktop app's XR server (`XR_REMOTE_TARGET` at build time, default 166's Sandbox) and shows
//! its stereo stream in the XR view: each eye its half of the picture, drawn with the eyes the
//! desktop rendered it with. Its poses, controllers and hands go upstream every display frame.
//! The panel (the window, a quad in the headset) shows the link and its stats; over the stereo
//! view it is hidden until the left thumbstick is clicked. No app state of the remote app lives
//! here.

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_xr_stream::{DrawXrStream, XrStreamClient};

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
    #[new]
    pass: DrawPass,
    /// The XR pass's own draw list (draws inside a pass need one).
    #[new]
    draw_list: DrawList2d,
    #[rust]
    color: Option<Texture>,
    #[rust]
    depth: Option<Texture>,
    #[rust]
    presenting: bool,
    #[rust]
    client: Option<XrStreamClient>,
    /// When the debug switches were last read.
    #[rust]
    switches_at: Option<std::time::Instant>,
    #[rust]
    clear_tint: bool,
}

impl XrRemoteView {
    /// Once: the XR pass and the link.
    fn start(&mut self, cx: &mut Cx) {
        if self.presenting || !cx.xr_display() {
            return;
        }
        self.presenting = true;
        let color = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true });
        let depth = Texture::new_with_format(cx, TextureFormat::DepthD32Sampled { size: TextureSize::Auto, initial: true });
        self.pass.set_pass_name(cx, "xr remote");
        self.pass.set_color_texture(cx, &color, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        self.pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        self.pass.set_keep_camera_matrix(cx, true);
        self.pass.set_as_xr_pass(cx);
        cx.xr_start_presenting();
        // mixed reality: the room shows wherever the desktop drew nothing (the stream's coverage)
        cx.xr_set_see_through(option_env!("XR_REMOTE_OPAQUE").is_none());
        self.color = Some(color);
        self.depth = Some(depth);
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
        let refresh = cx.xr_display_refresh_rate_hz().unwrap_or(72.0);
        log!("xr remote: linking to {host}:{port} at {refresh} Hz");
        self.client = Some(XrStreamClient::connect(host, port, [1680, 1760], refresh));
    }
}

impl Widget for XrRemoteView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(client) = self.client.as_mut() {
            client.handle_event(cx, event);
        }
        if self.presenting {
            // the debug switches (`adb shell setprop debug.xrremote.<name> 0|1`), read once a
            // second: `occlusion` (default 1) the room's depth hides the picture, `opaque`
            // (default 0) 1 the picture covers the room whatever its coverage, 2 a flat magenta
            // instead (the draw reaches the view), `clear` (default 0) 1 a green tint under it
            // (the XR pass reaches the view), `owneyes` (default 0) 1 the frame shows with the
            // headset's own eyes instead of the picture's
            if self.switches_at.map_or(true, |t| t.elapsed().as_secs() >= 1) {
                self.switches_at = Some(std::time::Instant::now());
                let switch = |name: &str, default: u8| device_property(name).and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(default);
                let (occlusion, opaque, clear) = (switch("debug.xrremote.occlusion", 1) != 0, switch("debug.xrremote.opaque", 0), switch("debug.xrremote.clear", 0) != 0);
                let was = self.client.as_ref().map_or((true, 0), |c| (c.occlusion, c.opaque));
                if (occlusion, opaque, clear) != (was.0, was.1, self.clear_tint) {
                    log!("xr remote: occlusion {occlusion}, opaque {opaque}, clear tint {clear}");
                    if clear != self.clear_tint {
                        self.clear_tint = clear;
                        let c = if clear { vec4(0.0, 0.25, 0.0, 0.5) } else { vec4(0.0, 0.0, 0.0, 0.0) };
                        self.pass.set_window_clear_color(cx, c);
                    }
                }
                if let Some(client) = self.client.as_mut() {
                    client.occlusion = occlusion;
                    client.opaque = opaque;
                    let own = switch("debug.xrremote.owneyes", 0) != 0;
                    if own != client.own_eyes { log!("xr remote: own eyes {own}"); }
                    client.own_eyes = own;
                }
            }
        }
        // every XR frame is answered (with or without a link)
        if self.presenting && matches!(event, Event::XrUpdate(_) | Event::Signal) {
            self.draw_bg.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.start(cx.cx);
        let rect = cx.walk_turtle(walk);
        // every XR frame is answered (cleared until the first picture): the headset keeps asking
        // at its display rate and the poses flow at that rate
        if self.presenting {
            let dpi = cx.current_dpi_factor();
            cx.begin_pass(&self.pass, Some(dpi));
            self.draw_list.begin_always(cx);
            cx.begin_unclipped_root_turtle(dvec2(4096.0, 4096.0), Layout::flow_down());
            if let Some(client) = self.client.as_mut() {
                client.draw(cx, &mut self.draw_stream);
            }
            cx.end_pass_sized_turtle_no_clip();
            self.draw_list.end(cx);
            cx.end_pass(&self.pass);
        }
        self.draw_bg.draw_abs(cx, rect);
        let status = match &self.client {
            Some(c) => c.status(),
            None => "no headset: XR Remote shows a remote XR view on a headset".to_string(),
        };
        self.draw_text.draw_abs(cx, rect.pos + dvec2(24.0, 24.0), &status);
        DrawStep::done()
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
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
