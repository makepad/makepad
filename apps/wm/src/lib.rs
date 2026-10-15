//! makepad-app-wm (wm): an Omarchy-behaving tiling window manager as a
//! Makepad app. Nested mode: this window is the desktop; tiles host other
//! Makepad apps as child processes over the studio runview protocol, or
//! in-process as modules (module_host.rs) where the build links them.
//!
//! A LIBRARY: the desktop `wm` binary (src/main.rs) and the all-in-one
//! builds (apps/wm-all) are this one `App` with a different `WmBuild`
//! (build.rs) — the binary's `app_main!` `configure:` clause sets it on
//! `Cx` before startup.
//!
//! Chrome is styled entirely in splash: the theme is a `theme.splash` file
//! (imported from omarchy themes, see theme.rs) evaluated into the VM
//! before the app module, so the DSL below reads `mod.wm_theme.*`.

#![allow(dead_code)] // shell surface (icons, OSD, panels) built ahead of the flows that use it
pub use makepad_widgets;
use makepad_widgets::makepad_platform::thread::{Lane, SignalToUI, TaskHandle};
use makepad_widgets::makepad_platform::hosted_relay::HostRelayServer;
use makepad_widgets::tween::{ClockPolicy, LagSmoothing, TweenClock};
use makepad_widgets::*;

mod ai_bus;
mod apps;
mod binds;
mod build;
mod client_windows;
use client_windows::ClientWindows;
mod clients;
mod demo_home;
mod desk;
mod own_chrome;
mod desktop_layout;
mod desktop;
mod desktop_app;
mod snap;
mod mobile;
mod launcher_motion;
mod mobile_surface;
mod mobile_app;
mod jit_cache;
mod mobile_tiles;
mod scene;
mod screenshot;
mod screenshot_app;
mod backdrop;
mod skins;
mod dock_warp;
mod dock_layout;
mod dock_autohide;
mod hub_control;
#[cfg(not(target_arch = "wasm32"))]
mod hub_child;
mod hub_glow;
mod gpu_yield;
mod scroll_gesture;
use scroll_gesture::Gesture as ScrollGesture;
use hub_control::HubLink;
use gpu_yield::YieldPolicy;
mod motion;
mod spaces;
mod spacewarp;
mod shake;
mod history;
mod behind;
mod host;
mod layout;
mod screens;
mod stereo;
mod headset_imu;
#[cfg(target_os = "linux")]
mod linux_controls;
#[cfg(target_os = "linux")]
mod adopt;
#[cfg(target_os = "linux")]
mod sdbus;
#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "linux")]
mod portal_file_chooser;
#[cfg(target_os = "linux")]
use linux_controls::LinuxControls;
pub mod module_host;
pub mod module_view;
pub mod wayland_host;
pub mod wayland_view;
mod pane_links;
mod preview;
mod run_view;
mod domain_view;
#[cfg(not(target_arch = "wasm32"))]
mod domains;
#[cfg(not(target_arch = "wasm32"))]
use domains::Domains;
mod shell;
mod theme;
mod tile;

pub use build::WmBuild;
pub use desktop::DesktopStyle;
pub use run_view::MpRunViewAction;
pub use makepad_app_module;

use std::collections::HashMap;

use binds::{match_bind_armed, WmAction};
use desk::{WmDesk, WmDeskAction, WmState};
use clients::{spawn_client, ClientLine, LaunchPolicy, WarmPool, WarmStatus};
use makepad_app_registry::{BuildPolicy, LaunchEvent};
use makepad_widgets::makepad_platform::shared_framebuf::HostSwapchain;
#[cfg(not(target_os = "linux"))]
use makepad_widgets::makepad_platform::shared_framebuf::shared_swapchain_from_host_swapchain;
use makepad_app_host::{send_to_app, AppHub, ClientId, HubEvent};
use layout::{Axis, Dir, DividerHit, LRect, WindowMode};
use makepad_studio_protocol::{AppToStudio, ChildRelay, StudioToApp};
use makepad_wm_api::{ScreenSpan, WmEvent, WmRequest};
use preview::PreviewCache;
use run_view::MpRunView;
use makepad_widgets::makepad_micro_serde::*;
use shell::bar::{BarData, BarModule, SampledStatus, ShellBarAction};
use shell::menu::{MenuSkin, ShellMenu, ShellMenuAction};
use shell::panels::ShellPanelAction;
use ai_bus::{AiBus, Route};
use apps::{AppRegistry, Hosting, Launchable};
use mobile::{phone_size, PhoneHit};
use makepad_ai_services::engine::ServiceRegistry;
use makepad_ai_services::wire::{ServiceCall, ServiceDown, ToolResult};
use makepad_app_module::{AppModule, ExecOutcome, ModuleUpstream};
use module_host::ModuleHost;
use pane_links::{PaneCall, PaneLinks};
use makepad_widgets::ai_slot::AiSlotRequests;
use shell::ai_pane::ShellAiPane;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // A tray glyph: one of our own SVGs on a single quad. The icons are
    // drawn at their natural size in one 16-unit box, so `scale` keeps
    // their proportions honest against each other (a battery IS wider and
    // shorter than a speaker) at roughly the reference tray's 9-10px.
    let TrayIcon = Icon{
        width: Fit
        height: Fill
        align: Align{x: 0.5 y: 0.5}
        icon_walk: Walk{width: Fit height: Fit}
        draw_icon +: {
            scale: 0.9
            color: mod.wm_theme.foreground
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1400, 900)
                window.title: "makepad-wm"
                // No native caption: the Omarchy bar IS the caption. Our
                // WindowDragQuery answers Caption over the bar strip (minus
                // its buttons) so the window still drags.
                show_caption_bar: false
                body +: {
                    // The bar is drawn over the column, after the scene:
                    // a strip above the desk (`bar_space` holds its place)
                    // on one screen, floating over the top of every screen
                    // on a multi-screen desktop (`shell::bar::bar_placement`).
                    flow: Overlay
                    body_column := View{
                    width: Fill
                    height: Fill
                    flow: Down
                    bar_space := View{width: Fill height: 26}
                    // The wallpaper layer: the theme's image (crop-to-fill)
                    // over the theme's deep background.
                    scene := WmScene{
                    wallpaper := CachedView{width: Fill height: Fill flow: Overlay
                    bg_fill := RectView{
                        width: Fill
                        height: Fill
                        draw_bg +: {
                            color_top: uniform(mod.wm_theme.darker_background)
                            color_bottom: uniform(mod.wm_theme.background)
                            pixel: fn() {
                                return mix(self.color_top, self.color_bottom, self.pos.y)
                            }
                        }
                    }
                    wallpaper_backdrop := WmBackdrop{}
                    bg_image := Image{
                        width: Fill
                        height: Fill
                        fit: ImageFit.CropToFill
                        visible: false
                    }
                    }
                        desk_row := View{
                            width: Fill
                            height: Fill
                            flow: Right
                            shell_ai_pane := ShellAiPane{}
                            desk := WmDesk{
                                Tile := MpRunView{}
                                // An in-process module instance's tile (module_view.rs).
                                ModuleTile := MpModuleView{}
                                // A foreign Wayland window's tile (wayland_view.rs).
                                WaylandTile := MpWaylandView{}
                                DomainTile := MpDomainView{}
                            }
                        }
                    // The shell's floating surfaces, over the desk: the
                    // menu/launcher, the bar flyouts, the OSD and the
                    // notification stack. Each draws only when it is up.
                    shell_overlay := View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        desktop_shelf := DesktopShelf{}
                        snap_overlay := SnapOverlay{}
                        shell_panel := ShellPanel{}
                        shell_menu := ShellMenu{}
                        shell_notes := ShellNotifications{}
                        shell_osd := ShellOsd{}
                        shot_overlay := ShotOverlay{}
                        // Over everything: a client's file dialog (shell/file_picker.rs).
                        file_picker := ShellFilePicker{}
                    }
                    }
                    // `wm --gallery`: every ported omarchy surface with
                    // fixture data, over the desktop (see shell/gallery.rs).
                    // `wm --gallery` fills this at startup (handle_startup):
                    // the gallery is a deep fixture tree nobody else pays for
                    // — on the web its construction alone overflows the 1 MiB
                    // wasm stack.
                    gallery_holder := View{
                        width: Fill
                        height: Fill
                        visible: false
                        shell_gallery_host := ShellGalleryHost{}
                    }
                    }
                    bar := SolidView{
                        width: Fill
                        height: 26
                        flow: Overlay
                        draw_bg +: {
                            color: mod.wm_theme.background
                        }
                        desktop_controls := View{
                            width: Fill height: Fill
                            shell_bar := ShellBar{width: Fill height: Fill}
                        }
                        phone_controls := PhoneSurface{visible: false}
                    }
                }
            }
        }
    }
}

// ======================================================================
// App
// ======================================================================

/// Bar height when the platform reports no window-button rect (Linux,
/// Windows, and macOS before the first geometry event).
const BAR_HEIGHT_FALLBACK: f64 = 26.0;

/// The theme crossfade's clock: frame deltas capped at 50 ms (a stall does
/// not jump the fade), a first frame of 0, and the tween ticker's pause and
/// time scale (a pause holds the fade; the draw after the resume re-arms it).
const STYLE_CLOCK: ClockPolicy = ClockPolicy {
    lag: Some(LagSmoothing::clamp(0.05)),
    first_dt: 0.0,
    follow_ticker: true,
};

/// The bar's height and the left padding its content starts at. macOS puts
/// its traffic lights on the left; Linux and Windows put caption buttons on
/// the right, where they must not push the left cluster off screen.
type BarMetrics = (f64, f64);

fn bar_metrics_for_geom(geom: &WindowGeom) -> BarMetrics {
    let buttons = geom.window_chrome_buttons;
    if buttons.size.y <= 0.0 {
        return (BAR_HEIGHT_FALLBACK, 84.0);
    }
    let height = (buttons.pos.y * 2.0 + buttons.size.y).ceil();
    let buttons_are_on_left = buttons.pos.x + buttons.size.x <= geom.inner_size.x * 0.5;
    let pad_left = if buttons_are_on_left {
        buttons.pos.x + buttons.size.x + 12.0
    } else {
        8.0
    };
    (height, pad_left)
}

#[cfg(test)]
mod bar_chrome_tests {
    use super::*;

    #[test]
    fn left_caption_buttons_move_the_left_cluster_past_them() {
        let geom = WindowGeom {
            inner_size: dvec2(1000.0, 700.0),
            window_chrome_buttons: Rect {
                pos: dvec2(12.0, 9.0),
                size: dvec2(72.0, 24.0),
            },
            ..Default::default()
        };
        assert_eq!(bar_metrics_for_geom(&geom), (42.0, 96.0));
    }

    #[test]
    fn right_caption_buttons_do_not_push_the_left_cluster_off_screen() {
        let geom = WindowGeom {
            inner_size: dvec2(917.0, 1030.0),
            window_chrome_buttons: Rect {
                pos: dvec2(779.0, 0.0),
                size: dvec2(138.0, 29.0),
            },
            ..Default::default()
        };
        assert_eq!(bar_metrics_for_geom(&geom), (29.0, 8.0));
    }
}

// ----------------------------------------------------------------------
// The Wi-Fi dropdown's WM side (Linux): the iwd worker's link, its events
// into the panel, the panel's intents into worker commands, and the
// keyboard grab of its password prompt. See shell/wifi_linux.rs.
// ----------------------------------------------------------------------

#[cfg(target_os = "linux")]
impl App {
    /// The worker, started on demand. False when it could not start (the
    /// dropdown then shows why).
    fn wifi_link(&mut self, cx: &mut Cx) -> bool {
        use shell::wifi_linux::{WifiEvent, WifiLink, WifiSnapshot};
        if self.wifi.is_some() {
            return true;
        }
        match WifiLink::start(&cx.thread_spawner()) {
            Ok(link) => {
                self.wifi = Some(link);
                true
            }
            Err(error) => {
                log!("wm: wifi: could not start the iwd worker: {error}");
                let panel = self.ui.widget(cx, ids!(shell_panel));
                if let Some(mut p) = panel.borrow_mut::<shell::panels::ShellPanel>() {
                    p.wifi.apply(WifiEvent::Snapshot(WifiSnapshot {
                        unavailable: Some(format!("Wi-Fi worker did not start: {error}")),
                        ..Default::default()
                    }));
                }
                false
            }
        }
    }

    /// Non-blocking; a full channel keeps the command waiting in order and
    /// the tick pushes it through.
    fn wifi_send(&mut self, cx: &mut Cx, cmd: shell::wifi_linux::WorkerCommand) {
        if !self.wifi.as_mut().is_some_and(|link| link.send(cmd)) {
            let panel = self.ui.widget(cx, ids!(shell_panel));
            if let Some(mut panel) = panel.borrow_mut::<shell::panels::ShellPanel>() {
                panel.wifi.apply(shell::wifi_linux::WifiEvent::Notice {
                    text: "Wi-Fi is busy or unavailable; please try again".into(),
                    error: true,
                });
                panel.redraw(cx);
            };
        }
    }

    /// The Network flyout just opened: fresh state and a scan.
    fn wifi_panel_opened(&mut self, cx: &mut Cx) {
        use shell::wifi_linux::WorkerCommand;
        if !self.wifi_link(cx) {
            return;
        }
        self.wifi_send(cx, WorkerCommand::Refresh);
        self.wifi_send(cx, WorkerCommand::Scan);
        self.wifi_addr_tick = 0;
        let panel = self.ui.widget(cx, ids!(shell_panel));
        if let Some(mut p) = panel.borrow_mut::<shell::panels::ShellPanel>() {
            p.wifi.scan_requested_at = Some(host::now());
        };
    }

    /// A dropdown intent into a worker command. `ConnectWithPassphrase` is
    /// completed here from the panel's own field: the passphrase moves,
    /// it is never copied into an action.
    fn wifi_command(&mut self, cx: &mut Cx, cmd: shell::wifi_linux::WifiCommand) {
        use shell::wifi_linux::{WifiCommand, WorkerCommand};
        if !self.wifi_link(cx) {
            return;
        }
        let command = match cmd {
            WifiCommand::Scan => WorkerCommand::Scan,
            WifiCommand::Refresh => WorkerCommand::Refresh,
            WifiCommand::RefreshAddresses => WorkerCommand::RefreshAddresses,
            WifiCommand::Connect { path } => WorkerCommand::Connect { path, secret: None },
            WifiCommand::ConnectWithPassphrase { path } => {
                let panel = self.ui.widget(cx, ids!(shell_panel));
                let secret = panel
                    .borrow_mut::<shell::panels::ShellPanel>()
                    .and_then(|mut p| p.wifi.take_secret(&path));
                match secret {
                    Some(secret) => WorkerCommand::Connect {
                        path,
                        secret: Some(secret),
                    },
                    None => return,
                }
            }
            WifiCommand::Disconnect => WorkerCommand::Disconnect,
            WifiCommand::Forget { known } => WorkerCommand::Forget { known },
            WifiCommand::SetPowered(on) => WorkerCommand::SetPowered(on),
            WifiCommand::CancelConnect => WorkerCommand::CancelConnect,
            WifiCommand::HotspotSet(on) => WorkerCommand::HotspotSet(on),
            WifiCommand::HotspotRegenerate => WorkerCommand::HotspotRegenerate,
        };
        self.wifi_send(cx, command);
    }

    /// The worker's events into the panel (`try_recv`, never a wait). A
    /// worker that died reports itself as unavailable and is let go, so
    /// the next open starts a fresh one.
    fn drain_wifi(&mut self, cx: &mut Cx) {
        use shell::wifi_linux::{WifiEvent, WifiSnapshot};
        let Some(link) = self.wifi.as_mut() else {
            return;
        };
        let mut events = Vec::new();
        while let Some(ev) = link.try_recv() {
            events.push(ev);
        }
        if let Some(reason) = link.worker_stopped() {
            log!("wm: wifi: {reason}");
            events.push(WifiEvent::Snapshot(WifiSnapshot {
                unavailable: Some(reason),
                ..Default::default()
            }));
            self.wifi = None;
        }
        if events.is_empty() {
            return;
        }
        let panel = self.ui.widget(cx, ids!(shell_panel));
        {
            let mut borrowed = panel.borrow_mut::<shell::panels::ShellPanel>();
            if let Some(p) = borrowed.as_mut() {
                for ev in events {
                    p.wifi.apply(ev);
                }
                if p.open == Some(shell::panels::PanelKind::Network) {
                    p.redraw(cx);
                }
            }
        }
        // Replies drained means the worker is moving: push what waited.
        if let Some(link) = self.wifi.as_mut() {
            link.retry();
        }
    }

    /// The 1s tick: drain, push waiting commands, and while the dropdown
    /// is up re-read the addresses every other second (DHCP lands after
    /// iwd says "connected") and expire a scan reading iwd never confirmed.
    fn wifi_tick(&mut self, cx: &mut Cx) {
        use shell::wifi_linux::WorkerCommand;
        self.drain_wifi(cx);
        let Some(link) = self.wifi.as_mut() else {
            return;
        };
        link.retry();
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let mut open = false;
        let mut changed = false;
        {
            let mut borrowed = panel.borrow_mut::<shell::panels::ShellPanel>();
            if let Some(p) = borrowed.as_mut() {
                open = p.open == Some(shell::panels::PanelKind::Network);
                if open && p.wifi.tick(host::now()) {
                    changed = true;
                }
            }
        }
        if open {
            self.wifi_addr_tick = self.wifi_addr_tick.wrapping_add(1);
            if self.wifi_addr_tick % 2 == 0 {
                self.wifi_send(cx, WorkerCommand::RefreshAddresses);
            }
        }
        if changed {
            self.redraw_all(cx);
        }
    }

    fn wifi_shutdown(&mut self) {
        // Dropping the link closes the command channel; the worker
        // unregisters its agent and closes the bus on its own thread.
        self.wifi = None;
    }

    /// The flyout's keyboard grab, ahead of the WM binds, the AI pane and
    /// the tiles: while the password prompt is up, every key (down and up),
    /// the typed text and the clipboard/IME queries are its own; with no
    /// prompt up, Escape closes the Wi-Fi dropdown or, while the Display
    /// panel's scale slider is held, drops that preview (and its key-up
    /// goes with it, so nothing underneath sees half a press).
    fn shell_panel_secret_input(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let mut consumed = false;
        {
            let mut borrowed = panel.borrow_mut::<shell::panels::ShellPanel>();
            let Some(p) = borrowed.as_mut() else {
                return false;
            };
            if !p.wants_keys() {
                return false;
            }
            match event {
                Event::KeyDown(e) => consumed = p.key(cx, e),
                Event::KeyUp(e) => consumed = p.key_up(e),
                Event::TextInput(e) => consumed = p.wifi_text_input(cx, &e.input),
                Event::TextRangeReplace(_)
                | Event::TextCopy(_)
                | Event::TextCut(_)
                | Event::ImeAction(_) => consumed = p.secret_input_active(),
                _ => {}
            }
        }
        if consumed {
            self.redraw_all(cx);
        }
        consumed
    }
}

#[cfg(not(target_os = "linux"))]
impl App {
    fn wifi_panel_opened(&mut self, _cx: &mut Cx) {}
    fn drain_wifi(&mut self, _cx: &mut Cx) {}
    fn wifi_tick(&mut self, _cx: &mut Cx) {}
    fn wifi_shutdown(&mut self) {}
    fn shell_panel_secret_input(&mut self, _cx: &mut Cx, _event: &Event) -> bool {
        false
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: Option<WmState>,
    #[rust]
    snap_hover_timer: Timer,
    /// The wake-up for the dock auto-hide's running delay.
    #[rust]
    dock_autohide_timer: Timer,
    /// The wake-up for the shelves' auto-hide delays, and the pointer they saw last.
    #[rust]
    shelf_autohide_timer: Timer,
    /// SUPER + scroll: one desktop switch per gesture.
    #[rust]
    space_scroll: ScrollGesture,
    /// The screenshot's frame clock, its last tick, the pointer it saw last and the screen
    /// the thumbnail floats on (screenshot_app.rs).
    #[rust]
    shot_next: NextFrame,
    #[rust]
    shot_last: f64,
    #[rust]
    shot_pointer: Vec2d,
    #[rust]
    shot_thumb_screen: Rect,
    /// The AI hub the top-bar panel controls.
    #[rust]
    hub_link: HubLink,
    /// The steps the WM takes when the GPU is short of memory for the UI (`gpu_yield`).
    #[rust]
    yield_policy: YieldPolicy,
    #[rust]
    shelf_pointer: Vec2d,
    /// The last pointer position the dock auto-hide saw.
    #[rust]
    dock_pointer: Vec2d,
    /// A keyboard focus that could not land yet (the tile hadn't drawn);
    /// re-asserted when that client's first frame arrives.
    #[rust]
    pending_focus: Option<ClientId>,
    /// `--gallery`: the shell-surface gallery instead of a desktop.
    #[rust]
    gallery: bool,
    /// What the WM does for hosted children that have no OS window or JVM
    /// of their own (HTTP with TLS, the clipboard menu, URLs, permission
    /// prompts, file pickers): platform hosted_relay.rs.
    #[rust]
    relay: HostRelayServer,
    /// What the shell bar's status modules show, sampled on the tick.
    #[rust]
    bar_sample: BarData,
    #[rust]
    status_rx: Option<std::sync::mpsc::Receiver<SampledStatus>>,
    #[rust]
    status_worker: Option<TaskHandle<()>>,
    #[rust]
    status_sample: SampledStatus,
    #[rust]
    status_tick: u32,
    /// The clock's alternate format (right-click cycles it, `formatAlt`).
    #[rust]
    clock_alt: bool,
    /// The theme's wallpapers are being fetched (a fresh install ships the
    /// colours, not the pictures); the tick applies the first one to land.
    #[rust]
    backgrounds_pending: bool,
    #[rust]
    background_task: Option<TaskHandle<usize>>,
    /// Which bar module's flyout is open, for the accent pill.
    #[rust]
    shell_panel_open: Option<BarModule>,
    /// The bar segment that flyout was opened from: its pill and anchor.
    #[rust]
    shell_panel_segment: usize,
    /// The bar segment the last press landed in: the style menu anchors
    /// to that segment's switch.
    #[rust]
    bar_press_segment: usize,
    /// The screens the menu, the OSD and the notification stack were last
    /// opened on (the active one at the time): each draws there on a
    /// multi-screen desktop (`sync_surface_targets`).
    #[rust]
    menu_screen: usize,
    #[rust]
    osd_screen: usize,
    #[rust]
    notes_screen: usize,
    /// The Wi-Fi dropdown's iwd link (Linux, shell/wifi_linux.rs): started
    /// when the dropdown first opens, alive for the session, dropped at
    /// shutdown.
    #[cfg(target_os = "linux")]
    #[rust]
    wifi: Option<shell::wifi_linux::WifiLink>,
    /// The WM's xdg-desktop-portal backend (portal.rs): the appearance for browsers, and
    /// the interfaces the UI answers. None off Linux, without a session bus, or with
    /// MAKEPAD_WM_PORTAL=0.
    #[cfg(target_os = "linux")]
    #[rust]
    portal: Option<portal::Portal>,
    /// Ticks the Wi-Fi dropdown has been open, for its address re-reads.
    #[cfg(target_os = "linux")]
    #[rust]
    wifi_addr_tick: u32,
    #[cfg(target_os = "linux")]
    #[rust]
    linux_controls: LinuxControls,
    #[rust]
    hub: Option<AppHub>,
    /// The WM socket apps started elsewhere in the session find it by (adopt.rs).
    #[cfg(target_os = "linux")]
    #[rust]
    adoptions: Option<adopt::Adoptions>,
    #[rust]
    next_id: ClientId,
    /// The hosted apps' later windows, each a window of its own (`client_windows.rs`).
    #[rust]
    client_windows: ClientWindows,
    #[rust]
    tick: Timer,
    /// Module instances whose root panicked, with when: their tiles show
    /// "crashed" for [`CRASH_HOLD_SECS`] before the WM closes them, so the
    /// person sees what happened instead of an app that just vanished.
    #[rust]
    crashed_holds: Vec<(ClientId, f64)>,
    #[cfg(not(target_arch = "wasm32"))]
    #[rust]
    domains: Domains,
    /// Bar height + left padding last applied from the OS window buttons.
    #[rust]
    bar_metrics: Option<BarMetrics>,
    /// SUPER+mouse:272/273 move & resize (tiling.lua).
    #[rust]
    drag: Option<DragState>,
    /// A divider drag in flight: a plain press IN THE GAP between two
    /// tiles moves that split (see `DividerDrag`).
    #[rust]
    div_drag: Option<DividerDrag>,
    /// The axis of the divider band the pointer is hovering, so the resize
    /// cursor is set once on the way in and cleared once on the way out.
    #[rust]
    div_hover: Option<Axis>,
    /// The one-shot SUPER+ALT layer prefix (see binds.rs).
    #[rust]
    alt_armed: bool,
    #[rust]
    vr_pointer_capture: Option<Rect>,
    /// How many times the shell menu has opened: its open line carries the
    /// number, so a script can wait for the one it asked for.
    #[rust]
    shell_menu_opens: u64,
    /// The bar was hidden because a window went fullscreen, not by
    /// SUPER+SHIFT+SPACE — so it comes back on its own.
    #[rust]
    bar_hidden_by_fullscreen: bool,
    /// Where the bar was last placed (`sync_bar_placement`): a strip above
    /// the desk, or floating over the scene.
    #[rust]
    bar_placed: Option<shell::bar::BarPlacement>,
    /// Output lines from every child, for the tile's status line.
    #[rust]
    client_lines: Option<ClientLines>,
    /// The workspaces each bar segment's cluster is currently showing,
    /// left to right — what a click on it maps to (one entry on a
    /// one-segment bar).
    #[rust]
    bar_workspaces: Vec<Vec<usize>>,
    /// Quick Look's warm-viewer cache (see `preview::PreviewCache`).
    #[rust]
    preview_cache: PreviewCache,
    /// The warm-instance pool: which standby processes exist, per app.
    #[rust]
    warm_pool: WarmPool,
    /// The off-desk framebuffer each warm instance draws into before it has
    /// a tile (see `WarmFrame`).
    #[rust]
    warm_frames: HashMap<ClientId, WarmFrame>,
    /// Drives the dormant instances (see `pump_warm`).
    #[rust]
    warm_tick: Timer,
    /// What the bar last showed (`update_bar`): it redraws only when that
    /// changes, not on every second's tick — an idle phone repainted once a
    /// second for a clock that shows minutes.
    #[rust]
    bar_shown: String,
    /// When each warm client was last ticked. Kept apart from `WarmFrame`
    /// because the FIRST ticks are what make a frame possible at all — see
    /// `pump_warm`.
    #[rust]
    warm_last_tick: HashMap<ClientId, f64>,
    /// The desktop window's dpi factor, as the platform last reported it —
    /// a warm instance is configured with the same one its tile will use,
    /// so the glyph atlas and shaders it warms up are the right ones.
    #[rust]
    dpi_factor: f64,
    /// The AI services bus: which client is the pane, every client's last
    /// registration, the routing (see ai_bus.rs).
    #[rust]
    ai_bus: AiBus,
    /// Which of the theme's wallpapers is up (SUPER+CTRL+SPACE cycles).
    #[rust]
    background_index: usize,
    /// The instances this process hosts itself (module_host.rs): one
    /// isolate each, seated in a `ModuleTile`.
    #[rust]
    module_host: ModuleHost,
    /// The in-process transport to an assistant seated in the pane as a
    /// module: the WM's services as links its registry adopts (pane_links.rs).
    #[rust]
    pane_links: PaneLinks,
    /// The WM's own test service `wm` (motion_trace): opened only in an instance
    /// launched for an agent or a test (`--remote`), for apps/wm/tools/frametrace.
    #[rust]
    wm_port: Option<makepad_ai_services::port::AiServicePort>,
    /// Which apps are linked as modules and which the person switched to
    /// module hosting (apps.rs).
    #[rust]
    apps: AppRegistry,
    /// What the binary decided this window manager is (build.rs): read
    /// from `Cx` when the app is made, before startup.
    #[rust]
    build: WmBuild,
    /// Drives the theme crossfade (`WmState::style`): its own frames, deltas
    /// capped at 50 ms, the first frame after a switch moving 0.
    #[rust(TweenClock::new(STYLE_CLOCK))]
    style_clock: TweenClock,
    #[rust] title_press: Option<(ClientId, f64, Vec2d)>,
    #[rust] stylesheet: Option<desktop_style::StyleSheet>,
    #[rust] phone_frame: NextFrame,
    #[rust] phone_time: f64,
    /// The storage read of the home page's order, answered in `Event::Storage`.
    #[rust] home_order_request: Option<StorageRequestId>,
    #[rust] shell_look_request: Option<StorageRequestId>,
    /// The storage read of the desktop behaviour (the web's way of restoring it).
    #[rust] desktop_mode_request: Option<StorageRequestId>,
}

/// A warm instance's own swapchain: the host end of the frames a DORMANT
/// client draws while it has no tile.
///
/// This is what makes the pool worth more than a running process. Handed a
/// geometry and a framebuffer, the child does its FIRST full draw pass
/// while nobody is waiting — compiling its shaders, rasterizing its glyph
/// atlas, laying out its UI — so adoption costs one geometry message and a
/// repaint by an app that is already warm all the way to the GPU.
pub struct WarmFrame {
    /// `None` on Linux, where sharing a framebuffer needs the aux-chan
    /// socket a TILE owns; a warm instance there is process-warm only and
    /// gets its framebuffer at adoption.
    swapchain: Option<HostSwapchain>,
    /// The instance drew at least once: it is warm all the way through,
    /// and drops to the heartbeat tick rate.
    presented: bool,
    /// Set at adoption. The tile has its own swapchain from that moment,
    /// but the child may still have a frame in flight against this one, so
    /// it is held briefly before being dropped — the same hazard
    /// `MpRunView::last_swapchain_with_completed_draws` covers on resize.
    retired: Option<f64>,
}

/// How long a retired warm swapchain is kept after adoption.
const WARM_FRAME_RETIRE: f64 = 2.0;

/// The pump interval: fast enough that a warm instance reaches its first
/// frame promptly, slow enough to be nothing on the WM's own clock.
const WARM_PUMP: f64 = 0.05;

/// What a DORMANT instance gets once it has drawn: enough to keep its
/// watchdog and any app-side timer alive, little enough to be free.
const WARM_HEARTBEAT: f64 = 1.0;

/// The channel every client's reader thread writes its output into.
pub struct ClientLines {
    tx: std::sync::mpsc::Sender<ClientLine>,
    rx: std::sync::mpsc::Receiver<ClientLine>,
}

/// A SUPER+drag in flight (tiling.lua: mouse:272 moves, mouse:273 resizes).
pub struct DragState {
    client: ClientId,
    /// Right button: resize instead of move.
    resize: bool,
    resize_x: bool,
    resize_y: bool,
    /// The window was floating when the drag began.
    floating: bool,
    start: Vec2d,
    last: Vec2d,
    start_rect: LRect,
    /// Which corner the grab belongs to, by the QUADRANT of the grab point
    /// around the window center (DragController.cpp:213-226). The opposite
    /// corner stays put while the drag moves these two edges.
    grab_left: bool,
    grab_top: bool,
    /// `binds:drag_threshold` — a press is not a drag until it moves.
    armed: bool,
    /// SHIFT as the pointer last reported it. A drop with SHIFT held makes
    /// the dragged window a TAB of the tile under the pointer instead of
    /// swapping the two, so the flag has to be read from the drag's own
    /// events, not from the keyboard state at some later moment.
    shift: bool,
    /// The screen whose layout holds the window: the drag moves and
    /// resizes it there, whatever screen the pointer crosses.
    screen: usize,
    /// When the press happened (a quick press is judged by a wider slop).
    pressed: std::time::Instant,
}

/// Hyprland's `binds:drag_threshold` (DragController.cpp:346).
const DRAG_THRESHOLD: f64 = 4.0;

/// A divider drag in flight: pressing IN THE GAP between two tiles grabs
/// the split that draws that gap and moves it with the pointer.
///
/// This is Hyprland's `resize_on_border` gesture scoped to the gap. On the
/// border it competes with every click near a window edge, which is why
/// omarchy ships `resize_on_border = false` (`default/hypr/looknfeel.lua`);
/// in the gap there is nothing to compete with, so it needs no modifier
/// and leaves the SUPER paths untouched — SUPER+drag still moves/swaps a
/// window, SUPER+right-drag still does the quadrant resize.
pub struct DividerDrag {
    /// The split, its box and its ratio AT THE GRAB — the move is measured
    /// from here every frame, never accumulated, so the divider stays
    /// exactly under the pointer.
    hit: DividerHit,
    start: Vec2d,
    /// The screen whose layout owns the split.
    screen: usize,
}

impl App {
    fn desk(&self, cx: &mut Cx) -> WidgetRef {
        self.ui.widget(cx, ids!(desk))
    }

    fn domains_enabled(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        { return self.domains.enabled; }
        #[cfg(target_arch = "wasm32")]
        { false }
    }

    /// Whether THIS window manager hosts apps as child processes: the
    /// platform can (`host::processes_available`) and the build wants to
    /// (not `modules_only`). Everything a process needs — the hub, the
    /// pool, cargo launches, wallpaper fetches — asks here.
    fn processes(&self) -> bool {
        host::processes_available() && !self.build.modules_only
    }

    fn theme_name_from_env() -> String {
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            if arg == "--theme" {
                if let Some(name) = args.next() {
                    return name;
                }
            }
        }
        std::env::var("MAKEPAD_WM_THEME").unwrap_or_else(|_| {
            // Last chosen theme, omarchy-style state file.
            std::fs::read_to_string(theme::themes_dir().join("../current-theme"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| theme::DEFAULT_THEME.to_string())
        })
    }

    fn state_mut(&mut self) -> &mut WmState {
        self.state.as_mut().expect("state after startup")
    }

    /// The active screen's tiling area: where new windows, previews and
    /// the keyboard's layout actions go.
    fn desk_area(&self, cx: &mut Cx) -> LRect {
        let active = self.state.as_ref().map_or(0, |s| s.screens.active);
        self.screen_area(cx, active)
    }

    /// Screen `i`'s tiling area on the multi-screen desktop (the dock only
    /// reserved on the main screen); otherwise the one desk's area. A
    /// mobile style lays out across the whole desk, as today.
    fn screen_area(&self, cx: &mut Cx, i: usize) -> LRect {
        if let Some(s) = self
            .state
            .as_ref()
            .filter(|s| s.per_screen() && !s.style.target.mobile() && i < s.screens.live_count())
        {
            return s.screens.area(i, s.style.reserved_height(), s.gaps_out);
        }
        self.single_desk_area(cx)
    }

    fn single_desk_area(&self, cx: &mut Cx) -> LRect {
        let desk = self.desk(cx);
        let rect = desk
            .borrow_mut::<WmDesk>()
            .map(|d| d.desk_rect)
            .unwrap_or_default();
        let gap = self
            .state
            .as_ref()
            .map(|s| s.gaps_out)
            .unwrap_or(desk::GAPS_OUT);
        // Before the first draw the desk has no rect yet; fall back to the
        // window's startup proportions so the first splits pick the right
        // axis (A | B side-by-side on a wide screen).
        let (w, h) = if rect.size.x > 1.0 {
            (rect.size.x, rect.size.y)
        } else {
            (1400.0, 860.0)
        };
        LRect::new(
            rect.pos.x + gap,
            rect.pos.y + gap,
            (w - gap * 2.0).max(1.0),
            (h - gap * 2.0 - self.state.as_ref().map(|s| s.style.reserved_height()).unwrap_or(0.0)).max(1.0),
        )
    }

    // --------------------------------------------------------------
    // Client lifecycle
    // --------------------------------------------------------------

    /// The oldest window of the app `app_id` (by its class or title), for a
    /// launch-or-focus app: `omarchy-launch-or-focus` matches `\b<pattern>\b`
    /// case-insensitively against the window's CLASS OR TITLE and focuses the
    /// first hit.
    fn running_client(&mut self, app_id: &str) -> Option<ClientId> {
        let mut existing: Vec<ClientId> = self
            .state_mut()
            .clients
            .iter()
            .filter(|(_, slot)| {
                // A warm instance is NOT a window: matching one here
                // would "focus" something invisible and the app would
                // never open at all. Nor is the AI pane's child.
                !slot.warm
                    && !slot.pane
                    && !slot.is_preview
                    && slot.closing.is_none()
                    && (clients::word_match(&slot.app, app_id) || clients::word_match(&slot.title, app_id))
            })
            .map(|(id, _)| *id)
            .collect();
        // `head -n1` over hyprctl's client list: take the oldest.
        existing.sort_unstable();
        existing.first().copied()
    }

    fn launch_app(&mut self, cx: &mut Cx, app_id: &str) {
        if (!self.domains_enabled() || app_id.starts_with("wl.") || app_id.starts_with("wayland="))
            && self.wayland_launch(cx, app_id) {
            return;
        }
        let Some(app) = crate::clients::find_app(app_id) else {
            log!("wm: no app '{}' in the registry", app_id);
            return;
        };
        let app = &app;
        let hub_port = self.state_mut().hub_port;

        // launch-or-focus for non-terminal apps: `omarchy-launch-or-focus`
        // matches `\b<pattern>\b` case-insensitively against the window's
        // CLASS OR TITLE and focuses the first hit.
        if app.policy == LaunchPolicy::OrFocus {
            if let Some(client) = self.running_client(&app.id) {
                #[cfg(not(target_arch = "wasm32"))]
                if self.domains.ended(client) { self.relaunch_domain(cx, client); }
                self.activate_client(cx, client);
                return;
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.enabled {
            self.launch_domain(cx, app);
            return;
        }

        // In-process hosting (aicontrol §3): a linked module the person
        // switched on (or a dev `--module` run) becomes an instance in an
        // isolate of its own — never a process, never the pool.
        if self.apps.hosting(app_id) == Hosting::Module {
            if let Some(module) = self.apps.module(app_id) {
                self.launch_module(cx, module);
                return;
            }
        }

        // From here it is a PROCESS: nothing a build without a process
        // host can do (the web, iOS and the all-in-one host their linked
        // modules and nothing else).
        if !self.processes() {
            log!("wm: {} is not linked into this build; it cannot start here", app_id);
            return;
        }

        // THE POOL: a standby instance of this app becomes the window now,
        // already drawn, and the pool tops itself back up behind it.
        if self.adopt_warm(cx, app_id) {
            return;
        }

        let id = self.next_id;
        self.next_id += 1;
        let cwd = self.launch_cwd(app_id, true);
        let state = self.state_mut();
        let term_env = state.term_env.clone();
        let lines = self.line_sender();
        let pool = cx.task_pool();
        let state = self.state_mut();
        // Opening an app builds it when it is missing or out of date: its
        // tile shows the build, then the app.
        let policy = if app.is_current() { BuildPolicy::IfMissing } else { BuildPolicy::Rebuild };
        let spawned = spawn_client(
            &pool, &cx.thread_spawner(), app, id, hub_port, cwd.as_ref(),
            (app.id == "terminal").then_some(term_env.as_str()), &[], false, policy, lines,
        );
        match spawned {
            Ok(slot) => {
                let verb = if slot.is_building() { "building" } else { "launched" };
                log!("wm: {} {} as client {}", verb, app.id, id);
                state.clients.insert(id, slot);
                let area = self.desk_area(cx);
                let state = self.state_mut();
                let gap = state.gap;
                state.layout_mut().insert(id, area, gap);
                let hub_port = state.hub_port;
                self.desk(cx)
                    .borrow_mut::<WmDesk>()
                    .map(|mut d| d.with_run_view(cx, id, |cx, v| v.set_run_target(cx, id, 0, hub_port)));
                self.activate_client(cx, id);
                self.redraw_all(cx);
            }
            Err(err) => {
                log!("wm: launch {} failed: {}", app.id, err);
            }
        }
    }

    // --------------------------------------------------------------
    // The warm pool
    //
    // A new terminal or browser should APPEAR, not launch. One dormant
    // instance of each pooled app (two for terminals) is already running,
    // connected and drawn into a framebuffer of its own; opening a window
    // hands it a tile and wakes it up, and the pool immediately starts its
    // replacement for the next time.
    //
    // The mechanisms are Quick Look's, not new ones: a client that is not
    // in the LAYOUT has no tile, is not drawn, and is invisible to
    // everything the desk enumerates (alt-tab, workspace counts, the bar's
    // active window) — exactly how a hidden warm viewer waits between
    // panels — plus `takes_focus: false`, which is what keeps a preview
    // from stealing the keyboard, and the same `spawn_client` path with
    // the same cargo, env and per-client log.
    // --------------------------------------------------------------

    /// Where a new window of `app_id` opens.
    ///
    /// `inherit` is omarchy's terminal rule (`omarchy-cmd-terminal-cwd`):
    /// a new terminal opens in the FOCUSED terminal's directory. A warm
    /// instance passes `false` — its shell started long before the launch
    /// and cannot be moved after the fact — which is exactly why the pool
    /// stands aside whenever there is a cwd to inherit.
    fn launch_cwd(&mut self, app_id: &str, inherit: bool) -> Option<std::path::PathBuf> {
        if app_id != "terminal" {
            return None;
        }
        let focused = if inherit {
            self.state_mut().focused_terminal_cwd()
        } else {
            None
        };
        focused.or_else(|| {
            // In demo mode a terminal with nothing to inherit opens inside
            // the generated demo home so `ls` shows plausible content,
            // never the user's real files.
            if std::env::var("MAKEPAD_WM_FILES_REAL").is_err() {
                crate::demo_home::ensure_demo_home()
            } else {
                None
            }
        })
    }

    /// The pool's half of a launch. True when a standby instance became
    /// the window (the caller is done); false to launch cold exactly as
    /// before — the pool is an optimization and never the only path.
    fn adopt_warm(&mut self, cx: &mut Cx, app_id: &str) -> bool {
        if !WarmPool::is_warm_app(app_id) {
            return false;
        }
        // THE CWD CARVE-OUT: correctness beats instant. A new terminal
        // opened from a terminal must land in that terminal's directory,
        // and no warm shell can claim to have started there.
        let cwd_override = app_id == "terminal" && self.state_mut().focused_terminal_cwd().is_some();
        let status: Vec<WarmStatus> = self
            .state_mut()
            .clients
            .iter()
            .map(|(id, slot)| WarmStatus {
                client: *id,
                alive: true,
                // Connected AND past CreateWindow: it has a window and a
                // frame, so a tile can show it this instant.
                connected: slot.sender.is_some() && slot.ready && slot.closing.is_none(),
            })
            .collect();
        let Some(client) = self.warm_pool.adopt(app_id, cwd_override, &status) else {
            if cwd_override {
                log!("wm: warm {} skipped — new terminal inherits a cwd", app_id);
            }
            return false;
        };

        // It is a real window from here: out of the pool, into the layout
        // at the ordinary insertion point, focused like any launch.
        let area = self.desk_area(cx);
        let window_id = self
            .state_mut()
            .clients
            .get(&client)
            .map(|s| s.window_id)
            .unwrap_or(0);
        {
            let state = self.state_mut();
            if let Some(slot) = state.clients.get_mut(&client) {
                slot.warm = false;
                slot.takes_focus = true;
                slot.open_at = Some(host::now());
                slot.opened_warm = true;
            }
            let gap = state.gap;
            state.layout_mut().insert(client, area, gap);
        }
        let hub_port = self.state_mut().hub_port;
        // The tile configures the child to its REAL rect on its next tick
        // (one WindowGeomChange plus the tile's own swapchain) — the same
        // bootstrap every launched client gets, except this one is already
        // running, so the arrival crossfade is all the user sees.
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.with_run_view(cx, client, |cx, v| {
                v.set_run_target(cx, client, window_id, hub_port);
                v.app_ready(cx, client, window_id);
            })
        });
        self.activate_client(cx, client);
        // Wake it: samplers, refresh timers, everything a dormant instance
        // was told to hold back (`makepad_wm_api::warm_start`).
        self.send_wm_event(client, WmEvent::Adopted);
        // …and where the screens are, once its tile is laid out.
        self.resend_screens_event(client);
        // The tile owns the framebuffer now; the warm one is held briefly
        // in case a frame is still in flight against it.
        if let Some(frame) = self.warm_frames.get_mut(&client) {
            frame.retired = Some(host::now());
        }
        log!("wm: adopted warm {} as client {}", app_id, client);
        self.redraw_all(cx);
        // …and the next one starts warming immediately.
        self.spawn_warm(cx, app_id);
        true
    }

    /// Put one dormant instance of `app_id` in the pool. Quiet by design:
    /// the pool is invisible infrastructure, and anything it cannot do
    /// simply means the next launch of that app is a cold one.
    fn spawn_warm(&mut self, cx: &mut Cx, app_id: &str) {
        if !self.warm_pool.wants(app_id, host::now()) {
            return;
        }
        // Let retired browsers release their profile lock before a replacement
        // starts. This also coalesces quick light/dark toggles into one launch
        // with the latest child environment.
        if app_id == "browser" && self.state_mut().clients.values().any(|slot|
            slot.app == "browser" && slot.warm && slot.closing.is_some()) {
            return;
        }
        let Some(app) = crate::clients::find_app(app_id) else {
            return;
        };
        // Warming never compiles: an app that is not built, or whose
        // binary is out of date, stays cold until the person opens it.
        if !self.warm_pool.warmable(&app) {
            return;
        }
        let hub_port = self.state_mut().hub_port;
        if hub_port == 0 {
            // No hub: a child could never connect, so warming is pointless.
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let cwd = self.launch_cwd(app_id, false);
        let term_env = self.state_mut().term_env.clone();
        let lines = self.line_sender();
        let pool = cx.task_pool();
        match spawn_client(
            &pool,
            &cx.thread_spawner(),
            &app,
            id,
            hub_port,
            cwd.as_ref(),
            (app.id == "terminal").then_some(term_env.as_str()),
            &[],
            true,
            BuildPolicy::IfMissing,
            lines,
        ) {
            Ok(slot) => {
                self.state_mut().clients.insert(id, slot);
                self.warm_pool.note_spawned(app_id, id);
                log!("wm: warming {} as client {}", app_id, id);
            }
            Err(err) => {
                // A spawn that cannot even start (an installed layout without
                // this binary) counts as a crash: the budget gives the app up
                // after a few, instead of retrying it on every tick forever.
                self.warm_pool.note_crash(app_id, host::now());
                log!("wm: warming {} failed: {}", app_id, err);
            }
        }
    }

    /// Top the pool up, ONE spawn per tick: a cold desktop otherwise forks
    /// four cargo builds into the same target-dir lock at once, and they
    /// would only queue behind each other anyway.
    fn top_up_warm_pool(&mut self, cx: &mut Cx) {
        // A phone keeps no desktop warm pool: each child is a whole process
        // (~190 MB), the desktop apps it warms (terminal, browser, task) are
        // not in the APK, and the phone shell starts its own tile clients.
        if cfg!(target_os = "android") {
            return;
        }
        let pool = &self.warm_pool;
        let browser_replaced = wayland_host::menu_apps().iter().any(|(id, _)| id == "wl.chromium");
        let built = |app: &str| !(browser_replaced && app == "browser") && crate::clients::find_app(app).is_some_and(|app| app.is_built() && !pool.known_stale(&app));
        let Some(app) = self.warm_pool.next_missing(host::now(), built) else {
            return;
        };
        self.spawn_warm(cx, &app);
    }

    /// Hand a dormant instance a geometry and a framebuffer of its own so
    /// it does its FIRST FULL DRAW now, with nobody waiting: shaders
    /// compiled, glyph atlas rasterized, UI laid out. Sized at the desk's
    /// full-tile rect — the common case is a first window, and any other
    /// size costs exactly one reconfigure at adoption.
    fn warm_bootstrap(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) {
        // Linux shares a swapchain over an aux-chan socket owned by the
        // TILE (`MpRunView::setup_aux_chan`), and a warm instance has no
        // tile; there an instance stays process-warm and gets its
        // framebuffer at adoption. mac and Windows share by IOSurface /
        // HANDLE, which needs no channel.
        #[cfg(target_os = "linux")]
        let swapchain: Option<HostSwapchain> = {
            let _ = (window_id, &cx);
            None
        };
        #[cfg(not(target_os = "linux"))]
        let swapchain: Option<HostSwapchain> = {
            let rect = self.desk_area(cx);
            let dpi = if self.dpi_factor > 0.0 {
                self.dpi_factor
            } else {
                1.0
            };
            let (w, h) = (rect.w.max(1.0), rect.h.max(1.0));
            // The tile's own allocation rule, so a warm instance warms up
            // at the size a full-desk tile would have asked for.
            let alloc = |px: f64| ((px * dpi).ceil() as u32).max(64).next_power_of_two();
            let swapchain = HostSwapchain::new(window_id, alloc(w), alloc(h), cx);
            let shared = shared_swapchain_from_host_swapchain(&swapchain, cx);
            let mut msgs = vec![
                StudioToApp::WindowGeomChange {
                    window_id,
                    dpi_factor: dpi,
                    left: 0.0,
                    top: 0.0,
                    width: w,
                    height: h,
                },

            ];
            if let Some(shared) = shared { msgs.push(StudioToApp::Swapchain(shared)); }
            if let Some(sender) = self
                .state_mut()
                .clients
                .get(&client)
                .and_then(|s| s.sender.clone())
            {
                send_to_app(&sender, msgs);
            }
            Some(swapchain)
        };
        self.warm_frames.insert(
            client,
            WarmFrame {
                swapchain,
                presented: false,
                retired: None,
            },
        );
    }

    /// Drive the dormant instances, and only as far as they need driving.
    ///
    /// A `--stdin-loop` child does ALL of its work inside
    /// `StudioToApp::Tick` — it flushes its platform ops there (which is
    /// how a window is ANNOUNCED at all), runs its timers, draws and
    /// repaints — and between ticks it blocks on its socket at no cost
    /// whatsoever. A tile pumps its child every 8ms; a warm instance has
    /// no tile, so the WM pumps it here instead: at the pump rate until it
    /// has drawn its first frame — without those first ticks it would
    /// never even send `CreateWindow`, and the pool would deadlock waiting
    /// for a window that needs a tick to exist — and a once-a-second
    /// heartbeat after that. Together with `MAKEPAD_WM_WARM_START` (the app's
    /// own half of the deal: no samplers, no refresh while dormant) that
    /// is what keeps a cached task manager from burning a core in the
    /// background.
    fn pump_warm(&mut self, _cx: &mut Cx) {
        let now = host::now();
        // A retired framebuffer outlives adoption only long enough for any
        // frame still in flight against it.
        self.warm_frames.retain(|_, frame| {
            frame
                .retired
                .map(|t| now - t < WARM_FRAME_RETIRE)
                .unwrap_or(true)
        });
        let warm = self.warm_pool.clients();
        self.warm_last_tick.retain(|client, _| warm.contains(client));
        let due: Vec<ClientId> = warm
            .into_iter()
            .filter(|client| {
                // Still coming up (no window yet), or it has a framebuffer
                // and has not drawn into it: full rate. Anything else —
                // drawn, or a platform where a warm instance cannot be
                // handed a framebuffer at all — only needs the heartbeat.
                let warming = match self.warm_frames.get(client) {
                    None => true,
                    Some(frame) => frame.swapchain.is_some() && !frame.presented,
                };
                match self.warm_last_tick.get(client) {
                    Some(t) if !warming => now - *t >= WARM_HEARTBEAT,
                    _ => true,
                }
            })
            .collect();
        for client in due {
            let Some(sender) = self
                .state_mut()
                .clients
                .get(&client)
                .and_then(|s| s.sender.clone())
            else {
                // Not connected yet (still building, still starting): the
                // pump has nothing to send it down.
                continue;
            };
            #[cfg(not(target_os = "linux"))]
            if let Some(frame) = self.warm_frames.get(&client).filter(|frame| !frame.presented) {
                if let Some(swapchain) = frame.swapchain.as_ref() {
                    if let Some(shared) = shared_swapchain_from_host_swapchain(swapchain, _cx) {
                        send_to_app(&sender, vec![StudioToApp::Swapchain(shared)]);
                    }
                }
            }
            self.warm_last_tick.insert(client, now);
            send_to_app(&sender, vec![StudioToApp::Tick]);
        }
    }

    /// The pool's honest measurement: how long from the launch gesture to
    /// this window's first frame ON THE DESK, and by which path.
    fn note_first_frame(&mut self, client: ClientId) {
        let Some(slot) = self.state_mut().clients.get_mut(&client) else {
            return;
        };
        let Some(at) = slot.open_at.take() else {
            return;
        };
        let (app, warm) = (slot.app.clone(), slot.opened_warm);
        log!(
            "wm: {} client {} first frame in {} ms ({})",
            app,
            client,
            ((host::now() - at) * 1000.0) as u64,
            if warm { "warm" } else { "cold" }
        );
    }

    /// The channel children write their output into (created lazily so
    /// the very first launch in handle_startup already has one).
    fn line_sender(&mut self) -> std::sync::mpsc::Sender<ClientLine> {
        if self.client_lines.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            self.client_lines = Some(ClientLines { tx, rx });
        }
        self.client_lines.as_ref().unwrap().tx.clone()
    }

    /// Put every child's newest output line on its tile — cargo's
    /// "Compiling …" while the app is still being built.
    fn drain_client_lines(&mut self, cx: &mut Cx) {
        let mut latest: Vec<(ClientId, String, bool)> = Vec::new();
        if let Some(lines) = self.client_lines.as_ref() {
            while let Ok(line) = lines.rx.try_recv() {
                let Some(launch) = self.state.as_mut()
                    .and_then(|state| state.clients.get_mut(&line.client))
                    .and_then(|slot| slot.launch.as_mut()) else { continue };
                let Some((text, linked)) = launch.output(&line.output) else { continue };
                match latest.iter_mut().find(|(c, _, _)| *c == line.client) {
                    Some(slot) => { slot.1 = text; slot.2 |= linked; }
                    None => latest.push((line.client, text, linked)),
                }
            }
        }
        for (client, text, linked) in latest {
            let mut warm = false;
            let mut pane = false;
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                warm = slot.warm;
                pane = slot.pane;
                slot.status = text.clone();
                // cargo's last word before the app takes over. macOS then
                // scans a freshly linked binary on its FIRST exec, which
                // can hold main() for tens of seconds — see the tick.
                // Only cargo's handover means the binary is fresh on disk
                // and about to be exec'd for the first time.
                slot.linked |= linked;
                if linked {
                    slot.linked_at = Some(host::now());
                }
            }
            // A warm instance has no tile to put a status line on, and
            // asking the desk for one would BUILD it — a widget nothing
            // draws, with an 8ms child-tick timer behind it. The pool's
            // invariant is literal: no tile until adoption.
            if warm {
                continue;
            }
            // The pane's child has its own run view, never a desk tile.
            if pane {
                self.with_pane_run_view(cx, |cx, v| v.set_status_line(cx, &text));
                continue;
            }
            if !self.state_mut().clients.contains_key(&client) {
                continue;
            }
            self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                d.with_run_view(cx, client, |cx, v| v.set_status_line(cx, &text))
            });
        }
    }

    /// Everything a hosted app can ask of the compositor.
    fn on_wm_request(&mut self, cx: &mut Cx, client: ClientId, req: WmRequest) {
        match &req {
            // Quick Look: the warm-viewer cache (`handle_preview_request`).
            // A plain `Open` still tiles a real window every time.
            WmRequest::Preview { app, path } => {
                self.handle_preview_request(cx, client, app.clone(), path.clone());
            }
            WmRequest::Open { .. } => {
                if let Some(open) = preview::OpenRequest::from_request(&req) {
                    self.open_request(cx, open);
                }
            }
            // The REQUESTER hiding its own panel. A stray close from
            // anyone else (or after the panel already moved on to a
            // different requester) is ignored.
            WmRequest::PreviewClose => {
                if self
                    .preview_cache
                    .active
                    .as_ref()
                    .map(|a| a.requester == client)
                    .unwrap_or(false)
                {
                    self.hide_active_preview(cx);
                }
            }
            WmRequest::Launch { app, .. } => {
                let app = app.clone();
                self.launch_app(cx, &app);
            }
            WmRequest::Title { title } => {
                if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                    slot.title = title.clone();
                }
                self.update_bar(cx);
            }
            WmRequest::Cwd { path } => {
                if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                    slot.pwd = Some(std::path::PathBuf::from(path));
                }
            }
            WmRequest::Notify { title, body } => {
                // The notifications surface is the shell-UI lane's; until
                // it lands the notification is at least not lost.
                log!("wm: notify from client {}: {} — {}", client, title, body);
            }
            WmRequest::Close => self.request_close(cx, client),
            WmRequest::SetFloating { floating } => {
                let state = self.state_mut();
                let screen = state.screens.screen_of(client).unwrap_or(state.screens.active);
                let area = self.screen_area(cx, screen);
                let gap = self.state_mut().gap;
                let is_float = self.state_mut().layout_of(client).is_float(client);
                if is_float != *floating {
                    self.state_mut().layout_of_mut(client).toggle_float(client, area, gap);
                    self.redraw_all(cx);
                }
            }
            // `SetFullscreen` is the `Current` span (or leaving it).
            WmRequest::SetFullscreen { fullscreen } => {
                let span = fullscreen.then_some(ScreenSpan::Current);
                self.set_client_span(cx, client, span);
            }
            WmRequest::SetFullscreenSpan { span } => self.set_client_span(cx, client, span.clone()),
        }
    }

    /// `client` asked to be fullscreen across `span` (`None`: leave it): a
    /// fullscreen space of its own, right after its desktop
    /// (`ScreenSet::set_span` / `clear_span`; the span is more than one
    /// screen only on a multi-screen desktop). A span that cannot be honoured
    /// (an unknown or non-adjacent screen, a client that is not on a live
    /// screen) is logged and changes nothing: the client keeps the span or
    /// fullscreen it had (or stays a normal window), and is sent its
    /// `WmEvent::Screens` again, whose `span` (`fullscreen_span_of`) says what
    /// it still covers.
    fn set_client_span(&mut self, cx: &mut Cx, client: ClientId, span: Option<ScreenSpan>) {
        if span.is_some() && !self.state_mut().layout_of(client).is_fullscreen_space(client) {
            self.spaces_note_underlay(cx, client);
        }
        let state = self.state_mut();
        let (gap, reserved, gaps_out) = (state.gap, state.style.reserved_height(), state.gaps_out);
        let result = match &span {
            None => {
                state.screens.clear_span(client, gap, reserved, gaps_out);
                Ok(())
            }
            Some(span) => state.screens.set_span(client, span, gap, reserved, gaps_out),
        };
        match result {
            Err(err) => {
                log!("wm: client {} cannot span {:?}: {:?}; it keeps its current state", client, span, err);
                self.state_mut().spaces.underlay.clear();
                self.resend_screens_event(client);
            }
            Ok(()) => {
                self.state_mut().spaces.stale = true;
                if span.is_some() {
                    self.focus_client(cx, client);
                } else {
                    self.focus_after_layout(cx);
                }
            }
        }
        self.sync_bar_for_fullscreen(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// Bring every tiled client's `WmEvent::Screens` up to date after a
    /// desk draw: the live screens in that client's window coordinates and
    /// what it spans, sent only to clients whose event changed.
    fn sync_screens_events(&mut self) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        if !state.screens_dirty {
            return;
        }
        state.screens_dirty = false;
        let clients = &state.clients;
        state.tile_origins.retain(|c, _| clients.contains_key(c));
        state.screens_sent.retain(|c, _| clients.contains_key(c));
        let live = state.screens.live_wm_screens();
        let mut out = Vec::new();
        for (client, tile) in &state.tile_origins {
            let Some(slot) = state.clients.get(client) else { continue };
            // A later window's app is told its screens once, for its first window.
            if slot.warm || slot.sender.is_none() || slot.parent.is_some() {
                continue;
            }
            let span = state.screens.fullscreen_span_of(*client);
            let ev = crate::screens::screens_event_for(*tile, &live, span);
            if state.screens_sent.get(client) != Some(&ev) {
                out.push((*client, ev));
            }
        }
        for (client, ev) in out {
            if self.send_wm_event(client, ev.clone()) {
                self.state_mut().screens_sent.insert(client, ev);
            }
        }
    }

    /// Forget what `client` was last sent and send its `WmEvent::Screens`
    /// again on the next event (a reply, or a fresh socket after connect).
    fn resend_screens_event(&mut self, client: ClientId) {
        if let Some(state) = self.state.as_mut() {
            state.screens_sent.remove(&client);
            state.screens_dirty = true;
        }
    }

    /// A client asked us to open a file in its associated app as a normal
    /// tiled window (`WmRequest::Open`). `WmRequest::Preview` (Quick Look)
    /// goes through `handle_preview_request`'s warm-viewer cache instead.
    fn open_request(&mut self, cx: &mut Cx, req: preview::OpenRequest) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.enabled {
            if let Some((app, extra)) = preview::app_for_request(&req) {
                // A launch-or-focus app that runs already takes the file
                // itself: it comes up and shows it (Camera's last shot in
                // the Photos that is open), no second instance.
                if app.policy == LaunchPolicy::OrFocus && !req.preview {
                    if let Some(client) = self.running_client(&app.id) {
                        if self.domains.ended(client) { self.relaunch_domain(cx, client); }
                        self.activate_client(cx, client);
                        self.send_wm_event(client, WmEvent::Open { path: req.path.to_string_lossy().to_string() });
                        return;
                    }
                }
                let client = self.start_domain_client(cx, &app, &extra, false);
                self.seat_domain(cx, client);
            }
            return;
        }
        let hub_port = self.state_mut().hub_port;
        let id = self.next_id;
        self.next_id += 1;
        let lines = self.line_sender();
        let pool = cx.task_pool();
        let slot = match preview::spawn_for_request(&pool, &cx.thread_spawner(), &req, id, hub_port, lines) {
            Ok(slot) => slot,
            Err(err) => {
                log!("wm: open request failed: {}", err);
                return;
            }
        };
        log!("wm: opening {} as client {}", req.path.display(), id);
        self.state_mut().clients.insert(id, slot);
        self.sync_geometry(cx);
        let area = self.desk_area(cx);
        let state = self.state_mut();
        let gap = state.gap;
        state.layout_mut().insert(id, area, gap);
        let hub_port = state.hub_port;
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.with_run_view(cx, id, |cx, v| v.set_run_target(cx, id, 0, hub_port))
        });
        self.focus_client(cx, id);
        self.redraw_all(cx);
    }

    /// Send a hosted client a `WmEvent` over its own studio socket — the
    /// WM->app half of the `makepad_wm_api` protocol. False when the client is
    /// gone or has not connected yet, so a caller whose message MUST land
    /// (a Quick-Look retarget) can park it for `HubEvent::Connected`.
    fn send_wm_event(&mut self, client: ClientId, ev: WmEvent) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.owns(client) { return self.domains.send_message(client, ev.to_json()); }
        if let Some(slot) = self.state_mut().clients.get(&client) {
            if let Some(sender) = &slot.sender {
                makepad_app_host::send_to_app(sender, vec![StudioToApp::Custom(ev.to_json())]);
                return true;
            }
        }
        false
    }

    /// Retarget a warm viewer at `path`. A viewer spawned moments ago has
    /// no socket yet — park the newest file and let `drain_hub` flush it
    /// the instant the child connects, or a fast arrow-dial silently keeps
    /// the file the viewer was launched with.
    fn send_preview_file(&mut self, client: ClientId, path: &str) {
        log!("wm: preview retarget client {} -> {}", client, path);
        let ev = WmEvent::PreviewFile {
            path: path.to_string(),
        };
        if !self.send_wm_event(client, ev) {
            self.preview_cache.queue_pending(client, path.to_string());
        }
    }

    /// `WmRequest::Preview`: Quick Look through the warm-viewer cache.
    /// Reuses a live viewer of the right TYPE if one exists — sending it
    /// `PreviewFile` to retarget in place, no respawn, no popin, no lost
    /// selection — else spawns one exactly as before and remembers it.
    /// Switching type hides (not kills) whatever was showing. The
    /// requester keeps key focus the entire time (the FOCUS RULE) and is
    /// told `PreviewShown` once the panel is actually up.
    fn handle_preview_request(
        &mut self,
        cx: &mut Cx,
        requester: ClientId,
        app: Option<String>,
        path: String,
    ) {
        let Some(open) = preview::OpenRequest::from_request(&WmRequest::Preview { app, path })
        else {
            return;
        };
        let viewer_app = open.app.clone();
        let path_str = open.path.to_string_lossy().to_string();

        // A different TYPE is currently the visible panel: hide it (it
        // stays warm) before showing this one. The requester is NOT told
        // `PreviewHidden` here — from its side the panel never goes away,
        // it just changes what it shows, and a Hidden/Shown pair inside one
        // keystroke would make its event-driven state stutter.
        //
        // That silence has to be paid back if the new viewer never comes
        // up: the requester would go on believing a panel is there, its
        // Space would close a panel that is not, and Quick Look would wedge
        // until something else resynced it. `suppressed_hide` says the debt
        // is outstanding; every failure path below settles it.
        let mut suppressed_hide = false;
        if self.preview_cache.switching_type(&viewer_app) {
            self.hide_active_preview_ex(cx, false);
            suppressed_hide = true;
        }

        // Reuse this type's warm viewer if the process behind it is still
        // usable; a dead or CLOSING one clears its own slot in there, so a
        // viewer already on its way out never gets the next file.
        let alive: Vec<ClientId> = self
            .state_mut()
            .clients
            .iter()
            .filter(|(_, s)| s.closing.is_none())
            .map(|(id, _)| *id)
            .collect();
        let warm = self
            .preview_cache
            .warm_client(&viewer_app, |id| alive.contains(&id));
        if let Some(id) = warm {
            self.send_preview_file(id, &path_str);
            if !self.preview_cache.is_showing(id) {
                // Was hidden (or this is its first request this session
                // after a hide) — bring the float back, same reused rect.
                self.show_preview_float(cx, id);
            }
            self.preview_cache.active = Some(preview::ActivePreview {
                requester,
                viewer_app,
                client: id,
                path: open.path.clone(),
            });
            self.send_wm_event(requester, WmEvent::PreviewShown { path: path_str });
            self.redraw_all(cx);
            return;
        }

        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.enabled {
            let Some((app, extra)) = preview::app_for_request(&open) else { return };
            let id = self.start_domain_client(cx, &app, &extra, false);
            if let Some(slot) = self.state_mut().clients.get_mut(&id) {
                slot.is_preview = true;
                slot.takes_focus = false;
            }
            self.preview_cache.remember(&viewer_app, id);
            self.sync_geometry(cx);
            self.show_preview_float(cx, id);
            self.preview_cache.active = Some(preview::ActivePreview {
                requester, viewer_app, client: id, path: open.path,
            });
            self.send_wm_event(requester, WmEvent::PreviewShown { path: path_str });
            self.redraw_all(cx);
            return;
        }

        // Spawn fresh, exactly as an ordinary preview always has, except it
        // never takes focus and is remembered as this type's warm slot.
        let hub_port = self.state_mut().hub_port;
        let id = self.next_id;
        self.next_id += 1;
        let lines = self.line_sender();
        let pool = cx.task_pool();
        let slot = match preview::spawn_for_request(&pool, &cx.thread_spawner(), &open, id, hub_port, lines) {
            Ok(mut slot) => {
                slot.is_preview = true;
                slot.takes_focus = false;
                slot
            }
            Err(err) => {
                log!("wm: preview request failed: {}", err);
                if suppressed_hide {
                    self.send_wm_event(requester, WmEvent::PreviewHidden);
                }
                return;
            }
        };
        log!(
            "wm: previewing {} as client {} ({})",
            open.path.display(),
            id,
            viewer_app
        );
        self.state_mut().clients.insert(id, slot);
        self.preview_cache.remember(&viewer_app, id);
        self.sync_geometry(cx);
        let hub_port = self.state_mut().hub_port;
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.with_run_view(cx, id, |cx, v| v.set_run_target(cx, id, 0, hub_port))
        });
        self.show_preview_float(cx, id);
        self.preview_cache.active = Some(preview::ActivePreview {
            requester,
            viewer_app,
            client: id,
            path: open.path,
        });
        self.send_wm_event(requester, WmEvent::PreviewShown { path: path_str });
        self.redraw_all(cx);
    }

    /// Float a (warm or fresh) preview client into the single reused
    /// popup rect — `popup_rect` is a pure function of the desk, so every
    /// show of every viewer type lands on exactly the same rect and a
    /// retarget never moves the panel. `add_preview_float` keeps the
    /// layout's focus where it was (the FOCUS RULE), and the tile itself
    /// is marked so its own clicks cannot take the keyboard either.
    fn show_preview_float(&mut self, cx: &mut Cx, client: ClientId) {
        let area = self.desk_area(cx);
        let state = self.state_mut();
        let gap = state.gap;
        let ws = state.layout().active;
        let rect = state.layout().popup_rect(area, gap, 900.0, 700.0);
        state.layout_mut().add_preview_float(client, rect, ws);
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.with_tile(cx, client, |_cx, v| v.set_takes_key_focus(false))
        });
        self.redraw_all(cx);
    }

    /// Hide the active Quick Look panel: the float goes away, the warm
    /// viewer is told to `PreviewUnload` (drop decoders/textures, idle)
    /// but is NEVER killed here, and the requester is told `PreviewHidden`.
    fn hide_active_preview(&mut self, cx: &mut Cx) {
        self.hide_active_preview_ex(cx, true);
    }

    /// `notify_requester` is false only for the hide inside a TYPE SWITCH,
    /// where the panel is about to come straight back with another viewer:
    /// the requester is told `PreviewShown` for the new file instead, and
    /// never sees a phantom close.
    fn hide_active_preview_ex(&mut self, cx: &mut Cx, notify_requester: bool) {
        let Some(active) = self.preview_cache.active.take() else {
            return;
        };
        // Out of the layout, but NOT out of the desk: `WmDesk::remove_client`
        // would drop the tile widget (and with it the child's swapchain and
        // this viewer's warmth) once its close animation finished.
        self.state_mut().layout_of_mut(active.client).remove(active.client);
        log!(
            "wm: preview hide client {} ({}), viewer stays warm",
            active.client,
            active.viewer_app
        );
        self.send_wm_event(active.client, WmEvent::PreviewUnload);
        // A retarget parked for a viewer that never connected dies with the
        // panel: showing it later would resurrect a file nobody asked for.
        self.preview_cache.take_pending(active.client);
        if notify_requester {
            self.send_wm_event(active.requester, WmEvent::PreviewHidden);
        }
        self.focus_after_layout(cx);
        self.redraw_all(cx);
    }

    /// Escape / Space dismisses the active Quick-Look panel, WM-level —
    /// handled here rather than forwarded to whatever has key focus,
    /// because under the FOCUS RULE that is always the REQUESTER (files),
    /// never the preview, so this can no longer key off
    /// `layout.focused_client()` the way it used to.
    fn close_focused_preview(&mut self, cx: &mut Cx) -> bool {
        if self.preview_cache.active.is_none() {
            return false;
        }
        // Only the app that is dialing loses its Space/Escape to the panel.
        // Focus somewhere else (a terminal the user tabbed to while the
        // panel stayed up) means Space is a space again.
        let focus = self.state_mut().layout().focused_client();
        match focus {
            Some(client) if !self.preview_cache.is_requester(client) => return false,
            _ => {}
        }
        self.hide_active_preview(cx);
        true
    }

    /// SUPER+W / SUPER+Q — `hl.dsp.window.close()`: a POLITE close the app
    /// honors itself. The tile leaves the layout at once (so the rest
    /// reflow and the close animation plays with its last frame), the
    /// process gets `CLOSE_GRACE` to exit, and only then is it killed.
    fn close_focused(&mut self, cx: &mut Cx) {
        let Some(focus) = self.state_mut().layout().focused_client() else {
            return;
        };
        self.request_close(cx, focus);
    }

    fn request_close(&mut self, cx: &mut Cx, client: ClientId) {
        // One of an app's later windows: the app is asked to close that window only, and
        // its tile goes when the app says the window is gone.
        if let Some((owner, window_id)) = self.client_windows.owner(client) {
            if let Some(sender) = self.state_mut().clients.get(&owner).and_then(|slot| slot.sender.clone()) {
                let ask = makepad_platform::ime::HostedWindow::new(window_id, makepad_platform::ime::hosted_window_op::CLOSE_REQUEST);
                send_to_app(&sender, vec![StudioToApp::Custom(ask.to_json())]);
            } else {
                self.remove_client(cx, client);
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.close_domain(cx, client) { return; }
        // A foreign Wayland window is asked to close and ends when its client does.
        if self.wayland_close(client) {
            return;
        }
        // A module instance has no process to ask politely and nothing to
        // reap later: it ends now, through the same removal as a death.
        if self.module_host.is_module(client) {
            self.remove_client(cx, client);
            self.update_bar(cx);
            return;
        }
        let mut polite = false;
        if let Some(slot) = self.state_mut().clients.get_mut(&client) {
            if slot.closing.is_some() {
                return;
            }
            slot.closing = Some(host::now());
            if let Some(sender) = &slot.sender {
                // Ask over the API first (the app may want to save), then
                // the protocol's own Kill; the reaper is the fallback.
                send_to_app(
                    sender,
                    vec![
                        StudioToApp::Custom(WmEvent::CloseRequested.to_json()),
                        StudioToApp::Kill,
                    ],
                );
                polite = true;
            }
        }
        if !polite {
            // Never connected (still building): nothing to ask politely.
            let pool = cx.task_pool();
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                if let Some(child) = slot.child.as_mut() {
                    makepad_app_host::launch::kill_child_group(child, makepad_app_host::launch::GROUP_KILL_GRACE, &pool);
                }
            }
        }
        // Out of the layout now: the tiles reflow and the desk plays the
        // popin-out with the frame the tile already has.
        self.state_mut().layout_of_mut(client).remove(client);
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
            desk.remove_client(client);
        }
        self.focus_after_layout(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// A client the pool is holding: no tile, no focus, invisible.
    fn is_warm(&mut self, client: ClientId) -> bool {
        self.state_mut()
            .clients
            .get(&client)
            .map(|s| s.warm)
            .unwrap_or(false)
    }

    /// Answers for hosted children's relays, each to its client.
    fn send_relays(&self, answers: Vec<(ClientId, makepad_widgets::makepad_platform::studio::HostRelay)>) {
        let Some(state) = self.state.as_ref() else { return };
        for (client, relay) in answers {
            if let Some(sender) = state.clients.get(&client).and_then(|slot| slot.sender.as_ref()) {
                send_to_app(sender, vec![StudioToApp::Relay(relay)]);
            }
        }
    }

    /// Calls on the portal's UI-answered interfaces: a browser's file dialog opens the WM's
    /// picker (portal_file_chooser.rs); its answer is the call's reply (`file_picker_event`).
    #[cfg(target_os = "linux")]
    fn portal_calls(&mut self, cx: &mut Cx) {
        let Some(calls) = self.portal.as_ref().map(|p| p.take_calls()) else { return };
        for call in calls {
            let request = (call.interface == portal_file_chooser::NAME)
                .then(|| call.args.downcast::<makepad_widgets::makepad_platform::studio::RelayFileDialog>().ok())
                .flatten();
            let Some(request) = request else {
                if let Some(portal) = &self.portal {
                    portal.reply_error(call.id, "org.freedesktop.DBus.Error.UnknownMethod".into(), format!("{}.{} is not served", call.interface, call.member));
                }
                continue;
            };
            log!("wm portal: FileChooser.{} kind {} location {:?} name {:?} filters {:?} multiple {}", call.member, request.kind, request.location, request.filename, request.filters, request.multiple);
            let picker = self.ui.widget(cx, ids!(file_picker));
            if let Some(mut p) = picker.borrow_mut::<shell::file_picker::ShellFilePicker>() {
                p.request(cx, shell::file_picker::PickRequest { from: shell::file_picker::Requester::Portal(call.id), relay: *request, anchor: None });
            }
            cx.set_key_focus(Area::Empty);
            self.redraw_all(cx);
        }
    }

    /// The file picker's share of an event: true when it took it (`handle_event`).
    fn file_picker_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let picker = self.ui.widget(cx, ids!(file_picker));
        let answer = {
            let Some(mut p) = picker.borrow_mut::<shell::file_picker::ShellFilePicker>() else { return false };
            if !p.is_open() {
                return false;
            }
            match event {
                Event::MouseMove(_) | Event::MouseDown(_) | Event::Scroll(_) => p.pointer(cx, event),
                Event::MouseUp(_) | Event::TouchUpdate(_) | Event::FingerCancel(_) | Event::KeyUp(_) => None,
                Event::KeyDown(e) => p.key(cx, e),
                Event::TextInput(e) => {
                    p.typed(cx, &e.input);
                    None
                }
                Event::TextRangeReplace(_) | Event::TextCopy(_) | Event::TextCut(_) | Event::ImeAction(_) => None,
                _ => return false,
            }
        };
        if let Some((from, result)) = answer {
            match from {
                shell::file_picker::Requester::Client(client) => {
                    self.send_relays(vec![(client, makepad_widgets::makepad_platform::studio::HostRelay::FileDialog(result))]);
                }
                #[cfg(target_os = "linux")]
                shell::file_picker::Requester::Portal(id) => {
                    if let Some(portal) = &self.portal {
                        portal.reply(id, portal_file_chooser::reply(&result));
                    }
                }
                #[cfg(not(target_os = "linux"))]
                shell::file_picker::Requester::Portal(_) => {}
            }
            if let Some(focus) = self.state.as_mut().and_then(|s| s.layout().focused_client()) {
                self.focus_client(cx, focus);
            }
            self.redraw_all(cx);
        }
        true
    }

    fn remove_client(&mut self, cx: &mut Cx, client: ClientId) {
        // An app's later windows go with it; a later window going is forgotten here.
        for window in self.client_windows.forget(client) {
            self.remove_client(cx, window);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.forget_domain(client);
        log!("wm: removing client {}", client);
        self.relay.forget_client(cx, client);
        if let Some(mut p) = self.ui.widget(cx, ids!(file_picker)).borrow_mut::<shell::file_picker::ShellFilePicker>() {
            p.forget_client(cx, client);
        }
        // A finger held on the closing app lets go of it first.
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
            if desk.phone_finger_on(client) {
                desk.cancel_phone_finger(cx);
            }
        }
        // A module instance: the tile lets go of the root FIRST, then the
        // instance and its isolate go (module_host.rs). The bus hears an
        // Unregister for it below, like for any client.
        if self.module_host.is_module(client) {
            self.desk(cx)
                .borrow_mut::<WmDesk>()
                .map(|mut d| d.with_module_view(cx, client, |cx, v| v.clear_root(cx)));
            self.module_host.teardown(cx, client);
        }
        // The bus: the pane's own death empties the slot (the next F10
        // launches afresh); anyone else's death is an `Unregister` the
        // pane hears on their behalf, so its tool table shrinks at once.
        if self.ai_bus.is_pane(client) {
            log!("wm: the AI pane's child died");
            self.with_ai_pane(cx, |cx, p| p.reset(cx));
        }
        if let Some(bye) = self.ai_bus.client_died(client) {
            self.send_to_pane(bye);
        }
        // An in-process assistant sees the instance's link close.
        self.pane_links.close_instance(client);
        // A dying warm instance clears its pool slot, so the next launch
        // of that app falls back to a cold spawn instead of talking to a
        // dead socket, and the pool heals itself. A death nobody asked for
        // is a crash and is counted: three of those a minute and the pool
        // gives that app up quietly rather than respawning forever.
        let asked_for_it = self
            .state_mut()
            .clients
            .get(&client)
            .map(|s| s.closing.is_some())
            .unwrap_or(false);
        if let Some(app) = self.warm_pool.forget(client) {
            if !asked_for_it {
                self.warm_pool.note_crash(&app, host::now());
                log!("wm: warm {} (client {}) died", app, client);
            }
            // The top-up runs on the tick — one spawn at a time, and it
            // will only happen at all if the crash budget allows it.
        }
        self.warm_frames.remove(&client);
        self.warm_last_tick.remove(&client);
        // A home tile's client is gone: the tile shows the launcher again
        // (within its relaunch budget) on the next phone frame.
        if let Some(app) = self.state_mut().phone.tiles.forget_client(client) {
            log!("wm: home tile {} lost client {}", app, client);
        }
        // A closing spanned window frees its screens; its fullscreen space
        // goes with the window.
        self.state_mut().screens.forget_span(client);
        self.state_mut().layout_of_mut(client).remove(client);
        self.state_mut().clients.remove(&client);
        // A dying warm viewer (or its requester) clears the cache's
        // reference to it — "if a viewer process dies clear its slot, so
        // the next Space respawns" instead of talking to a dead socket.
        self.preview_cache.forget_client(client);
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
            desk.remove_client(client);
        }
        self.focus_after_layout(cx);
        // `layout.remove` clears the workspace's fullscreen when the closing
        // client held it; a client that exits on its own reaches here
        // outside `do_action`, so restore the bar on this path too.
        self.sync_bar_for_fullscreen(cx);
        self.redraw_all(cx);
    }

    fn activate_client(&mut self, cx: &mut Cx, client: ClientId) {
        // A client the home page launched for its tile becomes a real
        // window on its first open — the same client, seated in the layout.
        self.promote_tile_client(cx, client);
        if self.state_mut().style.target.mobile() {
            // Another app coming to the front takes the finger off the old one.
            let held_elsewhere = self
                .desk(cx)
                .borrow::<WmDesk>()
                .is_some_and(|d| d.phone_finger_active() && !d.phone_finger_on(client));
            if held_elsewhere {
                self.cancel_app_finger(cx);
            }
            self.state_mut().phone.activate(client);
            // In front now: its full face and full viewport go out at once,
            // so the zoom-in never plays over a compact frame.
            self.sync_home_tiles(cx);
            self.animate_phone(cx);
        } else {
            self.replay_tile_face(cx, client);
        }
        self.focus_client(cx, client);
    }

    fn focus_client(&mut self, cx: &mut Cx, client: ClientId) {
        // The pane's child is not in the layout: it has its own focus path.
        if self.ai_bus.is_pane(client) {
            self.focus_pane(cx);
            return;
        }
        // FOCUS RULE: a Quick Look preview never takes key focus.
        let takes_focus = self
            .state_mut()
            .clients
            .get(&client)
            .map(|s| s.takes_focus)
            .unwrap_or(true);
        if !takes_focus {
            return;
        }
        // Defensive: the menu is modal and must never still read "open"
        // once a hosted client is about to take key focus — every launch
        // path already closes it first, but force it shut here too so a
        // stray leftover state can never hijack this client's keystrokes.
        {
            let menu = self.ui.widget(cx, ids!(shell_menu));
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                if m.is_open() {
                    m.close(cx);
                }
            }
        }
        // A window on another screen makes that screen the active one
        // first (a click, a bar entry, move-to-screen with follow).
        let state = self.state_mut();
        if state.screens.activate_screen_of(client).is_none() && state.screens.screen_of(client).is_some() {
            // On a screen waiting out its removal: not drawn, so it must
            // not take the keyboard.
            return;
        }
        if let Some(ws) = self.state_mut().layout().workspace_of(client) {
            let state = self.state_mut();
            if ws != state.layout().active {
                let screen = state.screens.active;
                if let Some(to) = state.layout().spaces.index_of_slot(ws) {
                    let from = state.layout().spaces.cur();
                    state.spaces.slides.begin(screen, from, to);
                }
                state.layout_mut().show_slot(ws);
            }
            state.layout_mut().workspaces[ws].focus = Some(client);
            state.layout_mut().note_focus(client);
            if state.layout().desktop.enabled {state.layout_mut().raise_float(client);}
        }
        // The tile widget only exists once the desk has drawn it, and its
        // Area only after its first draw — a focus at launch time lands on
        // nothing. Keep it pending and re-assert when the child's first
        // frame arrives (the PresentableDraw path below).
        let focused = self
            .desk(cx)
            .borrow_mut::<WmDesk>()
            .and_then(|mut d| d.with_tile(cx, client, |cx, v| v.focus_keyboard(cx)))
            .unwrap_or(false);
        self.pending_focus = if focused { None } else { Some(client) };
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// A freshly linked binary stalls on its first exec while macOS scans
    /// it (XprotectService/syspolicyd; the second exec is instant). Say so
    /// instead of leaving the tile silent.
    fn explain_first_exec_scan(&mut self, cx: &mut Cx) {
        if !cfg!(target_os = "macos") {
            return;
        }
        const GRACE: f64 = 3.0;
        let waiting: Vec<ClientId> = self
            .state_mut()
            .clients
            .iter()
            .filter(|(_, s)| {
                // Warm instances excluded: nobody is watching a tile that
                // does not exist, and asking for one would build it.
                !s.warm
                    && s.linked
                    && s.sender.is_none()
                    && s.linked_at.map(|t| host::now() - t > GRACE).unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        for client in waiting {
            let text = "macOS is verifying the new binary…".to_string();
            let mut pane = false;
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                if slot.status == text {
                    continue;
                }
                slot.status = text.clone();
                slot.linked = false;
                pane = slot.pane;
            }
            // Asking the desk for the pane child's tile would BUILD one.
            if pane {
                self.with_pane_run_view(cx, |cx, v| v.set_status_line(cx, &text));
                continue;
            }
            self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                d.with_run_view(cx, client, |cx, v| v.set_status_line(cx, &text))
            });
        }
    }

    fn reap_exited(&mut self, cx: &mut Cx) {
        // A client that ignored the polite close gets the fallback.
        let pool = cx.task_pool();
        for slot in self.state_mut().clients.values_mut() {
            let Some(since) = slot.closing else { continue };
            if host::now() - since < makepad_app_host::launch::CLOSE_GRACE.as_secs_f64() {
                continue;
            }
            if let Some(child) = slot.child.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    makepad_app_host::launch::kill_child_group(child, makepad_app_host::launch::GROUP_KILL_GRACE, &pool);
                }
            }
        }
        let mut dead = Vec::new();
        let mut failures = Vec::new();
        let lines = self.line_sender();
        let spawner = cx.thread_spawner();
        #[cfg(not(target_arch = "wasm32"))]
        let domain_clients: std::collections::HashSet<_> = self.state.as_ref().unwrap().clients.keys().copied()
            .filter(|id| self.domains.owns(*id)).collect();
        let state = self.state_mut();
        let hub_port = state.hub_port;
        let term_env = state.term_env.clone();
        for (id, slot) in state.clients.iter_mut() {
            #[cfg(not(target_arch = "wasm32"))]
            if domain_clients.contains(id) { continue; }
            // Closing a build must never launch the app on its way out.
            if slot.closing.is_some() || slot.launch.is_none() {
                if slot.child.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))))
                    || (slot.child.is_none() && slot.closing.is_some()) {
                    dead.push(*id);
                }
                continue;
            }
            let colors = (slot.app == "terminal").then_some(term_env.as_str());
            match slot.poll_launch(&spawner, hub_port, colors, lines.clone()) {
                Ok(Some(LaunchEvent::Started)) => {
                    log!("wm: built {}, launched as client {}", slot.app, id);
                    // Keep the tile, pane and original click time through the build.
                    slot.linked = true;
                    slot.linked_at = Some(host::now());
                    slot.status = "launching…".into();
                }
                Ok(Some(LaunchEvent::Exited(exit))) => {
                    // The end of a child that never connected: no deadline
                    // waits for it, its exit is what is reported.
                    log!("wm: {} (client {}) exited ({exit}); removing its tile", slot.app, id);
                    dead.push(*id);
                }
                Ok(None) => {}
                Err(error) => {
                    log!("wm: launching {} failed: {}", slot.app, error);
                    slot.status = format!("{error} — see {}", clients::client_log(*id, true).display());
                    failures.push((*id, slot.pane, slot.status.clone()));
                    // Keep the error tile without polling a failed process again.
                    slot.launch = None;
                }
            }
        }
        for (id, pane, text) in failures {
            if pane {
                self.with_pane_run_view(cx, |cx, view| view.set_status_line(cx, &text));
            } else {
                self.desk(cx).borrow_mut::<WmDesk>().map(|mut desk| {
                    desk.with_run_view(cx, id, |cx, view| view.set_status_line(cx, &text))
                });
            }
        }
        for id in dead {
            self.remove_client(cx, id);
        }
    }

    // --------------------------------------------------------------
    // The AI pane and its bus
    //
    // The chat is a CHILD (apps/aichat), seated in the pane slot instead
    // of a tile. The WM gives it three things and nothing more: the slot
    // (shell/ai_pane.rs), the bus — every other client's service frames,
    // stamped and routed (ai_bus.rs) — and its own `os` service on that
    // bus. The child is never in the layout, so nothing the desk
    // enumerates (alt-tab, the bar, workspace counts) can see it.
    // --------------------------------------------------------------

    fn with_ai_pane<R>(
        &mut self,
        cx: &mut Cx,
        f: impl FnOnce(&mut Cx, &mut ShellAiPane) -> R,
    ) -> Option<R> {
        let pane = self.ui.widget(cx, ids!(shell_ai_pane));
        let mut borrowed = pane.borrow_mut::<ShellAiPane>()?;
        Some(f(cx, &mut borrowed))
    }

    fn with_pane_run_view<R>(
        &mut self,
        cx: &mut Cx,
        f: impl FnOnce(&mut Cx, &mut MpRunView) -> R,
    ) -> Option<R> {
        self.with_ai_pane(cx, |cx, p| p.with_run_view(cx, f)).flatten()
    }

    fn ai_pane_is_open(&mut self, cx: &mut Cx) -> bool {
        self.with_ai_pane(cx, |_, p| p.is_open()).unwrap_or(false)
    }

    /// The desk ROW the pane and the desk share: its last drawn rect, or
    /// the window's startup proportions before the first draw. The desk
    /// also learns whether the pane is mid-slide, so its tiles snap to the
    /// narrowing layout every frame instead of tweening after it.
    fn sync_ai_pane_geometry(&mut self, cx: &mut Cx) {
        let row = self.ui.view(cx, ids!(desk_row)).area();
        let rect = if row.is_valid(cx) { row.rect(cx) } else { Rect::default() };
        let rect = if rect.size.x > 1.0 {
            rect
        } else {
            Rect {
                pos: dvec2(0.0, BAR_HEIGHT_FALLBACK),
                size: dvec2(1400.0, 860.0),
            }
        };
        // The pane sits under the leftmost bar segment, which reaches the
        // window's left edge: floating over the scene, the pane starts
        // below it (unless a span hides that segment).
        let top = self.state.as_ref().map_or(0.0, |s| s.screens.reserved_top_for(0));
        let rect = Rect {
            pos: dvec2(rect.pos.x, rect.pos.y + top),
            size: dvec2(rect.size.x, (rect.size.y - top).max(0.0)),
        };
        let gap = self.state_mut().gaps_out;
        let sliding = self
            .with_ai_pane(cx, |_, p| {
                p.set_geometry(rect, gap);
                p.is_sliding()
            })
            .unwrap_or(false);
        self.state_mut().pane_sliding = sliding;
    }

    /// F10 / `--test-action ai`.
    fn toggle_ai_pane(&mut self, cx: &mut Cx) {
        if self.ai_pane_is_open(cx) {
            self.close_ai_pane(cx);
        } else {
            self.open_ai_pane(cx);
        }
    }

    /// Slide the pane in, launching the aichat child the first time (or
    /// again after it died). The keyboard goes to the child as soon as it
    /// has a frame to receive it in; every registered app hears
    /// `ChatOpen`.
    fn open_ai_pane(&mut self, cx: &mut Cx) {
        if self.apps.pane_in_process() {
            if !self.ensure_local_pane(cx) {
                return;
            }
        } else if self.ai_bus.pane_client.is_none() && !self.launch_ai_pane(cx, true) {
            return;
        }
        self.sync_ai_pane_geometry(cx);
        self.with_ai_pane(cx, |cx, p| p.set_open(cx, true));
        self.focus_pane(cx);
        self.broadcast_chat_open(cx, true);
        self.redraw_all(cx);
    }

    /// Slide the pane out; the child keeps running behind it. The
    /// keyboard returns to the layout's focused tile.
    fn close_ai_pane(&mut self, cx: &mut Cx) {
        if !self.ai_pane_is_open(cx) {
            return;
        }
        self.with_ai_pane(cx, |cx, p| p.set_open(cx, false));
        self.focus_after_layout(cx);
        self.broadcast_chat_open(cx, false);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// Spawn the aichat child as the pane's client: the ordinary launch
    /// path (env, per-client log) minus the layout — `launch_app` is never
    /// used for it, since launch-or-focus would "focus" a pane. `opened`:
    /// the person opened the pane, so an aichat that is not built or out of
    /// date is built (in the pane); the desktop's own start runs only an
    /// up-to-date aichat and never compiles.
    fn launch_ai_pane(&mut self, cx: &mut Cx, _opened: bool) -> bool {
        let Some(app) = clients::find_app("aichat") else {
            log!("wm: no aichat entry in the registry");
            return false;
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.enabled {
            let client = self.start_domain_client(cx, &app, &[], true);
            self.ai_bus.pane_client = Some(client);
            return true;
        }
        let hub_port = self.state_mut().hub_port;
        if hub_port == 0 {
            log!("wm: no hub; the AI pane cannot host a child");
            return false;
        }
        let id = self.next_id;
        self.next_id += 1;
        let lines = self.line_sender();
        let pool = cx.task_pool();
        // Like every app: from a checkout `cargo run` compiles what changed (the pane shows
        // cargo's progress); installed, a binary out of date is built first.
        let policy = if app.is_current() { BuildPolicy::IfMissing } else { BuildPolicy::Rebuild };
        let spawned = spawn_client(&pool, &cx.thread_spawner(), &app, id, hub_port, None, None, &[], false, policy, lines);
        match spawned {
            Ok(mut slot) => {
                slot.pane = true;
                self.state_mut().clients.insert(id, slot);
                self.ai_bus.pane_client = Some(id);
                self.with_ai_pane(cx, |cx, p| {
                    p.set_client(Some(id));
                    p.with_run_view(cx, |cx, v| v.set_run_target(cx, id, 0, hub_port));
                });
                log!("wm: launched aichat as client {} (the AI pane)", id);
                true
            }
            Err(err) => {
                log!("wm: launching the AI pane failed: {}", err);
                false
            }
        }
    }

    /// The pane's keyboard goes to the child's run view; the pane itself
    /// keeps the request pending until its run view has drawn (see
    /// `ShellAiPane::focus_keyboard`).
    fn focus_pane(&mut self, cx: &mut Cx) {
        let local = self.with_ai_pane(cx, |_, p| p.is_local()).unwrap_or(false);
        if self.ai_bus.pane_client.is_none() && !local {
            return;
        }
        self.with_ai_pane(cx, |cx, p| p.focus_keyboard(cx));
    }

    /// A pointer event inside the OPEN pane is the pane's alone — routed
    /// to it directly, the way an open menu or flyout takes the pointer,
    /// so the tile underneath and the WM's own gestures never see it. A
    /// drag already in flight keeps its events: a window carried across
    /// the pane must still land.
    fn ai_pane_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let abs = match event {
            Event::MouseMove(e) => e.abs,
            Event::MouseDown(e) => e.abs,
            Event::MouseUp(e) => e.abs,
            Event::Scroll(e) => e.abs,
            _ => return false,
        };
        if self.drag.is_some() || self.div_drag.is_some() {
            return false;
        }
        let inside = self
            .with_ai_pane(cx, |_, p| p.contains(abs))
            .unwrap_or(false);
        if !inside {
            return false;
        }
        let pane = self.ui.widget(cx, ids!(shell_ai_pane));
        pane.handle_event(cx, event, &mut Scope::empty());
        true
    }

    /// One `Custom` frame to a connected client; false when it has no
    /// socket yet (or is gone).
    fn send_custom(&mut self, client: ClientId, json: String) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.owns(client) { return self.domains.send_message(client, json); }
        if let Some(slot) = self.state_mut().clients.get(&client) {
            if let Some(sender) = &slot.sender {
                send_to_app(sender, vec![StudioToApp::Custom(json)]);
                return true;
            }
        }
        false
    }

    /// A frame for the pane child. Dropped when the pane is not up: the
    /// bus already holds every registration for the replay, and a
    /// result nobody is waiting for has no one to go to.
    fn send_to_pane(&mut self, json: String) -> bool {
        match self.ai_bus.pane_client {
            Some(client) => self.send_custom(client, json),
            None => false,
        }
    }

    fn broadcast_chat_open(&mut self, cx: &mut Cx, open: bool) {
        for (client, json) in self.ai_bus.chat_open_frames(open) {
            self.send_custom(client, json);
        }
        // In-process instances hear it from the host directly.
        self.module_host.chat_open(cx, open);
    }

    /// The registry as the `os` brief names it: (id, label) — only what
    /// this build can start (apps.rs `Launchable`).
    fn registry_apps(&self) -> Vec<(String, String)> {
        let launchable = self.apps.launchable();
        clients::registry()
            .iter()
            .filter(|a| launchable.allows(a))
            .map(|a| (a.id.clone(), a.label.clone()))
            .collect()
    }

    /// The pane connected: the WM's own registration, then every client's
    /// last one, in one batch.
    fn replay_bus_to_pane(&mut self) {
        let Some(client) = self.ai_bus.pane_client else {
            return;
        };
        let manifest = AiBus::os_manifest(&self.registry_apps());
        let frames = self.ai_bus.replay(manifest);
        #[cfg(not(target_arch = "wasm32"))]
        if self.domains.owns(client) {
            for json in frames { self.send_custom(client, json); }
            return;
        }
        let sender = self
            .state_mut()
            .clients
            .get(&client)
            .and_then(|s| s.sender.clone());
        if let Some(sender) = sender {
            log!("wm: replaying {} registrations to the AI pane", frames.len());
            send_to_app(
                &sender,
                frames.into_iter().map(StudioToApp::Custom).collect(),
            );
        }
    }

    /// The pane child's own window requests. Its `Close` (Esc on an idle,
    /// empty composer) means HIDE — the process stays for the next F10;
    /// the rest of the window vocabulary has no window to apply to.
    fn on_pane_request(&mut self, cx: &mut Cx, req: WmRequest) {
        match req {
            WmRequest::Close => self.close_ai_pane(cx),
            WmRequest::Title { .. } | WmRequest::Cwd { .. } => {}
            other => log!(
                "wm: the AI pane asked for {:?}; a pane has no window for that",
                other
            ),
        }
    }

    fn on_bus_route(&mut self, cx: &mut Cx, route: Route) {
        match route {
            Route::ToPane(json) => {
                self.send_to_pane(json);
            }
            Route::ToClient(client, json) => {
                self.send_custom(client, json);
            }
            Route::Os(call) => {
                let result = self.answer_os_call(cx, &call);
                self.send_to_pane(AiBus::os_reply(result));
            }
            Route::Local(client, msg) => self.on_local_frame(cx, client, msg),
            Route::Drop => {}
        }
    }

    /// Calls on the WM's test service (`--remote` instances only).
    fn drain_wm_port(&mut self, cx: &mut Cx, event: &Event) {
        let Some(port) = self.wm_port.as_mut() else { return };
        for ev in port.handle_event(cx, event) {
            let makepad_ai_services::port::PortEvent::Call(call) = ev else { continue };
            let result = match call.tool.as_str() {
                "motion_trace" => {
                    // `{"path":"/abs/file.jsonl"}`, or a fresh file in the temp dir.
                    let path = call.args.split("\"path\"").nth(1)
                        .and_then(|rest| rest.split('"').nth(1))
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::env::temp_dir().join(format!("wm-motion-{}-{:.0}.jsonl", std::process::id(), host::wall_now())));
                    match desk::frametrace::dump(&path) {
                        Ok(n) => ToolResult::ok(&call.call_id, format!("{{\"path\":\"{}\",\"frames\":{}}}", path.display(), n), "motion"),
                        Err(e) => ToolResult::refused(&call.call_id, format!("motion_trace: {}: {e}", path.display())),
                    }
                }
                other => ToolResult::refused(&call.call_id, format!("wm has no tool `{other}`")),
            };
            port.reply(result);
        }
    }

    /// The bus's traffic with the process registry: the calls it sent
    /// down to clients, the `Registered` answers the bus made, and the pid
    /// of each client that joined (so a caller can reach its own endpoint).
    fn drain_bus_registry(&mut self) {
        let mut frames = self.ai_bus.drain_registry();
        frames.extend(self.ai_bus.take_client_frames());
        for (client, json) in frames {
            self.send_custom(client, json);
        }
        for client in self.ai_bus.take_joined() {
            let pid = self.state_mut().clients.get(&client).and_then(|s| s.child.as_ref()).map(|c| c.id());
            if let (Some(pid), Some(registry)) = (pid, self.ai_bus.registry()) {
                registry.set_pid(&AiBus::endpoint_of(client), pid);
            }
        }
    }

    /// The WM is going down: the pane's child has no window of its own
    /// and no tile, so nothing else would reap it.
    fn kill_pane_child(&mut self, cx: &mut Cx) {
        let Some(client) = self.ai_bus.pane_client else {
            return;
        };
        let pool = cx.task_pool();
        if let Some(slot) = self.state_mut().clients.get_mut(&client) {
            if let Some(child) = slot.child.as_mut() {
                log!("wm: shutdown kills the AI pane's child {}", client);
                makepad_app_host::launch::kill_child_group(child, makepad_app_host::launch::GROUP_KILL_GRACE, &pool);
            }
        }
    }

    // ---- in-process instances (module_host.rs) ----

    /// Open `module` as an instance of its own in this process: an isolate,
    /// a tile in the layout, a local endpoint on the bus. The ordinary
    /// launch path minus everything a process needs.
    fn launch_module(&mut self, cx: &mut Cx, module: &'static dyn AppModule) {
        let open = match module.open_schema().empty_open() {
            Ok(open) => open,
            Err(e) => {
                log!("wm: {} cannot open without arguments: {}", module.id(), e);
                return;
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        let area = self.desk_area(cx);
        if let Err(e) = self.module_host.create(cx, id, module, open, dvec2(area.w, area.h)) {
            log!("wm: module {} failed to start: {}", module.id(), e);
            return;
        }
        let (manifest, root, vm_id) = match self.module_host.get(id) {
            Some(instance) => (instance.manifest(), instance.root.clone(), instance.vm_id),
            None => return,
        };
        self.state_mut()
            .clients
            .insert(id, clients::ClientSlot::module(id, module.id(), module.label()));
        let gap = self.state_mut().gap;
        self.state_mut().layout_mut().insert(id, area, gap);
        // The tile is a module tile from its first draw; the root is seated
        // in it before anything asks it to draw.
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.mark_module(id);
            d.with_module_view(cx, id, |cx, v| v.set_root(cx, id, vm_id, root));
        });
        // On the bus. An in-process assistant adopts the instance as a
        // link (waiting on `Cx` until the pane's root exists); the aichat
        // child learns of it as a local endpoint now, or at the replay
        // when it connects later.
        if self.apps.pane_in_process() {
            self.pane_links.open_instance(cx, id, manifest);
        } else {
            let frame = self.ai_bus.register_local(id, manifest);
            self.send_to_pane(frame);
        }
        log!("wm: launched {} as client {} (in-process)", module.id(), id);
        self.activate_client(cx, id);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// A frame the pane addressed to an in-process instance.
    fn on_local_frame(&mut self, cx: &mut Cx, client: ClientId, msg: ServiceDown) {
        match msg {
            ServiceDown::Call(call) => {
                let result = match self.module_host.execute(cx, client, &call) {
                    Some(ExecOutcome::Done(result)) => result,
                    // Answered later through the instance's reply sink.
                    Some(ExecOutcome::Pending) => return,
                    None => ToolResult::unavailable(&call.call_id, "that app is gone"),
                };
                let frame = self.ai_bus.local_reply(client, result);
                self.send_to_pane(frame);
            }
            ServiceDown::Cancel { call_id } => self.module_host.cancel(cx, client, &call_id),
            ServiceDown::Subscribe { sub_id, topic, filter } => {
                self.module_host.subscribe(cx, client, &sub_id, &topic, filter.as_deref())
            }
            ServiceDown::Unsubscribe { sub_id } => self.module_host.unsubscribe(cx, client, &sub_id),
            // The pane state reaches instances through `broadcast_chat_open`.
            ServiceDown::ChatOpen { .. } | ServiceDown::Registered { .. } => {}
        }
    }

    /// Results in-process executors answered later, up to the pane: down
    /// the instance's link when the assistant is in-process, as a frame to
    /// the aichat child otherwise.
    fn drain_module_upstream(&mut self) {
        for (client, upstream) in self.module_host.drain_upstream() {
            match upstream {
                ModuleUpstream::Result(result) => {
                    if self.pane_links.is_instance(client) {
                        self.pane_links.reply(client, result);
                    } else {
                        let frame = self.ai_bus.local_reply(client, result);
                        self.send_to_pane(frame);
                    }
                }
                ModuleUpstream::Message { sub_id, message } => {
                    if self.pane_links.is_instance(client) {
                        self.pane_links.publish(client, sub_id, message);
                    } else {
                        let frame = self.ai_bus.local_message(client, sub_id, message);
                        self.send_to_pane(frame);
                    }
                }
            }
        }
    }

    // ---- the assistant in-process (pane_links.rs) ----

    /// The pane's body is the aichat module: seat it (by name), and put
    /// the WM's own `os` service on its registry. False, logged, when this
    /// build does not link the assistant.
    fn ensure_local_pane(&mut self, cx: &mut Cx) -> bool {
        let seated = self.with_ai_pane(cx, |cx, p| p.ensure_overlay(cx)).unwrap_or(false);
        if !seated {
            return false;
        }
        let manifest = AiBus::os_manifest(&self.registry_apps());
        self.pane_links.open_os(cx, manifest);
        true
    }

    /// What the in-process assistant asked through its links: the `os`
    /// calls are the WM's, the rest go to the instances' executors.
    fn drain_pane_links(&mut self, cx: &mut Cx) {
        for call in self.pane_links.drain() {
            match call {
                PaneCall::Os(call) => {
                    let result = self.answer_os_call(cx, &call);
                    self.pane_links.reply_os(result);
                }
                PaneCall::Instance(client, call) => {
                    let result = match self.module_host.execute(cx, client, &call) {
                        Some(ExecOutcome::Done(result)) => result,
                        // Answered later through the instance's reply sink.
                        Some(ExecOutcome::Pending) => continue,
                        None => ToolResult::unavailable(&call.call_id, "that app is gone"),
                    };
                    self.pane_links.reply(client, result);
                }
                PaneCall::Cancel(client, call_id) => self.module_host.cancel(cx, client, &call_id),
                PaneCall::Subscribe { client, sub_id, topic, filter } => {
                    self.module_host.subscribe(cx, client, &sub_id, &topic, filter.as_deref())
                }
                PaneCall::Unsubscribe { client, sub_id } => {
                    self.module_host.unsubscribe(cx, client, &sub_id)
                }
            }
        }
        // The chat's own Escape (an idle, empty composer) asks the slot it
        // would sit in to close; in the pane that slot is us.
        if cx.global::<AiSlotRequests>().open.take() == Some(false) {
            self.close_ai_pane(cx);
        }
    }

    // ---- the `os` service ----

    /// Every app the desktop knows, with its window state: the registry
    /// rows first, then any window of an app outside the menu (a viewer
    /// opened on a file).
    fn app_rows(&mut self) -> Vec<OsAppRow> {
        let launchable = self.apps.launchable();
        let state = self.state_mut();
        let focused = state.layout().focused_client();
        let mut rows: Vec<OsAppRow> = clients::registry()
            .iter()
            .filter(|a| launchable.allows(a))
            .map(|a| OsAppRow {
                id: a.id.clone(),
                label: a.label.clone(),
                running: false,
                focused: false,
            })
            .collect();
        let mut windows: Vec<(ClientId, String)> = state
            .clients
            .iter()
            .filter(|(id, s)| {
                !s.warm && !s.pane && s.closing.is_none() && state.screens.screen_of(**id).is_some_and(|i| i < state.screens.live_count())
            })
            .map(|(id, s)| (*id, s.app.clone()))
            .collect();
        windows.sort_unstable();
        // the Wayland apps of the launcher's catalog (Linux)
        for (id, label) in wayland_host::menu_apps() {
            if !rows.iter().any(|r| r.id == id) {
                rows.push(OsAppRow { id, label, running: false, focused: false });
            }
        }
        for (id, app) in windows {
            if !rows.iter().any(|r| r.id == app) {
                rows.push(OsAppRow {
                    id: app.clone(),
                    label: clients::find_app(&app)
                        .map(|a| a.label)
                        .unwrap_or_else(|| app.clone()),
                    running: false,
                    focused: false,
                });
            }
            let row = rows.iter_mut().find(|r| r.id == app).unwrap();
            row.running = true;
            if focused == Some(id) {
                row.focused = true;
            }
        }
        rows
    }

    /// The oldest window of `app` (launch-or-focus's `head -n1` rule).
    fn client_for_app(&mut self, app: &str) -> Option<ClientId> {
        let state = self.state_mut();
        let mut ids: Vec<ClientId> = state
            .clients
            .iter()
            .filter(|(id, s)| {
                s.app == app
                    && !s.warm
                    && !s.pane
                    && s.closing.is_none()
                    && state.screens.screen_of(**id).is_some_and(|i| i < state.screens.live_count())
            })
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids.first().copied()
    }

    /// The WM as a service. Every branch answers; every answer is bounded.
    fn answer_os_call(&mut self, cx: &mut Cx, call: &ServiceCall) -> ToolResult {
        let id = call.call_id.as_str();
        let mut result = match call.tool.as_str() {
            "list" => os_list_result(id, &self.app_rows()),
            "launch" => match AiBus::app_arg(call) {
                None => ToolResult::refused(id, "launch needs an `app` id from os.list"),
                // a Wayland app of the launcher's catalog
                Some(app) if app.starts_with("wl.") => {
                    if !wayland_host::menu_apps().iter().any(|(a, _)| *a == app) {
                        ToolResult::refused(id, format!("no foreign app `{app}`; os.list lists them"))
                    } else {
                        let running = self.client_for_app(&app).is_some();
                        self.launch_app(cx, &app);
                        os_launch_answer(id, &app, running)
                    }
                }
                Some(app) if app == "aichat" => {
                    ToolResult::ok(id, "the assistant is already open", "already open")
                }
                Some(app) => match clients::find_app(&app) {
                    None => ToolResult::refused(
                        id,
                        format!("no app `{app}`; known: {}", known_app_ids()),
                    ),
                    Some(def) if !self.processes() && !self.domains_enabled()
                        && self.apps.hosting(&def.id) != Hosting::Module => {
                        ToolResult::unavailable(
                            id,
                            format!(
                                "{} is not part of this build; it links {}",
                                def.label,
                                self.apps.linked_ids().join(", ")
                            ),
                        )
                    }
                    Some(def) => {
                        // Already up: say so plainly, so the model calls the
                        // app's tools instead of waiting for it to appear.
                        let running = def.policy == LaunchPolicy::OrFocus && self.client_for_app(&def.id).is_some();
                        if let (true, Some(client)) = (running, self.client_for_app(&def.id)) {
                            self.focus_client(cx, client);
                        } else {
                            self.launch_app(cx, &def.id);
                        }
                        // The person is talking to the chat: the new
                        // window takes the layout's focus, not their keys
                        // — including the focus a tile that has not drawn
                        // yet would otherwise claim on its first frame.
                        self.pending_focus = None;
                        self.focus_pane(cx);
                        os_launch_answer(id, &def.label, running)
                    }
                },
            },
            "run" => match AiBus::str_arg(call, "command") {
                None => ToolResult::refused(id, "run needs a `command` line"),
                Some(command) if !cfg!(target_os = "linux") => {
                    ToolResult::unavailable(id, format!("no Wayland server here to run `{command}`"))
                }
                Some(command) => {
                    self.launch_app(cx, &format!("wayland={command}"));
                    ToolResult::ok(id, format!("started `{command}` as a Wayland client"), "started")
                }
            },
            "focus" | "close" | "maximize" => match AiBus::app_arg(call) {
                None => ToolResult::refused(id, format!("{} needs an `app` id", call.tool)),
                Some(app) => match self.client_for_app(&app) {
                    None => ToolResult::failed(id, format!("{app} is not running")),
                    Some(client) if call.tool == "maximize" => {
                        self.apply_snap(cx, client, crate::snap::Zone::Maximize);
                        self.pending_focus = None;
                        self.focus_pane(cx);
                        ToolResult::ok(id, format!("{app} fills the desktop"), "maximized")
                    }
                    Some(client) if call.tool == "focus" => {
                        self.focus_client(cx, client);
                        self.pending_focus = None;
                        self.focus_pane(cx);
                        ToolResult::ok(id, format!("{app} is in front"), "focused")
                    }
                    Some(client) => {
                        self.request_close(cx, client);
                        ToolResult::ok(id, format!("{app} is closing"), "closing")
                    }
                },
            },
            "open" => match AiBus::str_arg(call, "path") {
                None => ToolResult::refused(id, "open needs a `path`"),
                Some(path) => {
                    let req = WmRequest::Open {
                        app: AiBus::app_arg(call),
                        path: path.clone(),
                    };
                    match preview::OpenRequest::from_request(&req) {
                        Some(open) if clients::find_app(&open.app).is_some() => {
                            let app = open.app.clone();
                            self.open_request(cx, open);
                            ToolResult::ok(id, format!("opening {path} in {app}"), "opening")
                        }
                        _ => ToolResult::refused(id, format!("no app can open {path}")),
                    }
                }
            },
            other => ToolResult::refused(
                id,
                format!("os has no tool `{other}`; it has list, launch, run, focus, maximize, close, open"),
            ),
        };
        result.bound();
        result
    }

    // --------------------------------------------------------------
    // Hub routing
    // --------------------------------------------------------------

    fn drain_hub(&mut self, cx: &mut Cx) {
        #[cfg(target_os = "linux")]
        self.drain_adoptions(cx);
        let Some(hub) = self.hub.as_ref() else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(event) = hub.rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                HubEvent::Connected {
                    client,
                    socket,
                    sender,
                } => {
                    let theme_splash = theme::theme_splash_path(&self.state_mut().theme_name)
                        .to_string_lossy()
                        .to_string();
                    if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                        slot.sender = Some(sender);
                        slot.socket = Some(socket);
                        if let Some(sender) = &slot.sender {
                            send_to_app(
                                sender,
                                vec![StudioToApp::Custom(
                                    WmEvent::Hosted { theme_splash }.to_json(),
                                )],
                            );
                        }
                    }
                    self.send_desktop_style(client);
                    // The pane just connected (or reconnected): it learns
                    // every current registration, the WM's own first.
                    if self.ai_bus.is_pane(client) {
                        self.replay_bus_to_pane();
                    }
                    // A Quick-Look retarget that arrived while this viewer
                    // was still starting: it has a socket now, so land it.
                    if let Some(path) = self.preview_cache.take_pending(client) {
                        self.send_wm_event(client, WmEvent::PreviewFile { path });
                    }
                    // Its screens, once its tile is laid out.
                    self.resend_screens_event(client);
                }
                HubEvent::Disconnected { socket } => {
                    let client = self
                        .state_mut()
                        .clients
                        .iter()
                        .find(|(_, s)| s.socket == Some(socket))
                        .map(|(id, _)| *id);
                    if let Some(client) = client {
                        let mut adopted = false;
                        if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                            slot.sender = None;
                            slot.socket = None;
                            adopted = slot.external;
                        }
                        // An app that came by itself has no process here to reap: its
                        // socket closing is its end.
                        if adopted {
                            self.remove_client(cx, client);
                            self.update_bar(cx);
                        }
                    }
                }
                HubEvent::FromApp { client, msgs } => {
                    for msg in msgs {
                        self.on_app_msg(cx, client, msg);
                    }
                }
                // Built from other protocol sources than this WM: it would
                // hang before its first frame. Say so and let go of it.
                HubEvent::Mismatch { client, .. } => {
                    let app = self.state_mut().clients.get(&client).map(|slot| slot.app.clone()).unwrap_or_default();
                    let label = makepad_app_registry::find(&app).map_or(app.as_str(), |entry| entry.label).to_string();
                    log!("wm: client {client} ({label}) was built from other protocol sources; closing it");
                    self.notify(cx, &format!("{label} needs a rebuild"), &format!("{label} was built for another version of the WM (its hosted protocol differs): rebuild it"));
                    self.remove_client(cx, client);
                }
            }
        }
    }

    /// The app `client` made another window (`window_id`): a window of its own on the desk,
    /// a tile showing that window, with the app's sender and no process of its own.
    fn open_later_window(&mut self, cx: &mut Cx, client: ClientId, window_id: usize) {
        if self.client_windows.route(client, window_id) != client || self.is_warm(client) || self.ai_bus.is_pane(client) {
            return;
        }
        let id = self.next_id;
        let Some(owner) = self.state_mut().clients.get(&client) else { return };
        if owner.parent.is_some() || owner.window_id == window_id {
            return;
        }
        let slot = clients::ClientSlot::later_window(id, client, owner, window_id);
        self.next_id += 1;
        let origin = self.client_windows.next_origin(client);
        self.client_windows.add(client, window_id, id);
        log!("wm: client {client} opened window {window_id}: client {id}");
        self.state_mut().clients.insert(id, slot);
        let area = self.desk_area(cx);
        let state = self.state_mut();
        let gap = state.gap;
        // It opens on the app's screen, on a desktop (never inside the app's fullscreen
        // space, which holds the app's window only).
        if let Some(screen) = state.screens.screen_of(client) {
            state.screens.active = screen;
        }
        state.layout_mut().insert_on_desktop(id, area, gap);
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.set_window_owner(id, client);
            // What the app said of its pointer input holds for this window too.
            let mouse_cancel = d.with_run_view(cx, client, |_, v| v.hosted.takes_mouse_cancel()).unwrap_or(false);
            d.with_run_view(cx, id, |_, v| {
                v.hosted.set_pointer_caps(makepad_widgets::makepad_platform::ime::HostedPointerCaps { mouse_cancel })
            });
            d.with_run_view(cx, id, |cx, v| {
                v.set_window_origin(dvec2(origin, 0.0));
                v.set_later_window_target(cx, id, window_id, client);
                v.app_ready(cx, id, window_id);
            })
        });
        self.activate_client(cx, id);
        self.redraw_all(cx);
    }

    /// The app `client` closed one of its later windows, or asked something of it.
    fn later_window_op(&mut self, cx: &mut Cx, client: ClientId, window: makepad_platform::ime::HostedWindow) {
        use makepad_platform::ime::hosted_window_op as op;
        // Any window's asked size, the app's first window included.
        if window.op == op::SIZE {
            let tile = self.client_windows.route(client, window.window_id);
            self.apply_requested_size(cx, tile, window.x, window.y);
            return;
        }
        if window.op == op::CLOSED {
            if let Some(tile) = self.client_windows.closed(client, window.window_id) {
                log!("wm: client {client} closed window {}: client {tile} goes", window.window_id);
                self.remove_client(cx, tile);
                // The app's own window is where the person goes back to.
                if self.state_mut().clients.contains_key(&client) {
                    self.focus_client(cx, client);
                }
            }
            return;
        }
        let tile = self.client_windows.route(client, window.window_id);
        if tile == client || !self.state_mut().clients.contains_key(&tile) {
            return;
        }
        match window.op {
            op::MINIMIZE => self.minimize_desktop(cx, tile),
            op::MAXIMIZE => self.maximise_toggle(cx, tile),
            op::FULLSCREEN => self.fullscreen_request(cx, tile, true),
            op::RESTORE => self.fullscreen_request(cx, tile, false),
            op::MOVE => self.move_later_window(cx, client, tile, dvec2(window.x, window.y)),
            op::TITLE => {
                if let Some(slot) = self.state_mut().clients.get_mut(&tile) {
                    slot.title = window.title;
                }
                self.update_bar(cx);
                self.redraw_all(cx);
            }
            _ => {}
        }
    }

    /// The inner size an app asked for its window shown by `tile`: on a floating desktop the
    /// window opens at that size (plus the desk's title bar), within its screen's usable
    /// area (minus bar and dock), centred.
    fn apply_requested_size(&mut self, cx: &mut Cx, tile: ClientId, w: f64, h: f64) {
        if !self.state_mut().style.target.floating() || self.is_warm(tile) {
            return;
        }
        let Some(screen) = self.state_mut().screens.screen_of(tile) else { return };
        let mut area = self.screen_area(cx, screen);
        let state = self.state_mut();
        let reserve = state.layout_of(tile).desktop.shelf_reserve;
        area.h = (area.h - reserve).max(1.0);
        let title = state.style.title_height();
        if state.layout_of_mut(tile).desktop.request_size(tile, w, h + title, area) {
            log!("wm: client {tile} opens at the {w}x{h} it asked for (within {:.0}x{:.0})", area.w, area.h);
            self.redraw_all(cx);
        }
    }

    /// Put a later window on the screen at `at`, a point in the app's coordinates (those of
    /// the screens it was told: relative to its first window's tile).
    fn move_later_window(&mut self, cx: &mut Cx, client: ClientId, tile: ClientId, at: Vec2d) {
        let origin = self.state_mut().tile_origins.get(&client).map_or(dvec2(0.0, 0.0), |r| dvec2(r.x, r.y));
        let p = origin + at;
        let state = self.state_mut();
        let target = state.screen_under(p.x, p.y);
        let (gap, reserved, gaps_out) = (state.gap, state.style.reserved_height(), state.gaps_out);
        for _ in 0..state.screens.live_count() {
            let state = self.state_mut();
            if state.screens.screen_of(tile) == Some(target) {
                break;
            }
            if state.screens.move_to_screen(tile, true, gap, reserved, gaps_out).is_none() {
                break;
            }
        }
        self.focus_client(cx, tile);
        self.redraw_all(cx);
    }

    fn on_app_msg(&mut self, cx: &mut Cx, client: ClientId, msg: AppToStudio) {
        match msg {
            AppToStudio::CreateWindow { window_id, .. } => {
                // A multi-window app (the VJ: console + output window)
                // announces every window; the tile hosts the FIRST — the
                // app's main UI — never a later output/aux window.
                let first = self
                    .state_mut()
                    .clients
                    .get_mut(&client)
                    .map(|slot| {
                        if slot.ready {
                            false
                        } else {
                            slot.window_id = window_id;
                            slot.ready = true;
                            true
                        }
                    })
                    .unwrap_or(false);
                if !first {
                    self.open_later_window(cx, client, window_id);
                }
                if first {
                    // A warm instance has no tile to be ready IN: it gets
                    // a framebuffer of its own instead, and draws into it
                    // until someone adopts it. The pane's child is ready
                    // in the pane's own run view.
                    if self.is_warm(client) {
                        self.warm_bootstrap(cx, client, window_id);
                    } else if self.ai_bus.is_pane(client) {
                        self.with_pane_run_view(cx, |cx, v| v.app_ready(cx, client, window_id));
                    } else {
                        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                            d.with_run_view(cx, client, |cx, v| v.app_ready(cx, client, window_id))
                        });
                    }
                    self.send_desktop_style(client);
                    // A home tile's client hears which face to show before
                    // its first frame (mobile_app.rs).
                    self.replay_tile_face(cx, client);
                }
            }
            AppToStudio::DrawCompleteAndFlip(pd) => {
                // A later window's frame is its own tile's.
                let client = self.client_windows.route(client, pd.window_id);
                makepad_app_host::hosted::trace_host(host::wall_now(), &format!("rx-flip c{}", client));
                // A CLOSING client's tile is frozen on its last good frame:
                // the zoom-out plays over that, never over the app's own
                // shutdown relayout/clipping.
                let closing = self
                    .state_mut()
                    .clients
                    .get(&client)
                    .map(|s| s.closing.is_some())
                    .unwrap_or(false);
                if closing {
                    return;
                }
                // A dormant instance's frames land in its own off-desk
                // framebuffer; there is no tile to present them in, and
                // touching the desk here would build one. The first frame
                // is the moment it is warm all the way through.
                if self.is_warm(client) {
                    if let Some(frame) = self.warm_frames.get_mut(&client) {
                        if !frame.presented {
                            frame.presented = true;
                            log!("wm: warm client {} drew its first frame", client);
                        }
                    }
                    return;
                }
                if self.ai_bus.is_pane(client) {
                    self.with_pane_run_view(cx, |cx, v| v.set_presentable_draw(cx, pd));
                    // The pane's app is drawn into the scene itself, not as a desk layer.
                    if let Some(mut scene) = self.ui.widget(cx, ids!(scene)).borrow_mut::<scene::WmScene>() {
                        scene.redraw_content_all(cx);
                    }
                    self.note_first_frame(client);
                    return;
                }
                // Which of the phone's captures this frame may refresh: a
                // tile client's frames are sorted by size, so a card never
                // shows a stretched tile and a tile never a squeezed window.
                let face = self.note_client_frame_face(cx, client, pd.width, pd.height);
                self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                    d.note_client_frame(client, face);
                    d.with_run_view(cx, client, |cx, v| v.set_presentable_draw(cx, pd))
                });
                // The hosted frame is produced in a separate capture pass;
                // invalidate the outer cached scene explicitly.
                self.redraw_scene_content(cx);
                self.note_first_frame(client);
                // A focus that couldn't land at launch (tile not yet
                // drawn) lands now that the client has a frame.
                // (Not when the person has gone to another space since: the
                // window's first frame is no reason to pull them back.)
                if self.pending_focus == Some(client) {
                    let shown = {
                        let l = self.state_mut().layout_of(client);
                        l.workspace_of(client) == Some(l.active)
                    };
                    if shown {
                        self.focus_client(cx, client);
                    } else {
                        self.pending_focus = None;
                    }
                }
            }
            AppToStudio::SetCursor(cursor) => {
                if self.ai_bus.is_pane(client) {
                    self.with_pane_run_view(cx, |cx, v| v.set_remote_cursor(cx, cursor.into()));
                } else {
                    self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                        d.with_run_view(cx, client, |cx, v| v.set_remote_cursor(cx, cursor.into()))
                    });
                }
                // Keep the child's request for its next hover, but a delayed
                // reply must not replace the cursor of a WM grab or menu.
                if self.drag.is_some() {
                    self.apply_drag_cursor(cx);
                } else if let Some(drag) = self.div_drag.as_ref() {
                    let axis = drag.hit.axis;
                    self.apply_divider_cursor(cx, Some(axis));
                } else {
                    let menu_open = self.ui.widget(cx, ids!(shell_menu))
                        .borrow::<ShellMenu>().is_some_and(|menu| menu.is_open());
                    let panel_open = self.ui.widget(cx, ids!(shell_panel))
                        .borrow::<shell::panels::ShellPanel>().is_some_and(|panel| panel.open.is_some());
                    if menu_open || panel_open {
                        cx.set_cursor(MouseCursor::Default);
                    }
                }
                // VR mode draws its own marker over its output.
                if let Some((x, y)) = self.state.as_ref().and_then(|s| s.vr.pointer) {
                    if self.state.as_ref().is_some_and(|s| s.vr.hides_cursor(x, y)) {
                        cx.set_cursor(MouseCursor::Hidden);
                    }
                }
            }
            AppToStudio::RequestAnimationFrame | AppToStudio::WakeIn(_) => {
                // The child's demand for its next Tick. A warm instance is
                // pumped by heartbeat, not by a tile.
                if self.is_warm(client) {
                    return;
                }
                let secs = if let AppToStudio::WakeIn(secs) = &msg { Some(*secs) } else { None };
                let demand = move |cx: &mut Cx, v: &mut MpRunView| match secs {
                    Some(secs) => v.wake_in(cx, secs),
                    None => v.request_tick(cx),
                };
                if self.ai_bus.is_pane(client) {
                    self.with_pane_run_view(cx, demand);
                } else {
                    self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| d.with_run_view(cx, client, demand));
                }
            }
            AppToStudio::TickDone => {
                makepad_app_host::hosted::trace_host(host::wall_now(), &format!("rx-tickdone c{}", client));
                // A warm instance is pumped by heartbeat, not by a tile.
                if self.is_warm(client) {
                    return;
                }
                if self.ai_bus.is_pane(client) {
                    self.with_pane_run_view(cx, |cx, v| v.tick_done(cx));
                } else {
                    self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                        d.with_run_view(cx, client, |cx, v| v.tick_done(cx))
                    });
                }
            }
            AppToStudio::SetClipboard(text) => {
                cx.copy_to_clipboard(&text);
                // the Wayland clients (a browser) paste it too
                wayland_host::send(wayland_host::ViewCmd::Selection { primary: false, text });
            }
            AppToStudio::Relay(relay) => {
                // Only a client the WM launched and still holds a channel to
                // is served (the hub listens on 127.0.0.1 with no per-launch
                // token yet; see local/agent_state/wm-protocol/report.md).
                let known = self.state.as_ref().is_some_and(|state| {
                    state.clients.get(&client).is_some_and(|slot| slot.sender.is_some())
                });
                if !known {
                    log!("wm: relay from unknown client {} ignored", client);
                    return;
                }
                // A file dialog: the WM draws the picker over the window that asked
                // (a hosted app has no display of its own to open a system one on).
                #[cfg(not(target_os = "android"))]
                if let makepad_widgets::makepad_platform::studio::ChildRelay::FileDialog(dialog) = relay {
                    let anchor = self
                        .desk(cx)
                        .borrow_mut::<WmDesk>()
                        .and_then(|mut d| d.with_run_view(cx, client, |cx, v| v.area().rect(cx)));
                    let picker = self.ui.widget(cx, ids!(file_picker));
                    if let Some(mut p) = picker.borrow_mut::<shell::file_picker::ShellFilePicker>() {
                        p.request(cx, shell::file_picker::PickRequest { from: shell::file_picker::Requester::Client(client), relay: dialog, anchor });
                    }
                    cx.set_key_focus(Area::Empty);
                    self.redraw_all(cx);
                    return;
                }
                // The child's window starts where its view is drawn.
                let origin = self
                    .desk(cx)
                    .borrow_mut::<WmDesk>()
                    .and_then(|mut d| d.with_run_view(cx, client, |cx, v| v.area().rect(cx).pos))
                    .unwrap_or_default();
                // a hosted app's selection is the Wayland clients' primary selection too
                if let ChildRelay::SetPrimarySelection { text } = &relay {
                    wayland_host::send(wayland_host::ViewCmd::Selection { primary: true, text: text.clone() });
                }
                let answers = self.relay.on_child(cx, client, origin, relay);
                self.send_relays(answers);
            }
            AppToStudio::Custom(json) => {
                if let Some(status) = makepad_app_host::AppStatus::parse(&json) {
                    let text = status.label();
                    if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                        slot.status = text.clone();
                    }
                    if self.ai_bus.is_pane(client) {
                        self.with_pane_run_view(cx, |cx, view| view.set_status_line(cx, &text));
                    } else if !self.is_warm(client) {
                        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
                            desk.with_run_view(cx, client, |cx, view| view.set_status_line(cx, &text));
                        }
                    }
                    return;
                }
                // What the child's pointer input understands: a child that
                // never says so gets no `MouseCancel` (see MpRunView).
                if let Some(caps) = makepad_platform::ime::HostedPointerCaps::parse(&json) {
                    // Every window of the app: a later window's view relays its cancelled
                    // presses only when it knows the app takes them.
                    let tiles = self.client_windows.tiles_of(client);
                    if let Some(mut d) = self.desk(cx).borrow_mut::<WmDesk>() {
                        for tile in tiles {
                            d.with_run_view(cx, tile, |_, v| v.hosted.set_pointer_caps(caps.clone()));
                        }
                    }
                    return;
                }
                // One of the app's later windows: closed by the app, or what it asked of it.
                if let Some(window) = makepad_platform::ime::HostedWindow::parse(&json) {
                    self.later_window_op(cx, client, window);
                    return;
                }
                // The app's own caption buttons (own_chrome.rs): the desk does
                // what its own close / minimise / maximise would.
                if let Some(op) = makepad_studio_protocol::HostedWindowOp::parse(&json) {
                    let state = {
                        let layout = self.state_mut().layout_of(client);
                        own_chrome::WindowState { maximised: layout.is_maximised(client), fullscreen: layout.is_fullscreen_space(client) }
                    };
                    match own_chrome::route_window_op(&op, state) {
                        Some(own_chrome::WindowAction::Close) => self.request_close(cx, client),
                        Some(own_chrome::WindowAction::Minimize) => self.minimize_desktop(cx, client),
                        // The app's maximise button toggles maximise, as a double click on its
                        // caption does (the window keeps its caption; fullscreen is the app's
                        // own fullscreen request).
                        Some(own_chrome::WindowAction::Maximise(_)) => self.maximise_toggle(cx, client),
                        Some(own_chrome::WindowAction::LeaveFullscreen) => self.fullscreen_request(cx, client, false),
                        Some(own_chrome::WindowAction::EnterFullscreen) => self.fullscreen_request(cx, client, true),
                        None => {}
                    }
                    return;
                }
                if let Some(answer) = makepad_studio_protocol::HostedDragAnswer::parse(&json) {
                    let now = host::now();
                    // The answer comes on the app's channel; the press was made in one of its
                    // windows (its later windows are desk clients of their own).
                    let tiles = self.client_windows.tiles_of(client);
                    let mut start = None;
                    if let Some(mut d) = self.desk(cx).borrow_mut::<WmDesk>() {
                        for tile in tiles {
                            if let Some(press) = d.with_run_view(cx, tile, |cx, v| v.take_caption_press(cx, &answer, now)).flatten() {
                                start = Some((tile, press));
                                break;
                            }
                        }
                    }
                    if let Some((client, press)) = start {
                        // The app's caption is a title bar: a second press there soon after
                        // the first maximises, as on the desk's own title bars (a quick click
                        // is released before the answer comes: it counts, but drags nothing).
                        let abs = press.abs();
                        let double = self.title_press.is_some_and(|(c, time, pos)| {
                            c == client && now - time < 0.5 && (pos - abs).length() < 5.0
                        });
                        if double {
                            self.title_press = None;
                            // What the window declared (`double_click` on its `Window`):
                            // maximise in place, or fullscreen on its screen (an output window).
                            let state = {
                                let layout = self.state_mut().layout_of(client);
                                own_chrome::WindowState { maximised: layout.is_maximised(client), fullscreen: layout.is_fullscreen_space(client) }
                            };
                            match own_chrome::caption_double_click(answer.double_click, state) {
                                own_chrome::WindowAction::EnterFullscreen => self.fullscreen_request(cx, client, true),
                                own_chrome::WindowAction::LeaveFullscreen => self.fullscreen_request(cx, client, false),
                                _ => self.maximise_toggle(cx, client),
                            }
                        } else {
                            self.title_press = Some((client, now, abs));
                            if let own_chrome::CaptionPress::Drag(abs) = press {
                                self.begin_drag_on(cx, client, abs, false, false);
                            }
                        }
                    }
                    return;
                }
                // The app's compact forms, declared at connect (spacewarp/compact.rs).
                if let Some(declared) = makepad_platform::ime::HostedCompactForms::parse(&json) {
                    if let Some(mut d) = self.desk(cx).borrow_mut::<WmDesk>() {
                        d.warp.compact.declare(client, declared.forms);
                    }
                    return;
                }
                if let Some(answer) = makepad_platform::ime::HostedCompact::parse(&json) {
                    let undo = self.desk(cx).borrow_mut::<WmDesk>().and_then(|mut d| d.warp.compact.answer(client, answer.form, host::now()));
                    if let Some(undo) = undo {
                        if let Some(mut d) = self.desk(cx).borrow_mut::<WmDesk>() {
                            d.with_run_view(cx, client, |cx, v| v.send_parked(cx, &undo));
                        }
                    }
                    // The slot is the compact window's size now: pack the zone again.
                    if let Some(screen) = self.state_mut().screens.screen_of(client) {
                        self.warp_settle(cx, screen, None);
                    }
                    self.redraw_all(cx);
                    return;
                }
                if let Some(back) = makepad_platform::ime::HostedBack::parse(&json) {
                    if back.handled == Some(false) && self.state_mut().phone.client == Some(client) {
                        self.state_mut().phone.navigate(mobile::PhoneScreen::Home);
                        self.animate_phone(cx);
                    }
                    return;
                }
                if let Some(ime) = makepad_platform::ime::HostedImeState::parse(&json) {
                    if let Some(mut d) = self.desk(cx).borrow_mut::<WmDesk>() {
                        d.with_run_view(cx, client, |cx, v| v.set_child_ime(cx, ime));
                    }
                    self.state_mut().phone.ime.insert(client,ime);
                    self.sync_phone_keyboard(cx);
                    return;
                }
                // The typed app<->WM vocabulary (libs/wm_api) first; what
                // is not the WM's own envelope is the AI bus's (or noise).
                if let Some(req) = WmRequest::parse(&json) {
                    if self.ai_bus.is_pane(client) {
                        self.on_pane_request(cx, req);
                    } else {
                        self.on_wm_request(cx, client, req);
                    }
                    return;
                }
                let route = self.ai_bus.on_custom(client, &json);
                self.on_bus_route(cx, route);
                self.drain_bus_registry();
            }
            AppToStudio::LogItem(item) => {
                // Hosted children log over the studio socket, not stdout —
                // dropping these makes them undebuggable. Into our own log
                // (and thus the remote bridge's /log) they go.
                log!("client {}: {}", client, item.message);
            }
            _ => {}
        }
    }

    // --------------------------------------------------------------
    // Menu
    // --------------------------------------------------------------

    // --------------------------------------------------------------
    // Theme / background
    // --------------------------------------------------------------

    fn set_theme(&mut self, cx: &mut Cx, name: &str) {
        let state = self.state_mut();
        state.theme_name = name.to_string();
        let source = theme::load_theme_source(name);
        if let Some(palette) = theme::scan_term_palette(&source) {
            state.term_env = palette.env_value();
        }
        if let Some(rgb) = scan_theme_color(&source, "accent") {
            state.accent = rgb;
        }
        state.borders = desk::BorderTheme::from_theme_source(&source);
        if let Some(skin) = skins::Skin::from_theme_source(&source).filter(|_| skins::Skin::from_env().is_none()) {
            state.skin = skin;
        }
        // The desk theme choice outlives this run (a state file natively,
        // the desk's storage on the web). Apps keep the system stylesheet.
        host::persist_theme_choice(cx, name);
        let style = self.state_mut().style.target;
        if style == desktop::DesktopStyle::Omarchy {
            let dark = desktop_app::browser_appearance(style, false, &source);
            self.refresh_warm_browser_appearance(cx, dark);
        }
        // Chrome DSL colors refresh fully on restart; borders, terminal
        // palette and backgrounds apply immediately.
        self.apply_background(cx, 0);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// A theme with no pictures yet fetches them from the omarchy repo on a
    /// thread of its own; the desk shows the first one when it lands (the
    /// tick polls `theme_backgrounds` while `backgrounds_pending`). The
    /// bundled default theme ships without wallpapers, so a fresh install
    /// gets the omarchy look on its own; no network, no pictures, no harm.
    fn fetch_backgrounds_if_missing(&mut self, cx: &mut Cx) {
        if !self.processes() {
            return;
        }
        let name = self.state_mut().theme_name.clone();
        if !theme::theme_backgrounds(&name).is_empty() {
            return;
        }
        self.backgrounds_pending = true;
        log!("wm: theme '{}' has no wallpapers; fetching them", name);
        match cx.task_pool().submit(Lane::Heavy, move || {
                let n = theme::fetch_backgrounds_if_missing(&name);
                log!("wm: fetched {} wallpaper(s) for '{}'", n, name);
                n
            }) {
            Ok(task) => self.background_task = Some(task),
            Err(error) => {
                self.backgrounds_pending = false;
                log!("wm: could not queue wallpaper fetch: {error}");
            }
        }
    }

    fn poll_backgrounds(&mut self, cx: &mut Cx) {
        let Some(task) = self.background_task.as_mut() else {
            return;
        };
        let Some(result) = task.try_take() else {
            return;
        };
        self.background_task = None;
        self.backgrounds_pending = false;
        match result {
            Ok(count) if count > 0 => self.apply_background(cx, 0),
            Ok(_) => {}
            Err(error) => log!("wm: wallpaper fetch task failed: {error}"),
        }
    }

    /// SUPER+CTRL+SPACE — the theme's next wallpaper.
    fn next_background(&mut self, cx: &mut Cx) {
        self.background_index += 1;
        let idx = self.background_index;
        self.apply_background(cx, idx);
    }

    fn apply_background(&mut self, cx: &mut Cx, index: usize) {
        let name = self.state_mut().theme_name.clone();
        let mut choices = backdrop::choices(theme::theme_backgrounds(&name));
        let classic = self.state_mut().warp.mode == spacewarp::Mode::Classic;
        // A flat desktop has the theme's own background, not the procedural ones.
        if classic {
            choices.retain(|c| !matches!(c, backdrop::Choice::Procedural(_)));
        }
        // MAKEPAD_WM_BACKDROP names the procedural background to start on;
        // the warp desktop's own is the warp grid.
        let choice = backdrop::env_kind()
            .filter(|_| index == 0 && !classic)
            .or_else(|| (index == 0 && !classic).then_some(backdrop::Kind::WarpGrid))
            .map(backdrop::Choice::Procedural)
            .unwrap_or_else(|| choices[index % choices.len()].clone());
        let kind = match &choice {
            backdrop::Choice::Procedural(kind) => Some(*kind),
            _ => None,
        };
        // Without the procedural layer (a script error dropped it) the gradient stays:
        // the desk is never left to whatever the wallpaper cache was cleared to.
        let kind = match self.ui.widget(cx, ids!(wallpaper_backdrop)).borrow_mut::<backdrop::WmBackdrop>() {
            Some(mut layer) => {
                layer.set_kind(cx, kind);
                kind
            }
            None => {
                if kind.is_some() { error!("wm: no wallpaper_backdrop layer; the procedural background falls back to the gradient"); }
                None
            }
        };
        self.state_mut().backdrop.kind = kind;
        // A procedural background replaces the gradient and the picture.
        self.ui.widget(cx, ids!(bg_fill)).set_visible(cx, kind.is_none());
        let image = self.ui.widget(cx, ids!(bg_image));
        match choice {
            backdrop::Choice::Image(path) => {
                let image_ref = self.ui.image(cx, ids!(bg_image));
                if image_ref.load_image_file_by_path_async(cx, &path).is_ok() {
                    image.set_visible(cx, true);
                }
            }
            _ => image.set_visible(cx, false),
        }
        self.redraw_all(cx);
    }

    fn open_shell_menu(&mut self, cx: &mut Cx, path: &str, skin: MenuSkin) {
        cx.set_cursor(MouseCursor::Default);
        let path = if path.is_empty() && self.state_mut().style.target == desktop::DesktopStyle::NextStep {
            "workspace"
        } else { path };
        self.menu_screen = self.state_mut().screens.active;
        let menu = self.ui.widget(cx, ids!(shell_menu));
        {
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                m.open_at(cx, path, skin);
            }
        }
        self.shell_menu_opens += 1;
        log!("wm: shell menu open #{} at {path:?}", self.shell_menu_opens);
        self.dock_autohide_refresh(cx);
        self.redraw_all(cx);
    }

    /// The dock's "more" slot: a menu of the apps drawn after its first `shown`.
    fn open_dock_overflow(&mut self, cx: &mut Cx, shown: usize) {
        let rest: Vec<_> = desktop::dock_items(self.state_mut()).into_iter().skip(shown).collect();
        cx.set_cursor(MouseCursor::Default);
        self.menu_screen = self.state_mut().screens.active;
        let menu = self.ui.widget(cx, ids!(shell_menu));
        if let Some(m) = menu.borrow_mut::<ShellMenu>().as_mut() {
            m.open_dock_rest(cx, rest);
        }
        self.shell_menu_opens += 1;
        log!("wm: shell menu open #{} at dock overflow ({} apps)", self.shell_menu_opens, shown);
        self.dock_autohide_refresh(cx);
        self.redraw_all(cx);
    }

    fn close_shell_menu(&mut self, cx: &mut Cx) {
        log!("wm: shell menu closed");
        let menu = self.ui.widget(cx, ids!(shell_menu));
        {
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                m.close(cx);
            }
        }
        if let Some(focus) = self.state_mut().layout().focused_client() {
            self.focus_client(cx, focus);
        }
        self.dock_autohide_refresh(cx);
        self.redraw_all(cx);
    }

    /// The menu owns the keyboard while it is up (`Menu.qml` grabs it).
    fn shell_menu_key(&mut self, cx: &mut Cx, e: &KeyEvent) -> bool {
        let menu = self.ui.widget(cx, ids!(shell_menu));
        let mut consumed = false;
        {
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                if m.is_open() {
                    consumed = m.key(cx, e);
                }
            }
        }
        if consumed {
            self.redraw_all(cx);
        }
        consumed
    }

    /// The menu owns the POINTER while it is up too, for the same reason
    /// `shell_menu_key` grabs the keyboard: `ShellMenu::handle_pointer`
    /// does its own raw-coordinate hit testing (card / row rects) instead
    /// of Makepad's `Event::hits` area-exclusivity, so it never marks a
    /// click "handled" — routed through the ordinary widget tree, the SAME
    /// click that activates a menu row also falls through the scrim to
    /// whatever tile is dimmed underneath it (a click on the launcher's
    /// Browser row also landed on — and clicked — the terminal tile behind
    /// it). Called directly, before `self.ui.handle_event` ever sees the
    /// event, so nothing else gets a look at it while the menu is open.
    fn shell_menu_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let menu = self.ui.widget(cx, ids!(shell_menu));
        let mut consumed = false;
        {
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                if m.is_open() {
                    consumed = m.pointer(cx, event);
                }
            }
        }
        if consumed {
            self.redraw_all(cx);
        }
        consumed
    }

    /// As `shell_menu_pointer`, for a bar flyout: `ShellPanel::handle_event`
    /// has the identical raw-coordinate architecture (card / hit_at, no
    /// `Event::hits`), so a click, drag or release meant for an open panel
    /// (the clock/audio/power/monitor popup) would otherwise ALSO reach
    /// whatever tile sits behind the scrim. Scroll is left alone unless the
    /// panel asks for it (the Wi-Fi list over the card, once it is longer
    /// than the dropdown shows) — routing it blindly would only turn off
    /// SUPER+scroll workspace cycling for no reason.
    fn shell_panel_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let panel = self.ui.widget(cx, ids!(shell_panel));
        #[cfg(target_os = "linux")]
        if let Event::MouseDown(e) = event {
            let bar = self.ui.widget(cx, ids!(shell_bar));
            let module = bar.borrow::<shell::bar::ShellBar>().and_then(|bar| bar.module_at(e.abs));
            if module.and_then(shell::panels::PanelKind::for_module).is_some() {
                // Route only to the bar. The panel must not also close on
                // this outside click before the queued bar action toggles it.
                bar.handle_event(cx, event, &mut Scope::empty());
                return true;
            }
        }
        if let Event::Scroll(e) = event {
            let wants = panel
                .borrow::<shell::panels::ShellPanel>()
                .map(|p| p.wants_scroll(e.abs))
                .unwrap_or(false);
            if !wants {
                return false;
            }
        }
        let is_open = panel
            .borrow::<shell::panels::ShellPanel>()
            .map(|p| p.open.is_some())
            .unwrap_or(false);
        if !is_open {
            return false;
        }
        panel.handle_event(cx, event, &mut Scope::empty());
        self.redraw_all(cx);
        true
    }

    /// What a menu row does. The ids are the jsonc's dotted paths, with
    /// `apps.<id>` and `style.theme[.import].<name>` from the providers.
    fn shell_menu_activate(&mut self, cx: &mut Cx, target: &str) {
        log!("wm: shell menu activate {target}");
        if target=="start.documents" {self.launch_app(cx,"files");return;}
        if target=="start.power" {let seg=self.active_bar_segment();self.toggle_shell_panel(cx,seg,BarModule::Power);return;}
        if let Some(name) = target.strip_prefix("desktop.") {
            if let Some(entry) = desktop_style::find(name) {
                // The appearance toggle owns dark or light; a style pick
                // leaves it where it is.
                self.set_desktop_sheet(cx, entry);
                self.persist_desktop_look(cx);
                return;
            }
        }
        if let Some(app) = target.strip_prefix("dock.") {
            // An app from the dock's "more" list: what its dock icon would do.
            self.close_shell_menu(cx);
            self.activate_shelf(cx, desktop::ShelfHit::App(app.to_string()));
            return;
        }
        if let Some(app) = target.strip_prefix("apps.") {
            let app = app.to_string();
            self.close_shell_menu(cx);
            self.launch_app(cx, &app);
            return;
        }
        if let Some(name) = target.strip_prefix("style.theme.import.") {
            let slug = name.replace(' ', "-");
            self.close_shell_menu(cx);
            match theme::import_omarchy_theme(&slug) {
                Ok(msg) => log!("wm: {}", msg),
                Err(err) => log!("wm: import failed: {}", err),
            }
            self.set_theme(cx, &slug);
            return;
        }
        if let Some(name) = target.strip_prefix("style.theme.") {
            let name = name.replace(' ', "-");
            self.close_shell_menu(cx);
            self.set_theme(cx, &name);
            return;
        }
        match target {
            "desktop.window-maximise" | "desktop.window-fullscreen" | "desktop.window-restore" => {
                self.close_shell_menu(cx);
                if let Some(c) = self.state_mut().layout().focused_client() {
                    let mode = self.state_mut().layout_of(c).window_mode(c);
                    match (target, mode) {
                        ("desktop.window-maximise", WindowMode::Normal) => self.maximise_toggle(cx, c),
                        ("desktop.window-fullscreen", WindowMode::Fullscreen) => {}
                        ("desktop.window-fullscreen", _) => self.fullscreen_request(cx, c, true),
                        ("desktop.window-restore", WindowMode::Maximised) => self.maximise_toggle(cx, c),
                        ("desktop.window-restore", WindowMode::Fullscreen) => self.fullscreen_request(cx, c, false),
                        _ => {}
                    }
                }
            }
            "desktop.warp-shelves" => {
                let mode = if self.state_mut().warp.mode == spacewarp::Mode::Warp { spacewarp::Mode::Classic } else { spacewarp::Mode::Warp };
                self.set_desktop_mode(cx, mode);
            }
            "desktop.autohide-left-shelf" | "desktop.autohide-right-shelf" => {
                self.close_shell_menu(cx);
                let side = (target == "desktop.autohide-right-shelf") as usize;
                let on = !self.state_mut().shelf_autohide.enabled(side);
                self.set_shelf_autohide(cx, side, on);
            }
            "desktop.natural-scroll" => {
                self.close_shell_menu(cx);
                let on = !host::load_natural_scroll();
                host::persist_natural_scroll(on);
                host::apply_natural_scroll(on);
            }
            "desktop.tap-to-click" => {
                self.close_shell_menu(cx);
                let on = !host::load_tap_to_click();
                host::persist_tap_to_click(on);
                host::apply_tap_to_click(on);
            }
            "desktop.autohide-dock" => {
                self.close_shell_menu(cx);
                let on = !self.state_mut().dock_autohide.enabled;
                self.set_dock_autohide(cx, on);
            }
            "desktop.appearance" => {
                self.close_shell_menu(cx);
                self.toggle_desktop_appearance(cx);
            }
            "style.background" => {
                self.close_shell_menu(cx);
                self.next_background(cx);
            }
            "style.bar" => {
                self.close_shell_menu(cx);
                self.do_action(cx, WmAction::ToggleBar);
            }
            "style.vr" => {
                self.close_shell_menu(cx);
                self.do_action(cx, WmAction::ToggleVr);
            }
            "style.vr-recenter" => {
                self.close_shell_menu(cx);
                self.do_action(cx, WmAction::VrRecenter);
            }
            other => {
                log!("wm: menu row '{}' does nothing in nested mode", other);
                self.close_shell_menu(cx);
            }
        }
    }

    /// True where the shell bar has a clickable module under the point:
    /// those points answer Client to the drag query so the press reaches
    /// the widget instead of moving the OS window.
    fn shell_bar_claims(&self, cx: &mut Cx, p: Vec2d) -> bool {
        if self.state.as_ref().is_some_and(|state| state.style.target.mobile()) {
            return false;
        }
        let bar = self.ui.widget(cx, ids!(shell_bar));
        let borrowed = bar.borrow::<shell::bar::ShellBar>();
        borrowed
            .as_ref()
            .map(|b| b.module_at(p).is_some())
            .unwrap_or(false)
    }

    /// The bar is under `p`: anywhere on the strip, or, floating over the
    /// scene, on a segment it painted (not where a span's window shows
    /// through a hidden one).
    fn bar_covers(&self, cx: &mut Cx, p: Vec2d) -> bool {
        if self.bar_placed.is_none_or(|placed| placed.strip.is_some()) {
            return true;
        }
        // Hidden (ToggleBar): its last segments are stale.
        if self.bar_placed.is_some_and(|placed| placed.reserved_top <= 0.0) {
            return false;
        }
        let bar = self.ui.widget(cx, ids!(shell_bar));
        let borrowed = bar.borrow::<shell::bar::ShellBar>();
        borrowed.as_ref().is_some_and(|b| b.covers(p))
    }

    /// The bar floats over the scene (per-screen segments) and painted
    /// `p`: the desk below does not own that point.
    fn bar_floats_over(&self, cx: &mut Cx, p: Vec2d) -> bool {
        self.bar_placed.is_some_and(|placed| placed.strip.is_none()) && self.bar_covers(cx, p)
    }

    /// Toggle a bar module's flyout, anchored to the module itself.
    /// `seg` is the bar segment it was pressed in, so the flyout opens on
    /// that screen; pressed on another segment while its flyout is up, it
    /// moves there.
    fn toggle_shell_panel(&mut self, cx: &mut Cx, seg: usize, module: BarModule) {
        let Some(kind) = shell::panels::PanelKind::for_module(module) else {
            return;
        };
        let anchor = {
            let bar = self.ui.widget(cx, ids!(shell_bar));
            let borrowed = bar.borrow::<shell::bar::ShellBar>();
            borrowed
                .as_ref()
                .and_then(|b| b.module_rect(seg, module))
                .unwrap_or_default()
        };
        let open_seg = self.shell_panel_segment;
        self.shell_panel_segment = seg;
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let mut opened_network = false;
        {
            let mut borrowed = panel.borrow_mut::<shell::panels::ShellPanel>();
            if let Some(p) = borrowed.as_mut() {
                // The same module pressed on another screen's segment: the
                // flyout moves there instead of closing.
                if shell::bar::flyout_press_moves(p.open == Some(kind), open_seg, seg) {
                    p.move_to(cx, anchor);
                } else {
                    p.toggle(cx, kind, anchor);
                    opened_network = p.open == Some(shell::panels::PanelKind::Network);
                }
                self.shell_panel_open = p.open.map(|k| k.module());
            }
        }
        if opened_network {
            self.wifi_panel_opened(cx);
        }
        self.hub_poll(cx);
        self.update_bar(cx);
    }

    /// The AI hub: while its panel is open the whole snapshot is read into
    /// it; otherwise only the cheap activity flag, which pulses the bar's
    /// icon. Once a second (the status tick) and on every panel command.
    fn hub_poll(&mut self, cx: &mut Cx) {
        let processes = self.processes();
        self.hub_link.tick(processes, host::now());
        self.gpu_yield_report(cx);
        let panel = self.ui.widget(cx, ids!(shell_panel));
        let open = panel.borrow::<shell::panels::ShellPanel>().is_some_and(|p| p.open == Some(shell::panels::PanelKind::Hub));
        let active = if open {
            let snap = self.hub_link.get(processes).snapshot();
            let busy = snap.busy();
            if let Some(mut p) = panel.borrow_mut::<shell::panels::ShellPanel>() {
                p.hub_update(cx, snap);
            }
            busy
        } else {
            self.hub_link.get(processes).active()
        };
        if self.bar_sample.hub_active != active {
            self.bar_sample.hub_active = active;
            self.update_bar(cx);
        }
    }

    /// The HAL says the GPU is short of memory for the UI: ask the AI hub for room, in
    /// steps (`gpu_yield`), over the panel's own hub link.
    fn gpu_pressure(&mut self, e: &GpuPressureEvent) {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let now = START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64();
        let was_exhausted = self.yield_policy.exhausted();
        let processes = self.processes();
        match self.yield_policy.on_pressure(e.wanted_mb, e.reserve_mb, now) {
            Some(ask) if ask.mb == 0 => {
                log!("wm: GPU memory for the UI is back ({} MB free): the AI hub may grow its model back", e.free_mb);
                self.hub_link.get(processes).send(hub_control::HubCommand::Yield { mb: 0, unload: false, reserve_mb: ask.reserve_mb });
            }
            Some(ask) => {
                log!(
                    "wm: GPU short of {} MB for the UI ({} MB free, reserve {} MB{}): asking the AI hub for {} MB{}",
                    e.wanted_mb, e.free_mb, e.reserve_mb, if e.failed { ", a texture create failed" } else { "" },
                    ask.mb, if ask.unload { ", the model may be unloaded" } else { "" }
                );
                self.hub_link.get(processes).send(hub_control::HubCommand::Yield { mb: ask.mb, unload: ask.unload, reserve_mb: ask.reserve_mb });
            }
            None if e.wanted_mb == 0 => log!("wm: GPU memory for the UI is back ({} MB free)", e.free_mb),
            None if self.yield_policy.exhausted() && !was_exhausted => {
                log!("wm: GPU still short of {} MB after the AI hub's steps; textures wait for room", e.wanted_mb)
            }
            None => {}
        }
    }

    /// The hub's answer to a yield: logged, and shown when the user loses something.
    fn gpu_yield_report(&mut self, cx: &mut Cx) {
        let processes = self.processes();
        let Some(report) = self.hub_link.get(processes).take_report() else { return };
        log!("wm: AI hub: {report}");
        if let Some(notice) = gpu_yield::notice_for(&report) {
            self.notify(cx, "GPU memory", notice);
        }
    }

    /// The in-process notification API — `WmRequest::Notify{title, body}`
    /// lands here.
    pub fn notify(&mut self, cx: &mut Cx, title: &str, body: &str) {
        if let Some(state) = self.state.as_ref() {
            self.notes_screen = state.screens.active;
        }
        let notes = self.ui.widget(cx, ids!(shell_notes));
        {
            let mut borrowed = notes.borrow_mut::<shell::notifications::ShellNotifications>();
            if let Some(n) = borrowed.as_mut() {
                n.notify(cx, title, body);
            }
        }
        self.redraw_all(cx);
    }

    /// Show the volume OSD (the wheel over the audio module, and the
    /// volume keys).
    fn show_osd(&mut self, cx: &mut Cx, show: shell::osd::OsdShow) {
        if let Some(state) = self.state.as_ref() {
            self.osd_screen = state.screens.active;
        }
        let osd = self.ui.widget(cx, ids!(shell_osd));
        {
            let mut borrowed = osd.borrow_mut::<shell::osd::ShellOsd>();
            if let Some(o) = borrowed.as_mut() {
                o.present(cx, show);
            }
        }
        self.redraw_all(cx);
    }

    /// `osascript` the output volume, then say so on the OSD.
    fn set_volume(&mut self, cx: &mut Cx, level: u32) {
        #[cfg(target_os = "linux")]
        self.linux_control(cx, linux_controls::ControlAction::OutputVolume(level));
        #[cfg(not(target_os = "linux"))]
        {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("osascript")
                .args(["-e", &format!("set volume output volume {}", level)])
                .output();
        }
        self.bar_sample.volume = Some(level);
        self.show_osd(cx, shell::osd::OsdShow::volume(level, self.bar_sample.muted));
        self.update_bar(cx);
        }
    }

    fn toggle_output_mute(&mut self, cx: &mut Cx) {
        #[cfg(target_os = "linux")]
        self.linux_control(cx, linux_controls::ControlAction::Command(shell::system_linux::SystemCommand::ToggleOutputMute));
        #[cfg(not(target_os = "linux"))]
        {
            self.bar_sample.muted = !self.bar_sample.muted;
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("osascript")
                    .args(["-e", &format!("set volume output muted {}", self.bar_sample.muted)])
                    .output();
            }
            self.update_bar(cx);
        }
    }

    /// Feed the shell bar. Workspaces and the active window come from the
    /// WM; the status modules come from `bar_sample`, refreshed on the tick
    /// (`shell/bar.rs` does the sampling — cheap things every second, the
    /// expensive ones every fifth).
    fn update_bar(&mut self, cx: &mut Cx) {
        self.sync_bar_placement(cx);
        if let Some(clock)=self.bar_sample.clock.split_whitespace().find(|s|s.contains(':')).map(str::to_string) {
            self.state_mut().phone.clock=clock;
        }
        // One segment per live screen on a multi-screen desktop, else the
        // one bar for the active screen, as always.
        let segmented = self.bar_segmented();
        let (active, screens): (usize, Vec<usize>) = {
            let screens = &self.state_mut().screens;
            let list = if segmented { (0..screens.live_count()).collect() } else { vec![screens.active] };
            (screens.active, list)
        };
        let mut shown: Vec<Vec<usize>> = Vec::with_capacity(screens.len());
        let mut parts: Vec<(Vec<shell::bar::WorkspaceCell>, Option<String>, LRect, bool, bool)> = Vec::new();
        {
            let state = self.state_mut();
            for &i in &screens {
                let layout = state.layout_at(i);
                let (cells, cell_ws) = shell::bar::workspace_cells(layout);
                let title = layout
                    .focused_client()
                    .and_then(|c| state.clients.get(&c))
                    .map(|s| s.display_title().to_string())
                    .filter(|t| !t.is_empty());
                shown.push(cell_ws);
                // A span's window covers this screen's bar segment.
                // ...as does a stereo headset's (nothing flat is drawn there).
                let hidden = segmented && (state.screens.spanned(i).is_some() || state.screens.is_headset(i));
                parts.push((cells, title, state.screens.screens[i].rect, i == active, hidden));
            }
        }
        self.bar_workspaces = shown;
        let mut base = self.bar_sample.clone();
        // The middle window control reads "restore" while maximized.
        base.maximized = self.ui.window(cx, ids!(main_window)).is_fullscreen(cx);
        let mut segments: Vec<shell::bar::BarSegment> = Vec::new();
        let mut data = base.clone();
        for (seg, (cells, title, rect, is_active, hidden)) in parts.into_iter().enumerate() {
            let mut d = base.clone();
            d.workspaces = cells;
            d.active_window = title;
            d.open_panel = if !segmented || seg == self.shell_panel_segment { self.shell_panel_open } else { None };
            if is_active {
                data = d.clone();
            }
            if segmented {
                segments.push(shell::bar::BarSegment { x0: rect.x, x1: rect.x + rect.w, data: d, active: is_active, hidden });
            }
        }
        let window_controls = shell::bar::window_controls_default()
            && !matches!(cx.os_type(), OsType::LinuxDirect);
        let launcher = self.state.as_ref().is_some_and(|s| s.style.target == desktop::DesktopStyle::Omarchy);
        let shown = format!("{:?} {:?} {} {}", data, segments, window_controls, launcher);
        if shown == self.bar_shown {
            return;
        }
        self.bar_shown = shown;
        let bar = self.ui.widget(cx, ids!(shell_bar));
        {
            let mut borrowed = bar.borrow_mut::<shell::bar::ShellBar>();
            if let Some(b) = borrowed.as_mut() {
                b.data = data;
                b.segments = segments;
                // The direct-display session has no outer window to control.
                b.window_controls = window_controls;
                b.launcher = launcher;
            }
        }
        self.redraw_all(cx);
    }

    /// Put the bar where `shell::bar::bar_placement` says: on one screen
    /// (and nested, in a mobile style, in the gallery) the strip above the
    /// desk, exactly as before; with per-screen segments it floats over
    /// the top of the scene, the desk starts at the window's top edge and
    /// every screen a span does not cover reserves the bar's height
    /// (`ScreenSet::bar_top`). The bar is drawn outside the texture-cached
    /// scene, so its redraws never invalidate it. Cheap when nothing moved.
    fn sync_bar_placement(&mut self, cx: &mut Cx) {
        if self.state.is_none() {
            return;
        }
        let height = self.bar_metrics.map_or(BAR_HEIGHT_FALLBACK, |(h, _)| h);
        let visible = self.ui.widget(cx, ids!(bar)).visible();
        let placed = shell::bar::bar_placement(self.bar_segmented(), visible, height);
        self.state_mut().screens.bar_top = placed.reserved_top;
        if self.bar_placed == Some(placed) {
            return;
        }
        self.bar_placed = Some(placed);
        let space = self.ui.widget(cx, ids!(bar_space));
        space.set_visible(cx, placed.strip.is_some());
        if let Some(h) = placed.strip {
            let mut space = space;
            script_apply_eval!(cx, space, {
                height: #(h)
            });
        }
        let mut bar = self.ui.widget(cx, ids!(bar));
        if placed.background {
            script_apply_eval!(cx, bar, { show_bg: true });
        } else {
            script_apply_eval!(cx, bar, { show_bg: false });
        }
        self.redraw_all(cx);
    }

    /// Whether the bar draws one segment per live screen: two or more of
    /// them in a desktop style (a mobile style shows the active screen's
    /// layout only, and its own bar).
    /// Where a shell surface opened on screen `screen` draws: that live
    /// screen's rect (the active one's for a screen that went away) on a
    /// multi-screen desktop style; `None`, the whole overlay, on one screen,
    /// off Linux direct, in the gallery and in a mobile style.
    fn surface_target(&self, screen: usize) -> Option<Rect> {
        let state = self.state.as_ref()?;
        if state.style.target.mobile() {
            return None;
        }
        state.screens.surface_rect(screen).map(|r| rect(r.x, r.y, r.w, r.h))
    }

    /// Point the menu, the flyout, the OSD and the notification stack at
    /// the screens they were opened on, right before every draw: the rects
    /// follow a reconcile or the AI pane's clip within a frame. A surface
    /// whose target changed is redrawn; nothing else is touched.
    fn sync_surface_targets(&mut self, cx: &mut Cx) {
        let menu = self.surface_target(self.menu_screen);
        let panel = self.surface_target(self.bar_screen(self.shell_panel_segment));
        let osd = self.surface_target(self.osd_screen);
        let notes = self.surface_target(self.notes_screen);
        if let Some(mut m) = self.ui.widget(cx, ids!(shell_menu)).borrow_mut::<ShellMenu>() {
            if m.target != menu {
                m.target = menu;
                m.redraw(cx);
            }
        }
        if let Some(mut p) = self.ui.widget(cx, ids!(shell_panel)).borrow_mut::<shell::panels::ShellPanel>() {
            if p.target != panel {
                p.target = panel;
                p.redraw(cx);
            }
        }
        if let Some(mut o) = self.ui.widget(cx, ids!(shell_osd)).borrow_mut::<shell::osd::ShellOsd>() {
            if o.target != osd {
                o.target = osd;
                o.redraw(cx);
            }
        }
        if let Some(mut n) = self
            .ui
            .widget(cx, ids!(shell_notes))
            .borrow_mut::<shell::notifications::ShellNotifications>()
        {
            if n.target != notes {
                n.target = notes;
                n.redraw(cx);
            }
        }
    }

    /// Screen `i`'s focused window when it can take the keyboard: `None`
    /// for an empty screen and for a no-focus Quick Look preview, which
    /// `focus_client` refuses, so crossing into either must release the
    /// old screen's window instead.
    fn screen_keyboard_client(&mut self, i: usize) -> Option<ClientId> {
        let state = self.state_mut();
        let c = state.layout_at(i).focused_client()?;
        state.clients.get(&c).map_or(true, |s| s.takes_focus).then_some(c)
    }

    fn bar_segmented(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.screens.live_count() >= 2 && !s.style.target.mobile())
    }

    /// The screen bar segment `seg` stands for: live screen `seg` on a
    /// segmented bar, the active screen on the one-segment bar (or for a
    /// segment that no longer exists).
    fn bar_screen(&self, seg: usize) -> usize {
        let Some(state) = self.state.as_ref() else { return 0 };
        if self.bar_segmented() && seg < state.screens.live_count() {
            seg
        } else {
            state.screens.active
        }
    }

    /// The bar segment that shows the active screen.
    fn active_bar_segment(&self) -> usize {
        match self.state.as_ref() {
            Some(state) if self.bar_segmented() => state.screens.active,
            _ => 0,
        }
    }

    /// The focused window of bar segment `seg`'s screen (its title).
    fn bar_focused_client(&mut self, seg: usize) -> Option<ClientId> {
        let screen = self.bar_screen(seg);
        let spanned = self.state_mut().screens.spanned(screen);
        spanned.or_else(|| self.state_mut().layout_at(screen).focused_client())
    }

    /// A click on workspace cell `i` of bar segment `seg`: that segment's
    /// screen becomes active, then switches to the workspace. When the
    /// switch leaves the new screen without a window to focus, the
    /// keyboard leaves the old screen's window too. A segment or cell the
    /// last `update_bar` did not publish (a stale press across a reconcile)
    /// is ignored: `i` is a cell index, not a workspace number.
    fn bar_workspace(&mut self, cx: &mut Cx, seg: usize, i: usize) {
        let Some(ws) = self.bar_workspaces.get(seg).and_then(|v| v.get(i)).copied() else {
            return;
        };
        let screen = self.bar_screen(seg);
        // A screen a span covers from another screen shows that span, not
        // its own workspaces: its cells do nothing (the home screen's
        // cells switch the span's workspace).
        if self.state_mut().screens.span_home(screen).is_some_and(|home| home != screen) {
            return;
        }
        let old = self.state_mut().screens.active;
        self.state_mut().screens.active = screen;
        self.do_action(cx, WmAction::SpaceJump(ws));
        if screen != old && self.screen_keyboard_client(screen).is_none() {
            self.release_screen_keyboard(cx, old);
        }
    }

    /// The active screen changed to one with no window to focus: like
    /// Hyprland's empty monitor, the keyboard leaves screen `old`'s focused
    /// window (keys go nowhere rather than to a window on another screen),
    /// and no focus stays pending. The AI pane keeps the keyboard it holds.
    fn release_screen_keyboard(&mut self, cx: &mut Cx, old: usize) {
        if self.ai_pane_is_open(cx) {
            return;
        }
        self.pending_focus = None;
        let client = self
            .state
            .as_ref()
            .filter(|s| old < s.screens.screens.len())
            .and_then(|s| s.layout_at(old).focused_client());
        if let Some(client) = client {
            let _ = self
                .desk(cx)
                .borrow_mut::<WmDesk>()
                .and_then(|mut d| d.with_tile(cx, client, |cx, v| v.release_keyboard(cx)));
        }
    }

    /// The bar's window controls: the same three calls the stock caption
    /// bar makes on its own buttons (widgets/core/src/window.rs).
    fn window_control(&mut self, cx: &mut Cx, module: BarModule) -> bool {
        let window = self.ui.window(cx, ids!(main_window));
        match module {
            BarModule::WindowMin => window.minimize(cx),
            BarModule::WindowMax => {
                if window.is_fullscreen(cx) {
                    window.restore(cx);
                } else {
                    window.maximize(cx);
                }
            }
            BarModule::WindowClose => window.close(cx),
            _ => return false,
        }
        log!("wm: window control {:?}", module);
        true
    }

    /// The bar IS this window's caption, so it has to be tall enough to
    /// center the OS window buttons and start its own content after them.
    /// Both numbers come from the platform's traffic-light rect (points,
    /// top-left origin) exactly like the stock caption bar does. Right-side
    /// caption buttons affect the height only; the bar's left cluster stays
    /// at its normal edge inset.
    fn update_bar_chrome(&mut self, cx: &mut Cx, geom: &WindowGeom) {
        let (height, pad_left) = bar_metrics_for_geom(geom);
        if self.bar_metrics == Some((height, pad_left)) {
            return;
        }
        self.bar_metrics = Some((height, pad_left));
        let mut bar = self.ui.widget(cx, ids!(bar));
        script_apply_eval!(cx, bar, {
            height: #(height)
        });
        // The strip's spacer (or each screen's reservation) follows.
        self.sync_bar_placement(cx);
        // The bar's content starts after the OS window buttons.
        let shell_bar = self.ui.widget(cx, ids!(shell_bar));
        {
            let mut borrowed = shell_bar.borrow_mut::<shell::bar::ShellBar>();
            if let Some(b) = borrowed.as_mut() {
                b.pad_left = pad_left;
            }
        }
        if let Some(mut controls) = self.ui.widget(cx, ids!(phone_controls)).borrow_mut::<mobile_surface::PhoneSurface>() {
            controls.pad_left = pad_left;
        }
        self.redraw_all(cx);
    }

    /// The status modules. Volume every tick (it is one `osascript` and
    /// the user changes it constantly); battery, network and bluetooth
    /// every fifth, because they cost a process each.
    fn update_status(&mut self, cx: &mut Cx) {
        // The samplers fork subprocesses that can take hundreds of ms; on
        // this thread that starved the hosted tiles' 8ms Ticks (visible
        // ~0.5s hiccups in every child app). A background thread samples
        // and we only copy its cache here.
        if self.status_rx.is_none() {
            match shell::bar::start_status_sampler(&cx.thread_spawner()) {
                Ok((rx, worker)) => {
                    self.status_rx = Some(rx);
                    self.status_worker = Some(worker);
                }
                Err(error) => log!("wm: could not start status worker: {error}"),
            }
        }
        if let Some(rx) = &self.status_rx {
            while let Ok(sample) = rx.try_recv() {
                self.status_sample = sample;
            }
        }
        let s = self.status_sample.clone();
        #[cfg(not(target_os = "linux"))]
        {
            self.bar_sample.volume = s.volume;
            self.bar_sample.muted = s.muted;
            self.bar_sample.battery = s.battery;
        }
        // The clock is formatted here, on the UI thread: `localtime_r` reads
        // the process environment, which this thread also writes (child env,
        // MAKEPAD_WM_ROOT), so no worker may read it concurrently. Where
        // there is no local clock (the web) the platform's own epoch seconds
        // are formatted instead.
        self.bar_sample.clock = {
            let clock = shell::bar::sample_clock(self.clock_alt);
            if clock.is_empty() { host::fallback_clock(self.clock_alt) } else { clock }
        };
        self.status_tick = self.status_tick.wrapping_add(1);
        self.bar_sample.network = s.network;
        self.bar_sample.bluetooth = s.bluetooth;
        // Keep the flyout's own copy in step with the bar's.
        let panel = self.ui.widget(cx, ids!(shell_panel));
        {
            let mut borrowed = panel.borrow_mut::<shell::panels::ShellPanel>();
            if let Some(p) = borrowed.as_mut() {
                p.data.volume = self.bar_sample.volume;
                p.data.muted = self.bar_sample.muted;
                p.data.battery = self.bar_sample.battery;
                p.data.network = self.bar_sample.network.map(|up| {
                    if up { "Connected".to_string() } else { "Not connected".to_string() }
                });
                p.data.bluetooth = self.bar_sample.bluetooth;
            }
        }
        #[cfg(target_os = "linux")]
        self.poll_linux_controls(cx);
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
    }

    /// Hosted clients render into capture passes below the desktop scene.
    /// Their frame event therefore needs to invalidate the scene cache
    /// explicitly, including for the assistant pane.
    fn redraw_scene_content(&mut self, cx: &mut Cx) {
        if let Some(mut scene) = self.ui.widget(cx, ids!(scene)).borrow_mut::<scene::WmScene>() {
            scene.redraw_content(cx);
        }
    }

    // --------------------------------------------------------------
    // WM actions
    // --------------------------------------------------------------

    /// Hand the layout the true desk rect so fullscreen and the scratchpad
    /// console can reach past the outer gap.
    /// True when the screen set was reconciled (multi-screen only).
    fn sync_geometry(&mut self, cx: &mut Cx) -> bool {
        let rect = self
            .desk(cx)
            .borrow_mut::<WmDesk>()
            .map(|d| d.desk_rect)
            .unwrap_or_default();
        if rect.size.x > 1.0 {
            let desk = LRect::new(rect.pos.x, rect.pos.y, rect.size.x, rect.size.y);
            let state = self.state_mut();
            // Per screen, reconcile hands each layout its screen as outer.
            let changed = state.sync_screens(desk, host::now());
            if !state.per_screen() {
                state.layout_mut().set_outer(desk);
            }
            return changed;
        }
        false
    }

    /// Read the platform's screens (Linux direct only; elsewhere the WM
    /// keeps its single desk) and bring the screen set in line. Called on
    /// WindowGeomChange and the 1 s tick: the tick is also what lets a
    /// lost screen's removal debounce expire.
    fn sync_screens(&mut self, cx: &mut Cx) {
        if self.state.is_none() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let per_screen = screens::per_screen_enabled(matches!(cx.os_type(), OsType::LinuxDirect), self.gallery);
            let generation = cx.linux_display_generation();
            // On Linux direct the screens are re-read on every call (the
            // tick and WindowGeomChange): a DPI change does not bump the
            // generation, and the reconcile key is cheap to compare.
            // Elsewhere they are read once, when the gate changes.
            // `MAKEPAD_WM_FAKE_SCREENS=name:x,y,w,h;name:x,y,w,h` (tests only): the
            // per-screen desk of the Linux direct backend, under a nested window.
            if let Some(spec) = std::env::var("MAKEPAD_WM_FAKE_SCREENS").ok().filter(|s| !s.is_empty()) {
                let mut names = Vec::new();
                let mut geoms = Vec::new();
                for part in spec.split(';') {
                    let Some((name, r)) = part.split_once(':') else { continue };
                    let v: Vec<f64> = r.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if v.len() == 4 {
                        names.push(name.to_string());
                        geoms.push(LRect::new(v[0], v[1], v[2], v[3]));
                    }
                }
                if !names.is_empty() && !self.gallery {
                    let state = self.state_mut();
                    state.screen_source = desk::ScreenSource { per_screen: true, main_name: names.first().cloned(), geoms, names, generation };
                    self.sync_geometry(cx);
                    self.drain_screens_changed(cx);
                    return;
                }
            }
            let state = self.state_mut();
            if per_screen || per_screen != state.screen_source.per_screen {
                let geoms = makepad_widgets::makepad_platform::screens();
                let names = makepad_widgets::makepad_platform::linux_screen_names();
                // A stereo headset is never the main screen (the dock's).
                let headset = if per_screen { state.vr.resolve_headset(&names, generation) } else { None };
                let main_name = geoms
                    .iter()
                    .position(|g| g.is_primary)
                    .and_then(|i| names.get(i).cloned())
                    .filter(|n| Some(n) != headset.as_ref())
                    .or_else(|| names.iter().find(|n| Some(*n) != headset.as_ref()).cloned());
                state.screens.headset = headset;
                state.screen_source = desk::ScreenSource {
                    per_screen,
                    geoms: geoms
                        .iter()
                        .map(|g| LRect::new(g.bounds.pos.x, g.bounds.pos.y, g.bounds.size.x, g.bounds.size.y))
                        .collect(),
                    names,
                    main_name,
                    generation,
                };
            }
        }
        self.sync_geometry(cx);
        self.drain_screens_changed(cx);
    }

    /// After a reconcile (here, or from the desk draw, which cannot reach
    /// the bar): the bar follows, the fullscreen rule is re-applied, and a
    /// drag in flight is dropped, because screen indices may have shifted.
    fn drain_screens_changed(&mut self, cx: &mut Cx) {
        if !self.state.as_ref().is_some_and(|s| s.screens_changed) {
            return;
        }
        self.state_mut().screens_changed = false;
        if self.drag.take().is_some() || self.div_drag.take().is_some() {
            self.state_mut().dragging.clear();
            self.state_mut().drop_hint = None;
            cx.set_cursor(MouseCursor::Default);
        }
        self.sync_bar_for_fullscreen(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// CTRL+ALT+DELETE — `omarchy-hyprland-window-close-all`: close every
    /// client the same polite way, one by one, then focus workspace 1.
    ///
    /// The pool's dormant instances go too. They are ordinary children and
    /// "close everything" must not leave four of them running; the pool
    /// notices they were asked to go (not that they crashed) and refills
    /// itself on the ticks that follow, so the desktop is empty AND the
    /// next window still opens instantly.
    ///
    /// `clients` holds the hidden warm viewers too, so a close-all takes
    /// the whole Quick-Look cache with it — hide keeps a viewer, this does
    /// not.
    fn close_all_windows(&mut self, cx: &mut Cx) {
        self.preview_cache.active = None;
        let all: Vec<ClientId> = self.state_mut().clients.keys().copied().collect();
        for client in all {
            self.request_close(cx, client);
        }
        self.preview_cache.clear();
        for s in &mut self.state_mut().screens.screens {
            s.layout.goto_space(0);
        }
        self.redraw_all(cx);
    }

    /// The WM itself is going down: a hidden warm viewer has no tile and
    /// no window, so nothing else would ever reap it. Kill the cache's
    /// processes outright — `--stdin-loop` children do notice an orphaned
    /// parent eventually, but "eventually" is not a shutdown story.
    fn kill_warm_previews(&mut self, cx: &mut Cx) {
        let pool = cx.task_pool();
        for client in self.preview_cache.warm_clients() {
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                if let Some(child) = slot.child.as_mut() {
                    log!("wm: shutdown kills warm preview client {}", client);
                    makepad_app_host::launch::kill_child_group(child, makepad_app_host::launch::GROUP_KILL_GRACE, &pool);
                }
            }
        }
        self.preview_cache.clear();
    }

    /// The pointer is at `abs`: crossing into another screen makes it the
    /// active one and (Hyprland `follow_mouse` across monitors) gives that
    /// screen's focused window the keyboard, unless the AI pane holds it.
    /// Hover within a screen never changes focus.
    fn pointer_screen(&mut self, cx: &mut Cx, abs: Vec2d) {
        let old = self.state_mut().screens.active;
        let Some(i) = self.state_mut().screens.on_pointer(abs.x, abs.y) else {
            return;
        };
        if !self.ai_pane_is_open(cx) {
            // Over a span: its window (on its home screen `i`).
            let spanned = self.state_mut().screens.spanned(i);
            if let Some(c) = spanned.or_else(|| self.screen_keyboard_client(i)) {
                self.focus_client(cx, c);
            } else {
                // An empty screen, or one whose focused window is a
                // no-focus preview: the keyboard leaves the old one's
                // window.
                self.release_screen_keyboard(cx, old);
            }
        }
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// SUPER+F hides the bar with the window; anything that leaves
    /// fullscreen puts it back, unless SUPER+SHIFT+SPACE hid it.
    fn sync_bar_for_fullscreen(&mut self, cx: &mut Cx) {
        let fullscreen = self.state_mut().screens.one_screen_fullscreen();
        let bar = self.ui.widget(cx, ids!(bar));
        if fullscreen && bar.visible() {
            bar.set_visible(cx, false);
            self.bar_hidden_by_fullscreen = true;
        } else if !fullscreen && self.bar_hidden_by_fullscreen {
            bar.set_visible(cx, true);
            self.bar_hidden_by_fullscreen = false;
        }
        self.sync_bar_placement(cx);
    }

    /// The layout picked a new focus on its own (a window closed, a client
    /// died): the bar follows it, and the keyboard goes to that tile —
    /// UNLESS the AI pane is open. The person typing in the chat asked for
    /// the close; an automatic refocus must not take their keys away
    /// mid-line (an `os.close` used to send the next console line into a
    /// sheet). A click on a tile still moves focus: that comes through
    /// `focus_client` directly, never through here.
    fn focus_after_layout(&mut self, cx: &mut Cx) {
        if self.ai_pane_is_open(cx) {
            self.update_bar(cx);
            self.focus_pane(cx);
            return;
        }
        if let Some(focus) = self.state_mut().layout().focused_client() {
            self.focus_client(cx, focus);
        }
    }

    fn do_action(&mut self, cx: &mut Cx, action: WmAction) {
        if desk::spaces_trace() {
            log!("wm: action {:?}", action);
        }
        self.sync_geometry(cx);
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        let focus = self.state_mut().layout().focused_client();
        match action {
            WmAction::LaunchTerminal => self.launch_app(cx, "terminal"),
            WmAction::LaunchBrowser => self.launch_app(cx, "browser"),
            WmAction::CloseWindow => self.close_focused(cx),
            WmAction::CloseAllWindows => self.close_all_windows(cx),
            WmAction::ToggleSplit => {
                self.state_mut().layout_mut().toggle_split();
            }
            WmAction::TogglePseudo => {
                if let Some(focus) = focus {
                    self.state_mut().layout_mut().toggle_pseudo(focus, area, gap);
                }
            }
            WmAction::ToggleFloat => {
                if let Some(focus) = focus {
                    self.state_mut().layout_mut().toggle_float(focus, area, gap);
                    self.focus_client(cx, focus);
                }
            }
            WmAction::PopOut => {
                if let Some(focus) = focus {
                    self.state_mut().layout_mut().pop_out(focus, area, gap);
                    self.focus_client(cx, focus);
                }
            }
            WmAction::FullscreenSpace => {
                if let Some(focus) = focus {
                    self.fullscreen_toggle(cx, focus);
                }
            }
            WmAction::FullWidth => self.state_mut().layout_mut().toggle_full_width(),
            WmAction::TiledFullscreen => {
                if let Some(focus) = focus {
                    let on = self.state_mut().layout_mut().toggle_client_fullscreen(focus);
                    // fullscreenstate 0 2: the client is told, the layout is
                    // untouched. Our children learn it as a custom message.
                    if let Some(slot) = self.state_mut().clients.get(&focus) {
                        if let Some(sender) = &slot.sender {
                            send_to_app(
                                sender,
                                vec![StudioToApp::Custom(format!(
                                    "{{\"wm_fullscreen\":{}}}",
                                    on
                                ))],
                            );
                        }
                    }
                }
            }
            WmAction::FocusDir(dir) => {
                if self.state_mut().layout_mut().focus_dir(dir, area, gap) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::SwapDir(dir) => {
                self.state_mut().layout_mut().swap_dir(dir, area, gap);
            }
            WmAction::MoveToScreen { forward } => {
                if let Some(focus) = focus {
                    let (reserved, gaps_out) =
                        (self.state_mut().style.reserved_height(), self.state_mut().gaps_out);
                    if self
                        .state_mut()
                        .screens
                        .move_to_screen(focus, forward, gap, reserved, gaps_out)
                        .is_some()
                    {
                        self.focus_client(cx, focus);
                    }
                }
            }
            WmAction::ResizePx { axis, px } => {
                self.state_mut().layout_mut().resize_px(axis, px, area, gap);
            }
            WmAction::ToggleVr => {
                let state = self.state_mut();
                if state.vr.source.take().is_some() {
                    log!("wm: vr mode off");
                    state.vr.output = None;
                    if std::mem::take(&mut state.vr.cursor_hidden) {
                        cx.set_cursor(MouseCursor::Default);
                    }
                } else {
                    // The windows of the screen the person is on; on the
                    // headset itself, the main screen's.
                    let screens = &state.screens;
                    let i = if screens.is_headset(screens.active) { screens.main } else { screens.active };
                    let name = screens.screens.get(i).map(|s| s.name.clone()).unwrap_or_default();
                    log!("wm: vr mode on, windows of screen '{}', headset {:?}", name, state.vr.headset());
                    state.vr.source = Some(name);
                }
                self.redraw_all(cx);
            }
            WmAction::VrRecenter => {
                let vr = &mut self.state_mut().vr;
                vr.look = (0.0, 0.0);
                vr.recenter = vr.recenter.wrapping_add(1);
                self.redraw_all(cx);
            }
            WmAction::VrDepth { nearer } => {
                if let Some(focus) = focus {
                    let depth = self.state_mut().vr.scale_depth(focus, if nearer { 0.8 } else { 1.25 });
                    log!("wm: vr window {} at {:.2} m", focus, depth);
                    self.redraw_all(cx);
                }
            }
            WmAction::VrLook(dir) => {
                const STEP: f32 = 0.0873; // 5 degrees
                let look = &mut self.state_mut().vr.look;
                match dir {
                    Dir::Left => look.0 -= STEP,
                    Dir::Right => look.0 += STEP,
                    Dir::Up => look.1 = (look.1 + STEP).min(1.5),
                    Dir::Down => look.1 = (look.1 - STEP).max(-1.5),
                }
                self.redraw_all(cx);
            }
            WmAction::CycleFocus(forward) => {
                self.state_mut().layout_mut().cycle_focus(forward);
                self.focus_after_layout(cx);
            }
            WmAction::ToggleGroup => {
                self.state_mut().layout_mut().toggle_group(area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::MoveOutOfGroup => {
                self.state_mut().layout_mut().move_out_of_group(area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::MoveIntoGroup(dir) => {
                self.state_mut().layout_mut().move_into_group(dir, area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::GroupNext => {
                if self.state_mut().layout_mut().group_cycle(true) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::GroupPrev => {
                if self.state_mut().layout_mut().group_cycle(false) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::GroupActive(n) => {
                if self.state_mut().layout_mut().group_set_active(n) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::SpaceSlide(dir) => self.spaces_slide(cx, dir),
            WmAction::SpaceJump(n) => self.spaces_jump(cx, n),
            WmAction::SpaceNew => self.spaces_new(cx),
            WmAction::SpaceStrip => self.spaces_strip_toggle(cx),
            WmAction::CheatSheet => self.spaces_sheet_toggle(cx),
            WmAction::MoveToSpace(n) => self.spaces_move_focused(cx, n, true),
            WmAction::MoveToSpaceSilent(n) => self.spaces_move_focused(cx, n, false),
            WmAction::ToggleScratchpad => {
                self.state_mut().layout_mut().toggle_scratchpad();
                self.focus_after_layout(cx);
            }
            WmAction::MoveToScratchpad => {
                self.state_mut()
                    .layout_mut()
                    .move_focused_to_scratchpad(area, gap);
            }
            WmAction::ToggleWorkspaceLayout => {
                let mode = self.state_mut().layout_mut().toggle_workspace_layout();
                // The scrolling algorithm is a follow-on lane; the flag is
                // live and the layout still draws dwindle.
                log!("wm: workspace layout -> {:?}", mode);
            }
            // The omarchy menu surface (shell/menu.rs): one card, the
            // jsonc tree, the `apps` provider as the launcher.
            WmAction::Menu => self.open_shell_menu(cx, "", MenuSkin::Menu),
            WmAction::AppsMenu => self.open_shell_menu(cx, "apps", MenuSkin::Launcher),
            WmAction::SystemMenu => self.open_shell_menu(cx, "system", MenuSkin::Menu),
            WmAction::MenuRoute(route) => self.open_shell_menu(cx, route, MenuSkin::Menu),
            WmAction::Keybindings => {
                self.open_shell_menu(cx, "learn.keybindings", MenuSkin::Menu)
            }
            WmAction::ThemeMenu => self.open_shell_menu(cx, "style.theme", MenuSkin::Menu),
            WmAction::BackgroundNext => self.next_background(cx),
            WmAction::NextSkin => {
                let skin = self.state_mut().skin.next();
                self.state_mut().skin = skin;
                log!("wm: window skin -> {}", skin.name());
                self.redraw_all(cx);
            }
            WmAction::ToggleBar => {
                let bar = self.ui.widget(cx, ids!(bar));
                let visible = bar.visible();
                bar.set_visible(cx, !visible);
                self.bar_hidden_by_fullscreen = false;
                self.update_bar(cx);
                self.redraw_all(cx);
                return;
            }
            WmAction::ArmAltLayer => {
                self.alt_armed = true;
                log!("wm: SUPER+ALT layer armed for the next key");
                return;
            }
            WmAction::ToggleAi => self.toggle_ai_pane(cx),
            WmAction::SpacewarpPark { right } => self.warp_park(cx, right),
            WmAction::SpacewarpCycle => self.warp_cycle(cx),
            WmAction::SpacewarpUnshelf => self.warp_unshelf(cx),
        }
        // Every action that can enter or leave a workspace's fullscreen
        // funnels through here, so the bar's fullscreen sync is done once,
        // for all of them, instead of per-arm (where it was missing from
        // ToggleFloat/PopOut, the whole workspace-switch family, the
        // scratchpad moves, and window close — the bar stayed hidden after
        // fullscreen ended). ToggleBar returns before this: it is the user
        // overriding bar visibility by hand, not a fullscreen transition.
        self.sync_bar_for_fullscreen(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    // --------------------------------------------------------------
    // SUPER + mouse (tiling.lua mouse:272 / mouse:273)
    // --------------------------------------------------------------

    fn begin_drag(&mut self, cx: &mut Cx, abs: Vec2d, resize: bool) -> bool {
        self.sync_geometry(cx);
        // A span's window is fullscreen: nothing to move or resize, and
        // the screens it covers have no other window to grab.
        if self.state_mut().span_at(abs.x, abs.y).is_some() {
            return false;
        }
        // The window under the pointer is on the screen under the pointer.
        let screen = self.state_mut().screen_under(abs.x, abs.y);
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let parked = self.warp_window_at(cx, abs);
        let Some(client) = parked.or_else(|| self
            .state_mut()
            .layout_at(screen)
            .client_at(abs.x, abs.y, area, gap))
        else {
            return false;
        };
        self.begin_drag_on(cx, client, abs, resize, false)
    }

    /// The drag itself, once the window is known. `armed` starts a drag
    /// that has ALREADY cleared the threshold — a tab torn off a groupbar
    /// is mid-gesture by the time it becomes a window drag.
    fn begin_drag_on(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        abs: Vec2d,
        resize: bool,
        armed: bool,
    ) -> bool {
        self.sync_geometry(cx);
        let state = self.state_mut();
        let screen = state.screens.screen_of(client).unwrap_or(state.screens.active);
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let layout = self.state_mut().layout_at_mut(screen);
        let floating = layout.is_float(client);
        let Some(rect) = layout.rect_of(client, area, gap) else {
            return false;
        };
        if floating {
            layout.raise_float(client);
        }
        let (cx_center, cy_center) = rect.center();
        self.drag = Some(DragState {
            client,
            resize,
            resize_x: true,
            resize_y: true,
            floating,
            start: abs,
            last: abs,
            start_rect: rect,
            grab_left: abs.x < cx_center,
            grab_top: abs.y < cy_center,
            armed,
            shift: false,
            screen,
            pressed: std::time::Instant::now(),
        });
        if floating {
            // Hyprland moves a drag 1:1 with the pointer, not on the
            // tile's 379ms layout tween (`WmDesk::draw_walk`'s sync loop
            // reads this to skip `retarget` for this one client).
            self.state_mut().dragging = vec![client];
        }
        let shown = self.desk(cx).borrow::<WmDesk>().and_then(|d| d.warp.swollen_pose(client)).map(|p| p.shown);
        self.state_mut().warp.press_pose = shown.map(|r| (client, r));
        self.state_mut().warp.armed_drag = armed.then_some(client);
        self.state_mut().shake.reset();
        self.focus_client(cx, client);
        self.apply_drag_cursor(cx);
        true
    }

    fn apply_drag_cursor(&self, cx: &mut Cx) {
        let Some(drag) = self.drag.as_ref() else {
            return;
        };
        cx.set_cursor(if !drag.resize {
            MouseCursor::Grabbing
        } else {
            match (drag.resize_x, drag.resize_y) {
                (true, true) if drag.grab_left == drag.grab_top => MouseCursor::NwseResize,
                (true, true) => MouseCursor::NeswResize,
                (true, false) => MouseCursor::EwResize,
                (false, true) => MouseCursor::NsResize,
                (false, false) => MouseCursor::Default,
            }
        });
    }

    fn drag_move(&mut self, cx: &mut Cx, abs: Vec2d, shift: bool) {
        self.warp_edge(cx, abs);
        self.warp_take_out(cx, abs);
        let mut clear_swell = false;
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        drag.shift = shift;
        // A press only becomes a drag once it clears the threshold.
        if !drag.armed {
            // A press on a shelf item is a click until it moves the slop; any
            // other quick press on a parked window's title is judged by a
            // wider one (a touchpad button moves the finger by millimetres).
            let shelf_press = self.state.as_ref().is_some_and(|s| s.warp.shelf_press.as_ref().is_some_and(|(c, _)| *c == drag.client));
            if shelf_press {
                if !spacewarp::shelf::press_is_drag((drag.start.x, drag.start.y), (abs.x, abs.y)) {
                    return;
                }
            } else {
                let slop = DRAG_THRESHOLD;
                if (abs.x - drag.start.x).abs() < slop
                    && (abs.y - drag.start.y).abs() < slop
                {
                    return;
                }
            }
            clear_swell = true;
            if let Some(state) = self.state.as_mut() {
                if state.warp.shelf_press.as_ref().is_some_and(|(c, _)| *c == drag.client) {
                    state.warp.shelf_press = None;
                }
            }
            drag.armed = true;
            if let Some(s) = self.state.as_mut() {
                s.warp.armed_drag = Some(drag.client);
            }
            if !drag.resize {
                if let Some(w)=self.state.as_mut().and_then(|s|s.layout_of_mut(drag.client).desktop.get_mut(drag.client)).filter(|w|w.maximized || w.snap.is_some()) {
                    let anchor=((drag.start.x-drag.start_rect.x)/drag.start_rect.w).clamp(0.0,1.0);
                    w.rect.x=drag.start.x-w.rect.w*anchor;
                    w.rect.y=drag.start.y-16.0;
                    w.maximized=false;w.snap=None;drag.start_rect=w.rect;
                }
            }
        }
        let (client, resize, floating, start, last, start_rect, grab_left, grab_top) = (
            drag.client,
            drag.resize,
            drag.floating,
            drag.start,
            drag.last,
            drag.start_rect,
            drag.grab_left,
            drag.grab_top,
        );
        let (resize_x,resize_y)=(drag.resize_x,drag.resize_y);
        let screen = drag.screen;
        drag.last = abs;
        if clear_swell {
            // The drag is on: the shelf's fish-eye goes away at once.
            if let Some(mut d) = self.desk(cx).borrow_mut::<WmDesk>() {
                d.warp.swell_clear();
            }
        }
        if screen >= self.state_mut().screens.live_count() {
            // The screen went away under the drag (a hotplug): drop it.
            self.drag = None;
            self.state_mut().dragging.clear();
            return;
        }
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        if floating {
            let d = abs - start;
            let rect = if resize {
                // The grabbed corner moves with the pointer; the opposite
                // one is fixed (DragController.cpp:413-423).
                let (x, w) = if !resize_x {(start_rect.x,start_rect.w)}else if grab_left {
                    let w = (start_rect.w - d.x).max(80.0);
                    (start_rect.x + start_rect.w - w, w)
                } else {
                    (start_rect.x, (start_rect.w + d.x).max(80.0))
                };
                let (y, h) = if !resize_y {(start_rect.y,start_rect.h)}else if grab_top {
                    let h = (start_rect.h - d.y).max(60.0);
                    (start_rect.y + start_rect.h - h, h)
                } else {
                    (start_rect.y, (start_rect.h + d.y).max(60.0))
                };
                LRect::new(x, y, w, h)
            } else {
                let plain = LRect::new(start_rect.x + d.x, start_rect.y + d.y, start_rect.w, start_rect.h);
                self.warp_drag_rect(cx, screen, client, start_rect, start, abs, plain)
            };
            self.state_mut().layout_at_mut(screen).set_float_rect(client, rect);
            if !resize { self.shake_sample(cx, screen, client, abs); }
        } else if resize {
            // Tiled: the divider follows the pointer, one frame at a time,
            // exactly like CDwindleAlgorithm::resizeTarget's Δ.
            let d = abs - last;
            // A tiled resize moves the split ratios: the grabbed edge
            // follows the pointer, so a left/top grab inverts the delta.
            let layout = self.state_mut().layout_at_mut(screen);
            if d.x.abs() > 0.0 {
                layout.resize_px(Axis::Horizontal, d.x, area, gap);
            }
            if d.y.abs() > 0.0 {
                layout.resize_px(Axis::Vertical, d.y, area, gap);
            }
            let _ = (grab_left, grab_top);
        }
        // The drop hint: while SHIFT is down over some other tile, that
        // tile's ring turns accent — dropping there makes a tab, not a
        // swap. Only for a tiled move; a resize or a float has no such
        // drop.
        let (client, floating, resize) = {
            let d = self.drag.as_ref().unwrap();
            (d.client, d.floating, d.resize)
        };
        let hint = if shift && !floating && !resize {
            let gap = self.state_mut().gap;
            self.state_mut()
                .layout_at(screen)
                .client_at(abs.x, abs.y, area, gap)
                .filter(|target| *target != client)
        } else {
            None
        };
        self.state_mut().drop_hint = hint;
        if floating && !resize && self.snap_zones_on() {
            // Snap zones are the edges of the screen under the pointer.
            let pointer_screen = self.state_mut().screen_under(abs.x, abs.y);
            let snap_area = self.screen_area(cx, pointer_screen);
            self.state_mut().snap.drag(client,abs,snap_area);
        }
        let landing = if floating && !resize { self.warp_landing(cx) } else { None };
        self.state_mut().backdrop.observe_drag(landing);
        self.apply_drag_cursor(cx);
        self.redraw_all(cx);
    }

    fn end_drag(&mut self, cx: &mut Cx, abs: Vec2d, shift: bool) {
        self.state_mut().dragging.clear();
        self.state_mut().backdrop.observe_drag(None);
        self.state_mut().drop_hint = None;
        let Some(drag) = self.drag.take() else {
            return;
        };
        cx.set_cursor(MouseCursor::Default);
        // The button-up may not carry SHIFT any more (it is released
        // together with the mouse often enough); the drag's own last known
        // state stands in for it.
        let shift = shift || drag.shift;
        // Dropped on a thumbnail of the space strip: the window moves there.
        if drag.armed && !drag.resize && self.spaces_drop_window(cx, drag.client, abs, drag.start_rect) {
            self.state_mut().spaces.strip.hide();
            self.update_bar(cx);
            self.redraw_all(cx);
            return;
        }
        if drag.floating && !drag.resize && drag.armed && drag.screen < self.state_mut().screens.live_count() {
            // A float or desktop-style window whose centre lands on another
            // screen moves to that screen's layout, where it was dropped.
            let area = self.screen_area(cx, drag.screen);
            // A parked window's logical centre lies past the screen edge:
            // it stays on its screen.
            let parked = self.warp_is_parked(cx, drag.client);
            let state = self.state_mut();
            let centre = state
                .layout_at(drag.screen)
                .rect_of(drag.client, area, state.gap)
                .filter(|_| !parked)
                .map(|r| r.center());
            if let Some((x, y)) = centre {
                let to = state.screen_under(x, y);
                let (gap, reserved, gaps_out) = (state.gap, state.style.reserved_height(), state.gaps_out);
                if to != drag.screen && state.screens.drop_on_screen(drag.client, to, gap, reserved, gaps_out) {
                    self.focus_client(cx, drag.client);
                }
            }
        }
        if drag.floating && !drag.resize && drag.armed && self.snap_zones_on() {
            let pointer_screen = self.state_mut().screen_under(abs.x, abs.y);
            let area=self.screen_area(cx, pointer_screen);
            self.state_mut().snap.drag(drag.client,abs,area);
            if let Some(zone)=self.state_mut().snap.preview {
                // The snap applies on the screen under the pointer: the
                // window moves there first if its centre stayed behind.
                let state = self.state_mut();
                if state.screens.screen_of(drag.client) != Some(pointer_screen) {
                    let (gap, reserved, gaps_out) = (state.gap, state.style.reserved_height(), state.gaps_out);
                    state.screens.drop_on_screen(drag.client, pointer_screen, gap, reserved, gaps_out);
                }
                // Preserve the free window's size before the edge drag.
                if let Some(w)=self.state_mut().layout_of_mut(drag.client).desktop.get_mut(drag.client) {w.rect=drag.start_rect;}
                self.apply_snap(cx,drag.client,zone);
            }
        }
        self.state_mut().snap.clear();
        // A tiled window dropped on another screen snaps back (moving a
        // tile across screens is move-to-screen's job).
        if !drag.floating
            && !drag.resize
            && drag.armed
            && drag.screen < self.state_mut().screens.live_count()
            && self.state_mut().screen_under(abs.x, abs.y) == drag.screen
        {
            // Tiled move: dropping on another tile swaps the two, which is
            // what Hyprland's `movewindow` drag settles into.
            let area = self.screen_area(cx, drag.screen);
            let gap = self.state_mut().gap;
            if let Some(target) = self
                .state_mut()
                .layout_at(drag.screen)
                .client_at(abs.x, abs.y, area, gap)
            {
                if target != drag.client {
                    let layout = self.state_mut().layout_at_mut(drag.screen);
                    if shift {
                        layout.group_drop(drag.client, target);
                    } else {
                        layout.swap_clients(drag.client, target);
                    }
                    self.focus_client(cx, drag.client);
                }
            }
        }
        self.warp_end_drag(cx, &drag);
        // The drop is over: a shelf the drag called may go back.
        self.shelf_autohide_refresh(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    // --------------------------------------------------------------
    // Divider drag (a plain press IN THE GAP between two tiles)
    // --------------------------------------------------------------

    /// Grab the divider under `abs`, if the point is in a gap and not on
    /// any window. Returns false — leaving the press to the tiles — for
    /// every point that belongs to a client, so a drag that STARTS on a
    /// window behaves exactly as it always did.
    fn begin_divider_drag(&mut self, cx: &mut Cx, abs: Vec2d) -> bool {
        self.sync_geometry(cx);
        // Over a span the press is its window's: the covered screens'
        // hidden tiling has no dividers to grab.
        if self.state_mut().span_at(abs.x, abs.y).is_some() {
            return false;
        }
        let screen = self.state_mut().screen_under(abs.x, abs.y);
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        // A window under the pointer keeps its own press. `client_at`
        // covers floats and the scratchpad too, so anything drawn OVER a
        // gap shadows the divider the way it shadows the wallpaper.
        if self
            .state_mut()
            .layout_at(screen)
            .client_at(abs.x, abs.y, area, gap)
            .is_some()
        {
            return false;
        }
        let Some(hit) = self.state_mut().layout_at(screen).divider_at(abs.x, abs.y, area, gap) else {
            return false;
        };
        // Both sides of the split resize live: name every client under it
        // so `WmDesk` snaps their tiles instead of restarting the 379ms
        // layout tween on each pointer frame (see `WmState::dragging`).
        let clients = self.state_mut().layout_at(screen).clients_under(&hit);
        log!(
            "wm: divider grab {:?} depth {} ratio {:.3} over {} tiles",
            hit.axis,
            hit.depth,
            hit.ratio,
            clients.len()
        );
        self.state_mut().dragging = clients;
        self.div_drag = Some(DividerDrag { hit, start: abs, screen });
        self.apply_divider_cursor(cx, Some(hit.axis));
        true
    }

    fn divider_drag_move(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(drag) = self.div_drag.as_ref() else {
            return;
        };
        let (hit, start, screen) = (drag.hit, drag.start, drag.screen);
        if screen >= self.state_mut().screens.live_count() {
            self.div_drag = None;
            self.state_mut().dragging.clear();
            return;
        }
        let gap = self.state_mut().gap;
        let px = match hit.axis {
            Axis::Horizontal => abs.x - start.x,
            Axis::Vertical => abs.y - start.y,
        };
        // 1:1 with the pointer, always measured from the grab: no tween,
        // no accumulated delta, and a clamp at either end never eats part
        // of the way back.
        self.state_mut().layout_at_mut(screen).drag_divider_px(&hit, px, gap);
        self.apply_divider_cursor(cx, Some(hit.axis));
        self.redraw_all(cx);
    }

    fn end_divider_drag(&mut self, cx: &mut Cx) {
        self.state_mut().dragging.clear();
        if self.div_drag.take().is_none() {
            return;
        }
        log!("wm: divider drop");
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// Track the divider band under the pointer and wear the matching
    /// resize cursor over it. Called AFTER the tiles have seen the move:
    /// a tile's own hover-out resets the cursor to Default, and the last
    /// `set_cursor` of a frame is the one the platform applies.
    fn update_divider_cursor(&mut self, cx: &mut Cx, abs: Vec2d) {
        if self.state.is_none() || self.drag.is_some() || self.div_drag.is_some() {
            return;
        }
        let screen = self.state_mut().screen_under(abs.x, abs.y);
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        let on_client = self.state_mut().span_at(abs.x, abs.y).is_some()
            || self
                .state_mut()
                .layout_at(screen)
                .client_at(abs.x, abs.y, area, gap)
                .is_some();
        let axis = if on_client {
            None
        } else {
            self.state_mut()
                .layout_at(screen)
                .divider_at(abs.x, abs.y, area, gap)
                .map(|hit| hit.axis)
        };
        let was = self.div_hover;
        if axis != was {
            self.div_hover = axis;
            match axis {
                Some(a) => log!("wm: divider hover {:?}", a),
                None => log!("wm: divider hover none"),
            }
            // Leaving a band for bare desk: hand the cursor back. Leaving
            // it for a WINDOW does nothing — the tile's own hover-in just
            // set the cursor its child asked for.
            if axis.is_none() && was.is_some() && !on_client {
                self.apply_divider_cursor(cx, None);
            }
        }
        // Re-applied on every move while on a band, not only on the way
        // in: a tile's hover-out or a child's cursor request may have
        // reset it since the last one.
        if axis.is_some() {
            self.apply_divider_cursor(cx, axis);
        }
    }

    fn apply_divider_cursor(&mut self, cx: &mut Cx, axis: Option<Axis>) {
        cx.set_cursor(match axis {
            // A left|right split is moved sideways, a top/bottom one up
            // and down.
            Some(Axis::Horizontal) => MouseCursor::EwResize,
            Some(Axis::Vertical) => MouseCursor::NsResize,
            None => MouseCursor::Default,
        });
    }

    /// A groupbar tab dragged off its strip: the member leaves the group
    /// and takes a tile of its own, and the press carries on as the
    /// ordinary tiled SUPER-drag — drop it on another tile to swap, or
    /// with SHIFT to make it a tab there.
    fn tear_out_tab(&mut self, cx: &mut Cx, client: ClientId, abs: Vec2d) {
        // The strip may be on any screen: tear out in the tab's own layout.
        let state = self.state_mut();
        let screen = state.screens.screen_of(client).unwrap_or(state.screens.active);
        let area = self.screen_area(cx, screen);
        let gap = self.state_mut().gap;
        if !self.state_mut().layout_at_mut(screen).group_tear_out(client, area, gap) {
            return;
        }
        self.begin_drag_on(cx, client, abs, false, true);
        self.focus_client(cx, client);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// SUPER + wheel over the desk: the next / previous space of the screen
    /// under the pointer.
    fn scroll_spaces(&mut self, cx: &mut Cx, down: bool, abs: Vec2d) {
        let state = self.state_mut();
        // Over a span: its home screen.
        state.screens.active = match state.span_at(abs.x, abs.y) {
            Some((_, home)) => home,
            None => state.screen_under(abs.x, abs.y),
        };
        self.spaces_slide(cx, if down { 1 } else { -1 });
    }

    /// `--test-action <name>` fires one WmAction at startup, so every
    /// binding can be driven from a script even where the host OS keeps a
    /// chord for itself.
    fn run_test_actions(&mut self, cx: &mut Cx) {
        let args: Vec<String> = std::env::args().collect();
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--test-action" {
                if let Some(name) = args.get(i + 1) {
                    // launch-<app id>: spawn a registered app directly — the
                    // deterministic way to put one app on the desk in a test.
                    if let Some(app) = name.strip_prefix("launch-") {
                        let app = app.to_string();
                        log!("wm: --test-action launch {}", app);
                        self.launch_app(cx, &app);
                        i += 2;
                        continue;
                    }
                    match test_action(name) {
                        Some(action) => {
                            log!("wm: --test-action {} -> {:?}", name, action);
                            self.do_action(cx, action);
                        }
                        None => log!("wm: unknown --test-action '{}'", name),
                    }
                }
                i += 2;
            } else {
                i += 1;
            }
        }
    }
}

/// Names accepted by `--test-action`. Anything the keymap binds is
/// reachable by its Omarchy description, lowercased with dashes.
fn test_action(name: &str) -> Option<WmAction> {
    let name = name.trim().to_lowercase();
    let simple = match name.as_str() {
        "terminal" => Some(WmAction::LaunchTerminal),
        "close" => Some(WmAction::CloseWindow),
        "close-all" => Some(WmAction::CloseAllWindows),
        "toggle-split" => Some(WmAction::ToggleSplit),
        "pseudo" => Some(WmAction::TogglePseudo),
        "float" => Some(WmAction::ToggleFloat),
        "pop" => Some(WmAction::PopOut),
        "fullscreen" => Some(WmAction::FullscreenSpace),
        "maximize" | "full-width" => Some(WmAction::FullWidth),
        "tiled-fullscreen" => Some(WmAction::TiledFullscreen),
        "focus-left" => Some(WmAction::FocusDir(Dir::Left)),
        "focus-right" => Some(WmAction::FocusDir(Dir::Right)),
        "focus-up" => Some(WmAction::FocusDir(Dir::Up)),
        "focus-down" => Some(WmAction::FocusDir(Dir::Down)),
        "swap-left" => Some(WmAction::SwapDir(Dir::Left)),
        "swap-right" => Some(WmAction::SwapDir(Dir::Right)),
        "swap-up" => Some(WmAction::SwapDir(Dir::Up)),
        "swap-down" => Some(WmAction::SwapDir(Dir::Down)),
        "vr" | "toggle-vr" => Some(WmAction::ToggleVr),
        "vr-recenter" => Some(WmAction::VrRecenter),
        "vr-nearer" => Some(WmAction::VrDepth { nearer: true }),
        "vr-farther" => Some(WmAction::VrDepth { nearer: false }),
        "vr-look-left" => Some(WmAction::VrLook(Dir::Left)),
        "vr-look-right" => Some(WmAction::VrLook(Dir::Right)),
        "vr-look-up" => Some(WmAction::VrLook(Dir::Up)),
        "vr-look-down" => Some(WmAction::VrLook(Dir::Down)),
        "move-to-next-screen" => Some(WmAction::MoveToScreen { forward: true }),
        "move-to-prev-screen" => Some(WmAction::MoveToScreen { forward: false }),
        "grow" => Some(WmAction::ResizePx {
            axis: Axis::Horizontal,
            px: 100.0,
        }),
        "shrink" => Some(WmAction::ResizePx {
            axis: Axis::Horizontal,
            px: -100.0,
        }),
        "group" => Some(WmAction::ToggleGroup),
        "group-out" => Some(WmAction::MoveOutOfGroup),
        "group-into-left" => Some(WmAction::MoveIntoGroup(Dir::Left)),
        "group-into-right" => Some(WmAction::MoveIntoGroup(Dir::Right)),
        "group-next" => Some(WmAction::GroupNext),
        "group-prev" => Some(WmAction::GroupPrev),
        "scratchpad" => Some(WmAction::ToggleScratchpad),
        "to-scratchpad" => Some(WmAction::MoveToScratchpad),
        "space-next" => Some(WmAction::SpaceSlide(1)),
        "space-prev" => Some(WmAction::SpaceSlide(-1)),
        "space-new" => Some(WmAction::SpaceNew),
        "space-strip" => Some(WmAction::SpaceStrip),
        "cheat-sheet" => Some(WmAction::CheatSheet),
        "cycle" => Some(WmAction::CycleFocus(true)),
        "cycle-back" => Some(WmAction::CycleFocus(false)),
        "menu" => Some(WmAction::Menu),
        "apps" => Some(WmAction::AppsMenu),
        "system" => Some(WmAction::SystemMenu),
        "keys" => Some(WmAction::Keybindings),
        "theme" => Some(WmAction::ThemeMenu),
        "background" => Some(WmAction::BackgroundNext),
        "skin" => Some(WmAction::NextSkin),
        "ai" => Some(WmAction::ToggleAi),
        "bar" => Some(WmAction::ToggleBar),
        "layout" => Some(WmAction::ToggleWorkspaceLayout),
        _ => None,
    };
    if simple.is_some() {
        return simple;
    }
    if let Some(n) = name.strip_prefix("space-") {
        return n.parse::<usize>().ok().map(|n| WmAction::SpaceJump(n.saturating_sub(1)));
    }
    if let Some(n) = name.strip_prefix("move-to-") {
        return n
            .parse::<usize>()
            .ok()
            .map(|n| WmAction::MoveToSpace(n.saturating_sub(1)));
    }
    if let Some(n) = name.strip_prefix("group-") {
        return n.parse::<usize>().ok().map(WmAction::GroupActive);
    }
    None
}

/// The SUPER chord for mouse binds — Ctrl+Alt nested, the Logo key on a
/// Linux session (binds.rs carries the same law for keys).
/// No modifier at all: the bare function keys the WM owns (F10).
fn bare_key(m: &KeyModifiers) -> bool {
    !m.shift && !m.control && !m.alt && !m.logo
}

/// The `os.launch` answer: an app that was already running is in front
/// now and its tools are ready this turn; one that is starting connects
/// in a moment (the engine holds this result until it does).
fn os_launch_answer(call_id: &str, label: &str, already_running: bool) -> ToolResult {
    if already_running {
        ToolResult::ok(call_id, format!("{label} is already running and now in front; its tools are available now"), "already running")
    } else {
        ToolResult::ok(call_id, format!("{label} is starting; its tools become available once it connects"), "starting")
    }
}

/// The registry's ids, for a refusal that names what exists.
fn known_app_ids() -> String {
    clients::registry()
        .iter()
        .map(|a| a.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One row of `os.list`: what the model reads as text and as data.
#[derive(Clone, Debug, PartialEq, SerJson)]
struct OsAppRow {
    id: String,
    label: String,
    running: bool,
    focused: bool,
}

/// The WM's test service: the motion recorder's trace, for apps/wm/tools/frametrace.
fn wm_test_manifest() -> makepad_ai_services::wire::ServiceManifest {
    use makepad_ai_services::wire::{Risk, ServiceManifest, ToolDef};
    ServiceManifest::new("wm", "Desk", "The desk's motion recorder, for tests.")
        .with_tool(ToolDef::new(
            "motion_trace",
            "Write the motion recorder's last ~10 s of desk frames (every window's drawn and shown rect, source, tween, configured and presented size, slides, shelves) as JSONL; answers the path. For apps/wm/tools/frametrace/analyze.py.",
            r#"{"type":"object","properties":{"path":{"type":"string","description":"absolute .jsonl path; default a new file in the temp dir"}}}"#,
            Risk::Read,
        ))
}

/// `os.list`'s answer: one line per app, and the same rows as JSON.
fn os_list_result(call_id: &str, rows: &[OsAppRow]) -> ToolResult {
    let text = rows
        .iter()
        .map(|r| {
            let state = if r.focused {
                "focused"
            } else if r.running {
                "running"
            } else {
                "not running"
            };
            format!("{} (`{}`) — {}", r.label, r.id, state)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let running = rows.iter().filter(|r| r.running).count();
    ToolResult::ok(
        call_id,
        text,
        format!("{} apps, {} running", rows.len(), running),
    )
    .with_data(rows.to_vec().serialize_json())
}

#[cfg(test)]
mod os_service_tests {
    use super::*;

    #[test]
    fn os_list_reads_as_text_and_as_data() {
        let rows = vec![
            OsAppRow { id: "files".into(), label: "Files".into(), running: true, focused: true },
            OsAppRow { id: "route".into(), label: "Route".into(), running: false, focused: false },
        ];
        let result = os_list_result("c1", &rows);
        assert_eq!(result.outcome, makepad_ai_services::wire::ToolOutcome::Ok);
        assert_eq!(result.text, "Files (`files`) — focused\nRoute (`route`) — not running");
        assert_eq!(result.note, "2 apps, 1 running");
        assert!(result.data.contains(r#""id":"files""#) && result.data.contains(r#""running":true"#));
        assert!(result.data.starts_with('[') && result.data.ends_with(']'));
    }

    #[test]
    fn a_launch_answer_says_running_or_starting() {
        let running = os_launch_answer("c1", "Photos", true);
        assert!(running.text.contains("already running") && running.note == "already running");
        let starting = os_launch_answer("c2", "Photos", false);
        assert!(starting.text.contains("starting") && starting.note == "starting");
    }

    #[test]
    fn bare_keys_have_no_modifier() {
        let none = KeyModifiers { shift: false, control: false, alt: false, logo: false };
        assert!(bare_key(&none));
        assert!(!bare_key(&KeyModifiers { shift: true, ..none }));
        assert!(!bare_key(&KeyModifiers { logo: true, ..none }));
    }
}

/// How long a crashed module's tile stays, saying so, before it is closed.
const CRASH_HOLD_SECS: f64 = 1.0;

/// The crashed instances whose hold is over at `now`, taken out of `holds`.
fn crashed_holds_due(holds: &mut Vec<(ClientId, f64)>, now: f64) -> Vec<ClientId> {
    let mut due = Vec::new();
    holds.retain(|(client, since)| {
        if now - since >= CRASH_HOLD_SECS {
            due.push(*client);
            false
        } else {
            true
        }
    });
    due
}

#[cfg(test)]
mod crash_hold_tests {
    use super::*;

    /// A crashed tile is closed on the first tick after its hold, never
    /// before, and each instance on its own clock.
    #[test]
    fn crashed_tiles_close_after_their_hold() {
        let mut holds = vec![(7, 10.0), (8, 10.6)];
        assert!(crashed_holds_due(&mut holds, 10.5).is_empty());
        assert_eq!(crashed_holds_due(&mut holds, 11.0), vec![7]);
        assert_eq!(holds, vec![(8, 10.6)]);
        assert_eq!(crashed_holds_due(&mut holds, 11.6), vec![8]);
        assert!(holds.is_empty());
    }
}

pub(crate) fn super_chord(m: &KeyModifiers) -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        // Both spellings, exactly like the keymap: ⌘ IS Hyprland's SUPER,
        // and Ctrl+Alt is the fallback for what the host OS eats.
        m.logo || (m.control && m.alt)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        m.logo
    }
}

fn scan_theme_color(source: &str, key: &str) -> Option<Vec4f> {
    for line in source.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() == key {
            let rgb = theme::parse_hex(v.trim())?;
            return Some(Vec4f {
                x: rgb.r as f32 / 255.0,
                y: rgb.g as f32 / 255.0,
                z: rgb.b as f32 / 255.0,
                w: 1.0,
            });
        }
    }
    None
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        #[cfg(not(target_arch = "wasm32"))]
        { self.domains.enabled = cx.hal_can_host_domains(); }
        // The one bus owner of this process tree: hosted apps' tools are
        // served through the process registry (ai_bus.rs).
        self.ai_bus = AiBus::with_registry(ServiceRegistry::process());
        if makepad_widgets::makepad_platform::remote::requested() {
            self.wm_port = makepad_ai_services::port::AiServicePort::open(cx, wm_test_manifest());
        }
        self.load_home_order(cx);
        #[cfg(target_arch = "wasm32")]
        {
            self.desktop_mode_request = Some(cx.storage(host::STORAGE).get(cx, host::DESKTOP_MODE_KEY));
        }
        // CLI: --import-theme <name> pulls an omarchy theme and converts
        // it to splash before the desktop appears.
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            if arg == "--import-theme" {
                if let Some(name) = args.next() {
                    match theme::import_omarchy_theme(&name) {
                        Ok(msg) => log!("wm: {}", msg),
                        Err(err) => log!("wm: import failed: {}", err),
                    }
                }
            }
        }

        let theme_name = Self::theme_name_from_env();
        let wallpaper = self.ui.widget(cx, ids!(wallpaper));
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() { desk.wallpaper = wallpaper; }
        let sheet = crate::clients::with_app_artwork(desktop_style::StyleSheet::load(self.build.style.sheet(false)));
        // Children inherit the system stylesheet at spawn; makepad_style
        // events carry subsequent sheet changes to their running windows.
        host::set_child_env("MAKEPAD_WIDGET_STYLE", std::ffi::OsStr::new(&sheet.name));
        self.module_host.apply_style(cx, &sheet);
        self.stylesheet = Some(sheet);
        let source = theme::load_theme_source(&theme_name);
        // The pool as this build wants it: nothing to keep warm without
        // processes (the platform's, or the build's `modules_only`).
        self.warm_pool = WarmPool::from_env(self.processes());
        let browser_dark = desktop_app::browser_appearance(self.build.style, false, &source);
        self.warm_pool.set_browser_appearance(browser_dark);
        #[cfg(target_os = "linux")]
        if std::env::var_os("MAKEPAD_WM_PORTAL").is_none_or(|v| v != "0") {
            let scheme = if browser_dark { portal::PREFER_DARK } else { portal::PREFER_LIGHT };
            match portal::Portal::start(&cx.thread_spawner(), vec![portal_file_chooser::interface()], scheme) {
                Ok(p) => self.portal = Some(p),
                Err(e) => log!("wm portal: not served: {e}"),
            }
        }
        let term_env = theme::scan_term_palette(&source)
            .map(|p| p.env_value())
            .unwrap_or_default();
        let accent = scan_theme_color(&source, "accent").unwrap_or(Vec4f {
            x: 0.48,
            y: 0.63,
            z: 0.97,
            w: 1.0,
        });
        let borders = desk::BorderTheme::from_theme_source(&source);

        // The client hub is the PROCESS host's: a build without processes
        // (the web, iOS, the all-in-one) never binds one and hosts its
        // linked modules instead.
        let hub = if self.processes() {
            AppHub::start(cx.thread_spawner())
        } else {
            None
        };
        let hub_port = hub.as_ref().map(|h| h.port).unwrap_or(0);
        if hub.is_none() && self.processes() {
            log!("wm: could not bind the client hub; tiles will not start");
        }
        self.hub = hub;
        #[cfg(target_os = "linux")]
        if self.hub.is_some() {
            self.adoptions = adopt::Adoptions::start(cx, hub_port);
        }

        host::apply_natural_scroll(host::load_natural_scroll());
        host::apply_tap_to_click(host::load_tap_to_click());
        self.state = Some(WmState {
            snap: Default::default(),
            shot: Default::default(),
            phone: Default::default(),
            screens: crate::screens::ScreenSet::new(
                crate::layout::WmLayout::zoned(),
                LRect::new(0.0, 0.0, 1400.0, 860.0),
            ),
            screen_source: Default::default(),
            warp: { let mut w = spacewarp::Warp::default(); w.mode = host::load_desktop_mode().unwrap_or_default(); w.half_heights = host::load_shelf_half(); w },
            dock_autohide: { let mut d = dock_autohide::DockAutohide::default(); d.set_enabled(host::load_dock_autohide(), host::now()); d },
            shelf_autohide: crate::spacewarp::shelf::ShelfHiding::new(host::load_shelf_autohide()),
            shake: Default::default(),
            behind: Default::default(),
            history: history::History::load(),
            spaces: spaces::Ui::default(),
            vr: crate::stereo::VrState::load(),
            screen_key: Default::default(),
            screen_phys: Default::default(),
            screens_changed: false,
            tile_origins: Default::default(),
            screens_sent: Default::default(),
            screens_dirty: false,
            dock_backdrop: None,
            backdrop: Default::default(),
            skin: skins::Skin::from_env().or_else(|| skins::Skin::from_theme_source(&source)).unwrap_or_default(),
            clients: std::collections::HashMap::new(),
            hub_port,
            theme_name: theme_name.clone(),
            term_env,
            accent,
            borders,
            // gaps_in 5 sits on each side of a window, so two tiles are
            // 10 apart — the same as gaps_out to the desk edge.
            gap: desk::TILE_GAP,
            gaps_out: desk::GAPS_OUT,
            dragging: Vec::new(),
            live_resize: false,
            drop_hint: None,
            pane_sliding: false,
            style: Default::default(),
            launchable: Launchable::default(),
            accessibility: desktop::Accessibility::load(&theme::makepad_home().join("wm/accessibility.splash")),
        });
        self.next_id = 1;
        // The hosting registry: the build's linked modules, the person's
        // overrides in ~/.makepad/wm/apps.splash, a dev run's `--module
        // <id>` flags. What the menus may then offer follows from it.
        let args: Vec<String> = std::env::args().collect();
        let processes = self.processes();
        self.apps = AppRegistry::load(&theme::makepad_home().join("wm/apps.splash"), &args, &self.build, processes);
        log!(
            "wm: modules linked: {:?}{}",
            self.apps.linked_ids(),
            if processes { "" } else { " (no processes: every app is a module)" }
        );
        if self.domains_enabled() { self.apps.enable_domains(); }
        let launchable = self.apps.launchable();
        self.state_mut().launchable = launchable.clone();
        if let Some(mut menu) = self.ui.widget(cx, ids!(shell_menu)).borrow_mut::<ShellMenu>() {
            menu.model.launchable = launchable;
        }
        // The WM's own service in the process registry: the in-process
        // assistant and the process's `app` service both reach it there.
        let manifest = AiBus::os_manifest(&self.registry_apps());
        self.pane_links.open_os(cx, manifest);
        self.tick = cx.start_interval(1.0);
        // The warm pool's pump. Started even when the pool is off: it
        // costs one no-op wakeup and keeps the timer id stable.
        self.warm_tick = cx.start_interval(WARM_PUMP);
        // Nothing to pump (no pool, or a phone, which keeps none): no 20 Hz
        // wakeup of an idle WM.
        if !self.warm_pool.enabled() || cfg!(target_os = "android") {
            cx.stop_timer(self.warm_tick);
            self.warm_tick = Timer::empty();
        }
        if !self.warm_pool.enabled() {
            if self.processes() {
                log!("wm: warm pool disabled (MAKEPAD_WM_NO_WARM)");
            } else {
                log!("wm: warm pool off: this build hosts no processes");
            }
        }

        // `--gallery`: the shell surfaces with fixture data instead of a
        // desktop, so the port is verifiable over --remote (shell/gallery.rs).
        if shell::gallery::ShellGallery::requested() {
            self.gallery = true;
            // Built now, not in the DSL: the fixture tree is deep and only
            // a gallery run ever looks at it (shell/gallery.rs).
            let host = self.ui.widget(cx, ids!(shell_gallery_host));
            if let Some(mut host) = host.borrow_mut::<shell::gallery::ShellGalleryHost>() {
                host.ensure(cx);
            }
            self.ui.widget(cx, ids!(gallery_holder)).set_visible(cx, true);
            self.ui.widget(cx, ids!(main_column)).set_visible(cx, false);
            self.redraw_all(cx);
            return;
        }

        // The build's window and style. The window is not created yet, so
        // a phone-shaped desktop is asked for as the CREATE size (a resize
        // now would address a window that does not exist); on a phone the
        // screen is the window. Then the style itself, through the same
        // path a menu choice takes, before the first frame — no Omarchy
        // desk flashes first.
        // What the phone shell draws around the apps: a desktop window
        // playing a phone fakes the status bar, island and home indicator;
        // the phone itself has them from its OS and lays out against its
        // real insets (mobile::PhoneChrome), kept current on every
        // window geometry.
        self.state_mut().phone.chrome = mobile::PhoneChrome::for_host(host::device_phone(), cx.display_context.safe_area_insets);
        // On the phone the shell owns the bottom edge: the OS defers its
        // home gesture there, so the first swipe up reaches the shell (a
        // second one is the OS's, as in any immersive app).
        if matches!(self.state_mut().phone.chrome, mobile::PhoneChrome::Device { .. }) {
            cx.defer_system_gestures(ScreenEdges::BOTTOM);
        }
        let style = self.build.style;
        let title = self.build.title.clone();
        let window = self.ui.window(cx, ids!(main_window));
        if style.mobile() && !cfg!(any(target_os = "ios", target_os = "android")) {
            window.configure_window(cx, phone_size(style), dvec2(80.0, 60.0), false, title);
        } else {
            window.set_title(cx, &title);
        }
        if style != desktop::DesktopStyle::Omarchy {
            self.set_desktop_style(cx, style);
        }
        // The look the person chose last time (a desktop build only).
        if !style.mobile() {
            if let Some(entry) = host::load_desktop_look().and_then(|(id, dark)| {
                self.state_mut().style.dark = dark;
                desktop_style::find(&id)
            }) {
                self.set_desktop_sheet(cx, entry);
            }
        }

        // The assistant starts WITH the desktop (user, 2026-09-02), the
        // pane closed: its body — the aichat child, or the module in-process
        // — is running, configured and drawn off the desk's edge before any
        // F10, so the first slide-in is instant. Nothing takes the keyboard
        // here: the layout's focus stays where it is.
        if self.apps.pane_in_process() {
            if self.with_ai_pane(cx, |cx, p| p.ensure_overlay(cx)).unwrap_or(false) {
                log!("wm: the assistant starts with the desktop (in-process)");
            }
        } else if hub_port != 0 && self.launch_ai_pane(cx, false) {
            log!("wm: the assistant starts with the desktop (the aichat child)");
        }

        // Start EMPTY like omarchy — booting children is slow (first-exec
        // scan) and the desk is usable instantly. MAKEPAD_WM_TEST_APP=app[:count]
        // boots a test scene ("terminal:3" = the old A | (B / C)).
        // A comma list boots several apps ("notes,clock").
        if let Ok(list) = std::env::var("MAKEPAD_WM_TEST_APP") {
          for spec in list.split(',') {
            let (app, count) = match spec.split_once(':') {
                Some((app, n)) => (app.to_string(), n.parse::<usize>().unwrap_or(1).min(9)),
                None => (spec.to_string(), 1),
            };
            for _ in 0..count {
                self.launch_app(cx, &app);
            }
          }
        }

        self.history_restore();
        self.apply_background(cx, 0);
        self.fetch_backgrounds_if_missing(cx);
        self.update_bar(cx);
        self.update_status(cx);
        // The platform installs a default menu whose Quit is ⌘Q — which
        // would take the whole desktop down when the user means "close
        // this window" (omarchy's SUPER+Q). Replace it: Quit keeps a key
        // equivalent no window binding uses.
        #[cfg(target_os = "macos")]
        cx.update_macos_menu(MacosMenu::Main {
            items: vec![MacosMenu::Sub {
                name: "wm".to_string(),
                items: vec![MacosMenu::Item {
                    command: live_id!(quit),
                    key: KeyCode::KeyQ,
                    shift: true,
                    enabled: true,
                    name: "Quit wm".to_string(),
                }],
            }],
        });

        // Scripted verification: --test-action fires WM actions with no
        // keyboard involved (some chords belong to the host OS).
        self.run_test_actions(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let answers = self.relay.on_actions(actions);
        self.send_relays(answers);
        if self.gallery {
            let gallery = self.ui.widget(cx, ids!(shell_gallery));
            {
                let mut borrowed = gallery.borrow_mut::<shell::gallery::ShellGallery>();
                if let Some(g) = borrowed.as_mut() {
                    g.handle_actions(cx, actions);
                }
            }
        }
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            // The shell surfaces: the bar's presses and wheel, the menu's
            // activations, the flyouts' controls.
            match wa.cast::<ShellBarAction>() {
                ShellBarAction::Press(seg, module) => match module {
                    BarModule::Style => {
                        self.bar_press_segment = seg;
                        self.open_style_menu(cx)
                    }
                    BarModule::Menu => self.toggle_launcher(cx),
                    BarModule::Space(i) => self.bar_workspace(cx, seg, i),
                    BarModule::ActiveWindow => {
                        if let Some(focus) = self.bar_focused_client(seg) {
                            self.focus_client(cx, focus);
                        }
                    }
                    control @ (BarModule::WindowMin | BarModule::WindowMax | BarModule::WindowClose) => {
                        // The gallery's bar is a picture of one: its
                        // controls log (shell/gallery.rs), never close or
                        // shrink the window they are drawn in.
                        if !self.gallery {
                            self.window_control(cx, control);
                        }
                    }
                    other => self.toggle_shell_panel(cx, seg, other),
                },
                ShellBarAction::RightPress(seg, module) => match module {
                    // The Omarchy button's right click opens a terminal.
                    BarModule::Style => {
                        self.bar_press_segment = seg;
                        self.open_style_menu(cx)
                    }
                    BarModule::Menu => self.do_action(cx, WmAction::LaunchTerminal),
                    BarModule::ActiveWindow => {
                        if let Some(focus) = self.bar_focused_client(seg) {
                            self.request_close(cx, focus);
                        }
                    }
                    BarModule::Clock => {
                        self.clock_alt = !self.clock_alt;
                        self.update_status(cx);
                        self.update_bar(cx);
                    }
                    BarModule::Audio => {
                        self.toggle_output_mute(cx);
                    }
                    _ => {}
                },
                ShellBarAction::MiddlePress(seg, module) => {
                    // `ActiveWindow.qml`: middle click closes the focused
                    // window exactly like a right click — every other
                    // module ignores the middle button.
                    if module == BarModule::ActiveWindow {
                        if let Some(focus) = self.bar_focused_client(seg) {
                            self.request_close(cx, focus);
                        }
                    }
                }
                ShellBarAction::Wheel(module, dir) => {
                    if module == BarModule::Audio {
                        let level = self.bar_sample.volume.unwrap_or(0) as f64;
                        let next = (level + dir * 5.0).clamp(0.0, 100.0) as u32;
                        self.set_volume(cx, next);
                    }
                }
                ShellBarAction::None => {}
            }
            // ONLY the overlay's own menu instance may drive the desktop —
            // the gallery embeds fixture menus whose actions must never
            // launch apps or open the real card (the ghost-launch bug).
            if wa.widget_uid == self.ui.widget(cx, ids!(shell_menu)).widget_uid() {
                match wa.cast::<ShellMenuAction>() {
                    ShellMenuAction::Activate(target) => self.shell_menu_activate(cx, &target),
                    ShellMenuAction::Cancel => self.close_shell_menu(cx),
                    ShellMenuAction::None => {}
                }
            }
            if wa.widget_uid == self.ui.widget(cx, ids!(shell_panel)).widget_uid() {
            match wa.cast::<ShellPanelAction>() {
                ShellPanelAction::SetVolume(v) => self.set_volume(cx, v),
                ShellPanelAction::Hub(command) => {
                    let pool = cx.task_pool();
                    self.hub_link.command(command, host::now(), &pool);
                    self.hub_poll(cx);
                }
                ShellPanelAction::ToggleMute => {
                    self.toggle_output_mute(cx);
                }
                ShellPanelAction::Close => {
                    self.shell_panel_open = None;
                    self.update_bar(cx);
                }
                #[cfg(target_os = "linux")]
                ShellPanelAction::Wifi(cmd) => self.wifi_command(cx, cmd),
                #[cfg(target_os = "linux")]
                ShellPanelAction::System(action) => self.linux_control(cx, action),
                _ => {}
            }
            }
            match wa.cast::<MpRunViewAction>() {
                MpRunViewAction::ForwardToApp { client, msg_bin } => {
                    if let Some(state) = self.state.as_ref() {
                        if let Some(slot) = state.clients.get(&client) {
                            if let Some(sender) = &slot.sender {
                                let _ = sender.send(msg_bin);
                            }
                        }
                    }
                    // A later window does not pace its app: what it sent is drawn on the
                    // app's next Tick, which the app's first window sends.
                    if let Some((owner, _)) = self.client_windows.owner(client) {
                        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| d.with_run_view(cx, owner, |cx, v| v.request_tick(cx)));
                    }
                }
                MpRunViewAction::Clicked { client } => {
                    if !self.behind_deferred(cx, client) {
                        self.focus_client(cx, client);
                    }
                }
                MpRunViewAction::Crashed { client, message } => {
                    // The instance's tile has already let go of the root
                    // and shows "crashed"; after the hold the instance goes
                    // the way a module close does — shutdown in its
                    // isolate, isolate freed, tile out of the layout — and
                    // the desk goes on.
                    error!("wm: module instance for client {client} crashed: {message}");
                    if !self.crashed_holds.iter().any(|(c, _)| *c == client) {
                        self.crashed_holds.push((client, host::now()));
                    }
                }
                MpRunViewAction::None => {}
            }
            match wa.cast::<WmDeskAction>() {
                WmDeskAction::TearOutTab { client, abs } => self.tear_out_tab(cx, client, abs),
                WmDeskAction::None => {}
            }
        }
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        if self.state.is_some() {
            self.drain_hub(cx);
            self.drain_client_lines(cx);
            self.drain_module_upstream();
            self.wayland_poll(cx);
        }
    }
}

impl App {
    /// Map pointer input back through the fixed desktop camera.
    fn vr_pointer_event(&mut self, cx: &mut Cx, event: &Event) -> Option<Result<Event, ()>> {
        if !self.state.as_ref().is_some_and(|s| s.vr.active()) {
            self.vr_pointer_capture = None;
            return None;
        }
        let point = match event {
            Event::MouseDown(e) => e.abs,
            Event::MouseMove(e) => e.abs,
            Event::MouseUp(e) => e.abs,
            Event::Scroll(e) => e.abs,
            Event::Pinch(e) => e.abs,
            _ => return None,
        };
        let captured = self.vr_pointer_capture;
        self.state_mut().vr.physical_pointer = Some((point.x, point.y));
        if matches!(event, Event::MouseUp(_)) {
            self.vr_pointer_capture = None;
        }
        if captured.is_none() && self.ui.widget(cx, ids!(desktop_shelf))
            .borrow::<desktop::DesktopShelf>().is_some_and(|shelf| shelf.contains(point)) {
            return None;
        }
        let mapped = self.desk(cx).borrow::<WmDesk>()
            .and_then(|desk| desk.vr_pointer(point, captured));
        let Some((logical, client, viewport)) = mapped else { return None; };
        let pointer = Some((logical.x, logical.y));
        self.state_mut().vr.pointer = pointer;
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
            desk.vr_move_pointer(cx, pointer);
        }
        if let Event::MouseDown(_) = event {
            self.vr_pointer_capture = Some(viewport);
        }
        // Empty margins belong to the camera, not an untransformed widget
        // that happens to occupy those coordinates underneath the image.
        if client.is_none() && captured.is_none() && !matches!(event, Event::MouseMove(_)) {
            return Some(Err(()));
        }
        Some(Ok(match event {
            Event::MouseDown(e) => { let mut e = e.clone(); e.abs = logical; Event::MouseDown(e) }
            Event::MouseMove(e) => { let mut e = e.clone(); e.abs = logical; Event::MouseMove(e) }
            Event::MouseUp(e) => { let mut e = e.clone(); e.abs = logical; Event::MouseUp(e) }
            Event::Scroll(e) => { let mut e = e.clone(); e.abs = logical; Event::Scroll(e) }
            Event::Pinch(e) => { let mut e = e.clone(); e.abs = logical; Event::Pinch(e) }
            _ => return None,
        }))
    }
}

impl AppMain for App {
    /// The WM hosts other apps' windows; it never looks for another WM to host its own.
    fn window_manager() -> bool {
        true
    }

    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // The build the binary set on Cx (build.rs): the style the widgets
        // come up in, and whether the assistant's families are linked.
        let build = WmBuild::from_cx(vm.cx_mut());
        desktop_style::install(vm,crate::clients::with_app_artwork(desktop_style::StyleSheet::load(build.style.sheet(false))));
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);

        // The theme: evaluated before any module that reads
        // mod.wm_theme. This IS the theming system — splash.
        theme::ensure_default_theme();
        let theme_name = App::theme_name_from_env();
        let source = theme::load_theme_source(&theme_name);
        let eval_theme = |vm: &mut ScriptVm, name: &str, code: &str| -> bool {
            let script_mod_id = ScriptMod::new(env!("CARGO_MANIFEST_DIR"), &name, "theme.splash", 0, 0, &code, &[]);
            let value = vm.eval(script_mod_id);
            let errors = vm.take_errors();
            for e in &errors {
                log!("wm theme: {}", e);
            }
            !value.is_err() && errors.is_empty()
        };
        // Leading comment lines shift the runtime parser's span tracking
        // (the script_mod! gotcha applies to eval bodies too): start the
        // body at the first real statement.
        let source: String = {
            let mut lines = source.lines().peekable();
            while let Some(line) = lines.peek() {
                let t = line.trim();
                if t.is_empty() || t.starts_with("//") {
                    lines.next();
                } else {
                    break;
                }
            }
            let mut s = lines.collect::<Vec<_>>().join("\n");
            // Parser quirk: the FINAL statement of an eval body is treated
            // as its result expression — a trailing `mod.x = {...}` never
            // commits. A benign trailing statement makes the assignment a
            // real statement. (Same class as the splash auto-close notes.)
            s.push_str("\ntrue\n");
            s
        };
        if !eval_theme(vm, "wm_theme", &source) {
            // Fall back to the bundled default so the DSL still evaluates.
            let mut fallback = theme::BUNDLED_TOKYO_NIGHT_SPLASH.to_string();
            fallback.push_str("\ntrue\n");
            eval_theme(vm, "wm_theme_fallback", &fallback);
        }

        // The shell token object (`mod.wm_theme.shell`): the omarchy
        // `shell.toml.tpl` contract, resolved from this theme's palette —
        // unless the theme ships its own `shell: {...}` block, which
        // replaces it wholesale.
        if !theme::theme_defines_shell(&source) {
            let mut block = theme::shell_splash_block(&source);
            block.push_str("\ntrue\n");
            eval_theme(vm, "wm_theme_shell", &block);
        }

        run_view::script_mod(vm);
        domain_view::script_mod(vm);
        module_view::script_mod(vm);
        wayland_view::script_mod(vm);
        // The assistant as a module: its panel and overlay root, so the
        // pane can seat `mod.widgets.AiChatOverlay{}` by name — when the
        // build links it.
        if let Some(assistant) = build.assistant {
            assistant(vm);
        }
        shell::script_mod(vm);
        desktop::script_mod(vm);
        snap::script_mod(vm);
        mobile_surface::script_mod(vm);
        desk::phone::script_mod(vm);
        dock_warp::script_mod(vm);
        backdrop::script_mod(vm);
        spacewarp::panel::script_mod(vm);
        skins::script_mod(vm);
        stereo::script_mod(vm);
        desk::script_mod(vm);
        scene::script_mod(vm);
        screenshot::script_mod(vm);
        self::script_mod(vm)
    }

    /// The app is made: it keeps the build it was made for.
    fn after_new_from_script(vm: &mut ScriptVm, app: &mut Self) {
        app.build = WmBuild::from_cx(vm.cx_mut());
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.state.is_some() {
            self.domain_event(cx, event);
            self.flush_domain_messages(cx);
            if let Event::KeyDown(e) = event {
                if e.key_code == KeyCode::F5 && e.modifiers.control && !e.is_repeat {
                    if let Some(client) = self.state_mut().layout().focused_client() {
                        if self.relaunch_domain(cx, client) { return; }
                    }
                }
            }
        }
        if matches!(event, Event::WindowLostFocus(_)) {
            self.vr_pointer_capture = None;
        }
        if let Event::GpuPressure(e) = event {
            self.gpu_pressure(e);
        }
        if self.history_event(cx, event) { return; }
        self.dock_autohide_event(cx, event);
        self.shelf_autohide_event(cx, event);
        if matches!(event, Event::NetworkResponses(_) | Event::PermissionResult(_) | Event::Clipboard(_)) {
            let answers = self.relay.on_event(event);
            self.send_relays(answers);
        }
        if matches!(cx.os_type(), OsType::LinuxDirect) && matches!(event, Event::MouseMove(_)) {
            if let Some(mut window) = self.ui.window(cx, ids!(main_window)).borrow_mut() {
                window.handle_direct_mouse_cursor(cx, event);
            }
        }
        // Recording belongs to the WM, including on Home and in an OS menu.
        // Forwarding this chord also starts a recorder in the focused child.
        if let Event::KeyDown(e) | Event::KeyUp(e) = event {
            if e.key_code == KeyCode::F10 && e.modifiers.control && !e.modifiers.shift {
                if matches!(event, Event::KeyDown(_)) && !e.is_repeat {
                    if let Some(mut window) = self.ui.window(cx, ids!(main_window)).borrow_mut() {
                        window.toggle_recording(cx);
                    }
                }
                return;
            }
        }
        // The Wi-Fi password prompt owns the keyboard, the typed text and
        // the clipboard/IME queries while it is up (shell/panels.rs): taken
        // before the WM's own binds, the AI pane and any hosted child.
        if self.state.is_some() && self.shell_panel_secret_input(cx, event) {
            return;
        }
        // A client's file dialog is modal while it is up: the pointer, the keys and the
        // typed text are its own, before the WM's binds and any client.
        if self.file_picker_event(cx, event) {
            return;
        }
        // A reconcile in the last desk draw: the bar follows it now.
        if self.state.is_some() && !matches!(event, Event::Draw(_)) {
            self.drain_screens_changed(cx);
            self.sync_screens_events();
        }
        self.phone_animation_event(cx,event);
        if self.state.is_some() && self.state_mut().backdrop.take_flattened() {
            // The zones have flattened out: the plain background takes over.
            self.apply_background(cx, 0);
        }
        if let Event::Storage(responses) = event {
            self.desktop_mode_response(cx, responses);
            self.home_order_response(cx, responses);
            self.shell_look_response(cx, responses);
        }
        if let Some(dt) = self.style_clock.tick(cx, event) {
            if self.state.is_some() {
                let moving = self.state_mut().style.step(dt);
                self.style_clock.finish_frame(cx, moving);
                self.redraw_all(cx);
            } else {
                self.style_clock.stop();
            }
        }
        // A tween ticker pause holds the crossfade where it is; the ticker's
        // resume redraws everything, and the draw re-arms the fade.
        if let Event::Draw(_) = event {
            if self.style_clock.is_held() {
                let moving = self.state.as_ref().is_some_and(|s| s.style.active());
                self.style_clock.draw_check(cx, moving);
            }
        }
        if let Event::WindowGeomChange(ev) = event {
            if let Some(state) = self.state.as_mut() {
                state.phone.chrome.set_insets(ev.new_geom.safe_area_insets);
            }
            // A new phone size takes the finger away (what it held moved).
            if ev.old_geom.inner_size != ev.new_geom.inner_size
                && self.state.as_ref().is_some_and(|s| s.style.target.mobile())
            {
                self.cancel_phone_input(cx);
            }
            if ev.old_geom.inner_size != ev.new_geom.inner_size {
                if let Some(mut scene) = self.ui.widget(cx, ids!(scene)).borrow_mut::<scene::WmScene>() {scene.cut(cx);}
            }
            // The bar is the caption: keep it around the OS buttons.
            self.update_bar_chrome(cx, &ev.new_geom);
            // A warm instance is configured with the desktop's own dpi, so
            // what it warms up is what its tile will use — the desktop
            // window's, not any other window's.
            if Some(ev.window_id) == self.ui.window(cx, ids!(main_window)).window_id() {
                self.dpi_factor = ev.new_geom.dpi_factor;
            }
            // Screens added, removed, moved or rescaled (a DPI change does
            // not bump the display generation, so this is the hook for it).
            self.sync_screens(cx);
        }
        // The Linux controls watch the main window's geometry and their own
        // re-anchor frame (linux_controls.rs).
        #[cfg(target_os = "linux")]
        if !self.gallery {
            self.linux_controls_event(cx, event);
        }
        if let Event::WindowLostFocus(_) = event {
            if self.state.as_ref().is_some_and(|s| s.style.target.mobile()) {
                self.cancel_phone_input(cx);
            }
        }
        if let Event::WindowDragQuery(dq) = event {
            // Caption-less window: the bar is the drag handle (on the
            // floating multi-screen bar, only its painted segments); the
            // desk below answers Client so tile clicks never move the
            // window. macOS treats unanswered strip points as native drag.
            // The shell bar's own modules are BUTTONS, not a drag handle:
            // where it claims a point, the press reaches the widget.
            let bar = self.ui.view(cx, ids!(bar)).area();
            if self.phone_toolbar_hit(cx,dq.abs).is_some() || self.shell_bar_claims(cx, dq.abs) {
                dq.response.set(WindowDragQueryResponse::Client);
            } else if bar.is_valid(cx) && bar.rect(cx).contains(dq.abs) && self.bar_covers(cx, dq.abs) {
                dq.response.set(WindowDragQueryResponse::Caption);
            } else {
                dq.response.set(WindowDragQueryResponse::Client);
            }
        }
        if self.state.is_some() && !self.state_mut().style.target.mobile() {
            if let Event::MouseDown(e)=event {
                let hit=self.ui.widget(cx,ids!(shell_bar)).borrow::<shell::bar::ShellBar>().and_then(|b|b.hit_at(e.abs));
                if let Some((seg,BarModule::Style))=hit {
                    self.bar_press_segment=seg;
                    self.open_style_menu(cx);
                    return;
                }
            }
        }
        // The phone skin's simulated finger owns its press until it lifts:
        // its moves and release go to the app before any shell surface.
        if self.state.is_some() && self.phone_finger_route(cx, event) {
            return;
        }
        // The shell menu (and, for move/down/up, an open bar flyout) is
        // modal: while it is up, the pointer event is exclusively its own
        // — see `shell_menu_pointer` / `shell_panel_pointer`. Taken before
        // anything else, including SUPER+drag, so an open surface always
        // wins over the desk beneath it.
        if self.state.is_some()
            && matches!(
                event,
                Event::TouchUpdate(_) | Event::FingerCancel(_) | Event::MouseMove(_) | Event::MouseDown(_) | Event::MouseUp(_) | Event::Scroll(_)
            )
            && (self.shell_menu_pointer(cx, event) || self.shell_panel_pointer(cx, event))
        {
            cx.set_cursor(MouseCursor::Default);
            return;
        }
        let physical_pointer = match event {
            Event::MouseMove(e) => Some(e.abs),
            Event::MouseDown(e) => Some(e.abs),
            Event::MouseUp(e) => Some(e.abs),
            Event::Scroll(e) => Some(e.abs),
            _ => None,
        };
        let vr_mapped = self.vr_pointer_event(cx, event);
        let event = match &vr_mapped {
            Some(Err(())) => return,
            Some(Ok(mapped)) => mapped,
            None => event,
        };
        // Crossing into another screen makes it the active one (not
        // mid-drag: a drag keeps its own screen until it drops).
        if self.state.is_some()
            && self.drag.is_none()
            && self.div_drag.is_none()
            && !self.state_mut().style.target.mobile()
        {
            if let Event::MouseMove(MouseMoveEvent { abs, .. }) | Event::MouseDown(MouseDownEvent { abs, .. }) = event {
                self.pointer_screen(cx, *abs);
            }
        }
        if self.state.is_some() { self.behind_observe(cx, event); }
        if self.phone_search_event(cx,event) {return;}
        if self.state.is_some() && self.phone_pointer(cx,event) {return;}
        if self.state.is_some() && self.shot_event(cx, event) {return;}
        if self.state.is_some() && self.spaces_pointer(cx, event) {return;}
        if self.state.is_some() && self.snap_event(cx,event) {return;}
        if self.state.is_some() {
            self.warp_fold_on_press(cx, event);
            if self.warp_unfold_resize(cx, event) {return;}
        }
        if self.state.is_some() && self.desktop_pointer(cx,event,physical_pointer) {return;}
        // The AI pane owns the pointer inside its rect while it is open:
        // the event goes to the pane alone, so neither the WM's own drag
        // gestures nor the tile underneath ever see it.
        if self.state.is_some() && self.ai_pane_pointer(cx, event) {
            return;
        }
        // The pane rect follows the desk: refreshed from the desk's last
        // drawn rect right before every draw, one borrow's worth.
        if let Event::Draw(_) = event {
            if self.state.is_some() {
                self.sync_ai_pane_geometry(cx);
                self.sync_surface_targets(cx);
                let live = self.drag.as_ref().is_some_and(|d| d.resize && d.armed) || self.div_drag.is_some();
                let state = self.state_mut();
                state.live_resize = live || state.warp.unfold_resize.is_some();
            }
        }
        // SUPER + mouse:272 / mouse:273 — move and resize (tiling.lua).
        // Taken before the tiles see it, so the drag never reaches a child.
        if self.state.is_some() && !self.state_mut().style.target.mobile() {
            match event {
                Event::MouseDown(e) if super_chord(&e.modifiers) => {
                    let resize = e.button.contains(MouseButton::SECONDARY);
                    if self.begin_drag(cx, e.abs, resize) {
                        return;
                    }
                }
                Event::MouseMove(e) if self.drag.is_some() => {
                    self.drag_move(cx, e.abs, e.modifiers.shift);
                    return;
                }
                Event::MouseUp(e) if self.drag.is_some() => {
                    // A press on a shelf item that never became a drag was a
                    // click: the app gets it now.
                    let click = self.state.as_mut().and_then(|s| s.warp.shelf_press.take());
                    self.end_drag(cx, e.abs, e.modifiers.shift);
                    if let Some((client, _)) = click {
                        self.warp_unfold(cx, client);
                    }
                    return;
                }
                // A PLAIN press in the gap between two tiles grabs the
                // divider there (`resize_on_border` scoped to the gap —
                // see `DividerDrag`). It only ever fires where nothing
                // else wants the press: over a window `begin_divider_drag`
                // declines and the tiles get it, unchanged.
                Event::MouseDown(e)
                    if !super_chord(&e.modifiers)
                        && e.button.contains(MouseButton::PRIMARY)
                        && self.drag.is_none() =>
                {
                    if self.begin_divider_drag(cx, e.abs) {
                        return;
                    }
                }
                Event::MouseMove(e) if self.div_drag.is_some() => {
                    self.divider_drag_move(cx, e.abs);
                    return;
                }
                Event::MouseUp(_) if self.div_drag.is_some() => {
                    self.end_divider_drag(cx);
                    return;
                }
                // SUPER + wheel over the desk slides between spaces.
                Event::Scroll(e) if super_chord(&e.modifiers) => {
                    if self.history_scroll(cx, e.scroll.y, e.abs) {
                        return;
                    }
                    if let Some(dir) = self.space_scroll.feed(e.phase, e.scroll.y, 0.5) {
                        self.scroll_spaces(cx, dir > 0.0, e.abs);
                    }
                    return;
                }
                _ => {}
            }
        }
        if let Event::BackPressed { handled } = event {
            if self.state.is_some() && self.state_mut().style.target.mobile() {
                self.phone_action(cx, PhoneHit::Back);
                handled.set(true);
                return;
            }
        }
        // WM keybinds intercept before anything reaches the tiles.
        if let Event::KeyDown(e) = event {
            if self.state.is_some() {
                // Esc folds an unfolded shelf window back to its tile.
                if e.key_code == KeyCode::Escape && bare_key(&e.modifiers)
                    && self.desk(cx).borrow::<WmDesk>().is_some_and(|d| d.warp.unfold.is_some_and(|u| u.open()))
                {
                    self.warp_fold(cx);
                    return;
                }
                if self.shot_key(cx, e) { self.alt_armed = false; return; }
                // The shell menu grabs the keyboard while it is up.
                if self.shell_menu_key(cx, e) {
                    self.alt_armed = false;
                    return;
                }
                if self.phone_key(cx,e) {return;}
                // F10 is the assistant, everywhere (aicontrol decision 8):
                // under the WM the bare key opens the pane for whatever is
                // focused, before the keymap and before any tile.
                if e.key_code == KeyCode::F10 && bare_key(&e.modifiers) {
                    self.alt_armed = false;
                    self.do_action(cx, WmAction::ToggleAi);
                    return;
                }
                if self.spaces_key(cx, e) { self.alt_armed = false; return; }
                // F11 is fullscreen, everywhere: the focused window gets a
                // space of its own (or goes back to its desktop).
                if e.key_code == KeyCode::F11 && bare_key(&e.modifiers) {
                    self.alt_armed = false;
                    self.do_action(cx, WmAction::FullscreenSpace);
                    return;
                }
                if self.desktop_key(cx,e) {return;}
                let armed = self.alt_armed;
                if let Some(action) = match_bind_armed(e.key_code, &e.modifiers, armed) {
                    // Any key but the prefix itself disarms it.
                    if action != WmAction::ArmAltLayer {
                        self.alt_armed = false;
                    }
                    self.do_action(cx, action);
                    return;
                }
                if armed {
                    self.alt_armed = false;
                }
                // A Quick-Look preview closes on Escape or Space, before
                // the key reaches the viewer — unless the pane is up and
                // holds the keyboard: Escape and Space are the chat's then.
                if matches!(e.key_code, KeyCode::Escape | KeyCode::Space)
                    && !self.ai_pane_is_open(cx)
                    && self.close_focused_preview(cx)
                {
                    return;
                }
            }
        }
        if let Event::Shutdown = event {
            // The WM's AI hub ends with it (hub_child).
            let pool = cx.task_pool();
            self.hub_link.shutdown(&pool);
            if self.state.is_some() {
                self.kill_warm_previews(cx);
                self.kill_pane_child(cx);
            }
            self.wifi_shutdown();
            #[cfg(target_os = "linux")]
            {
                self.shutdown_linux_controls();
                // "Restart desktop now": exit 75, which the systemd units
                // restart (`RestartForceExitStatus`/`SuccessExitStatus`).
                // An ordinary quit is left alone and still exits 0 through
                // the platform's own shutdown path.
                let code = linux_controls::exit_code(self.restart_requested());
                if code != 0 {
                    std::process::exit(code);
                }
            }
        }
        if let Event::Timer(te) = event {
            if self.tick.is_timer(te).is_some() && self.state.is_some() {
                self.hub_poll(cx);
                for client in crashed_holds_due(&mut self.crashed_holds, host::now()) {
                    let foreground = self.state_mut().phone.client == Some(client);
                    self.request_close(cx, client);
                    // The phone's foreground app is gone: Home, with the
                    // frame chain armed so the openness ease actually runs
                    // — a close from the tick has no gesture driving frames,
                    // and without them the shell stays dimmed under a tile
                    // that is no longer there.
                    if foreground {
                        self.state_mut().phone.navigate(mobile::PhoneScreen::Home);
                        self.animate_phone(cx);
                    }
                }
                self.reap_exited(cx);
                self.history_poke(cx);
                self.poll_backgrounds(cx);
                self.drain_client_lines(cx);
                self.explain_first_exec_scan(cx);
                self.update_status(cx);
                self.wifi_tick(cx);
                // A hotplug published since, or a lost screen's removal
                // debounce running out.
                self.sync_screens(cx);
                self.update_bar(cx);
                // The pool fills itself here: at startup, after an
                // adoption, and after any death it healed from. One spawn
                // per second, so a cold desktop never forks four cargo
                // builds at once.
                if !self.gallery {
                    self.top_up_warm_pool(cx);
                }
            }
            if self.warm_tick.is_timer(te).is_some() && self.state.is_some() {
                self.pump_warm(cx);
            }
        }
        if let Event::Signal = event {
            if SignalToUI::check_and_clear_ui_signal() && self.state.is_some() {
                makepad_app_host::hosted::trace_host(host::wall_now(), "sig");
                self.poll_backgrounds(cx);
                self.drain_hub(cx);
                self.drain_client_lines(cx);
                self.drain_wifi(cx);
                #[cfg(target_os = "linux")]
                self.poll_linux_controls(cx);
            }
            // Every signal, whoever cleared the flag: a portal call is a cheap channel read.
            #[cfg(target_os = "linux")]
            if self.state.is_some() {
                self.portal_calls(cx);
            }
        }

        // An in-process assistant's calls arrive on channels, not events:
        // looked at on every event, cheap when there is nothing.
        if self.state.is_some() {
            self.drain_wm_port(cx, event);
            self.drain_pane_links(cx);
            self.drain_bus_registry();
        }

        self.match_event(cx, event);
        if let Some(state) = self.state.as_mut() {
            let mut scope = Scope::with_data(state);
            self.ui.handle_event(cx, event, &mut scope);
        } else {
            self.ui.handle_event(cx, event, &mut Scope::empty());
        }
        // The WM's own text fields copied or cut: the Wayland clients paste it too (a hosted
        // app's copy arrives as SetClipboard; its tile leaves the response empty).
        if let Event::TextCopy(e) | Event::TextCut(e) = event {
            if let Some(text) = e.response.borrow().clone().filter(|t| !t.is_empty()) {
                wayland_host::send(wayland_host::ViewCmd::Selection { primary: false, text });
            }
        }
        self.sync_phone_keyboard(cx);
        #[cfg(not(target_arch = "wasm32"))]
        self.flush_domain_messages(cx);
        // The gap cursor, LAST: a tile hover-out inside `ui.handle_event`
        // resets the cursor to Default, and the frame's final `set_cursor`
        // is the one the platform applies.
        if let Event::MouseMove(e) = event {
            self.update_divider_cursor(cx, e.abs);
            // Over the VR output the flat cursor would show in one eye
            // only: VR mode draws its own marker instead.
            if let Some(state) = self.state.as_mut() {
                let abs = physical_pointer.unwrap_or(e.abs);
                if state.vr.hides_cursor(abs.x, abs.y) {
                    state.vr.cursor_hidden = true;
                    cx.set_cursor(MouseCursor::Hidden);
                } else if std::mem::take(&mut state.vr.cursor_hidden) {
                    // Leaving the output (or VR mode): the cursor is back.
                    cx.set_cursor(MouseCursor::Default);
                }
            }
        }
    }
}
