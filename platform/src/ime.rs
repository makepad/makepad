use crate::makepad_script::*;
use makepad_micro_serde::*;
use makepad_studio_protocol::HostedWindowOp;
#[cfg(test)]
use makepad_studio_protocol::{drag_answer, drag_double, window_op, HostedDragAnswer, HostedDragQuery};

/// Text-input intent for a compositor hosting this application. Key focus alone
/// is insufficient: canvases, lists and terminal command shortcuts also own it.
#[derive(Clone, Copy, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedImeState {
    pub visible: bool,
    pub input_mode: InputMode,
    pub return_key: ReturnKeyType,
    pub multiline: bool,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(SerJson, DeJson)]
struct ImeEnvelope { makepad_ime: HostedImeState }
impl HostedImeState {
    pub fn to_json(&self) -> String {
        ImeEnvelope { makepad_ime: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_ime\"") { return None; }
        ImeEnvelope::deserialize_json(json).ok().map(|e| e.makepad_ime)
    }
}

#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct HostedKeyboard {
    pub height: f64,
    pub dismiss: bool,
}
#[derive(SerJson, DeJson)]
struct KeyboardEnvelope { makepad_keyboard: HostedKeyboard }
impl HostedKeyboard {
    pub fn to_json(&self) -> String {
        KeyboardEnvelope { makepad_keyboard: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_keyboard\"") { return None; }
        let state = KeyboardEnvelope::deserialize_json(json).ok()?.makepad_keyboard;
        (state.height.is_finite() && state.height >= 0.0).then_some(state)
    }
}

/// A hosted system Back gesture uses the same handled event as native Android.
/// `None` requests navigation; `Some` reports whether the app consumed it.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct HostedBack { pub handled: Option<bool> }
#[derive(SerJson, DeJson)]
struct BackEnvelope { makepad_back: HostedBack }
impl HostedBack {
    pub fn to_json(&self) -> String {
        BackEnvelope { makepad_back: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_back\"") { return None; }
        BackEnvelope::deserialize_json(json).ok().map(|e| e.makepad_back)
    }
}

/// What a hosted child's pointer input understands beyond the baseline
/// protocol, announced once right after `AppToStudio::AfterStartup` on the
/// existing `Custom` channel (older hosts route unknown custom JSON to their
/// bus and ignore it). `mouse_cancel`: the child decodes
/// `StudioToApp::MouseCancel` and ends the press as a cancellation; a host
/// never sends that message to a child that did not say so.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct HostedPointerCaps { pub mouse_cancel: bool }
#[derive(SerJson, DeJson)]
struct PointerCapsEnvelope { makepad_pointer_caps: HostedPointerCaps }
impl HostedPointerCaps {
    /// What this build of the platform understands.
    pub fn current() -> Self {
        Self { mouse_cancel: true }
    }
    pub fn to_json(&self) -> String {
        PointerCapsEnvelope { makepad_pointer_caps: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_pointer_caps\"") { return None; }
        PointerCapsEnvelope::deserialize_json(json).ok().map(|e| e.makepad_pointer_caps)
    }
}

// ---------------------------------------------------------------------------
// Compact forms: the window manager <-> app contract for a window that is
// parked small. Everything travels as JSON envelopes on the existing custom
// channel (`AppToStudio::Custom` up, `StudioToApp::Custom` down, which the
// app sees as `Event::Custom`). The host side is `apps/wm/src/spacewarp/compact.rs`,
// which has the same sequence from the window manager's point of view.
//
// An app that can show itself in a small fixed layout (a strip, a tile)
// declares those forms once; an app that declares nothing is never sent
// anything below and stays a scaled live view, with its input mapped.
//
//   app -> wm   HostedCompactForms { forms }     at connect (declare_compact_forms)
//   wm  -> app  HostedParked { scale: Some(s), max: [w, h] }
//                                                 the window is parked; `max` is the
//                                                 largest client area (points) that
//                                                 fits its slot
//   app -> wm   HostedCompact { form: Some(i) }  within COMPACT_ANSWER_SECS: the form
//                                                 it chose (index into its forms; it
//                                                 must fit `max`; the client area is then
//                                                 CompactForm::granted(max)) after switching its
//                                                 layout; `None` declines
//   wm  -> app  (window geometry)                 the client area becomes the form's
//                                                 size; the app presents its first
//                                                 compact frame; the host cross-fades
//                                                 from the scaled view to it
//   wm  -> app  HostedParked { scale: None, .. } unparked: the app switches to its full
//                                                 layout and answers HostedCompact
//                                                 { form: None }; the host resizes the
//                                                 client to the full size, and
//                                                 cross-fades back to the scaled view
//                                                 once the first full frame is in
//
// Rules for the app: switch the layout when told, in the same turn as the
// answer; ask for no window resize of its own while parked (the host owns the
// size, a form is a size the host grants); keep answering to a repeated
// `HostedParked` (the scale changes while the window moves) by repeating the
// same answer. An app that does not answer within COMPACT_ANSWER_SECS keeps
// being a scaled live view: the host sends `HostedParked { scale: None, .. }`
// to undo whatever it switched.
// ---------------------------------------------------------------------------

/// How long the host waits for an answer (and then for the first frame at the
/// answered size) before it gives up on a compact form for this park.
pub const COMPACT_ANSWER_SECS: f64 = 0.3;

/// One compact layout an app can render: its preferred client-area size in
/// points, and the narrowest it still lays out well at. A slot is often
/// narrower than a form's preferred width (a side zone on a small display),
/// so the host grants `granted(max)`: the preferred width, narrowed to the
/// slot but never under `min_w`. The app lays out to the size it is given.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct CompactForm {
    /// What the form is ("strip", "tile"); for logs and the host's tooltips.
    pub name: String,
    pub w: f64,
    pub h: f64,
    pub min_w: f64,
}

impl CompactForm {
    /// A form that is only good at exactly `w` x `h`.
    pub fn new(name: &str, w: f64, h: f64) -> Self {
        Self { name: name.to_string(), w, h, min_w: w }
    }
    /// A form that lays out to any width from `min_w` to `w`.
    pub fn flexible(name: &str, w: f64, min_w: f64, h: f64) -> Self {
        Self { name: name.to_string(), w, h, min_w }
    }
    /// Does the form fit a slot of at most `max` (w, h)?
    pub fn fits(&self, max: [f64; 2]) -> bool {
        self.min_w <= max[0] + 0.5 && self.h <= max[1] + 0.5
    }
    /// The client area the host grants in a slot of at most `max`.
    pub fn granted(&self, max: [f64; 2]) -> (f64, f64) {
        (self.w.min(max[0].max(self.min_w)), self.h)
    }
}

/// The form an app should answer with: the largest (by granted area) of
/// `forms` that fits `max`, as its index; `None` when none does.
pub fn pick_compact_form(forms: &[CompactForm], max: [f64; 2]) -> Option<usize> {
    let area = |f: &CompactForm| { let g = f.granted(max); g.0 * g.1 };
    forms
        .iter()
        .enumerate()
        .filter(|(_, f)| f.fits(max))
        .max_by(|a, b| area(a.1).total_cmp(&area(b.1)))
        .map(|(i, _)| i)
}

/// App to host, at connect: the compact forms the app can render.
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct HostedCompactForms { pub forms: Vec<CompactForm> }
#[derive(SerJson, DeJson)]
struct CompactFormsEnvelope { makepad_compact_forms: HostedCompactForms }
impl HostedCompactForms {
    pub fn to_json(&self) -> String {
        CompactFormsEnvelope { makepad_compact_forms: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_compact_forms\"") { return None; }
        CompactFormsEnvelope::deserialize_json(json).ok().map(|e| e.makepad_compact_forms)
    }
    /// What this process declared (None: nothing, the host sends no compact messages).
    pub fn declared() -> Option<Self> {
        let forms = DECLARED_FORMS.lock().ok()?.clone();
        (!forms.is_empty()).then_some(Self { forms })
    }
}

static DECLARED_FORMS: std::sync::Mutex<Vec<CompactForm>> = std::sync::Mutex::new(Vec::new());

/// Host to app: the window is parked (`scale`: shown size over full size) or
/// back at full size (`None`). `max` is the largest client area, in points,
/// that fits the slot; a form that does not fit it must not be chosen.
/// Arrives as `Event::Custom(json)`, answered by `HostedParked::parse(json)`.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedParked { pub scale: Option<f32>, pub max: [f64; 2] }
#[derive(SerJson, DeJson)]
struct ParkedEnvelope { makepad_parked: HostedParked }
impl HostedParked {
    pub fn to_json(&self) -> String {
        ParkedEnvelope { makepad_parked: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_parked\"") { return None; }
        ParkedEnvelope::deserialize_json(json).ok().map(|e| e.makepad_parked)
    }
}

/// App to host: the form the app switched to (an index into its declared
/// forms), or `None`: it shows its full layout (declined, or unparked).
#[derive(Clone, Debug, Default, SerJson, DeJson)]
pub struct HostedCompact { pub form: Option<u32> }
#[derive(SerJson, DeJson)]
struct CompactEnvelope { makepad_compact: HostedCompact }
impl HostedCompact {
    pub fn to_json(&self) -> String {
        CompactEnvelope { makepad_compact: self.clone() }.serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_compact\"") { return None; }
        CompactEnvelope::deserialize_json(json).ok().map(|e| e.makepad_compact)
    }
}

// Own chrome: a makepad app under the WM always draws its own caption (its theme's
// chrome, widgets `Window::chrome`); the WM draws no frame or title bar for it, and
// asks the app's window drag query (`Event::WindowDragQuery`, the one the native
// backends use for borderless windows) where a press landed.
//
//   wm  -> app  HostedFullscreen { window_id, on } the window is fullscreen (its own space):
//                                                 it draws no caption then
//   (the drag and window-op messages live in `makepad_studio_protocol`: the WM-hosted HAL
//   shell of `makepad run` relays them too)
//   wm  -> app  HostedDragQuery { id, x, y }     a press in the window (client points)
//   app -> wm   HostedDragAnswer { id, response, double_click }
//                                                 the query's answer: `drag_answer::*`
//                                                 Caption: the host moves the window and
//                                                 takes the press away (a cancel);
//                                                 anything else is the app's own press.
//                                                 `double_click` (`drag_double::*`): what a
//                                                 double click there does
// The window border stays the host's (resize); the app's caption buttons
// answer Client, so their presses are the app's.

/// Host to app: the app's window `window_id` (its `WindowId::id`; an app's later windows
/// are windows of their own on the host) is fullscreen (a space of its own), or not any
/// more. A fullscreen window draws no caption (edge to edge); a maximised one keeps it.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedFullscreen { pub window_id: usize, pub on: bool }
#[derive(SerJson, DeJson)]
struct FullscreenEnvelope { makepad_fullscreen: HostedFullscreen }
impl HostedFullscreen {
    pub fn to_json(&self) -> String { FullscreenEnvelope { makepad_fullscreen: self.clone() }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_fullscreen\"") { return None; }
        FullscreenEnvelope::deserialize_json(json).ok().map(|e| e.makepad_fullscreen)
    }
}

/// `HostedWindow::op` values.
pub mod hosted_window_op {
    /// Child to host: the window is gone (closed by the app, or after a `CLOSE_REQUEST`).
    pub const CLOSED: u8 = 0;
    /// Host to child: the person closed the window on the desk; the app closes it
    /// unless its `WindowCloseRequested` says no.
    pub const CLOSE_REQUEST: u8 = 1;
    /// Child to host: what the app asked of the window (`Window::minimize` and the like).
    pub const MINIMIZE: u8 = 2;
    pub const MAXIMIZE: u8 = 3;
    pub const RESTORE: u8 = 4;
    pub const FULLSCREEN: u8 = 5;
    /// Child to host: put the window at `x`, `y` (the app's screen coordinates, the
    /// `WmEvent::Screens` it was given: relative to its first window).
    pub const MOVE: u8 = 6;
    /// Child to host: the window's title is `title` (on its creation and when it changes).
    pub const TITLE: u8 = 7;
    /// Child to host, for any of its windows (the first too): the inner size the app asked
    /// for (`window.inner_size`), `x` by `y` points. The host honours it within its desk.
    pub const SIZE: u8 = 8;
}

/// A hosted app's windows after its first (an output window, a second view): the host
/// shows each as a window of its own. Both ways, as JSON in `Custom`.
#[derive(Clone, Debug, Default, PartialEq, SerJson, DeJson)]
pub struct HostedWindow { pub window_id: usize, pub op: u8, pub x: f64, pub y: f64, pub title: String }
#[derive(SerJson, DeJson)]
struct HostedWindowEnvelope { makepad_hosted_window: HostedWindow }
impl HostedWindow {
    pub fn new(window_id: usize, op: u8) -> Self { Self { window_id, op, x: 0.0, y: 0.0, title: String::new() } }
    pub fn title(window_id: usize, title: &str) -> Self { Self { title: title.to_string(), ..Self::new(window_id, hosted_window_op::TITLE) } }
    pub fn to_json(&self) -> String { HostedWindowEnvelope { makepad_hosted_window: self.clone() }.serialize_json() }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_hosted_window\"") { return None; }
        HostedWindowEnvelope::deserialize_json(json).ok().map(|e| e.makepad_hosted_window)
    }
}

impl crate::cx::Cx {
    /// Tell the host about one of this hosted app's later windows (`hosted_window_op`).
    pub fn send_hosted_window(message: HostedWindow) {
        crate::cx::Cx::send_studio_message(makepad_studio_protocol::AppToStudio::Custom(message.to_json()));
    }

    /// A hosted backend's answer to a window op: tell the host (`window_op::*`).
    pub fn send_host_window_op(op: u8) {
        crate::cx::Cx::send_studio_message(makepad_studio_protocol::AppToStudio::Custom(HostedWindowOp { op }.to_json()));
    }

    /// Tell the host this app draws its own window chrome (`on`) or no longer
    /// does. A no-op when no host is attached.
    /// A native window draws its own chrome (an app-drawn caption, `Window::set_drag_region`):
    /// the platform leaves its decorations off. Under a window manager every makepad
    /// window draws its own caption and nothing is said.
    pub fn set_own_window_chrome(&mut self, on: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        self.platform_ops.push_back(crate::cx_api::CxOsOp::SetOwnChrome(on));
        #[cfg(target_arch = "wasm32")]
        let _ = on;
    }

    /// The host says `window` is fullscreen (`HostedFullscreen`).
    pub fn hosted_fullscreen(&self, window: crate::window::WindowId) -> bool {
        self.windows.is_valid(window) && self.windows[window].hosted_fullscreen
    }
}

/// What a hosted child announces right after `AfterStartup`: its pointer
/// capabilities, and the compact forms it declared (if any).
pub fn connect_announcements() -> Vec<String> {
    let mut out = vec![HostedPointerCaps::current().to_json()];
    out.extend(HostedCompactForms::declared().map(|f| f.to_json()));
    out
}

impl crate::cx::Cx {
    /// Declare the compact forms this app can render (call from
    /// `Event::Startup`; they are announced to the host at connect). See the
    /// contract above.
    pub fn declare_compact_forms(&mut self, forms: &[CompactForm]) {
        if let Ok(mut declared) = DECLARED_FORMS.lock() {
            *declared = forms.to_vec();
        }
    }
    /// Answer a [`HostedParked`]: the form the app switched to (`None`: its
    /// full layout). A no-op when no host is attached.
    pub fn answer_compact(&mut self, form: Option<usize>) {
        crate::cx::Cx::send_studio_message(makepad_studio_protocol::AppToStudio::Custom(
            HostedCompact { form: form.map(|f| f as u32) }.to_json(),
        ));
    }
}

script_mod! {
    mod.ime = {
        InputMode: mod.std.set_type_default() do #(InputMode::script_api(vm)),
        ..me.InputMode,
        AutoCapitalize: mod.std.set_type_default() do #(AutoCapitalize::script_api(vm)),
        ..me.AutoCapitalize,
        AutoCorrect: mod.std.set_type_default() do #(AutoCorrect::script_api(vm)),
        ..me.AutoCorrect,
        ReturnKeyType: mod.std.set_type_default() do #(ReturnKeyType::script_api(vm)),
        ..me.ReturnKeyType,
        TextInputContentType: mod.std.set_type_default() do #(TextInputContentType::script_api(vm)),
        ..me.TextInputContentType,
    }
}

/// Input mode hint for soft keyboards (matches web standard `inputmode` attribute).
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq, SerJson, DeJson)]
pub enum InputMode {
    None,
    #[pick]
    Text,
    Ascii,
    Url,
    Numeric,
    Tel,
    Email,
    Decimal,
    Search,
}

/// Autocapitalization behavior for soft keyboards.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum AutoCapitalize {
    None,
    Words,
    #[pick]
    Sentences,
    AllCharacters,
}

/// Autocorrection behavior for soft keyboards.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum AutoCorrect {
    #[pick]
    Default,
    Enabled,
    Disabled,
}

/// Return key type - controls the visual appearance and action of the return key.
///
/// Supported on iOS and Android. On desktop platforms, this has no effect.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq, SerJson, DeJson)]
pub enum ReturnKeyType {
    #[pick]
    Default,
    None,
    Go,
    Google,
    Join,
    Next,
    Route,
    Search,
    Send,
    Yahoo,
    Done,
    EmergencyCall,
    Continue,
    Previous,
}

/// AutoFill / content-type hint for a text field (maps to iOS UITextContentType).
/// Independent of whether the text is obscured. iOS/Android only; no-op on desktop.
#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq)]
pub enum TextInputContentType {
    #[pick]
    None,
    Username,
    Password,
    NewPassword,
    EmailAddress,
    Url,
    FullStreetAddress,
    TelephoneNumber,
    OneTimeCode,
}

impl Default for InputMode {
    fn default() -> Self {
        InputMode::Text
    }
}

impl Default for AutoCapitalize {
    fn default() -> Self {
        AutoCapitalize::Sentences
    }
}

impl Default for AutoCorrect {
    fn default() -> Self {
        AutoCorrect::Default
    }
}

impl Default for ReturnKeyType {
    fn default() -> Self {
        ReturnKeyType::Default
    }
}

impl Default for TextInputContentType {
    fn default() -> Self {
        TextInputContentType::None
    }
}

/// Soft keyboard configuration for mobile platforms (iOS/Android).
/// These settings have no effect on desktop platforms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoftKeyboardConfig {
    pub input_mode: InputMode,
    pub autocapitalize: AutoCapitalize,
    pub autocorrect: AutoCorrect,
    pub return_key_type: ReturnKeyType,
}

/// Text input configuration combining cross-platform and mobile-specific settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextInputConfig {
    pub soft_keyboard: SoftKeyboardConfig,
    pub is_multiline: bool,
    pub is_secure: bool,
    pub submit_on_enter: bool,
    pub content_type: TextInputContentType,
    pub is_read_only: bool,
}

impl TextInputConfig {
    /// True for credential fields (username/password/email) iOS associates with login
    /// AutoFill, which "taint" a reused text view (so we recreate it when leaving one).
    pub fn taints_autofill(&self) -> bool {
        self.is_secure
            || matches!(
                self.content_type,
                TextInputContentType::Username
                    | TextInputContentType::Password
                    | TextInputContentType::NewPassword
                    | TextInputContentType::EmailAddress
            )
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn the_own_chrome_messages_round_trip_and_do_not_mistake_each_other() {
        let chrome = HostedFullscreen { window_id: 2, on: true }.to_json();
        assert_eq!(HostedFullscreen::parse(&chrome), Some(HostedFullscreen { window_id: 2, on: true }));
        let q = HostedDragQuery { id: 7, x: 12.5, y: 3.0 }.to_json();
        assert_eq!(HostedDragQuery::parse(&q), Some(HostedDragQuery { id: 7, x: 12.5, y: 3.0 }));
        let a = HostedDragAnswer { id: 7, response: drag_answer::CAPTION, double_click: drag_double::FULLSCREEN }.to_json();
        assert_eq!(HostedDragAnswer::parse(&a), Some(HostedDragAnswer { id: 7, response: drag_answer::CAPTION, double_click: drag_double::FULLSCREEN }));
        assert!(HostedFullscreen::parse(&q).is_none() && HostedDragQuery::parse(&a).is_none() && HostedDragAnswer::parse(&chrome).is_none());
        let op = HostedWindowOp { op: window_op::MINIMIZE }.to_json();
        assert_eq!(HostedWindowOp::parse(&op), Some(HostedWindowOp { op: window_op::MINIMIZE }));
        assert!(HostedWindowOp::parse(&chrome).is_none() && HostedFullscreen::parse(&op).is_none() && HostedDragQuery::parse(&op).is_none());
    }

    #[test]
    fn the_compact_messages_round_trip_and_do_not_mistake_each_other() {
        let forms = HostedCompactForms { forms: vec![CompactForm::new("strip", 320.0, 100.0), CompactForm::new("tile", 220.0, 220.0)] };
        let json = forms.to_json();
        assert_eq!(HostedCompactForms::parse(&json).unwrap().forms, forms.forms);
        assert!(HostedParked::parse(&json).is_none() && HostedCompact::parse(&json).is_none());
        let parked = HostedParked { scale: Some(0.25), max: [400.0, 300.0] };
        let back = HostedParked::parse(&parked.to_json()).unwrap();
        assert_eq!((back.scale, back.max), (Some(0.25), [400.0, 300.0]));
        assert_eq!(HostedParked::parse(&HostedParked { scale: None, max: [0.0, 0.0] }.to_json()).unwrap().scale, None);
        assert_eq!(HostedCompact::parse(&HostedCompact { form: Some(1) }.to_json()).unwrap().form, Some(1));
        assert_eq!(HostedCompact::parse(&HostedCompact { form: None }.to_json()).unwrap().form, None);
    }

    #[test]
    fn the_largest_form_that_fits_the_slot_is_picked() {
        let forms = [CompactForm::new("strip", 320.0, 100.0), CompactForm::new("tile", 220.0, 220.0)];
        assert_eq!(pick_compact_form(&forms, [400.0, 300.0]), Some(1));
        assert_eq!(pick_compact_form(&forms, [400.0, 150.0]), Some(0));
        assert_eq!(pick_compact_form(&forms, [200.0, 150.0]), None);
        assert_eq!(pick_compact_form(&[], [400.0, 300.0]), None);
    }
}

#[cfg(test)]
mod hosted_window_tests {
    use super::*;

    /// A later window's messages survive the trip, its title included (non-ASCII too).
    #[test]
    fn a_hosted_window_message_round_trips() {
        let title = HostedWindow::title(3, "Stage — Program Output");
        assert_eq!(HostedWindow::parse(&title.to_json()), Some(title));
        let mv = HostedWindow { x: 1610.5, y: -2.0, ..HostedWindow::new(3, hosted_window_op::MOVE) };
        assert_eq!(HostedWindow::parse(&mv.to_json()), Some(mv));
        assert_eq!(HostedWindow::parse(&HostedWindowOp { op: window_op::CLOSE }.to_json()), None);
    }
}
