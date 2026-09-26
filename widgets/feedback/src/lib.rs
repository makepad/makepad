//! Send feedback: a caption icon and a small panel that shows the person
//! exactly what they are about to send.
//!
//! The panel holds a message field, a "Send screenshot" check with a
//! thumbnail of the picture, and the system information as one compact row
//! per item (sender, app, OS, GPU, audio, RAM), each with its own ✕. Only
//! what is left when Send is pressed goes out: removing a row removes it
//! from the request, unchecking the screenshot leaves the picture out.
//!
//! The screenshot is the app's own window as it was the moment before the
//! panel opened: opening first reads one presented frame back
//! (`makepad_platform::screen_capture`, the recorder's seam) and only then
//! draws the panel, so the picture never contains the panel itself.
//!
//! An app opts in with two pieces:
//!
//! ```text
//! // In the caption: any button, with the feedback glyph.
//! feedback_open := ButtonIcon{draw_icon +: {svg: crate_resource("makepad_widgets_feedback:resources/feedback.svg")}}
//! // Over the window's body (an Overlay flow):
//! feedback := mod.widgets.FeedbackPanel{app_id: "amp" app_name: "Makepad Amp" app_version: "0.1.0"}
//! ```
//!
//! and `self.ui.feedback_panel(cx, ids!(feedback)).open(cx)` when the button
//! is clicked. `mod.widgets.FeedbackButton` is a ready-made caption button.
//!
//! The request is one `POST` of JSON (see [`FeedbackPanel::payload`]) to
//! `https://makepad.nl/api/feedback`, or to `MAKEPAD_FEEDBACK_URL` when it
//! is set (for testing against a local server). The sender's email is known
//! only when whoever launched the app says so in `MAKEPAD_FEEDBACK_EMAIL`
//! (the Makepad Builder does); without it the feedback is anonymous.

use makepad_widgets::makepad_platform::screen_capture::{
    add_screen_capture, remove_screen_capture, ScreenCaptureOptions,
};
use makepad_widgets::makepad_platform::system_info;
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    /** A caption button with the feedback glyph (a speech bubble). */
    mod.widgets.FeedbackButton = ButtonFlatterIcon{
        width: 24 height: 24 padding: 0 margin: 0
        icon_walk: Walk{width: 14 height: 14}
        draw_icon +: {
            svg: crate_resource("self:resources/feedback.svg")
            color: theme.color_label_inner
        }
    }

    let FbText = Label{
        padding: 0
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 9.5}}
    }
    let FbMuted = Label{
        padding: 0
        draw_text +: {color: mix(theme.color_bg_app, theme.color_text, 0.62) text_style: theme.font_regular{font_size: 8.5}}
    }

    // One item of system information: what it is, its value, and an ✕.
    let FbRow = RoundedView{
        width: Fill height: 24
        flow: Right spacing: 6 align: Align{y: 0.5}
        padding: Inset{left: 8 right: 2}
        show_bg: true
        draw_bg +: {
            color: mix(theme.color_bg_app, theme.color_text, 0.07)
            border_size: 0.0
            border_radius: 4.0
        }
        key := Label{
            width: 44 padding: 0
            draw_text +: {color: mix(theme.color_bg_app, theme.color_text, 0.62) text_style: theme.font_bold{font_size: 8.0}}
        }
        value := Label{
            width: Fill padding: 0
            draw_text +: {
                color: theme.color_text
                text_style: theme.font_regular{font_size: 9.0}
                max_lines: 1 text_overflow: TextOverflow.Ellipsis
            }
        }
        remove := ButtonFlatterIcon{
            width: 20 height: 20 padding: 0 margin: 0
            icon_walk: Walk{width: 8 height: 8}
            draw_icon +: {
                svg: crate_resource("self:resources/remove.svg")
                color: mix(theme.color_bg_app, theme.color_text, 0.62)
            }
        }
    }

    mod.widgets.FeedbackPanelBase = #(FeedbackPanel::register_widget(vm))
    mod.widgets.FeedbackPanel = set_type_default() do mod.widgets.FeedbackPanelBase{
        width: Fill height: Fill
        flow: Overlay
        app_id: ""
        app_name: ""
        app_version: ""
        modal := Modal{
            bg_view +: {draw_bg +: {color: #x00000059}}
            content +: {
                width: 400 height: Fit
                card := RoundedView{
                    width: Fill height: Fit
                    flow: Down spacing: 10
                    padding: Inset{left: 16 right: 16 top: 14 bottom: 14}
                    show_bg: true
                    draw_bg +: {
                        color: theme.color_bg_app
                        border_color: theme.color_bevel
                        border_size: 1.0
                        border_radius: 7.0
                    }
                    View{
                        width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                        Icon{
                            icon_walk: Walk{width: 15 height: 15}
                            draw_icon +: {
                                svg: crate_resource("self:resources/feedback.svg")
                                color: theme.color_text
                            }
                        }
                        title := Label{
                            width: Fill padding: 0 text: "Send feedback"
                            draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 11.0}}
                        }
                    }
                    message := TextInput{
                        width: Fill height: 92
                        is_multiline: true
                        empty_text: "What works, what doesn't, what you'd like…"
                    }
                    shot := View{
                        width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.0}
                        View{
                            width: Fill height: Fit flow: Down spacing: 4
                            shot_check := CheckBox{text: "Send screenshot"}
                            shot_note := FbMuted{width: Fill text: "Taking a picture of the window…"}
                        }
                        thumb_frame := RoundedView{
                            width: Fit height: Fit padding: 1
                            show_bg: true
                            draw_bg +: {color: theme.color_bevel border_size: 0.0 border_radius: 3.0}
                            thumb := Image{width: 150 height: 94 fit: ImageFit.Smallest}
                        }
                    }
                    clip := View{
                        width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.0}
                        View{
                            width: Fill height: Fit flow: Down spacing: 4
                            clip_check := CheckBox{text: "Send clipboard"}
                            clip_note := FbMuted{width: Fill text: "Off: the clipboard is not read."}
                            clip_text_frame := RoundedView{
                                visible: false width: Fill height: Fit
                                padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                                show_bg: true
                                draw_bg +: {color: mix(theme.color_bg_app, theme.color_text, 0.07) border_size: 0.0 border_radius: 3.0}
                                clip_text := Label{
                                    width: Fill padding: 0
                                    draw_text +: {
                                        color: theme.color_text
                                        text_style: theme.font_regular{font_size: 8.5}
                                        max_lines: 3 text_overflow: TextOverflow.Ellipsis
                                    }
                                }
                            }
                        }
                        clip_thumb_frame := RoundedView{
                            visible: false width: Fit height: Fit padding: 1
                            show_bg: true
                            draw_bg +: {color: theme.color_bevel border_size: 0.0 border_radius: 3.0}
                            clip_thumb := Image{width: 150 height: 94 fit: ImageFit.Smallest}
                        }
                    }
                    View{
                        width: Fill height: Fit flow: Right align: Align{y: 0.5}
                        info_title := FbMuted{width: Fill text: "Also sent · remove anything you'd rather not send"}
                        restore := ButtonFlatter{
                            visible: false height: 18 padding: Inset{left: 6 right: 6} text: "Restore all"
                            draw_text +: {text_style: theme.font_regular{font_size: 8.0}}
                        }
                    }
                    rows := View{
                        width: Fill height: Fit flow: Down spacing: 3
                        row_from := FbRow{key +: {text: "From"}}
                        row_app := FbRow{key +: {text: "App"}}
                        row_os := FbRow{key +: {text: "OS"}}
                        row_gpu := FbRow{key +: {text: "GPU"}}
                        row_audio := FbRow{key +: {text: "Audio"}}
                        row_ram := FbRow{key +: {text: "RAM"}}
                        rows_empty := FbMuted{visible: false text: "No system information will be sent."}
                    }
                    View{
                        width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                        status := FbText{width: Fill text: ""}
                        cancel := ButtonFlat{height: 28 text: "Cancel"}
                        send := ButtonPrimary{height: 28 text: "Send"}
                    }
                }
            }
        }
    }
}

/// The request goes here unless `MAKEPAD_FEEDBACK_URL` names another place.
pub const DEFAULT_ENDPOINT: &str = "https://makepad.nl/api/feedback";
/// Server-side limits, mirrored so the person hears about them before sending.
const MAX_MESSAGE_BYTES: usize = 60 * 1024;
const MAX_SCREENSHOT_BYTES: usize = 8 * 1024 * 1024;
/// How long opening waits for the window's frame before it opens without one.
const CAPTURE_WAIT_SECS: f64 = 4.0;
/// Clipboard text that is sent, in bytes (the server takes the same).
const MAX_CLIP_TEXT_BYTES: usize = 64 * 1024;
/// The longest side of the picture that is sent, in device pixels.
const SHOT_MAX_SIDE: u32 = 2560;
/// The thumbnail's width in device pixels (drawn at 150 points).
const THUMB_WIDTH: u32 = 320;

/// A picture ready to send: the window's frame, or an image from the
/// clipboard.
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    /// The PNG, base64-encoded for the JSON body.
    pub png_base64: String,
    pub png_bytes: usize,
    thumb_png: Vec<u8>,
}

/// One row of system information.
#[derive(Clone, Debug)]
pub struct InfoItem {
    /// The row widget, and the item's key in the payload.
    row: LiveId,
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
    pub kept: bool,
}

/// What "Send clipboard" holds. The clipboard is read only when the box is
/// checked, and read again each time it is.
#[derive(Default)]
enum ClipState {
    #[default]
    Off,
    /// An image is being turned into the PNG that is sent.
    Reading(TaskHandle<Result<Screenshot, String>>),
    Text(String),
    Image(Screenshot),
}

#[derive(Clone, Debug, Default, PartialEq)]
enum PanelState {
    #[default]
    Closed,
    /// Reading the window's frame back; the panel is not drawn yet.
    Capturing,
    Editing,
    Sending,
    Sent,
    Failed(String),
}

#[derive(Script, ScriptHook, Widget)]
pub struct FeedbackPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// A short identifier for the app, e.g. "amp".
    #[live]
    app_id: String,
    /// The app's name as people know it, e.g. "Makepad Amp".
    #[live]
    app_name: String,
    #[live]
    app_version: String,
    #[rust]
    state: PanelState,
    /// The window this panel is drawn in: the one the screenshot is of.
    #[rust]
    window_id: Option<usize>,
    #[rust]
    items: Vec<InfoItem>,
    #[rust]
    shot: Option<Screenshot>,
    #[rust]
    capture: Option<TaskHandle<Result<Screenshot, String>>>,
    #[rust]
    capture_timer: Timer,
    #[rust]
    capture_started: f64,
    #[rust]
    request_id: LiveId,
    #[rust]
    clip: ClipState,
    #[rust]
    clip_timer: Timer,
    /// Put the keyboard in the message field once the modal has drawn (it
    /// takes the focus into its content when it first draws).
    #[rust]
    focus_message: bool,
}

impl FeedbackPanel {
    /// Take the picture of the window, then show the panel.
    pub fn open(&mut self, cx: &mut Cx) {
        if self.state != PanelState::Closed {
            return;
        }
        self.state = PanelState::Capturing;
        self.shot = None;
        let window_id = self.window_id;
        self.capture = match cx.task_pool().submit(Lane::Heavy, move || capture_window(window_id)) {
            Ok(handle) => Some(handle),
            Err(error) => {
                log!("feedback: screenshot unavailable ({error:?})");
                None
            }
        };
        self.capture_started = Cx::time_now();
        self.capture_timer = cx.start_interval(0.05);
        // A frame has to be presented for the readback to see one.
        cx.redraw_all();
    }

    fn show(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.capture_timer);
        self.capture = None;
        self.state = PanelState::Editing;
        self.items = collect_info(cx, &self.app_name, &self.app_version);
        for item in &self.items {
            let row = self.view.view(cx, &[item.row]);
            row.set_visible(cx, true);
            self.view.label(cx, &[item.row, live_id!(value)]).set_text(cx, &item.value);
        }
        if !self.items.iter().any(|item| item.key == "from") {
            self.view.view(cx, ids!(row_from)).set_visible(cx, false);
        }
        let has_shot = self.shot.is_some();
        self.view.check_box(cx, ids!(shot_check)).set_active(cx, has_shot, Animate::No);
        if let Some(shot) = &self.shot {
            let _ = self.view.image(cx, ids!(thumb)).load_png_from_data(cx, &shot.thumb_png);
        }
        self.sync_shot(cx);
        self.sync_rows(cx);
        self.view.text_input(cx, ids!(message)).set_text(cx, "");
        self.set_status(cx, "");
        self.view.button(cx, ids!(send)).set_visible(cx, true);
        self.view.button(cx, ids!(cancel)).set_text(cx, "Cancel");
        self.set_clip(cx, ClipState::Off);
        self.view.modal(cx, ids!(modal)).open(cx);
        self.focus_message = true;
        self.view.redraw(cx);
    }

    fn close(&mut self, cx: &mut Cx) {
        if self.state == PanelState::Sending {
            cx.cancel_http_request(self.request_id);
        }
        if let Some(capture) = self.capture.take() {
            capture.cancel();
        }
        cx.stop_timer(self.capture_timer);
        self.state = PanelState::Closed;
        self.shot = None;
        self.set_clip(cx, ClipState::Off);
        self.view.modal(cx, ids!(modal)).close(cx);
        self.view.redraw(cx);
    }

    /// Read the clipboard now (the box was checked): text as it is, an
    /// image turned into the PNG that would be sent, on a worker.
    fn read_clip(&mut self, cx: &mut Cx) {
        use makepad_widgets::makepad_platform::clipboard_read::{read_clipboard, ClipboardContent, ClipboardImage};
        let next = match read_clipboard() {
            ClipboardContent::Text(text) if text.len() > MAX_CLIP_TEXT_BYTES => {
                Err("The text on the clipboard is over 64 KB, too long to send.")
            }
            ClipboardContent::Text(text) => Ok(ClipState::Text(text)),
            ClipboardContent::Image(image) => {
                let job = move || match image {
                    ClipboardImage::Rgba { width, height, rgba } => prepare_image(width, height, rgba),
                    ClipboardImage::Png(png) => {
                        let (width, height, rgba) = decode_png(&png)?;
                        prepare_image(width, height, rgba)
                    }
                };
                match cx.task_pool().submit(Lane::Heavy, job) {
                    Ok(handle) => {
                        cx.stop_timer(self.clip_timer);
                        self.clip_timer = cx.start_interval(0.05);
                        Ok(ClipState::Reading(handle))
                    }
                    Err(_) => Err("The clipboard's image could not be read."),
                }
            }
            ClipboardContent::Empty => Err("The clipboard is empty."),
            ClipboardContent::Unsupported => Err("Reading the clipboard is not available on this system yet."),
        };
        match next {
            Ok(state) => self.set_clip(cx, state),
            Err(note) => {
                self.set_clip(cx, ClipState::Off);
                self.view.check_box(cx, ids!(clip_check)).set_active(cx, false, Animate::No);
                self.view.label(cx, ids!(clip_note)).set_text(cx, note);
            }
        }
    }

    fn set_clip(&mut self, cx: &mut Cx, clip: ClipState) {
        if let ClipState::Reading(handle) = std::mem::replace(&mut self.clip, clip) {
            handle.cancel();
        }
        if !matches!(self.clip, ClipState::Reading(_)) {
            cx.stop_timer(self.clip_timer);
        }
        let (note, text, thumb) = match &self.clip {
            ClipState::Off => ("Off: the clipboard is not read.".to_string(), None, None),
            ClipState::Reading(_) => ("Reading the clipboard…".to_string(), None, None),
            ClipState::Text(text) => (
                format!("This text from the clipboard ({} characters):", text.chars().count()),
                Some(text.lines().filter(|line| !line.trim().is_empty()).take(3).collect::<Vec<_>>().join("\n")),
                None,
            ),
            ClipState::Image(image) => (
                format!(
                    "This image from the clipboard, {}×{} ({} KB).",
                    image.width,
                    image.height,
                    image.png_bytes.div_ceil(1024)
                ),
                None,
                Some(image.thumb_png.clone()),
            ),
        };
        self.view.check_box(cx, ids!(clip_check)).set_active(cx, !matches!(self.clip, ClipState::Off), Animate::No);
        self.view.label(cx, ids!(clip_note)).set_text(cx, &note);
        self.view.view(cx, ids!(clip_text_frame)).set_visible(cx, text.is_some());
        self.view.label(cx, ids!(clip_text)).set_text(cx, text.as_deref().unwrap_or(""));
        self.view.view(cx, ids!(clip_thumb_frame)).set_visible(cx, thumb.is_some());
        if let Some(thumb) = thumb {
            let _ = self.view.image(cx, ids!(clip_thumb)).load_png_from_data(cx, &thumb);
        }
        self.view.redraw(cx);
    }

    fn poll_clip(&mut self, cx: &mut Cx) {
        let ClipState::Reading(handle) = &mut self.clip else { return };
        match handle.try_take() {
            None => {}
            Some(Ok(Ok(image))) => self.set_clip(cx, ClipState::Image(image)),
            Some(Ok(Err(error))) => {
                log!("feedback: clipboard image: {error}");
                self.set_clip(cx, ClipState::Off);
                self.view.label(cx, ids!(clip_note)).set_text(cx, "The clipboard's image could not be read.");
            }
            Some(Err(_)) => self.set_clip(cx, ClipState::Off),
        }
    }

    fn sync_shot(&mut self, cx: &mut Cx) {
        let checked = self.shot.is_some() && self.view.check_box(cx, ids!(shot_check)).active(cx);
        let note = match &self.shot {
            None => "No picture of the window could be taken.".to_string(),
            Some(shot) if checked => format!(
                "This window as it was, {}×{} ({} KB).",
                shot.width,
                shot.height,
                shot.png_bytes.div_ceil(1024)
            ),
            Some(_) => "The picture stays on this computer.".to_string(),
        };
        self.view.label(cx, ids!(shot_note)).set_text(cx, &note);
        // Shown whenever there is a picture, so the panel does not jump;
        // the note says whether it goes.
        self.view.view(cx, ids!(thumb_frame)).set_visible(cx, self.shot.is_some());
        self.view.redraw(cx);
    }

    fn sync_rows(&mut self, cx: &mut Cx) {
        for item in &self.items {
            self.view.view(cx, &[item.row]).set_visible(cx, item.kept);
        }
        let kept = self.items.iter().filter(|item| item.kept).count();
        self.view.label(cx, ids!(rows_empty)).set_visible(cx, kept == 0);
        self.view.button(cx, ids!(restore)).set_visible(cx, kept < self.items.len());
        self.view.redraw(cx);
    }

    fn set_status(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(status)).set_text(cx, text);
        self.view.redraw(cx);
    }

    /// The request body: exactly what the panel shows, less what was removed.
    ///
    /// ```text
    /// {"format":1,
    ///  "message":"…",
    ///  "email":"name@example.com",                        (only if kept)
    ///  "app":{"id":"amp","name":"Makepad Amp","version":"0.1.0","build":"release aarch64"},  (only if kept)
    ///  "info":[{"key":"os","label":"OS","value":"macOS 15.5 (24F74)"}, …],  (the kept rows)
    ///  "screenshot":{"type":"image/png","width":2720,"height":1800,"data":"<base64>"}}  (only if checked)
    /// ```
    ///
    /// With "Send clipboard" checked it also carries
    /// `"clipboard":{"kind":"text","text":"…"}` or
    /// `"clipboard":{"kind":"image","type":"image/png","width":…,"height":…,"png":"<base64>"}`.
    pub fn payload(&self, message: &str, with_shot: bool) -> String {
        let mut out = String::with_capacity(1024);
        out.push_str("{\"format\":1,\"message\":");
        json_str(&mut out, message);
        let kept = |key: &str| self.items.iter().find(|item| item.key == key && item.kept);
        if let Some(from) = kept("from") {
            out.push_str(",\"email\":");
            json_str(&mut out, &from.value);
        }
        if kept("app").is_some() {
            out.push_str(",\"app\":{\"id\":");
            json_str(&mut out, &self.app_id);
            out.push_str(",\"name\":");
            json_str(&mut out, &self.app_name);
            out.push_str(",\"version\":");
            json_str(&mut out, &self.app_version);
            out.push_str(",\"build\":");
            json_str(&mut out, &build_description());
            out.push('}');
        }
        out.push_str(",\"info\":[");
        let mut first = true;
        for item in self.items.iter().filter(|item| item.kept && !matches!(item.key, "from" | "app")) {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"key\":");
            json_str(&mut out, item.key);
            out.push_str(",\"label\":");
            json_str(&mut out, item.label);
            out.push_str(",\"value\":");
            json_str(&mut out, &item.value);
            out.push('}');
        }
        out.push(']');
        if let (true, Some(shot)) = (with_shot, &self.shot) {
            out.push_str(&format!(
                ",\"screenshot\":{{\"type\":\"image/png\",\"width\":{},\"height\":{},\"data\":\"",
                shot.width, shot.height
            ));
            out.push_str(&shot.png_base64);
            out.push_str("\"}");
        }
        match &self.clip {
            ClipState::Text(text) => {
                out.push_str(",\"clipboard\":{\"kind\":\"text\",\"text\":");
                json_str(&mut out, text);
                out.push('}');
            }
            ClipState::Image(image) => {
                out.push_str(&format!(
                    ",\"clipboard\":{{\"kind\":\"image\",\"type\":\"image/png\",\"width\":{},\"height\":{},\"png\":\"",
                    image.width, image.height
                ));
                out.push_str(&image.png_base64);
                out.push_str("\"}");
            }
            ClipState::Off | ClipState::Reading(_) => {}
        }
        out.push('}');
        out
    }

    fn send(&mut self, cx: &mut Cx) {
        if matches!(self.state, PanelState::Sending | PanelState::Sent) {
            return;
        }
        let message = self.view.text_input(cx, ids!(message)).text();
        let message = message.trim();
        if message.is_empty() {
            self.set_status(cx, "Write a message first.");
            return;
        }
        if message.len() > MAX_MESSAGE_BYTES {
            self.set_status(cx, "The message is too long (60 KB at most).");
            return;
        }
        if matches!(self.clip, ClipState::Reading(_)) {
            self.set_status(cx, "Still reading the clipboard…");
            return;
        }
        let with_shot = self.view.check_box(cx, ids!(shot_check)).active(cx);
        let body = self.payload(message, with_shot);
        let url = std::env::var("MAKEPAD_FEEDBACK_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let mut request = HttpRequest::new(url, HttpMethod::POST);
        request.set_header("Content-Type".into(), "application/json".into());
        request.set_header("X-Makepad-Feedback".into(), "1".into());
        request.set_max_response_body_bytes(64 * 1024);
        request.set_body(body.into_bytes());
        self.request_id = LiveId::from_str(&format!("makepad-feedback-{}", Cx::time_now()));
        cx.http_request(self.request_id, request);
        self.state = PanelState::Sending;
        self.set_status(cx, "Sending…");
    }

    fn finished(&mut self, cx: &mut Cx, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.state = PanelState::Sent;
                self.set_status(cx, "Sent. Thank you!");
                self.view.button(cx, ids!(send)).set_visible(cx, false);
                self.view.button(cx, ids!(cancel)).set_text(cx, "Close");
            }
            Err(error) => {
                log!("feedback: not sent: {error}");
                self.set_status(cx, &format!("Not sent: {error}"));
                self.state = PanelState::Failed(error);
            }
        }
    }

    fn poll_capture(&mut self, cx: &mut Cx) {
        let result = self.capture.as_mut().and_then(|capture| capture.try_take());
        match result {
            Some(Ok(Ok(shot))) => {
                self.shot = Some(shot);
                self.show(cx);
            }
            Some(Ok(Err(error))) => {
                log!("feedback: no screenshot: {error}");
                self.show(cx);
            }
            Some(Err(error)) => {
                log!("feedback: no screenshot: {error:?}");
                self.show(cx);
            }
            None if self.capture.is_none() || Cx::time_now() - self.capture_started > CAPTURE_WAIT_SECS => {
                self.show(cx);
            }
            None => cx.redraw_all(),
        }
    }
}

impl Widget for FeedbackPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.state == PanelState::Capturing && self.capture_timer.is_event(event).is_some() {
            self.poll_capture(cx);
        }
        if self.clip_timer.is_event(event).is_some() {
            self.poll_clip(cx);
        }
        if let Event::NetworkResponses(responses) = event {
            if self.state == PanelState::Sending {
                for response in responses {
                    match response {
                        NetworkResponse::HttpResponse { request_id, response } if *request_id == self.request_id => {
                            let result = if (200..300).contains(&response.status_code) {
                                Ok(())
                            } else {
                                Err(server_error(response.status_code, response.body_string()))
                            };
                            self.finished(cx, result);
                        }
                        NetworkResponse::HttpError { request_id, error } if *request_id == self.request_id => {
                            self.finished(cx, Err(format!("no connection ({})", error.message)));
                        }
                        _ => {}
                    }
                }
            }
        }
        if self.state == PanelState::Closed || self.state == PanelState::Capturing {
            return;
        }
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };
        if self.view.modal(cx, ids!(modal)).dismissed(actions) || self.view.button(cx, ids!(cancel)).clicked(actions) {
            self.close(cx);
            return;
        }
        // Send, or Ctrl/Cmd+Return in the message field.
        if self.view.button(cx, ids!(send)).clicked(actions)
            || self.view.text_input(cx, ids!(message)).returned(actions).is_some()
        {
            self.send(cx);
        }
        // What was sent is settled: its rows no longer change.
        if matches!(self.state, PanelState::Sending | PanelState::Sent) {
            return;
        }
        if self.view.check_box(cx, ids!(shot_check)).changed(actions).is_some() {
            if self.shot.is_none() {
                self.view.check_box(cx, ids!(shot_check)).set_active(cx, false, Animate::No);
            }
            self.sync_shot(cx);
        }
        if let Some(on) = self.view.check_box(cx, ids!(clip_check)).changed(actions) {
            if on {
                self.read_clip(cx);
            } else {
                self.set_clip(cx, ClipState::Off);
            }
        }
        if self.view.button(cx, ids!(restore)).clicked(actions) {
            for item in &mut self.items {
                item.kept = true;
            }
            self.sync_rows(cx);
        }
        let removed: Vec<LiveId> = self
            .items
            .iter()
            .filter(|item| item.kept)
            .map(|item| item.row)
            .filter(|row| self.view.button(cx, &[*row, live_id!(remove)]).clicked(actions))
            .collect();
        if !removed.is_empty() {
            for item in &mut self.items {
                if removed.contains(&item.row) {
                    item.kept = false;
                }
            }
            self.sync_rows(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(window_id) = cx.get_current_window_id() {
            self.window_id = Some(window_id.id());
        }
        let step = self.view.draw_walk(cx, scope, walk);
        if self.focus_message && self.state != PanelState::Closed {
            self.focus_message = false;
            self.view.text_input(cx, ids!(message)).set_key_focus(cx);
        }
        step
    }
}

impl FeedbackPanelRef {
    /// Take the picture of the window, then show the panel.
    pub fn open(&self, cx: &mut Cx) {
        if let Some(mut panel) = self.borrow_mut() {
            panel.open(cx);
        }
    }

    /// Whether the panel is showing (or about to, while the picture is taken).
    pub fn is_open(&self) -> bool {
        self.borrow().is_some_and(|panel| panel.state != PanelState::Closed)
    }
}

/// The rows, in the order they are shown.
fn collect_info(cx: &Cx, app_name: &str, app_version: &str) -> Vec<InfoItem> {
    let mut items = Vec::new();
    let mut push = |row: LiveId, key: &'static str, label: &'static str, value: String| {
        items.push(InfoItem { row, key, label, value, kept: true });
    };
    if let Some(email) = std::env::var("MAKEPAD_FEEDBACK_EMAIL")
        .ok()
        .map(|email| email.trim().to_string())
        .filter(|email| plausible_email(email))
    {
        push(live_id!(row_from), "from", "From", email);
    }
    let mut app = app_name.to_string();
    if !app_version.is_empty() {
        app.push(' ');
        app.push_str(app_version);
    }
    push(live_id!(row_app), "app", "App", format!("{app} · {}", build_description()));
    push(live_id!(row_os), "os", "OS", system_info::os_name_and_version());
    let backend = match cx.gpu_backend() {
        GpuBackend::Metal => "Metal",
        GpuBackend::Direct3d11 => "Direct3D 11",
        GpuBackend::Vulkan => "Vulkan",
        GpuBackend::OpenGl => "OpenGL",
        #[allow(unreachable_patterns)]
        _ => "other",
    };
    let adapter = system_info::gpu_adapter_name().unwrap_or_else(|| "unknown adapter".to_string());
    push(live_id!(row_gpu), "gpu", "GPU", format!("{adapter} · {backend}"));
    push(live_id!(row_audio), "audio", "Audio", cx.audio_stack());
    let ram = system_info::physical_memory_bytes()
        .map(|bytes| format!("{} GB", (bytes as f64 / (1u64 << 30) as f64).round() as u64))
        .unwrap_or_else(|| "unknown".to_string());
    push(live_id!(row_ram), "ram", "RAM", ram);
    items
}

/// "release aarch64", "debug x86_64".
fn build_description() -> String {
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    format!("{profile} {}", std::env::consts::ARCH)
}

fn plausible_email(value: &str) -> bool {
    let mut parts = value.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && value.len() <= 254
        && value.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b'"' | b'\\'))
}

/// The server answers `{"error":"…"}` on refusal; say that, not the status.
fn server_error(status: u16, body: Option<String>) -> String {
    let message = body.and_then(|body| {
        let start = body.find("\"error\":\"")? + 9;
        let end = body[start..].find('"')?;
        Some(body[start..start + end].to_string())
    });
    match message {
        Some(message) if !message.is_empty() => message,
        _ => format!("the server answered {status}"),
    }
}

fn json_str(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// On a pool worker: read one presented frame of the window back, then
/// make the PNG that is sent (at most `SHOT_MAX_SIDE` on its long side and
/// `MAX_SCREENSHOT_BYTES` large) and the thumbnail's.
fn capture_window(window_id: Option<usize>) -> Result<Screenshot, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel::<(u32, u32, Vec<u8>)>(1);
    let taken = Arc::new(AtomicBool::new(false));
    let sink_taken = taken.clone();
    let sink = add_screen_capture(ScreenCaptureOptions { window_id, max_fps: 0.0 }, move |frame| {
        if !sink_taken.swap(true, Ordering::AcqRel) {
            let _ = sender.try_send((frame.width, frame.height, frame.rgba.to_vec()));
        }
    });
    let frame = receiver.recv_timeout(Duration::from_secs_f64(CAPTURE_WAIT_SECS - 0.5));
    remove_screen_capture(sink);
    let (width, height, mut rgba) = frame.map_err(|_| "the window presented no frame".to_string())?;
    // The drawable's alpha is not the picture's.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    prepare_image(width, height, rgba)
}

/// The PNG that is sent (at most `SHOT_MAX_SIDE` on its long side and
/// `MAX_SCREENSHOT_BYTES` large) and its thumbnail, from RGBA pixels.
fn prepare_image(width: u32, height: u32, rgba: Vec<u8>) -> Result<Screenshot, String> {
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return Err("empty image".into());
    }
    let mut side = SHOT_MAX_SIDE;
    let (png, png_width, png_height) = loop {
        let (w, h, pixels) = downscale(width, height, &rgba, side);
        let png = Cx::encode_rgba_as_png(w, h, &pixels)?;
        if png.len() <= MAX_SCREENSHOT_BYTES || side <= 640 {
            break (png, w, h);
        }
        side /= 2;
    };
    if png.len() > MAX_SCREENSHOT_BYTES {
        return Err("the picture is too large to send".into());
    }
    let (tw, th, thumb) = downscale(width, height, &rgba, THUMB_WIDTH.max(THUMB_WIDTH * height / width.max(1)));
    let thumb_png = Cx::encode_rgba_as_png(tw, th, &thumb)?;
    let png_base64 = String::from_utf8(makepad_base64::base64_encode(&png, &makepad_base64::BASE64_STANDARD))
        .map_err(|_| "base64".to_string())?;
    Ok(Screenshot {
        width: png_width,
        height: png_height,
        png_bytes: png.len(),
        png_base64,
        thumb_png,
    })
}

/// RGBA pixels from a PNG (8-bit gray, gray+alpha, RGB or RGBA).
fn decode_png(png: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    use makepad_widgets::makepad_zune_png::makepad_zune_core::{bytestream::ZCursor, options::DecoderOptions};
    use makepad_widgets::makepad_zune_png::PngDecoder;
    let options = DecoderOptions::default().set_max_width(16384).set_max_height(16384);
    let mut decoder = PngDecoder::new_with_options(ZCursor::new(png), options);
    decoder.decode_headers().map_err(|e| format!("{e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or("no size")?;
    let components = decoder.colorspace().ok_or("no colour space")?.num_components();
    let decoded = decoder.decode().map_err(|e| format!("{e:?}"))?;
    let src = decoded.u8().ok_or("not 8-bit")?;
    let pixels = width * height;
    if src.len() < pixels * components {
        return Err("short image".into());
    }
    let mut rgba = Vec::with_capacity(pixels * 4);
    for p in src.chunks_exact(components).take(pixels) {
        match components {
            4 => rgba.extend_from_slice(p),
            3 => rgba.extend_from_slice(&[p[0], p[1], p[2], 255]),
            2 => rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]),
            _ => rgba.extend_from_slice(&[p[0], p[0], p[0], 255]),
        }
    }
    Ok((width as u32, height as u32, rgba))
}

/// A box-filtered copy whose long side is at most `max_side`.
fn downscale(width: u32, height: u32, rgba: &[u8], max_side: u32) -> (u32, u32, Vec<u8>) {
    let long = width.max(height).max(1);
    if long <= max_side {
        return (width, height, rgba.to_vec());
    }
    let scale = max_side as f64 / long as f64;
    let tw = ((width as f64 * scale).round() as u32).max(1);
    let th = ((height as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        let y0 = (y as u64 * height as u64 / th as u64) as u32;
        let y1 = (((y + 1) as u64 * height as u64 / th as u64) as u32).max(y0 + 1).min(height);
        for x in 0..tw {
            let x0 = (x as u64 * width as u64 / tw as u64) as u32;
            let x1 = (((x + 1) as u64 * width as u64 / tw as u64) as u32).max(x0 + 1).min(width);
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                let row = (sy * width) as usize * 4;
                for sx in x0..x1 {
                    let o = row + sx as usize * 4;
                    for c in 0..4 {
                        sum[c] += rgba[o + c] as u32;
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let o = ((y * tw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    (tw, th, out)
}
