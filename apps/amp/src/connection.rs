//! The Spotify audio connection: the tap and route that bring the music
//! system's own sound into this app (`audio.rs`), as a state of its own.
//!
//! * **Its own switch.** Connected, the player's sound runs through our
//!   chain whether the equalizer is on or off (off, the bands are bypassed
//!   sample for sample), so the visuals, meters and the relief
//!   light always have it. The equalizer's switch never opens or closes it.
//! * **On by default.** Whenever the music system is there, the route
//!   opens by itself in monitor mode (the player plays untouched; the tap
//!   only feeds the pictures and meters); it processes only while the
//!   equalizer is on (`Knobs::processing`). Where the system asks
//!   nothing (Windows, Linux: `permission()` is `None`) it connects at once
//!   and again whenever the player's stream comes back; on macOS it does
//!   so when capture was allowed before, and the first time shows the
//!   explainer once (whose Connect raises macOS's prompt), never a silent
//!   prompt. Refused, the sheet says where to allow it and opens that pane.
//!   Disconnect turns it off until the person connects again (`connect`
//!   and `explained` in `eq.txt`). The decision is [`auto_step`].
//! * Local files never need it: our own player feeds the pictures.

use crate::audio::{AudioCommand, AudioState};
use crate::model::Engine;
use crate::App;
use makepad_audio_route::Permission;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A faint one-line hint that opens the sheet, on a dark well (the
    // screen's glass, the equalizer display).
    mod.widgets.HwConnectHint = ButtonFlat{
        width: Fit
        height: Fit
        margin: 0
        padding: Inset{left: 8 right: 8 top: 3 bottom: 3}
        draw_bg +: {
            pixel: fn() {
                return vec4(0.0, 0.0, 0.0, 0.0)
            }
        }
        draw_text +: {
            color: #x868c95
            color_hover: #xc8ccd2
            color_down: #xc8ccd2
            color_focus: #x868c95
            text_style: theme.font_regular{font_size: 9.0}
        }
        text: ""
    }

    // The sheet's words: the display's ink (palette `screen_text` and
    // `screen_muted`, set in hw.rs), wrapped to the window.
    mod.widgets.HwConnText = Label{
        width: Fill
        padding: 0
        draw_text +: {
            color: #xd8dce2
            text_style: theme.font_regular{font_size: 10.0}
        }
    }
    mod.widgets.HwConnMuted = mod.widgets.HwConnText{
        draw_text +: {color: #x868c95}
    }

    // The sheet the status key opens. Calm: the same size and the same
    // action row in every state; only words and the primary key's light
    // change.
    mod.widgets.HwConnectionSheet = View{
        width: Fill
        height: Fill
        flow: Overlay
        align: Align{x: 0.5 y: 0.3}
        visible: false
        conn_plate := mod.widgets.HwPlate{
            draw_bg +: {plate_id: 3.0}
            width: 520
            height: Fit
            depth: 7.0
            inset: 10
            flow: Down
            spacing: 10
            // Room at the foot for a style's decorations (stripes, rails).
            padding: Inset{left: 24 right: 24 top: 18 bottom: 48}
            View{
                width: Fill
                height: Fit
                flow: Right
                align: Align{y: 0.5}
                mod.widgets.HwTitle{width: Fill text: "SPOTIFY AUDIO"}
                conn_close_cap := mod.widgets.HwKey{width: 44 height: 34 draw_bg +: {role: 28.0} conn_close := mod.widgets.HwKeyButton{text: "\u{f00d}"}}
            }
            // The words, on a display window: its ink reads in every style.
            conn_well := mod.widgets.HwDisplay{
                width: Fill
                height: Fit
                flow: Down
                spacing: 8
                padding: Inset{left: 16 right: 16 top: 14 bottom: 14}
                // The state in a few words, then where the sound goes (or
                // what is missing) on its own line.
                conn_status := mod.widgets.HwConnText{text: "Not connected" draw_text +: {text_style: theme.font_bold{font_size: 12.0}}}
                conn_detail := mod.widgets.HwConnMuted{height: 36 max_lines: 2 text_overflow: TextOverflow.Ellipsis text: " "}
                // What the state needs said, in a body of one fixed height:
                // the sheet never jumps between states.
                View{
                    width: Fill
                    height: 132
                    flow: Overlay
                    conn_explain := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 10
                        mod.widgets.HwConnText{text: "Amp listens to Spotify's own sound so it can show the visuals and meters and apply the equalizer. Nothing is recorded or sent anywhere: the sound stays on this computer."}
                        conn_ask := mod.widgets.HwConnText{text: "macOS will ask you to allow \"Screen & System Audio Recording\". It is the only permission that lets one app hear another app's sound. Amp does not capture your screen."}
                    }
                    conn_denied := View{
                        visible: false
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 8
                        mod.widgets.HwConnText{text: "1.  Open System Settings › Privacy & Security › Screen & System Audio Recording."}
                        mod.widgets.HwConnText{text: "2.  Turn on Makepad Amp (under System Audio Recording Only, if it is listed there)."}
                        mod.widgets.HwConnText{text: "3.  Come back and press Try Again. If it still hears nothing, quit and reopen Amp."}
                    }
                }
                mod.widgets.HwConnMuted{text: "Local files don't need this: Amp plays them itself."}
            }
            // The action row: the primary key never moves or resizes; the
            // secondary sits after it when a state has one.
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 10
                align: Align{y: 0.5}
                conn_primary_cap := mod.widgets.HwKey{width: 230 height: 36 draw_bg +: {has_led: 1.0 role: 22.0} conn_primary := mod.widgets.HwKeyText{text: "CONNECT"}}
                conn_secondary_cap := mod.widgets.HwKey{visible: false width: 150 height: 36 draw_bg +: {role: 22.0} conn_secondary := mod.widgets.HwKeyText{text: "TRY AGAIN"}}
            }
        }
    }
}

/// The pane of System Settings that allows it.
const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";
/// A key whose permission macOS reports as refused, amber.
const AMBER: Vec4f = Vec4f { x: 0.93, y: 0.64, z: 0.20, w: 1.0 };
/// Connected, green.
const GREEN: Vec4f = Vec4f { x: 0.24, y: 0.80, z: 0.43, w: 1.0 };

/// Where the connection stands, as the key and the sheet show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// No music system to connect (local files only).
    Local,
    Off,
    /// Asked for; the player has no sound to tap yet.
    Waiting,
    Connected,
    NeedsPermission,
}

impl Link {
    /// The sheet's headline: a few words.
    pub(crate) fn status(self) -> &'static str {
        match self {
            Link::Local => "",
            Link::Off => "Not connected",
            Link::Waiting => "Spotify is not running",
            Link::Connected => "Connected",
            Link::NeedsPermission => "Needs permission",
        }
    }

    /// The key's tooltip.
    fn tip(self) -> &'static str {
        match self {
            Link::Local => "",
            Link::Off => "Spotify audio: not connected (click to connect)",
            Link::Waiting => "Spotify audio: Spotify is not running (connects when it plays)",
            Link::Connected => "Spotify audio: connected",
            Link::NeedsPermission => "Spotify audio: needs permission",
        }
    }
}

/// The connection's UI state (a global: the App is shared with others).
#[derive(Default)]
struct Connection {
    /// macOS's answer, last asked (`None`: it does not say).
    permission: Option<Permission>,
    asked: bool,
    open: bool,
    /// What the key and hints last showed, and under which style.
    shown: Option<(Link, bool, u64)>,
    /// The pointer is over the key (its tooltip shows).
    hovering: bool,
    /// When the route last failed to open (app seconds), for the retry.
    failed_at: Option<f64>,
}

/// A route that failed to open is tried again after this long.
const RETRY_FAILED_SECS: f64 = 10.0;

/// What the connection does by itself, each poll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoStep {
    Nothing,
    /// Open the route (monitoring unless the equalizer is on).
    Open,
    /// macOS, first use: show the explainer once; its Connect asks.
    Explain,
}

/// The connection's own decision. `wanted`: not disconnected by the person;
/// `present`: the music system is there; `permission`: macOS's answer, `None`
/// where the system asks nothing; `failed_for`: seconds since the route
/// last failed to open, while it is failed.
pub(crate) fn auto_step(wanted: bool, present: bool, permission: Option<Permission>, explained: bool, audio: &AudioState, failed_for: Option<f64>) -> AutoStep {
    if !wanted || !present {
        return AutoStep::Nothing;
    }
    let allowed = matches!(permission, None | Some(Permission::Granted));
    match audio {
        // Open, or waiting for the player's stream (the worker retries).
        AudioState::Routed { .. } | AudioState::Waiting => AutoStep::Nothing,
        AudioState::Failed(_) if allowed && failed_for.is_some_and(|secs| secs >= RETRY_FAILED_SECS) => AutoStep::Open,
        AudioState::Failed(_) => AutoStep::Nothing,
        AudioState::Off if allowed => AutoStep::Open,
        AudioState::Off if permission == Some(Permission::Unknown) && !explained => AutoStep::Explain,
        AudioState::Off => AutoStep::Nothing,
    }
}

fn state(cx: &mut Cx) -> &mut Connection {
    if !cx.has_global::<Connection>() {
        cx.set_global(Connection::default());
    }
    cx.global::<Connection>()
}

/// Whether this system asks the person before one app hears another
/// (macOS); elsewhere the permission steps never show.
fn asks_permission() -> bool {
    cfg!(target_os = "macos")
}

fn ask_permission(cx: &mut Cx) -> Option<Permission> {
    // macOS without an answer: unknown (never a silent prompt).
    let permission = makepad_audio_route::permission().or(asks_permission().then_some(Permission::Unknown));
    let s = state(cx);
    s.permission = permission;
    s.asked = true;
    permission
}

impl App {
    /// Where the connection stands now.
    pub(crate) fn link(&self, cx: &mut Cx) -> Link {
        if !self.model.has_route {
            return Link::Local;
        }
        if !self.model.eq.connect {
            return Link::Off;
        }
        // Where the system asks nothing, there is no permission to need:
        // silence is Spotify's own, and a failure shows as not connected.
        if !asks_permission() {
            return match &self.model.audio {
                AudioState::Routed { .. } => Link::Connected,
                AudioState::Waiting => Link::Waiting,
                AudioState::Failed(_) | AudioState::Off => Link::Off,
            };
        }
        let permission = state(cx).permission;
        if permission == Some(Permission::Denied) {
            return Link::NeedsPermission;
        }
        match &self.model.audio {
            AudioState::Routed { .. } if self.silent_polls >= 4 => Link::NeedsPermission,
            AudioState::Routed { .. } => Link::Connected,
            AudioState::Waiting => Link::Waiting,
            AudioState::Failed(_) => Link::NeedsPermission,
            AudioState::Off => Link::Off,
        }
    }

    /// The sound the pictures, and meters would use is missing:
    /// the music system plays and the connection is not up.
    fn wants_hint(&self, link: Link) -> bool {
        // Playing on another device, this machine has nothing to hear: the
        // connection is not what is missing.
        matches!(link, Link::Off | Link::NeedsPermission) && self.model.engine == Engine::System && self.model.playback.is_playing() && self.model.remote_device().is_none()
    }

    /// The settings came from the disk: connect by default where nothing
    /// needs asking ([`auto_step`]); never prompt from here.
    pub(crate) fn connection_restore(&mut self, cx: &mut Cx) {
        ask_permission(cx);
        self.connection_auto(cx);
        self.model.changed();
    }

    /// Each player poll (about a second): what macOS allows now, and the
    /// connection's own step. Allowed since the route opened (the person
    /// answered the prompt, or turned it on in System Settings): open it
    /// again so it hears.
    pub(crate) fn connection_tick(&mut self, cx: &mut Cx) {
        if !self.model.has_route {
            return;
        }
        let before = state(cx).permission;
        let now = ask_permission(cx);
        if before != now {
            if now == Some(Permission::Granted) && self.model.eq.connect && !matches!(self.model.audio, AudioState::Off) {
                if let Some(workers) = &self.workers {
                    workers.audio(AudioCommand::Reopen);
                }
                self.silent_polls = 0;
                self.model.error = None;
            }
            self.model.changed();
        }
        self.connection_auto(cx);
    }

    /// Act on [`auto_step`].
    fn connection_auto(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        let failed = matches!(self.model.audio, AudioState::Failed(_));
        let failed_at = match (failed, state(cx).failed_at) {
            (true, None) => Some(now),
            (true, at) => at,
            (false, _) => None,
        };
        state(cx).failed_at = failed_at;
        let present = self.model.has_route && self.model.system_ready();
        let step = auto_step(self.model.eq.connect, present, state(cx).permission, self.model.eq.explained, &self.model.audio, failed_at.map(|at| now - at));
        match step {
            AutoStep::Nothing => {}
            AutoStep::Open => {
                if let Some(workers) = &self.workers {
                    workers.audio(AudioCommand::Enable(true));
                }
                state(cx).failed_at = None;
                self.model.audio = AudioState::Waiting;
                self.model.changed();
            }
            AutoStep::Explain => {
                self.model.eq.explained = true;
                self.save_eq();
                self.open_connection_sheet(cx);
            }
        }
    }

    fn connect(&mut self, cx: &mut Cx) {
        self.model.eq.connect = true;
        self.save_eq();
        if let Some(workers) = &self.workers {
            // Opening the route is what raises macOS's prompt, the first time.
            if !workers.audio(AudioCommand::Reopen) {
                self.model.error = Some("The audio connection is busy; try again".into());
            }
        }
        self.model.audio = AudioState::Waiting;
        self.silent_polls = 0;
        self.model.error = None;
        ask_permission(cx);
        self.model.changed();
    }

    fn disconnect(&mut self, _cx: &mut Cx) {
        self.model.eq.connect = false;
        self.save_eq();
        if let Some(workers) = &self.workers {
            workers.audio(AudioCommand::Enable(false));
        }
        self.model.audio = AudioState::Off;
        self.silent_polls = 0;
        self.model.changed();
    }

    /// Open the explainer.
    /// The connection as a menu item with its state and a dot, whose
    /// submenu does what the sheet's keys do (for a narrow window's menu).
    pub(crate) fn connection_menu_row(&mut self, cx: &mut Cx) -> Option<MenuRow> {
        let link = self.link(cx);
        if link == Link::Local {
            return None;
        }
        let dot = match link {
            Link::Connected => GREEN,
            Link::NeedsPermission => AMBER,
            _ => self.model.theme.muted,
        };
        let mut rows = Vec::new();
        match link {
            Link::Off => rows.push(MenuRow::new(live_id!(conn_menu_connect), "Connect")),
            Link::NeedsPermission => {
                rows.push(MenuRow::new(live_id!(conn_menu_settings), "Open System Settings"));
                rows.push(MenuRow::new(live_id!(conn_menu_connect), "Try again"));
            }
            _ => rows.push(MenuRow::new(live_id!(conn_menu_disconnect), "Disconnect")),
        }
        Some(MenuRow::new(live_id!(conn_menu), &format!("Spotify · {}", link.status())).swatch(dot, dot).submenu(rows))
    }

    /// A pick in that submenu; `true` when it was one.
    pub(crate) fn connection_menu_pick(&mut self, cx: &mut Cx, id: LiveId) -> bool {
        if id == live_id!(conn_menu_connect) {
            self.connect(cx);
        } else if id == live_id!(conn_menu_disconnect) {
            self.disconnect(cx);
        } else if id == live_id!(conn_menu_settings) {
            cx.open_url(SETTINGS_URL, OpenUrlInPlace::No);
        } else {
            return false;
        }
        self.model.changed();
        true
    }

    pub(crate) fn open_connection_sheet(&mut self, cx: &mut Cx) {
        state(cx).open = true;
        ask_permission(cx);
        self.model.changed();
    }

    pub(crate) fn connection_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let opened = [ids!(connect_key), ids!(screen_hint), ids!(eq_hint), ids!(kar_hint)].into_iter().any(|path| self.ui.button(cx, path).clicked(actions));
        if opened {
            let open = !state(cx).open;
            state(cx).open = open;
            if open {
                ask_permission(cx);
            }
            self.model.changed();
        }
        if self.ui.button(cx, ids!(conn_close)).clicked(actions) {
            state(cx).open = false;
            self.model.changed();
        }
        if self.ui.button(cx, ids!(conn_primary)).clicked(actions) {
            match self.link(cx) {
                Link::Off => self.connect(cx),
                Link::Waiting | Link::Connected => self.disconnect(cx),
                Link::NeedsPermission => cx.open_url(SETTINGS_URL, OpenUrlInPlace::No),
                Link::Local => {}
            }
        }
        if self.ui.button(cx, ids!(conn_secondary)).clicked(actions) {
            self.connect(cx);
        }
    }

    /// The key's tooltip, on hover.
    pub(crate) fn connection_event(&mut self, cx: &mut Cx, event: &Event) {
        let Event::MouseMove(e) = event else { return };
        if self.ui.window(cx, ids!(main_window)).window_id() != Some(e.window_id) {
            return;
        }
        let key = self.ui.widget(cx, ids!(connect_key));
        let area = key.area();
        let rect = if key.visible() && area.is_valid(cx) { Some(area.rect(cx)) } else { None };
        let over = rect.is_some_and(|rect| rect.contains(e.abs));
        if over == state(cx).hovering {
            return;
        }
        state(cx).hovering = over;
        let mut tip = self.ui.callout_tooltip(cx, ids!(conn_tip));
        match rect.filter(|_| over) {
            Some(rect) => {
                let text = match self.model.remote_device() {
                    Some(device) => format!("Spotify: playing on {}", device.name),
                    None => self.link(cx).tip().to_string(),
                };
                tip.show_with_options(cx, &text, rect, CalloutTooltipOptions { position: TooltipPosition::Bottom, ..Default::default() });
            }
            None => tip.hide(cx),
        }
    }

    /// The key, the hints and the sheet, from the state.
    pub(crate) fn update_connection(&mut self, cx: &mut Cx) {
        if !state(cx).asked && self.model.has_route {
            ask_permission(cx);
        }
        let link = self.link(cx);
        let hint = self.wants_hint(link);
        self.model.connect_hint = hint;
        // A narrow window's caption has room for the style and the menu key
        // only: the connection is then an item of that menu (and its sheet,
        // too big for such a window, stays shut).
        let narrow = self.caption_narrow(cx);
        self.set_visible(cx, ids!(connect_cap), link != Link::Local && !narrow);
        if narrow && state(cx).open {
            state(cx).open = false;
        }
        self.set_visible(cx, ids!(screen_hint_row), hint);
        self.set_visible(cx, ids!(eq_hint_row), hint);
        self.set_visible(cx, ids!(kar_hint_row), hint);
        let style = self.model.style_gen;
        if state(cx).shown != Some((link, hint, style)) {
            state(cx).shown = Some((link, hint, style));
            // The rail key's LED: green connected, amber needs permission,
            // dark otherwise (off, or Spotify not running).
            let (ink, lit) = match link {
                Link::Connected => (GREEN, 1.0),
                Link::NeedsPermission => (AMBER, 1.0),
                _ => (self.model.theme.muted, 0.0),
            };
            let mut cap = self.ui.widget(cx, ids!(connect_cap));
            script_apply_eval!(cx, cap, {draw_bg +: {glow_ink: #(ink)}});
            self.ui.relief_view(cx, ids!(connect_cap)).set_lit(cx, lit);
        }
        let open = state(cx).open && link != Link::Local;
        self.set_visible(cx, ids!(connection_sheet), open);
        if !open {
            return;
        }
        let refused = link == Link::NeedsPermission;
        let remote = self.model.remote_device().map(|d| d.name.clone());
        let detail = match (&self.model.audio, link) {
            _ if remote.is_some() => format!("Playing on {}: Amp hears it when it plays on this Mac", remote.as_deref().unwrap_or("")),
            (AudioState::Routed { device, sample_rate }, Link::Connected) => format!("Playing at {:.1} kHz through {device}", sample_rate / 1000.0),
            (_, Link::Waiting) => "Waiting for Spotify to play".to_string(),
            (AudioState::Failed(error), Link::Off) => error.clone(),
            (_, Link::NeedsPermission) => "macOS is not letting Amp hear Spotify yet. To allow it:".to_string(),
            _ => " ".to_string(),
        };
        self.ui.label(cx, ids!(conn_status)).set_text(cx, link.status());
        self.ui.label(cx, ids!(conn_detail)).set_text(cx, &detail);
        self.set_visible(cx, ids!(conn_explain), !refused);
        self.set_visible(cx, ids!(conn_ask), link == Link::Off && asks_permission());
        self.set_visible(cx, ids!(conn_denied), refused);
        let (primary, lit) = match link {
            Link::Off => ("CONNECT", 1.0),
            Link::NeedsPermission => ("OPEN SYSTEM SETTINGS", 1.0),
            _ => ("DISCONNECT", 0.0),
        };
        self.ui.button(cx, ids!(conn_primary)).set_text(cx, primary);
        self.ui.relief_view(cx, ids!(conn_primary_cap)).set_lit(cx, lit);
        self.set_visible(cx, ids!(conn_secondary_cap), refused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFF: AudioState = AudioState::Off;

    /// Connected by default: where nothing asks (Windows, Linux) and where
    /// macOS allowed it before, the route opens whenever the music system
    /// is there; not while it is away or once the person disconnected.
    #[test]
    fn it_connects_by_default_where_nothing_asks() {
        for permission in [None, Some(Permission::Granted)] {
            assert_eq!(auto_step(true, true, permission, false, &OFF, None), AutoStep::Open);
            assert_eq!(auto_step(true, false, permission, false, &OFF, None), AutoStep::Nothing, "the music system is away");
            assert_eq!(auto_step(false, true, permission, false, &OFF, None), AutoStep::Nothing, "disconnected by the person");
            // Open or waiting for the player's stream: the worker has it.
            assert_eq!(auto_step(true, true, permission, false, &AudioState::Waiting, None), AutoStep::Nothing);
            let routed = AudioState::Routed { device: "d".into(), sample_rate: 44_100.0 };
            assert_eq!(auto_step(true, true, permission, false, &routed, None), AutoStep::Nothing);
        }
    }

    /// macOS the first time: the explainer, once, never a silent prompt;
    /// refused, nothing until the person acts.
    #[test]
    fn macos_asks_through_the_explainer_once() {
        let unknown = Some(Permission::Unknown);
        assert_eq!(auto_step(true, true, unknown, false, &OFF, None), AutoStep::Explain);
        assert_eq!(auto_step(true, true, unknown, true, &OFF, None), AutoStep::Nothing, "shown already");
        assert_eq!(auto_step(true, true, Some(Permission::Denied), false, &OFF, None), AutoStep::Nothing);
    }

    /// A route that failed to open is tried again after a while (the
    /// player relaunched, the device came back), not every poll.
    #[test]
    fn a_failed_route_is_tried_again_later() {
        let failed = AudioState::Failed("gone".into());
        assert_eq!(auto_step(true, true, None, false, &failed, Some(2.0)), AutoStep::Nothing);
        assert_eq!(auto_step(true, true, None, false, &failed, Some(RETRY_FAILED_SECS)), AutoStep::Open);
        assert_eq!(auto_step(true, true, Some(Permission::Denied), false, &failed, Some(60.0)), AutoStep::Nothing);
    }
}
