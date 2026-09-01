//! makepad-wm (mpwm): an Omarchy-behaving tiling window manager as a
//! Makepad app. Nested mode: this window is the desktop; tiles host other
//! Makepad apps as child processes over the studio runview protocol.
//!
//! Chrome is styled entirely in splash: the theme is a `theme.splash` file
//! (imported from omarchy themes, see theme.rs) evaluated into the VM
//! before the app module, so the DSL below reads `mod.mpwm_theme.*`.

#![allow(dead_code)] // shell surface (icons, OSD, panels) built ahead of the flows that use it
pub use makepad_widgets;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;

mod binds;
mod clients;
mod demo_home;
mod desk;
mod hub;
mod layout;
mod preview;
mod run_view;
mod shell;
mod theme;

use std::collections::HashMap;

use binds::{match_bind_armed, WmAction};
use desk::{WmDesk, WmDeskAction, WmState};
use clients::{spawn_client, ClientLine, LaunchPolicy, WarmPool, WarmStatus};
use makepad_widgets::makepad_platform::shared_framebuf::{
    shared_swapchain_from_host_swapchain, HostSwapchain,
};
use hub::{send_to_app, ClientId, HubEvent, WmHub};
use layout::{Axis, Dir, DividerHit, FullscreenMode, LRect};
use makepad_studio_protocol::{AppToStudio, StudioToApp};
use mp_wm_api::{WmEvent, WmRequest};
use preview::PreviewCache;
use run_view::MpRunViewAction;
use shell::bar::{BarData, BarModule, ShellBarAction};
use shell::menu::{MenuSkin, ShellMenu, ShellMenuAction};
use shell::panels::ShellPanelAction;

app_main!(App);

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
            color: mod.mpwm_theme.foreground
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
                    flow: Overlay
                    // The wallpaper layer: the theme's image (crop-to-fill)
                    // over the theme's deep background.
                    bg_fill := RectView{
                        width: Fill
                        height: Fill
                        draw_bg +: {
                            color_top: uniform(mod.mpwm_theme.darker_background)
                            color_bottom: uniform(mod.mpwm_theme.background)
                            pixel: fn() {
                                return mix(self.color_top, self.color_bottom, self.pos.y)
                            }
                        }
                    }
                    bg_image := Image{
                        width: Fill
                        height: Fill
                        fit: ImageFit.CropToFill
                        visible: false
                    }
                    main_column := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        // THE OMARCHY BAR, inside the caption strip and
                        // OPAQUE over the wallpaper like omarchy's shell
                        // (background-alpha = 1.0). Left: workspaces (room
                        // for the mac traffic lights first); center: the
                        // date; right: the tray.
                        // THE OMARCHY BAR — shell/bar.rs draws every
                        // module of `config/omarchy/shell.json` itself
                        // (menu, workspaces, active window, clock,
                        // keyboard layout, indicators, bluetooth, network,
                        // audio, monitor, power) with the tooltips, the
                        // open-panel pill and the press/wheel gestures.
                        // The wrapper keeps `ids!(bar)` for the caption
                        // strip: its height tracks the OS window buttons
                        // and the drag query answers Caption over it.
                        bar := SolidView{
                            width: Fill
                            height: 26
                            flow: Overlay
                            draw_bg +: {
                                color: mod.mpwm_theme.background
                            }
                            shell_bar := ShellBar{
                                width: Fill
                                height: Fill
                            }
                        }
                        desk := WmDesk{
                            Tile := MpRunView{}
                        }
                    }
                    // The shell's floating surfaces, over the desk: the
                    // menu/launcher, the bar flyouts, the OSD and the
                    // notification stack. Each draws only when it is up.
                    shell_overlay := View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        shell_panel := ShellPanel{}
                        shell_menu := ShellMenu{}
                        shell_notes := ShellNotifications{}
                        shell_osd := ShellOsd{}
                    }
                    // `mpwm --gallery`: every ported omarchy surface with
                    // fixture data, over the desktop (see shell/gallery.rs).
                    gallery_holder := View{
                        width: Fill
                        height: Fill
                        visible: false
                        shell_gallery := ShellGallery{}
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

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: Option<WmState>,
    /// A keyboard focus that could not land yet (the tile hadn't drawn);
    /// re-asserted when that client's first frame arrives.
    #[rust]
    pending_focus: Option<ClientId>,
    /// `--gallery`: the shell-surface gallery instead of a desktop.
    #[rust]
    gallery: bool,
    /// What the shell bar's status modules show, sampled on the tick.
    #[rust]
    bar_sample: BarData,
    /// The background sampler's cache (fork+exec off the main thread —
    /// see shell::bar::start_status_sampler).
    #[rust]
    status_cache: Option<std::sync::Arc<std::sync::Mutex<shell::bar::SampledStatus>>>,
    #[rust]
    status_tick: u32,
    /// The clock's alternate format (right-click cycles it, `formatAlt`).
    #[rust]
    clock_alt: bool,
    /// Which bar module's flyout is open, for the accent pill.
    #[rust]
    shell_panel_open: Option<BarModule>,
    #[rust]
    hub: Option<WmHub>,
    #[rust]
    next_id: ClientId,
    #[rust]
    tick: Timer,
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
    /// The bar was hidden because a window went fullscreen, not by
    /// SUPER+SHIFT+SPACE — so it comes back on its own.
    #[rust]
    bar_hidden_by_fullscreen: bool,
    /// Output lines from every child, for the tile's status line.
    #[rust]
    client_lines: Option<ClientLines>,
    /// The workspaces the bar cluster is currently showing, left to right —
    /// what a click on it maps to.
    #[rust]
    bar_workspaces: Vec<usize>,
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
    /// When each warm client was last ticked. Kept apart from `WarmFrame`
    /// because the FIRST ticks are what make a frame possible at all — see
    /// `pump_warm`.
    #[rust]
    warm_last_tick: HashMap<ClientId, std::time::Instant>,
    /// The desktop window's dpi factor, as the platform last reported it —
    /// a warm instance is configured with the same one its tile will use,
    /// so the glyph atlas and shaders it warms up are the right ones.
    #[rust]
    dpi_factor: f64,
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
    retired: Option<std::time::Instant>,
}

/// How long a retired warm swapchain is kept after adoption.
const WARM_FRAME_RETIRE: std::time::Duration = std::time::Duration::from_secs(2);

/// The pump interval: fast enough that a warm instance reaches its first
/// frame promptly, slow enough to be nothing on the WM's own clock.
const WARM_PUMP: f64 = 0.05;

/// What a DORMANT instance gets once it has drawn: enough to keep its
/// watchdog and any app-side timer alive, little enough to be free.
const WARM_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(1);

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
}

/// Hyprland's `binds:drag_threshold` (DragController.cpp:346).
const DRAG_THRESHOLD: f64 = 3.0;

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
}

impl App {
    fn desk(&self, cx: &mut Cx) -> WidgetRef {
        self.ui.widget(cx, ids!(desk))
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
        std::env::var("MPWM_THEME").unwrap_or_else(|_| {
            // Last chosen theme, omarchy-style state file.
            std::fs::read_to_string(theme::themes_dir().join("../current-theme"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| theme::DEFAULT_THEME.to_string())
        })
    }

    fn state_mut(&mut self) -> &mut WmState {
        self.state.as_mut().expect("state after startup")
    }

    fn desk_area(&self, cx: &mut Cx) -> LRect {
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
            (h - gap * 2.0).max(1.0),
        )
    }

    // --------------------------------------------------------------
    // Client lifecycle
    // --------------------------------------------------------------

    fn launch_app(&mut self, cx: &mut Cx, app_id: &str) {
        let Some(app) = crate::clients::find_app(app_id) else {
            log!("mpwm: no app '{}' in the registry", app_id);
            return;
        };
        let app = &app;
        let hub_port = self.state_mut().hub_port;

        // launch-or-focus for non-terminal apps: `omarchy-launch-or-focus`
        // matches `\b<pattern>\b` case-insensitively against the window's
        // CLASS OR TITLE and focuses the first hit.
        if app.policy == LaunchPolicy::OrFocus {
            let pattern = app.id.clone();
            let pattern = pattern.as_str();
            let mut existing: Vec<ClientId> = self
                .state_mut()
                .clients
                .iter()
                .filter(|(_, slot)| {
                    // A warm instance is NOT a window: matching one here
                    // would "focus" something invisible and the app would
                    // never open at all.
                    !slot.warm
                        && (clients::word_match(&slot.app, pattern)
                            || clients::word_match(&slot.title, pattern))
                })
                .map(|(id, _)| *id)
                .collect();
            // `head -n1` over hyprctl's client list: take the oldest.
            existing.sort_unstable();
            let existing = existing.first().copied();
            if let Some(client) = existing {
                self.focus_client(cx, client);
                return;
            }
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
        let state = self.state_mut();
        match spawn_client(
            app,
            id,
            hub_port,
            cwd.as_ref(),
            (app.id == "terminal").then_some(term_env.as_str()),
            &[],
            false,
            lines,
        ) {
            Ok(slot) => {
                log!("mpwm: launched {} as client {}", app.id, id);
                state.clients.insert(id, slot);
                let area = self.desk_area(cx);
                let state = self.state_mut();
                let gap = state.gap;
                state.layout.insert(id, area, gap);
                let hub_port = state.hub_port;
                self.desk(cx)
                    .borrow_mut::<WmDesk>()
                    .map(|mut d| d.with_run_view(cx, id, |cx, v| v.set_run_target(cx, id, 0, hub_port)));
                self.focus_client(cx, id);
                self.redraw_all(cx);
            }
            Err(err) => {
                log!("mpwm: launch {} failed: {}", app.id, err);
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
            if std::env::var("MPWM_FILES_REAL").is_err() {
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
                connected: slot.sender.is_some() && slot.ready,
            })
            .collect();
        let Some(client) = self.warm_pool.adopt(app_id, cwd_override, &status) else {
            if cwd_override {
                log!("mpwm: warm {} skipped — new terminal inherits a cwd", app_id);
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
                slot.open_at = Some(std::time::Instant::now());
                slot.opened_warm = true;
            }
            let gap = state.gap;
            state.layout.insert(client, area, gap);
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
        self.focus_client(cx, client);
        // Wake it: samplers, refresh timers, everything a dormant instance
        // was told to hold back (`mp_wm_api::warm_start`).
        self.send_wm_event(client, WmEvent::Adopted);
        // The tile owns the framebuffer now; the warm one is held briefly
        // in case a frame is still in flight against it.
        if let Some(frame) = self.warm_frames.get_mut(&client) {
            frame.retired = Some(std::time::Instant::now());
        }
        log!("mpwm: adopted warm {} as client {}", app_id, client);
        self.redraw_all(cx);
        // …and the next one starts warming immediately.
        self.spawn_warm(app_id);
        true
    }

    /// Put one dormant instance of `app_id` in the pool. Quiet by design:
    /// the pool is invisible infrastructure, and anything it cannot do
    /// simply means the next launch of that app is a cold one.
    fn spawn_warm(&mut self, app_id: &str) {
        if !self.warm_pool.wants(app_id, std::time::Instant::now()) {
            return;
        }
        let Some(app) = crate::clients::find_app(app_id) else {
            return;
        };
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
        match spawn_client(
            &app,
            id,
            hub_port,
            cwd.as_ref(),
            (app.id == "terminal").then_some(term_env.as_str()),
            &[],
            true,
            lines,
        ) {
            Ok(slot) => {
                self.state_mut().clients.insert(id, slot);
                self.warm_pool.note_spawned(app_id, id);
                log!("mpwm: warming {} as client {}", app_id, id);
            }
            Err(err) => log!("mpwm: warming {} failed: {}", app_id, err),
        }
    }

    /// Top the pool up, ONE spawn per tick: a cold desktop otherwise forks
    /// four cargo builds into the same target-dir lock at once, and they
    /// would only queue behind each other anyway.
    fn top_up_warm_pool(&mut self) {
        let Some(app) = self.warm_pool.next_missing(std::time::Instant::now()) else {
            return;
        };
        self.spawn_warm(&app);
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
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        let swapchain: Option<HostSwapchain> = {
            let _ = (window_id, &cx);
            None
        };
        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
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
            let msgs = vec![
                StudioToApp::WindowGeomChange {
                    window_id,
                    dpi_factor: dpi,
                    left: 0.0,
                    top: 0.0,
                    width: w,
                    height: h,
                },
                StudioToApp::Swapchain(shared),
            ];
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
    /// heartbeat after that. Together with `MPWM_WARM_START` (the app's
    /// own half of the deal: no samplers, no refresh while dormant) that
    /// is what keeps a cached task manager from burning a core in the
    /// background.
    fn pump_warm(&mut self, _cx: &mut Cx) {
        let now = std::time::Instant::now();
        // A retired framebuffer outlives adoption only long enough for any
        // frame still in flight against it.
        self.warm_frames.retain(|_, frame| {
            frame
                .retired
                .map(|t| now.saturating_duration_since(t) < WARM_FRAME_RETIRE)
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
                    Some(t) if !warming => now.saturating_duration_since(*t) >= WARM_HEARTBEAT,
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
            "mpwm: {} client {} first frame in {} ms ({})",
            app,
            client,
            at.elapsed().as_millis(),
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
        let mut latest: Vec<(ClientId, String)> = Vec::new();
        if let Some(lines) = self.client_lines.as_ref() {
            while let Ok(line) = lines.rx.try_recv() {
                match latest.iter_mut().find(|(c, _)| *c == line.client) {
                    Some(slot) => slot.1 = line.text,
                    None => latest.push((line.client, line.text)),
                }
            }
        }
        for (client, raw) in latest {
            // Cargo's real state, in the user's words — but NEVER raw
            // pathnames on screen (they leak the machine's layout into
            // recordings). Known states get a clean phrase; anything
            // path-shaped is summarized.
            let text = if raw.starts_with("Blocking waiting for file lock") {
                "waiting for another build\u{2026}".to_string()
            } else if raw.starts_with("Running ") || raw.starts_with("Finished ") {
                "launching\u{2026}".to_string()
            } else if let Some(rest) = raw.strip_prefix("   Compiling ") {
                // "Compiling foo v0.1.0 (/path/…)" → keep crate + version.
                let head = rest.split(" (").next().unwrap_or(rest).trim();
                format!("compiling {}\u{2026}", head)
            } else if let Some(rest) = raw.trim_start().strip_prefix("Compiling ") {
                let head = rest.split(" (").next().unwrap_or(rest).trim();
                format!("compiling {}\u{2026}", head)
            } else if raw.contains('/') {
                // The app's own chatter with a path in it: not on the desk.
                String::new()
            } else {
                raw.clone()
            };
            if text.is_empty() {
                continue;
            }
            let mut warm = false;
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                warm = slot.warm;
                slot.status = text.clone();
                // cargo's last word before the app takes over. macOS then
                // scans a freshly linked binary on its FIRST exec, which
                // can hold main() for tens of seconds — see the tick.
                // Only cargo's handover means the binary is fresh on disk
                // and about to be exec'd for the first time.
                slot.linked = raw.starts_with("Running ") || raw.starts_with("Finished ");
                if slot.linked {
                    slot.linked_at = Some(std::time::Instant::now());
                }
            }
            // A warm instance has no tile to put a status line on, and
            // asking the desk for one would BUILD it — a widget nothing
            // draws, with an 8ms child-tick timer behind it. The pool's
            // invariant is literal: no tile until adoption.
            if warm {
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
                log!("mpwm: notify from client {}: {} — {}", client, title, body);
            }
            WmRequest::Close => self.request_close(cx, client),
            WmRequest::SetFloating { floating } => {
                let area = self.desk_area(cx);
                let gap = self.state_mut().gap;
                let is_float = self.state_mut().layout.is_float(client);
                if is_float != *floating {
                    self.state_mut().layout.toggle_float(client, area, gap);
                    self.redraw_all(cx);
                }
            }
            WmRequest::SetFullscreen { fullscreen } => {
                self.focus_client(cx, client);
                let on = self.state_mut().layout.fullscreen_mode() != FullscreenMode::None;
                if on != *fullscreen {
                    self.do_action(cx, WmAction::Fullscreen(FullscreenMode::Fullscreen));
                }
            }
        }
    }

    /// A client asked us to open a file in its associated app as a normal
    /// tiled window (`WmRequest::Open`). `WmRequest::Preview` (Quick Look)
    /// goes through `handle_preview_request`'s warm-viewer cache instead.
    fn open_request(&mut self, cx: &mut Cx, req: preview::OpenRequest) {
        let hub_port = self.state_mut().hub_port;
        let id = self.next_id;
        self.next_id += 1;
        let lines = self.line_sender();
        let slot = match preview::spawn_for_request(&req, id, hub_port, lines) {
            Ok(slot) => slot,
            Err(err) => {
                log!("mpwm: open request failed: {}", err);
                return;
            }
        };
        log!("mpwm: opening {} as client {}", req.path.display(), id);
        self.state_mut().clients.insert(id, slot);
        self.sync_geometry(cx);
        let area = self.desk_area(cx);
        let state = self.state_mut();
        let gap = state.gap;
        state.layout.insert(id, area, gap);
        let hub_port = state.hub_port;
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.with_run_view(cx, id, |cx, v| v.set_run_target(cx, id, 0, hub_port))
        });
        self.focus_client(cx, id);
        self.redraw_all(cx);
    }

    /// Send a hosted client a `WmEvent` over its own studio socket — the
    /// WM->app half of the `mp_wm_api` protocol. False when the client is
    /// gone or has not connected yet, so a caller whose message MUST land
    /// (a Quick-Look retarget) can park it for `HubEvent::Connected`.
    fn send_wm_event(&mut self, client: ClientId, ev: WmEvent) -> bool {
        if let Some(slot) = self.state_mut().clients.get(&client) {
            if let Some(sender) = &slot.sender {
                crate::hub::send_to_app(sender, vec![StudioToApp::Custom(ev.to_json())]);
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
        log!("mpwm: preview retarget client {} -> {}", client, path);
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

        // Spawn fresh, exactly as an ordinary preview always has, except it
        // never takes focus and is remembered as this type's warm slot.
        let hub_port = self.state_mut().hub_port;
        let id = self.next_id;
        self.next_id += 1;
        let lines = self.line_sender();
        let slot = match preview::spawn_for_request(&open, id, hub_port, lines) {
            Ok(mut slot) => {
                slot.is_preview = true;
                slot.takes_focus = false;
                slot
            }
            Err(err) => {
                log!("mpwm: preview request failed: {}", err);
                if suppressed_hide {
                    self.send_wm_event(requester, WmEvent::PreviewHidden);
                }
                return;
            }
        };
        log!(
            "mpwm: previewing {} as client {} ({})",
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
        let ws = state.layout.active;
        let rect = state.layout.popup_rect(area, gap, 900.0, 700.0);
        state.layout.add_preview_float(client, rect, ws);
        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
            d.with_run_view(cx, client, |_cx, v| v.set_takes_key_focus(false))
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
        self.state_mut().layout.remove(active.client);
        log!(
            "mpwm: preview hide client {} ({}), viewer stays warm",
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
    /// because under the FOCUS RULE that is always the REQUESTER (mpfiles),
    /// never the preview, so this can no longer key off
    /// `layout.focused_client()` the way it used to.
    fn close_focused_preview(&mut self, cx: &mut Cx) -> bool {
        if self.preview_cache.active.is_none() {
            return false;
        }
        // Only the app that is dialing loses its Space/Escape to the panel.
        // Focus somewhere else (a terminal the user tabbed to while the
        // panel stayed up) means Space is a space again.
        let focus = self.state_mut().layout.focused_client();
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
        let Some(focus) = self.state_mut().layout.focused_client() else {
            return;
        };
        self.request_close(cx, focus);
    }

    fn request_close(&mut self, cx: &mut Cx, client: ClientId) {
        let mut polite = false;
        if let Some(slot) = self.state_mut().clients.get_mut(&client) {
            if slot.closing.is_some() {
                return;
            }
            slot.closing = Some(std::time::Instant::now());
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
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                if let Some(child) = slot.child.as_mut() {
                    clients::kill_child_group(child, clients::GROUP_KILL_GRACE);
                }
            }
        }
        // Out of the layout now: the tiles reflow and the desk plays the
        // popin-out with the frame the tile already has.
        self.state_mut().layout.remove(client);
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

    fn remove_client(&mut self, cx: &mut Cx, client: ClientId) {
        log!("mpwm: removing client {}", client);
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
                self.warm_pool.note_crash(&app, std::time::Instant::now());
                log!("mpwm: warm {} (client {}) died", app, client);
            }
            // The top-up runs on the tick — one spawn at a time, and it
            // will only happen at all if the crash budget allows it.
        }
        self.warm_frames.remove(&client);
        self.warm_last_tick.remove(&client);
        self.state_mut().layout.remove(client);
        self.state_mut().clients.remove(&client);
        // A dying warm viewer (or its requester) clears the cache's
        // reference to it — "if a viewer process dies clear its slot, so
        // the next Space respawns" instead of talking to a dead socket.
        self.preview_cache.forget_client(client);
        if let Some(mut desk) = self.desk(cx).borrow_mut::<WmDesk>() {
            desk.remove_client(client);
        }
        if let Some(focus) = self.state_mut().layout.focused_client() {
            self.focus_client(cx, focus);
        }
        self.redraw_all(cx);
    }

    fn focus_client(&mut self, cx: &mut Cx, client: ClientId) {
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
        if let Some(ws) = self.state_mut().layout.workspace_of(client) {
            let state = self.state_mut();
            if ws != state.layout.active {
                state.layout.switch_workspace(ws);
            }
            state.layout.workspaces[ws].focus = Some(client);
            state.layout.note_focus(client);
        }
        // The tile widget only exists once the desk has drawn it, and its
        // Area only after its first draw — a focus at launch time lands on
        // nothing. Keep it pending and re-assert when the child's first
        // frame arrives (the PresentableDraw path below).
        let focused = self
            .desk(cx)
            .borrow_mut::<WmDesk>()
            .and_then(|mut d| d.with_run_view(cx, client, |cx, v| v.focus_keyboard(cx)))
            .unwrap_or(false);
        self.pending_focus = if focused { None } else { Some(client) };
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// A freshly linked binary stalls on its first exec while macOS scans
    /// it (XprotectService/syspolicyd; the second exec is instant). Say so
    /// instead of leaving the tile silent.
    fn explain_first_exec_scan(&mut self, cx: &mut Cx) {
        const GRACE: std::time::Duration = std::time::Duration::from_secs(3);
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
                    && s.linked_at.map(|t| t.elapsed() > GRACE).unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        for client in waiting {
            let text = "macOS is verifying the new binary…".to_string();
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                if slot.status == text {
                    continue;
                }
                slot.status = text.clone();
                slot.linked = false;
            }
            self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                d.with_run_view(cx, client, |cx, v| v.set_status_line(cx, &text))
            });
        }
    }

    fn reap_exited(&mut self, cx: &mut Cx) {
        // A client that ignored the polite close gets the fallback.
        for slot in self.state_mut().clients.values_mut() {
            let Some(since) = slot.closing else { continue };
            if since.elapsed() < clients::CLOSE_GRACE {
                continue;
            }
            if let Some(child) = slot.child.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    clients::kill_child_group(child, clients::GROUP_KILL_GRACE);
                }
            }
        }
        let dead: Vec<ClientId> = self
            .state_mut()
            .clients
            .iter_mut()
            .filter_map(|(id, slot)| {
                let exited = slot
                    .child
                    .as_mut()
                    .map(|c| matches!(c.try_wait(), Ok(Some(_))))
                    .unwrap_or(false);
                exited.then_some(*id)
            })
            .collect();
        for id in dead {
            self.remove_client(cx, id);
        }
    }

    // --------------------------------------------------------------
    // Hub routing
    // --------------------------------------------------------------

    fn drain_hub(&mut self, cx: &mut Cx) {
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
                    // A Quick-Look retarget that arrived while this viewer
                    // was still starting: it has a socket now, so land it.
                    if let Some(path) = self.preview_cache.take_pending(client) {
                        self.send_wm_event(client, WmEvent::PreviewFile { path });
                    }
                }
                HubEvent::Disconnected { socket } => {
                    let client = self
                        .state_mut()
                        .clients
                        .iter()
                        .find(|(_, s)| s.socket == Some(socket))
                        .map(|(id, _)| *id);
                    if let Some(client) = client {
                        if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                            slot.sender = None;
                            slot.socket = None;
                        }
                    }
                }
                HubEvent::FromApp { client, msgs } => {
                    for msg in msgs {
                        self.on_app_msg(cx, client, msg);
                    }
                }
            }
        }
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
                if first {
                    // A warm instance has no tile to be ready IN: it gets
                    // a framebuffer of its own instead, and draws into it
                    // until someone adopts it.
                    if self.is_warm(client) {
                        self.warm_bootstrap(cx, client, window_id);
                    } else {
                        self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                            d.with_run_view(cx, client, |cx, v| v.app_ready(cx, client, window_id))
                        });
                    }
                }
            }
            AppToStudio::DrawCompleteAndFlip(pd) => {
                crate::run_view::trace_host(&format!("rx-flip c{}", client));
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
                            log!("mpwm: warm client {} drew its first frame", client);
                        }
                    }
                    return;
                }
                self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                    d.with_run_view(cx, client, |cx, v| v.set_presentable_draw(cx, pd))
                });
                self.note_first_frame(client);
                // A focus that couldn't land at launch (tile not yet
                // drawn) lands now that the client has a frame.
                if self.pending_focus == Some(client) {
                    self.focus_client(cx, client);
                }
            }
            AppToStudio::SetCursor(cursor) => {
                self.desk(cx).borrow_mut::<WmDesk>().map(|mut d| {
                    d.with_run_view(cx, client, |cx, v| v.set_remote_cursor(cx, cursor.into()))
                });
            }
            AppToStudio::SetClipboard(text) => {
                cx.copy_to_clipboard(&text);
            }
            AppToStudio::Custom(json) => {
                // The typed app<->WM vocabulary (libs/mp_wm_api).
                if let Some(req) = WmRequest::parse(&json) {
                    self.on_wm_request(cx, client, req);
                }
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
        // Children style themselves from the same file.
        std::env::set_var("MPWM_THEME_SPLASH", theme::theme_splash_path(name));
        let _ = std::fs::write(theme::themes_dir().join("../current-theme"), name);
        // Chrome DSL colors refresh fully on restart; borders, terminal
        // palette and backgrounds apply immediately.
        self.apply_background(cx, 0);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// SUPER+CTRL+SPACE — the theme's next wallpaper.
    fn next_background(&mut self, cx: &mut Cx) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let idx = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.apply_background(cx, idx);
    }

    fn apply_background(&mut self, cx: &mut Cx, index: usize) {
        let name = self.state_mut().theme_name.clone();
        let backgrounds = theme::theme_backgrounds(&name);
        if backgrounds.is_empty() {
            return;
        }
        let path = &backgrounds[index % backgrounds.len()];
        let image = self.ui.widget(cx, ids!(bg_image));
        let image_ref = self.ui.image(cx, ids!(bg_image));
        if image_ref.load_image_file_by_path_async(cx, path).is_ok() {
            image.set_visible(cx, true);
        }
        self.redraw_all(cx);
    }

    fn open_shell_menu(&mut self, cx: &mut Cx, path: &str, skin: MenuSkin) {
        let menu = self.ui.widget(cx, ids!(shell_menu));
        {
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                m.open_at(cx, path, skin);
            }
        }
        self.redraw_all(cx);
    }

    fn close_shell_menu(&mut self, cx: &mut Cx) {
        let menu = self.ui.widget(cx, ids!(shell_menu));
        {
            let mut borrowed = menu.borrow_mut::<ShellMenu>();
            if let Some(m) = borrowed.as_mut() {
                m.close(cx);
            }
        }
        if let Some(focus) = self.state_mut().layout.focused_client() {
            self.focus_client(cx, focus);
        }
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
    /// whatever tile sits behind the scrim. Scroll is left alone — the
    /// panel does nothing with it, so routing it here would only turn off
    /// SUPER+scroll workspace cycling for no reason.
    fn shell_panel_pointer(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if matches!(event, Event::Scroll(_)) {
            return false;
        }
        let panel = self.ui.widget(cx, ids!(shell_panel));
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
                Ok(msg) => log!("mpwm: {}", msg),
                Err(err) => log!("mpwm: import failed: {}", err),
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
            "style.background" => {
                self.close_shell_menu(cx);
                self.next_background(cx);
            }
            "style.bar" => {
                self.close_shell_menu(cx);
                self.do_action(cx, WmAction::ToggleBar);
            }
            other => {
                log!("mpwm: menu row '{}' does nothing in nested mode", other);
                self.close_shell_menu(cx);
            }
        }
    }

    /// True where the shell bar has a clickable module under the point:
    /// those points answer Client to the drag query so the press reaches
    /// the widget instead of moving the OS window.
    fn shell_bar_claims(&self, cx: &mut Cx, p: Vec2d) -> bool {
        let bar = self.ui.widget(cx, ids!(shell_bar));
        let borrowed = bar.borrow::<shell::bar::ShellBar>();
        borrowed
            .as_ref()
            .map(|b| b.module_at(p).is_some())
            .unwrap_or(false)
    }

    /// Toggle a bar module's flyout, anchored to the module itself.
    fn toggle_shell_panel(&mut self, cx: &mut Cx, module: BarModule) {
        let Some(kind) = shell::panels::PanelKind::for_module(module) else {
            return;
        };
        let anchor = {
            let bar = self.ui.widget(cx, ids!(shell_bar));
            let borrowed = bar.borrow::<shell::bar::ShellBar>();
            borrowed
                .as_ref()
                .and_then(|b| b.module_rect(module))
                .unwrap_or_default()
        };
        let panel = self.ui.widget(cx, ids!(shell_panel));
        {
            let mut borrowed = panel.borrow_mut::<shell::panels::ShellPanel>();
            if let Some(p) = borrowed.as_mut() {
                p.toggle(cx, kind, anchor);
                self.shell_panel_open = p.open.map(|k| k.module());
            }
        }
        self.update_bar(cx);
    }

    /// The in-process notification API — `WmRequest::Notify{title, body}`
    /// lands here.
    pub fn notify(&mut self, cx: &mut Cx, title: &str, body: &str) {
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

    /// Feed the shell bar. Workspaces and the active window come from the
    /// WM; the status modules come from `bar_sample`, refreshed on the tick
    /// (`shell/bar.rs` does the sampling — cheap things every second, the
    /// expensive ones every fifth).
    fn update_bar(&mut self, cx: &mut Cx) {
        let mut shown: Vec<usize> = Vec::new();
        let workspaces = {
            let state = self.state_mut();
            let active = state.layout.active;
            let mut cells: Vec<shell::bar::WorkspaceCell> = Vec::new();
            // Omarchy's bar: the active workspace is a dot, other POPULATED
            // ones show their number, empty ones are hidden.
            for i in 0..layout::WORKSPACES {
                let populated = !state.layout.clients_on(i).is_empty();
                if i != active && !populated {
                    continue;
                }
                shown.push(i);
                cells.push(shell::bar::WorkspaceCell {
                    label: format!("{}", (i + 1) % 10),
                    occupied: populated,
                    focused: i == active,
                });
            }
            cells
        };
        self.bar_workspaces = shown;
        let title = {
            let state = self.state_mut();
            state
                .layout
                .focused_client()
                .and_then(|c| state.clients.get(&c))
                .map(|s| s.display_title().to_string())
                .unwrap_or_default()
        };
        let mut data = self.bar_sample.clone();
        data.workspaces = workspaces;
        data.active_window = (!title.is_empty()).then_some(title);
        data.open_panel = self.shell_panel_open;
        let bar = self.ui.widget(cx, ids!(shell_bar));
        {
            let mut borrowed = bar.borrow_mut::<shell::bar::ShellBar>();
            if let Some(b) = borrowed.as_mut() {
                b.data = data;
            }
        }
        self.redraw_all(cx);
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
        // The bar's content starts after the OS window buttons.
        let shell_bar = self.ui.widget(cx, ids!(shell_bar));
        {
            let mut borrowed = shell_bar.borrow_mut::<shell::bar::ShellBar>();
            if let Some(b) = borrowed.as_mut() {
                b.pad_left = pad_left;
            }
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
        let cache = self
            .status_cache
            .get_or_insert_with(shell::bar::start_status_sampler);
        let s = cache.lock().map(|s| s.clone()).unwrap_or_default();
        self.bar_sample.volume = s.volume;
        self.bar_sample.muted = s.muted;
        self.bar_sample.clock = if self.clock_alt {
            s.clock_alt
        } else {
            s.clock
        };
        self.status_tick = self.status_tick.wrapping_add(1);
        self.bar_sample.battery = s.battery;
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
    }

    fn redraw_all(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
    }

    // --------------------------------------------------------------
    // WM actions
    // --------------------------------------------------------------

    /// Hand the layout the true desk rect so fullscreen and the scratchpad
    /// console can reach past the outer gap.
    fn sync_geometry(&mut self, cx: &mut Cx) {
        let rect = self
            .desk(cx)
            .borrow_mut::<WmDesk>()
            .map(|d| d.desk_rect)
            .unwrap_or_default();
        if rect.size.x > 1.0 {
            self.state_mut().layout.set_outer(LRect::new(
                rect.pos.x,
                rect.pos.y,
                rect.size.x,
                rect.size.y,
            ));
        }
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
        self.state_mut().layout.switch_workspace(0);
        self.redraw_all(cx);
    }

    /// The WM itself is going down: a hidden warm viewer has no tile and
    /// no window, so nothing else would ever reap it. Kill the cache's
    /// processes outright — `--stdin-loop` children do notice an orphaned
    /// parent eventually, but "eventually" is not a shutdown story.
    fn kill_warm_previews(&mut self) {
        for client in self.preview_cache.warm_clients() {
            if let Some(slot) = self.state_mut().clients.get_mut(&client) {
                if let Some(child) = slot.child.as_mut() {
                    log!("mpwm: shutdown kills warm preview client {}", client);
                    clients::kill_child_group(child, clients::GROUP_KILL_GRACE);
                }
            }
        }
        self.preview_cache.clear();
    }

    /// SUPER+F hides the bar with the window; anything that leaves
    /// fullscreen puts it back, unless SUPER+SHIFT+SPACE hid it.
    fn sync_bar_for_fullscreen(&mut self, cx: &mut Cx) {
        let fullscreen = {
            let layout = &self.state_mut().layout;
            let ws = layout.focus_ws();
            layout.workspaces[ws].fullscreen.is_some()
                && layout.workspaces[ws].fullscreen_mode == FullscreenMode::Fullscreen
        };
        let bar = self.ui.widget(cx, ids!(bar));
        if fullscreen && bar.visible() {
            bar.set_visible(cx, false);
            self.bar_hidden_by_fullscreen = true;
        } else if !fullscreen && self.bar_hidden_by_fullscreen {
            bar.set_visible(cx, true);
            self.bar_hidden_by_fullscreen = false;
        }
    }

    fn focus_after_layout(&mut self, cx: &mut Cx) {
        if let Some(focus) = self.state_mut().layout.focused_client() {
            self.focus_client(cx, focus);
        }
    }

    fn do_action(&mut self, cx: &mut Cx, action: WmAction) {
        self.sync_geometry(cx);
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        let focus = self.state_mut().layout.focused_client();
        match action {
            WmAction::LaunchTerminal => self.launch_app(cx, "terminal"),
            WmAction::LaunchBrowser => self.launch_app(cx, "browser"),
            WmAction::CloseWindow => self.close_focused(cx),
            WmAction::CloseAllWindows => self.close_all_windows(cx),
            WmAction::ToggleSplit => {
                self.state_mut().layout.toggle_split();
            }
            WmAction::TogglePseudo => {
                if let Some(focus) = focus {
                    self.state_mut().layout.toggle_pseudo(focus, area, gap);
                }
            }
            WmAction::ToggleFloat => {
                if let Some(focus) = focus {
                    self.state_mut().layout.toggle_float(focus, area, gap);
                    self.focus_client(cx, focus);
                }
            }
            WmAction::PopOut => {
                if let Some(focus) = focus {
                    self.state_mut().layout.pop_out(focus, area, gap);
                    self.focus_client(cx, focus);
                }
            }
            WmAction::Fullscreen(mode) => {
                self.state_mut().layout.toggle_fullscreen_mode(mode);
                self.sync_bar_for_fullscreen(cx);
            }
            WmAction::TiledFullscreen => {
                if let Some(focus) = focus {
                    let on = self.state_mut().layout.toggle_client_fullscreen(focus);
                    // fullscreenstate 0 2: the client is told, the layout is
                    // untouched. Our children learn it as a custom message.
                    if let Some(slot) = self.state_mut().clients.get(&focus) {
                        if let Some(sender) = &slot.sender {
                            send_to_app(
                                sender,
                                vec![StudioToApp::Custom(format!(
                                    "{{\"mpwm_fullscreen\":{}}}",
                                    on
                                ))],
                            );
                        }
                    }
                }
            }
            WmAction::FocusDir(dir) => {
                if self.state_mut().layout.focus_dir(dir, area, gap) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::SwapDir(dir) => {
                self.state_mut().layout.swap_dir(dir, area, gap);
            }
            WmAction::ResizePx { axis, px } => {
                self.state_mut().layout.resize_px(axis, px, area, gap);
            }
            WmAction::CycleFocus(forward) => {
                self.state_mut().layout.cycle_focus(forward);
                self.focus_after_layout(cx);
            }
            WmAction::ToggleGroup => {
                self.state_mut().layout.toggle_group(area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::MoveOutOfGroup => {
                self.state_mut().layout.move_out_of_group(area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::MoveIntoGroup(dir) => {
                self.state_mut().layout.move_into_group(dir, area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::GroupNext => {
                if self.state_mut().layout.group_cycle(true) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::GroupPrev => {
                if self.state_mut().layout.group_cycle(false) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::GroupActive(n) => {
                if self.state_mut().layout.group_set_active(n) {
                    self.focus_after_layout(cx);
                }
            }
            WmAction::Workspace(n) => {
                self.state_mut().layout.switch_workspace(n);
                self.focus_after_layout(cx);
                self.sync_bar_for_fullscreen(cx);
            }
            WmAction::MoveToWorkspace(n) => {
                self.state_mut().layout.move_focused_to_ex(n, true, area, gap);
                self.focus_after_layout(cx);
            }
            WmAction::MoveToWorkspaceSilent(n) => {
                self.state_mut()
                    .layout
                    .move_focused_to_ex(n, false, area, gap);
            }
            WmAction::WorkspaceNext => {
                // Hyprland `e+1`: cycle occupied workspaces, not a march
                // through the empty ones.
                let layout = &self.state_mut().layout;
                let n = layout.cycle_occupied(layout.active, true);
                self.state_mut().layout.switch_workspace(n);
                self.focus_after_layout(cx);
            }
            WmAction::WorkspacePrev => {
                let layout = &self.state_mut().layout;
                let n = layout.cycle_occupied(layout.active, false);
                self.state_mut().layout.switch_workspace(n);
                self.focus_after_layout(cx);
            }
            WmAction::WorkspaceFormer => {
                let former = self.state_mut().layout.former;
                self.state_mut().layout.switch_workspace(former);
                self.focus_after_layout(cx);
            }
            WmAction::ToggleScratchpad => {
                self.state_mut().layout.toggle_scratchpad();
                self.focus_after_layout(cx);
            }
            WmAction::MoveToScratchpad => {
                self.state_mut()
                    .layout
                    .move_focused_to_scratchpad(area, gap);
            }
            WmAction::ToggleWorkspaceLayout => {
                let mode = self.state_mut().layout.toggle_workspace_layout();
                // The scrolling algorithm is a follow-on lane; the flag is
                // live and the layout still draws dwindle.
                log!("mpwm: workspace layout -> {:?}", mode);
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
            WmAction::ToggleBar => {
                let bar = self.ui.widget(cx, ids!(bar));
                let visible = bar.visible();
                bar.set_visible(cx, !visible);
                self.bar_hidden_by_fullscreen = false;
            }
            WmAction::ArmAltLayer => {
                self.alt_armed = true;
                log!("mpwm: SUPER+ALT layer armed for the next key");
                return;
            }
        }
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    // --------------------------------------------------------------
    // SUPER + mouse (tiling.lua mouse:272 / mouse:273)
    // --------------------------------------------------------------

    fn begin_drag(&mut self, cx: &mut Cx, abs: Vec2d, resize: bool) -> bool {
        self.sync_geometry(cx);
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        let Some(client) = self
            .state_mut()
            .layout
            .client_at(abs.x, abs.y, area, gap)
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
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        let layout = &mut self.state_mut().layout;
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
            floating,
            start: abs,
            last: abs,
            start_rect: rect,
            grab_left: abs.x < cx_center,
            grab_top: abs.y < cy_center,
            armed,
            shift: false,
        });
        if floating {
            // Hyprland moves a drag 1:1 with the pointer, not on the
            // tile's 379ms layout tween (`WmDesk::draw_walk`'s sync loop
            // reads this to skip `retarget` for this one client).
            self.state_mut().dragging = vec![client];
        }
        self.focus_client(cx, client);
        true
    }

    fn drag_move(&mut self, cx: &mut Cx, abs: Vec2d, shift: bool) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        drag.shift = shift;
        // A press only becomes a drag once it clears the threshold.
        if !drag.armed {
            if (abs.x - drag.start.x).abs() < DRAG_THRESHOLD
                && (abs.y - drag.start.y).abs() < DRAG_THRESHOLD
            {
                return;
            }
            drag.armed = true;
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
        drag.last = abs;
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        if floating {
            let d = abs - start;
            let rect = if resize {
                // The grabbed corner moves with the pointer; the opposite
                // one is fixed (DragController.cpp:413-423).
                let (x, w) = if grab_left {
                    let w = (start_rect.w - d.x).max(80.0);
                    (start_rect.x + start_rect.w - w, w)
                } else {
                    (start_rect.x, (start_rect.w + d.x).max(80.0))
                };
                let (y, h) = if grab_top {
                    let h = (start_rect.h - d.y).max(60.0);
                    (start_rect.y + start_rect.h - h, h)
                } else {
                    (start_rect.y, (start_rect.h + d.y).max(60.0))
                };
                LRect::new(x, y, w, h)
            } else {
                LRect::new(start_rect.x + d.x, start_rect.y + d.y, start_rect.w, start_rect.h)
            };
            self.state_mut().layout.set_float_rect(client, rect);
        } else if resize {
            // Tiled: the divider follows the pointer, one frame at a time,
            // exactly like CDwindleAlgorithm::resizeTarget's Δ.
            let d = abs - last;
            // A tiled resize moves the split ratios: the grabbed edge
            // follows the pointer, so a left/top grab inverts the delta.
            let layout = &mut self.state_mut().layout;
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
            let area = self.desk_area(cx);
            let gap = self.state_mut().gap;
            self.state_mut()
                .layout
                .client_at(abs.x, abs.y, area, gap)
                .filter(|target| *target != client)
        } else {
            None
        };
        self.state_mut().drop_hint = hint;
        self.redraw_all(cx);
    }

    fn end_drag(&mut self, cx: &mut Cx, abs: Vec2d, shift: bool) {
        self.state_mut().dragging.clear();
        self.state_mut().drop_hint = None;
        let Some(drag) = self.drag.take() else {
            return;
        };
        // The button-up may not carry SHIFT any more (it is released
        // together with the mouse often enough); the drag's own last known
        // state stands in for it.
        let shift = shift || drag.shift;
        if !drag.floating && !drag.resize && drag.armed {
            // Tiled move: dropping on another tile swaps the two, which is
            // what Hyprland's `movewindow` drag settles into.
            let area = self.desk_area(cx);
            let gap = self.state_mut().gap;
            if let Some(target) = self
                .state_mut()
                .layout
                .client_at(abs.x, abs.y, area, gap)
            {
                if target != drag.client {
                    let layout = &mut self.state_mut().layout;
                    if shift {
                        layout.group_drop(drag.client, target);
                    } else {
                        layout.swap_clients(drag.client, target);
                    }
                    self.focus_client(cx, drag.client);
                }
            }
        }
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
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        // A window under the pointer keeps its own press. `client_at`
        // covers floats and the scratchpad too, so anything drawn OVER a
        // gap shadows the divider the way it shadows the wallpaper.
        if self
            .state_mut()
            .layout
            .client_at(abs.x, abs.y, area, gap)
            .is_some()
        {
            return false;
        }
        let Some(hit) = self.state_mut().layout.divider_at(abs.x, abs.y, area, gap) else {
            return false;
        };
        // Both sides of the split resize live: name every client under it
        // so `WmDesk` snaps their tiles instead of restarting the 379ms
        // layout tween on each pointer frame (see `WmState::dragging`).
        let clients = self.state_mut().layout.clients_under(&hit);
        log!(
            "mpwm: divider grab {:?} depth {} ratio {:.3} over {} tiles",
            hit.axis,
            hit.depth,
            hit.ratio,
            clients.len()
        );
        self.state_mut().dragging = clients;
        self.div_drag = Some(DividerDrag { hit, start: abs });
        self.apply_divider_cursor(cx, Some(hit.axis));
        true
    }

    fn divider_drag_move(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(drag) = self.div_drag.as_ref() else {
            return;
        };
        let (hit, start) = (drag.hit, drag.start);
        let gap = self.state_mut().gap;
        let px = match hit.axis {
            Axis::Horizontal => abs.x - start.x,
            Axis::Vertical => abs.y - start.y,
        };
        // 1:1 with the pointer, always measured from the grab: no tween,
        // no accumulated delta, and a clamp at either end never eats part
        // of the way back.
        self.state_mut().layout.drag_divider_px(&hit, px, gap);
        self.apply_divider_cursor(cx, Some(hit.axis));
        self.redraw_all(cx);
    }

    fn end_divider_drag(&mut self, cx: &mut Cx) {
        self.state_mut().dragging.clear();
        if self.div_drag.take().is_none() {
            return;
        }
        log!("mpwm: divider drop");
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
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        let on_client = self
            .state_mut()
            .layout
            .client_at(abs.x, abs.y, area, gap)
            .is_some();
        let axis = if on_client {
            None
        } else {
            self.state_mut()
                .layout
                .divider_at(abs.x, abs.y, area, gap)
                .map(|hit| hit.axis)
        };
        let was = self.div_hover;
        if axis != was {
            self.div_hover = axis;
            match axis {
                Some(a) => log!("mpwm: divider hover {:?}", a),
                None => log!("mpwm: divider hover none"),
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
        let area = self.desk_area(cx);
        let gap = self.state_mut().gap;
        if !self.state_mut().layout.group_tear_out(client, area, gap) {
            return;
        }
        self.begin_drag_on(cx, client, abs, false, true);
        self.focus_client(cx, client);
        self.update_bar(cx);
        self.redraw_all(cx);
    }

    /// SUPER + wheel: `focus({ workspace = "e+1" / "e-1" })`.
    fn scroll_workspace(&mut self, cx: &mut Cx, down: bool) {
        let layout = &self.state_mut().layout;
        let n = layout.cycle_occupied(layout.active, down);
        self.state_mut().layout.switch_workspace(n);
        self.focus_after_layout(cx);
        self.sync_bar_for_fullscreen(cx);
        self.update_bar(cx);
        self.redraw_all(cx);
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
                    match test_action(name) {
                        Some(action) => {
                            log!("mpwm: --test-action {} -> {:?}", name, action);
                            self.do_action(cx, action);
                        }
                        None => log!("mpwm: unknown --test-action '{}'", name),
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
        "fullscreen" => Some(WmAction::Fullscreen(FullscreenMode::Fullscreen)),
        "maximize" => Some(WmAction::Fullscreen(FullscreenMode::Maximized)),
        "tiled-fullscreen" => Some(WmAction::TiledFullscreen),
        "focus-left" => Some(WmAction::FocusDir(Dir::Left)),
        "focus-right" => Some(WmAction::FocusDir(Dir::Right)),
        "focus-up" => Some(WmAction::FocusDir(Dir::Up)),
        "focus-down" => Some(WmAction::FocusDir(Dir::Down)),
        "swap-left" => Some(WmAction::SwapDir(Dir::Left)),
        "swap-right" => Some(WmAction::SwapDir(Dir::Right)),
        "swap-up" => Some(WmAction::SwapDir(Dir::Up)),
        "swap-down" => Some(WmAction::SwapDir(Dir::Down)),
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
        "workspace-next" => Some(WmAction::WorkspaceNext),
        "workspace-prev" => Some(WmAction::WorkspacePrev),
        "workspace-former" => Some(WmAction::WorkspaceFormer),
        "cycle" => Some(WmAction::CycleFocus(true)),
        "cycle-back" => Some(WmAction::CycleFocus(false)),
        "menu" => Some(WmAction::Menu),
        "apps" => Some(WmAction::AppsMenu),
        "system" => Some(WmAction::SystemMenu),
        "keys" => Some(WmAction::Keybindings),
        "theme" => Some(WmAction::ThemeMenu),
        "background" => Some(WmAction::BackgroundNext),
        "bar" => Some(WmAction::ToggleBar),
        "layout" => Some(WmAction::ToggleWorkspaceLayout),
        _ => None,
    };
    if simple.is_some() {
        return simple;
    }
    if let Some(n) = name.strip_prefix("workspace-") {
        return n.parse::<usize>().ok().map(|n| WmAction::Workspace(n - 1));
    }
    if let Some(n) = name.strip_prefix("move-to-") {
        return n
            .parse::<usize>()
            .ok()
            .map(|n| WmAction::MoveToWorkspace(n - 1));
    }
    if let Some(n) = name.strip_prefix("group-") {
        return n.parse::<usize>().ok().map(WmAction::GroupActive);
    }
    None
}

/// The SUPER chord for mouse binds — Ctrl+Alt nested, the Logo key on a
/// Linux session (binds.rs carries the same law for keys).
fn super_chord(m: &KeyModifiers) -> bool {
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
        // CLI: --import-theme <name> pulls an omarchy theme and converts
        // it to splash before the desktop appears.
        let mut args = std::env::args();
        while let Some(arg) = args.next() {
            if arg == "--import-theme" {
                if let Some(name) = args.next() {
                    match theme::import_omarchy_theme(&name) {
                        Ok(msg) => log!("mpwm: {}", msg),
                        Err(err) => log!("mpwm: import failed: {}", err),
                    }
                }
            }
        }

        let theme_name = Self::theme_name_from_env();
        // Children inherit the theme file path so every mp* app styles
        // itself from the same theme.splash.
        std::env::set_var(
            "MPWM_THEME_SPLASH",
            theme::theme_splash_path(&theme_name),
        );
        let source = theme::load_theme_source(&theme_name);
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

        let hub = WmHub::start();
        let hub_port = hub.as_ref().map(|h| h.port).unwrap_or(0);
        if hub.is_none() {
            log!("mpwm: could not bind the client hub; tiles will not start");
        }
        self.hub = hub;

        self.state = Some(WmState {
            layout: crate::layout::WmLayout::new(),
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
            drop_hint: None,
        });
        self.next_id = 1;
        self.tick = cx.start_interval(1.0);
        // The warm pool's pump. Started even when the pool is off: it
        // costs one no-op wakeup and keeps the timer id stable.
        self.warm_tick = cx.start_interval(WARM_PUMP);
        if !self.warm_pool.enabled() {
            log!("mpwm: warm pool disabled (MPWM_NO_WARM)");
        }

        // `--gallery`: the shell surfaces with fixture data instead of a
        // desktop, so the port is verifiable over --remote (shell/gallery.rs).
        if shell::gallery::ShellGallery::requested() {
            self.gallery = true;
            self.ui.widget(cx, ids!(gallery_holder)).set_visible(cx, true);
            self.ui.widget(cx, ids!(main_column)).set_visible(cx, false);
            self.redraw_all(cx);
            return;
        }

        // Start EMPTY like omarchy — booting children is slow (first-exec
        // scan) and the desk is usable instantly. MPWM_TEST_APP=app[:count]
        // boots a test scene ("terminal:3" = the old A | (B / C)).
        if let Ok(spec) = std::env::var("MPWM_TEST_APP") {
            let (app, count) = match spec.split_once(':') {
                Some((app, n)) => (app.to_string(), n.parse::<usize>().unwrap_or(1).min(9)),
                None => (spec, 1),
            };
            for _ in 0..count {
                self.launch_app(cx, &app);
            }
        }

        self.apply_background(cx, 0);
        self.update_bar(cx);
        self.update_status(cx);
        // The platform installs a default menu whose Quit is ⌘Q — which
        // would take the whole desktop down when the user means "close
        // this window" (omarchy's SUPER+Q). Replace it: Quit keeps a key
        // equivalent no window binding uses.
        #[cfg(target_os = "macos")]
        cx.update_macos_menu(MacosMenu::Main {
            items: vec![MacosMenu::Sub {
                name: "mpwm".to_string(),
                items: vec![MacosMenu::Item {
                    command: live_id!(quit),
                    key: KeyCode::KeyQ,
                    shift: true,
                    enabled: true,
                    name: "Quit mpwm".to_string(),
                }],
            }],
        });

        // Scripted verification: --test-action fires WM actions with no
        // keyboard involved (some chords belong to the host OS).
        self.run_test_actions(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
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
                ShellBarAction::Press(module) => match module {
                    BarModule::Menu => self.do_action(cx, WmAction::Menu),
                    BarModule::Workspace(i) => {
                        let ws = self.bar_workspaces.get(i).copied().unwrap_or(i);
                        self.do_action(cx, WmAction::Workspace(ws));
                    }
                    BarModule::ActiveWindow => {
                        if let Some(focus) = self.state_mut().layout.focused_client() {
                            self.focus_client(cx, focus);
                        }
                    }
                    other => self.toggle_shell_panel(cx, other),
                },
                ShellBarAction::RightPress(module) => match module {
                    // The Omarchy button's right click opens a terminal.
                    BarModule::Menu => self.do_action(cx, WmAction::LaunchTerminal),
                    BarModule::ActiveWindow => {
                        if let Some(focus) = self.state_mut().layout.focused_client() {
                            self.request_close(cx, focus);
                        }
                    }
                    BarModule::Clock => {
                        self.clock_alt = !self.clock_alt;
                        self.update_status(cx);
                        self.update_bar(cx);
                    }
                    BarModule::Audio => {
                        self.bar_sample.muted = !self.bar_sample.muted;
                        #[cfg(target_os = "macos")]
                        {
                            let _ = std::process::Command::new("osascript")
                                .args([
                                    "-e",
                                    &format!(
                                        "set volume output muted {}",
                                        self.bar_sample.muted
                                    ),
                                ])
                                .output();
                        }
                        self.update_bar(cx);
                    }
                    _ => {}
                },
                ShellBarAction::MiddlePress(module) => {
                    // `ActiveWindow.qml`: middle click closes the focused
                    // window exactly like a right click — every other
                    // module ignores the middle button.
                    if module == BarModule::ActiveWindow {
                        if let Some(focus) = self.state_mut().layout.focused_client() {
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
            match wa.cast::<ShellPanelAction>() {
                ShellPanelAction::SetVolume(v) => self.set_volume(cx, v),
                ShellPanelAction::ToggleMute => {
                    self.bar_sample.muted = !self.bar_sample.muted;
                    self.update_bar(cx);
                }
                ShellPanelAction::Close => {
                    self.shell_panel_open = None;
                    self.update_bar(cx);
                }
                _ => {}
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
                }
                MpRunViewAction::Clicked { client } => {
                    self.focus_client(cx, client);
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
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);

        // The theme: evaluated before any module that reads
        // mod.mpwm_theme. This IS the theming system — splash.
        theme::ensure_default_theme();
        let theme_name = App::theme_name_from_env();
        let source = theme::load_theme_source(&theme_name);
        let eval_theme = |vm: &mut ScriptVm, name: &str, code: &str| -> bool {
            let script_mod_id = ScriptMod {
                cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
                module_path: name.to_string(),
                file: "theme.splash".to_string(),
                line: 0,
                column: 0,
                code: code.to_string(),
                values: vec![],
            };
            let value = vm.eval(script_mod_id);
            let errors = vm.take_errors();
            for e in &errors {
                log!("mpwm theme: {}", e);
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
        if !eval_theme(vm, "mpwm_theme", &source) {
            // Fall back to the bundled default so the DSL still evaluates.
            let mut fallback = theme::BUNDLED_TOKYO_NIGHT_SPLASH.to_string();
            fallback.push_str("\ntrue\n");
            eval_theme(vm, "mpwm_theme_fallback", &fallback);
        }

        // The shell token object (`mod.mpwm_theme.shell`): the omarchy
        // `shell.toml.tpl` contract, resolved from this theme's palette —
        // unless the theme ships its own `shell: {...}` block, which
        // replaces it wholesale.
        if !theme::theme_defines_shell(&source) {
            let mut block = theme::shell_splash_block(&source);
            block.push_str("\ntrue\n");
            eval_theme(vm, "mpwm_theme_shell", &block);
        }

        run_view::script_mod(vm);
        desk::script_mod(vm);
        shell::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::WindowGeomChange(ev) = event {
            // The bar is the caption: keep it around the OS buttons.
            self.update_bar_chrome(cx, &ev.new_geom);
            // A warm instance is configured with the desktop's own dpi, so
            // what it warms up is what its tile will use.
            self.dpi_factor = ev.new_geom.dpi_factor;
        }
        if let Event::WindowDragQuery(dq) = event {
            // Caption-less window: the bar strip is the drag handle; the
            // desk below answers Client so tile clicks never move the
            // window. macOS treats unanswered strip points as native drag.
            // The shell bar's own modules are BUTTONS, not a drag handle:
            // where it claims a point, the press reaches the widget.
            let bar = self.ui.view(cx, ids!(bar)).area();
            if self.shell_bar_claims(cx, dq.abs) {
                dq.response.set(WindowDragQueryResponse::Client);
            } else if bar.is_valid(cx) && bar.rect(cx).contains(dq.abs) {
                dq.response.set(WindowDragQueryResponse::Caption);
            } else {
                dq.response.set(WindowDragQueryResponse::Client);
            }
        }
        // The shell menu (and, for move/down/up, an open bar flyout) is
        // modal: while it is up, the pointer event is exclusively its own
        // — see `shell_menu_pointer` / `shell_panel_pointer`. Taken before
        // anything else, including SUPER+drag, so an open surface always
        // wins over the desk beneath it.
        if self.state.is_some()
            && matches!(
                event,
                Event::MouseMove(_) | Event::MouseDown(_) | Event::MouseUp(_) | Event::Scroll(_)
            )
            && (self.shell_menu_pointer(cx, event) || self.shell_panel_pointer(cx, event))
        {
            return;
        }
        // SUPER + mouse:272 / mouse:273 — move and resize (tiling.lua).
        // Taken before the tiles see it, so the drag never reaches a child.
        if self.state.is_some() {
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
                    self.end_drag(cx, e.abs, e.modifiers.shift);
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
                // SUPER + wheel over the desk cycles workspaces.
                Event::Scroll(e) if super_chord(&e.modifiers) => {
                    if e.scroll.y.abs() > 0.5 {
                        self.scroll_workspace(cx, e.scroll.y > 0.0);
                    }
                    return;
                }
                _ => {}
            }
        }
        // WM keybinds intercept before anything reaches the tiles.
        if let Event::KeyDown(e) = event {
            if self.state.is_some() {
                // The shell menu grabs the keyboard while it is up.
                if self.shell_menu_key(cx, e) {
                    self.alt_armed = false;
                    return;
                }
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
                // the key reaches the viewer.
                if matches!(e.key_code, KeyCode::Escape | KeyCode::Space)
                    && self.close_focused_preview(cx)
                {
                    return;
                }
            }
        }
        if let Event::Shutdown = event {
            if self.state.is_some() {
                self.kill_warm_previews();
            }
        }
        if let Event::Timer(te) = event {
            if self.tick.is_timer(te).is_some() && self.state.is_some() {
                self.reap_exited(cx);
                self.drain_client_lines(cx);
                self.explain_first_exec_scan(cx);
                self.update_status(cx);
                self.update_bar(cx);
                // The pool fills itself here: at startup, after an
                // adoption, and after any death it healed from. One spawn
                // per second, so a cold desktop never forks four cargo
                // builds at once.
                if !self.gallery {
                    self.top_up_warm_pool();
                }
            }
            if self.warm_tick.is_timer(te).is_some() && self.state.is_some() {
                self.pump_warm(cx);
            }
        }
        if let Event::Signal = event {
            if SignalToUI::check_and_clear_ui_signal() && self.state.is_some() {
                crate::run_view::trace_host("sig");
                self.drain_hub(cx);
                self.drain_client_lines(cx);
            }
        }

        self.match_event(cx, event);
        if let Some(state) = self.state.as_mut() {
            let mut scope = Scope::with_data(state);
            self.ui.handle_event(cx, event, &mut scope);
        } else {
            self.ui.handle_event(cx, event, &mut Scope::empty());
        }
        // The gap cursor, LAST: a tile hover-out inside `ui.handle_event`
        // resets the cursor to Default, and the frame's final `set_cursor`
        // is the one the platform applies.
        if let Event::MouseMove(e) = event {
            self.update_divider_cursor(cx, e.abs);
        }
    }
}
