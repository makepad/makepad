//! The chat panel: the one widget every host shows. It owns the engine —
//! the service registry, the model, the transcript — and draws
//! `EngineState` as a transcript of user lines, assistant text, tool cards
//! (running, done, waiting for a confirm), and system lines, over a
//! composer.
//!
//! Hosts talk to it in two ways: they hand it service links
//! (`registry()`) or bus frames (`on_custom`), and they listen for
//! [`AiChatPanelAction`]s. The engine runs on the panel's own events: every
//! event pumps it, and while a turn is in flight the panel asks for the
//! next frame so streaming, deadlines and the cloud provider's polling all
//! advance without a host timer.

use crate::attach::{self, DecodeQueue, DecodeTask, Dropped, PictureDraft, PictureError, PreparedPicture, SlotState, Source};
use crate::bus::ServiceBus;
use crate::pictures::{self, ChatPictures, PictureKept};
#[cfg(feature = "gen")]
use crate::gen::GenService;
use crate::settings::AiSettings;
#[cfg(feature = "engine")]
use makepad_ai_services::engine::models::{build_model, fallback_choice, hub_generation, hub_rows, provider_rows};
use makepad_ai_services::engine::NoModelWithReason;
use makepad_ai_services::engine::{EngineCore, EngineEvent, ModelImage, PictureRoom, ReturnedPicture, SentPicture, ServiceRegistry};
use makepad_ai_services::mcp::mcpb;
use makepad_ai_services::provider::{CLAUDE_DESKTOP, PROVIDER_ENV};
use makepad_ai_services::card::{Card, CardValues, Field, MAX_FIELDS};
use makepad_ai_services::state::*;
use makepad_ai_services::progress_eta::ProgressTracker;
use makepad_ai_services::wire::{api_name, split_name, Risk, ToolDef, ToolOutcome};
use makepad_widgets::makepad_platform::file_dialogs::{FileDialog, FileDialogAction};
use makepad_widgets::tip::TipWidgetRefExt;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use makepad_widgets::ai_slot::AiSlotRequests;
use makepad_widgets::*;

/// Starting points the empty conversation shows.
pub const MAX_CHIPS: usize = 4;

/// The picture picker's dialog id.
const ATTACH_DIALOG: LiveId = live_id!(ai_chat_attach);
/// The host's pointing chips, in order.
const PICK_CHIPS: [LiveId; 6] = [live_id!(pick0), live_id!(pick1), live_id!(pick2), live_id!(pick3), live_id!(pick4), live_id!(pick5)];
/// The buttons under a finished turn, in display order.
const TURN_BUTTONS: [LiveId; 3] = [live_id!(turn_act0), live_id!(turn_act1), live_id!(turn_act2)];

fn slot_id(slot: usize) -> LiveId {
    LiveId::from_str(&format!("f{slot}"))
}

fn prepend_text(returned: &str, draft: &str) -> String {
    match (returned.is_empty(), draft.is_empty()) {
        (true, _) => draft.into(),
        (_, true) => returned.into(),
        _ => format!("{returned}\n{draft}"),
    }
}

fn log_picture_error(name: &str, detail: &str) {
    log!("aichat: {name}: {detail}");
}

fn read_original(stored: &SentPicture) -> Result<Arc<[u8]>, PictureError> {
    let path = match stored.kept_now() {
        Some(Ok(path)) => path,
        _ => match stored.data() {
            Ok(bytes) => return Ok(bytes),
            // A write may have settled between the two calls. Use the file's
            // typed I/O error if it has a path now. A cleared chat has none.
            Err(detail) => stored.kept_now().and_then(Result::ok)
                .ok_or_else(|| PictureError::rewritten("not found", detail))?,
        },
    };
    std::fs::read(path).map(Arc::from)
        .map_err(|e| PictureError::open(e.kind(), format!("its file does not read ({e})")))
}

/// Only the original needs a read. Reuse both the fitted PNG and the chip.
fn returned_picture(picture: ReturnedPicture, thumb: Option<Arc<[u8]>>) -> Result<Dropped, PictureError> {
    let stored = picture.original;
    let original = read_original(&stored)?;
    let ext = stored.file.rsplit_once('.').map(|(_, ext)| ext).unwrap_or_default();
    if makepad_ai_services::engine::picture_file(&original, ext) != stored.file {
        return Err(PictureError::rewritten("changed since it was sent", "its saved file has changed"));
    }
    let (width, height) = makepad_image_decode::image_size_by_data(&picture.for_model, Path::new("picture.png"))
        .map_err(|e| PictureError::rewritten("damaged or unreadable picture", format!("its fitted picture could not be read ({e})")))?;
    let sent = attach::Attachment { name: stored.name, png: picture.for_model, width, height, thumb };
    Ok(Dropped { original, file: stored.file, width: stored.width, height: stored.height, sent })
}

enum PictureLoad {
    Reading(TaskHandle<Result<PreparedPicture, PictureError>>),
    Decoding(DecodeTask<Result<Dropped, PictureError>>),
}

impl PictureLoad {
    fn poll(&mut self, cx: &mut Cx, name: &str) -> Option<Result<Dropped, PictureError>> {
        if let Self::Reading(handle) = self {
            let picture = match handle.try_take()?.map_err(PictureError::load).and_then(|r| r) {
                Ok(picture) => picture,
                Err(error) => return Some(Err(error)),
            };
            let name = name.to_owned();
            let pool = cx.task_pool();
            *self = Self::Decoding(cx.global::<DecodeQueue>().submit(pool, Class::Visible, picture.is_large(), move || picture.decode(&name)));
        }
        match self {
            Self::Decoding(task) => task.try_take().map(|r| r.unwrap_or_else(|e| Err(PictureError::load(e)))),
            Self::Reading(_) => None,
        }
    }

    fn cancel(self) {
        match self {
            Self::Reading(handle) => { handle.cancel(); handle.detach(); }
            Self::Decoding(task) => task.cancel(),
        }
    }
}

struct ReturningPictures {
    line: u64,
    names: Vec<String>,
    read: TaskHandle<Vec<Result<Dropped, PictureError>>>,
}

fn pictures_that_go(room: PictureRoom, attached: usize) -> usize {
    match room {
        PictureRoom::NoPictures => 0,
        PictureRoom::Fits(n) => n.min(attached),
        PictureRoom::Unknown => attached,
    }
}

fn room_notice(room: PictureRoom, attached: usize) -> Option<String> {
    let left = attached - pictures_that_go(room, attached);
    if left == 0 {
        return None;
    }
    match room {
        PictureRoom::NoPictures => Some("This model cannot look at pictures: they stay attached.".into()),
        PictureRoom::Fits(n) => {
            let pictures = if n == 1 { "picture" } else { "pictures" };
            let stay = if left == 1 { "stays" } else { "stay" };
            Some(format!("This model takes {n} {pictures} with a line: the other {left} {stay} attached for the next."))
        }
        PictureRoom::Unknown => None,
    }
}

/// Update the list when its pictures change. The file hash identifies the thumbnail across
/// draft edits, recycled transcript rows and restored conversations.
fn fill_picture_list<'a>(cx: &mut Cx, list: FlatListRef, pictures: impl Iterator<Item = (&'a str, &'a Arc<[u8]>)>) {
    let Some(mut list) = list.borrow_mut() else { return };
    for (slot, (file, png)) in pictures.enumerate() {
        if let Some(item) = list.item(cx, LiveId(slot as u64 + 1), id!(Thumb)) {
            let key = Path::new(file).with_extension("thumb.png");
            let _ = item.image(cx, ids!(pic)).load_image_from_data_async(cx, &key, png.clone());
        }
    }
    let before = list.items.len();
    list.items.retain_visible();
    if list.items.len() < before {
        cx.widget_tree_mark_dirty(list.widget_uid());
    }
}

fn draw_picture_list(cx: &mut Cx2d, scope: &mut Scope, list: &FlatList) {
    for id in 1..=list.items.len() {
        list.items[LiveId(id as u64)].widget.draw_all(cx, scope);
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.AiChatPanelBase = #(AiChatPanel::register_widget(vm))

    let Line = Label{
        width: Fill
        height: Fit
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    let Meta = Label{
        width: Fit
        height: Fit
        draw_text +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: 8.5}
        }
    }

    let Row = View{
        width: Fill
        height: Fit
        flow: Down
        padding: Inset{left: 14 right: 14 top: 4 bottom: 4}
    }

    // A bare icon button whose glyph is drawn by its shader: no picture
    // files, so every host of the panel gets the same marks.
    let GlyphButton = ButtonFlatterIcon{
        width: 26
        height: 26
        padding: 0
        margin: 0
        draw_bg +: {
            border_radius: 5.0
            face_hover: uniform(theme.color_u_1)
            ink: uniform(theme.color_text_meta)
            ink_hover: uniform(theme.color_text)
        }
    }

    // New conversation: a plus.
    let NewChatButton = GlyphButton{
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1., 1., self.rect_size.x - 2., self.rect_size.y - 2., self.border_radius)
                sdf.fill(vec4(self.face_hover.xyz, self.face_hover.w * max(self.hover, self.down)))
                let mid = self.rect_size * 0.5
                let h = 5.0
                sdf.move_to(mid.x - h, mid.y)
                sdf.line_to(mid.x + h, mid.y)
                sdf.move_to(mid.x, mid.y - h)
                sdf.line_to(mid.x, mid.y + h)
                sdf.stroke(mix(self.ink, self.ink_hover, self.hover), 1.4)
                return sdf.result
            }
        }
    }

    // One thing the host attached to the composer (a pick on the page): its kind,
    // its name, and its own remove button.
    let PickChip = RoundedView{
        visible: false
        width: Fit
        height: Fit
        flow: Right
        spacing: 4
        padding: Inset{left: 7 right: 2 top: 2 bottom: 2}
        align: Align{y: 0.5}
        draw_bg +: { color: theme.color_u_1 border_radius: 5.0 }
        pick_kind := Meta{
            text: ""
            draw_text +: { text_style: theme.font_regular{font_size: 8.0} }
        }
        pick_label := Label{
            max_lines: 1
            text: ""
            draw_text +: {
                color: theme.color_text
                text_style: theme.font_regular{font_size: 8.0}
            }
        }
        Tip{
            text: "Remove from the message"
            place: TopEnd
            pick_remove := CloseButtonSm{}
        }
    }

    let PictureList = FlatList{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        spacing: 4 wrap_spacing: 4
        scroll_bars +: {show_scroll_x: false show_scroll_y: false}
    }

    // One picture waiting in the composer, with its own remove button.
    let AttachPic = RoundedView{
        width: 108
        height: 36
        flow: Overlay
        align: Align{x: 1.0 y: 0.0}
        draw_bg +: {color: theme.color_u_1 border_radius: 5.0}
        View{
            width: Fill height: Fill align: Align{x: 0.5 y: 0.5}
            pic := Image{width: Fill height: Fill fit: ImageFit.Smallest}
        }
        placeholder := View{
            width: Fill height: Fill flow: Down spacing: 0
            padding: Inset{left: 4 right: 3 top: 0 bottom: 0}
            align: Align{y: 0.5}
            chip_name := Meta{width: Fill height: 15 padding: 0 margin: Inset{right: 16} max_lines: 1 text_overflow: TextOverflow.Ellipsis}
            chip_state := Meta{width: Fill height: 15 padding: 0 max_lines: 1}
        }
        Tip{
            text: "Remove this picture"
            pic_remove := CloseButtonSm{}
        }
    }

    // Attach a picture: a frame with a hill and a sun.
    let AttachButton = GlyphButton{
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1., 1., self.rect_size.x - 2., self.rect_size.y - 2., self.border_radius)
                sdf.fill(vec4(self.face_hover.xyz, self.face_hover.w * max(self.hover, self.down)))
                let ink = mix(self.ink, self.ink_hover, self.hover)
                let mid = self.rect_size * 0.5
                sdf.box(mid.x - 6.5, mid.y - 5., 13., 10., 2.)
                sdf.stroke(ink, 1.2)
                sdf.move_to(mid.x - 5., mid.y + 3.5)
                sdf.line_to(mid.x - 1.5, mid.y - 0.5)
                sdf.line_to(mid.x + 1., mid.y + 2.)
                sdf.line_to(mid.x + 2.5, mid.y + 0.5)
                sdf.line_to(mid.x + 5., mid.y + 3.5)
                sdf.stroke(ink, 1.2)
                sdf.circle(mid.x + 2.5, mid.y - 2.2, 1.2)
                sdf.fill(ink)
                return sdf.result
            }
        }
    }

    // Annotate: a marker pen, slanted; the lit one has the accent behind it.
    let AnnotateButton = GlyphButton{
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1., 1., self.rect_size.x - 2., self.rect_size.y - 2., self.border_radius)
                sdf.fill(vec4(self.face_hover.xyz, self.face_hover.w * max(self.hover, self.down)))
                let ink = mix(self.ink, self.ink_hover, self.hover)
                let mid = self.rect_size * 0.5
                // the barrel, from lower left to upper right, and its tip
                sdf.move_to(mid.x - 5.5, mid.y + 5.5)
                sdf.line_to(mid.x + 4.5, mid.y - 4.5)
                sdf.stroke(ink, 4.0)
                sdf.move_to(mid.x - 6.5, mid.y + 6.5)
                sdf.line_to(mid.x - 5.0, mid.y + 5.0)
                sdf.stroke(ink, 1.6)
                return sdf.result
            }
        }
    }
    let AnnotateOnButton = GlyphButton{
        draw_bg +: {
            accent: uniform(theme.color_primary)
            ink_on: uniform(theme.color_on_primary)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1., 1., self.rect_size.x - 2., self.rect_size.y - 2., self.border_radius)
                sdf.fill(mix(self.accent, mix(self.accent, #xFFFFFFFF, 0.15), self.hover))
                let mid = self.rect_size * 0.5
                sdf.move_to(mid.x - 5.5, mid.y + 5.5)
                sdf.line_to(mid.x + 4.5, mid.y - 4.5)
                sdf.stroke(self.ink_on, 4.0)
                sdf.move_to(mid.x - 6.5, mid.y + 6.5)
                sdf.line_to(mid.x - 5.0, mid.y + 5.0)
                sdf.stroke(self.ink_on, 1.6)
                return sdf.result
            }
        }
    }

    // Send: an arrow on the accent; greyed while there is nothing to send.
    let SendButton = GlyphButton{
        width: 28
        height: 28
        draw_bg +: {
            accent: uniform(theme.color_primary)
            ink_on: uniform(theme.color_on_primary)
            off: uniform(theme.color_u_2)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let mid = self.rect_size * 0.5
                let face = mix(mix(self.accent, mix(self.accent, #xFFFFFFFF, 0.15), self.hover), self.off, self.disabled)
                sdf.circle(mid.x, mid.y, min(mid.x, mid.y) - 1.0)
                sdf.fill(face)
                sdf.move_to(mid.x, mid.y + 5.)
                sdf.line_to(mid.x, mid.y - 5.)
                sdf.move_to(mid.x - 4.5, mid.y - 0.5)
                sdf.line_to(mid.x, mid.y - 5.)
                sdf.line_to(mid.x + 4.5, mid.y - 0.5)
                sdf.stroke(mix(self.ink_on, self.ink, self.disabled), 1.6)
                return sdf.result
            }
        }
    }

    // Stop the turn: a square on a ring, in place of Send while it runs.
    let StopButton = GlyphButton{
        width: 28
        height: 28
        draw_bg +: {
            ring: uniform(theme.color_u_3)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let mid = self.rect_size * 0.5
                sdf.circle(mid.x, mid.y, min(mid.x, mid.y) - 1.0)
                sdf.fill_keep(vec4(self.face_hover.xyz, self.face_hover.w * (0.6 + 0.4 * self.hover)))
                sdf.stroke(self.ring, 1.0)
                sdf.box(mid.x - 4., mid.y - 4., 8., 8., 1.5)
                sdf.fill(mix(self.ink, self.ink_hover, self.hover))
                return sdf.result
            }
        }
    }

    // Speak to the chat: a microphone. Listening turns it red and a click stops it.
    let MicButton = GlyphButton{
        width: 28
        height: 28
        draw_bg +: {
            live: uniform(theme.color_error)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1., 1., self.rect_size.x - 2., self.rect_size.y - 2., self.border_radius)
                sdf.fill(vec4(self.face_hover.xyz, self.face_hover.w * max(self.hover, self.down)))
                let ink = mix(self.ink, self.ink_hover, self.hover)
                let mid = self.rect_size * 0.5
                sdf.box(mid.x - 2.5, mid.y - 7., 5., 9., 2.5)
                sdf.stroke(ink, 1.3)
                sdf.move_to(mid.x - 5., mid.y - 1.)
                sdf.line_to(mid.x - 5., mid.y + 0.5)
                sdf.line_to(mid.x - 3., mid.y + 3.5)
                sdf.line_to(mid.x + 3., mid.y + 3.5)
                sdf.line_to(mid.x + 5., mid.y + 0.5)
                sdf.line_to(mid.x + 5., mid.y - 1.)
                sdf.move_to(mid.x, mid.y + 3.5)
                sdf.line_to(mid.x, mid.y + 7.)
                sdf.stroke(ink, 1.3)
                return sdf.result
            }
        }
    }

    // Stop listening: the microphone on a red disc, in place of the microphone while it listens.
    let MicLiveButton = GlyphButton{
        width: 28
        height: 28
        draw_bg +: {
            live: uniform(theme.color_error)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let mid = self.rect_size * 0.5
                sdf.circle(mid.x, mid.y, min(mid.x, mid.y) - 1.0)
                sdf.fill(mix(self.live, #xFFFFFFFF, 0.15 * self.hover))
                sdf.box(mid.x - 2.5, mid.y - 7., 5., 9., 2.5)
                sdf.fill_keep(#xFFFFFFFF)
                sdf.move_to(mid.x - 5., mid.y - 1.)
                sdf.line_to(mid.x - 5., mid.y + 0.5)
                sdf.line_to(mid.x - 3., mid.y + 3.5)
                sdf.line_to(mid.x + 3., mid.y + 3.5)
                sdf.line_to(mid.x + 5., mid.y + 0.5)
                sdf.line_to(mid.x + 5., mid.y - 1.)
                sdf.move_to(mid.x, mid.y + 3.5)
                sdf.line_to(mid.x, mid.y + 7.)
                sdf.stroke(#xFFFFFFFF, 1.3)
                return sdf.result
            }
        }
    }

    // A starting point in the empty conversation: a quiet full-width row.
    let StarterChip = ButtonFlat{
        visible: false
        width: Fill
        height: 30
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 10 right: 10}
        margin: 0
        text: ""
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 9.0}
        }
        draw_bg +: {
            border_radius: 6.0
            color: theme.color_u_hidden
            color_default: theme.color_u_hidden
            color_hover: theme.color_u_1
            color_down: theme.color_u_2
            border_color: theme.color_outline_variant
            border_color_hover: theme.color_outline
            border_color_down: theme.color_outline
            border_color_focus: theme.color_outline
        }
    }

    // One row of a settings card: its label and both editors; the panel
    // shows the one the field needs.
    let FieldSlot = View{
        visible: false
        width: Fill
        height: Fit
        flow: Right
        spacing: 8
        align: Align{y: 0.5}
        label := Label{
            width: 110
            text: ""
            draw_text +: {
                color: theme.color_text_meta
                text_style: theme.font_regular{font_size: 8.5}
            }
        }
        pick := DropDown{
            width: Fill
            labels: ["-"]
        }
        line := TextInput{
            width: Fill
            height: Fit
            empty_text: ""
        }
    }

    mod.widgets.AiChatPanel = set_type_default() do mod.widgets.AiChatPanelBase{
        width: Fill
        height: Fill
        flow: Down
        draw_bg +: { color: theme.color_bg_app }

        // Which model answers, how it is doing, and a fresh start.
        header := View{
            width: Fill
            height: 36
            flow: Right
            spacing: 2
            padding: Inset{left: 14 right: 8}
            align: Align{y: 0.5}
            // Ready (a green circle), working (a breathing ring), nothing
            // answers (a red square): a shape as well as a colour.
            status_dot := StatusDot{width: 10 height: 10 status: StatusKind.Success}
            provider_pick := DropDownFlat{
                width: Fit
                margin: Inset{left: 2}
                labels: ["Model"]
                draw_text +: { text_style: theme.font_regular{font_size: 9.0} }
                popup_menu +: {width: Fit}
            }
            View{width: Fill height: 1}
            Tip{
                text: "New conversation (clears this one)"
                place: BottomEnd
                new_chat := NewChatButton{}
            }
        }
        SolidView{width: Fill height: 1 draw_bg +: { color: theme.color_outline_variant }}

        // A desktop assistant is the provider: this app's tools are served to it.
        mcp_row := View{
            visible: false
            width: Fill
            height: Fit
            flow: Down
            spacing: 4
            padding: Inset{left: 14 right: 14 top: 8 bottom: 4}
            mcp_connect := Button{ text: "Connect the desktop assistant" }
            mcp_status := Label{
                width: Fill
                text: "The desktop assistant reaches this app through makepad-remote mcp."
                draw_text +: {
                    color: theme.color_text
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
            mcp_hint := Label{
                width: Fill
                text: "Type in the desktop assistant to use this app's tools."
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
        }

        // Nothing said yet: what the panel can do here, and where to start.
        empty_state := View{
            visible: false
            width: Fill
            height: Fill
            flow: Down
            spacing: 6
            padding: Inset{left: 12 right: 12 top: 16}
            empty_hint := Label{
                width: Fill
                margin: Inset{left: 2 bottom: 6}
                text: ""
                draw_text +: {
                    color: theme.color_text_meta
                    text_style: theme.font_regular{font_size: 9.0}
                }
            }
            chip0 := StarterChip{}
            chip1 := StarterChip{}
            chip2 := StarterChip{}
            chip3 := StarterChip{}
        }

        transcript := PortalList{
            width: Fill
            height: Fill
            auto_tail: true
            selectable: true
            select_all_entry_first: true

            UserRow := Row{
                padding: Inset{left: 12 right: 12 top: 12 bottom: 4}
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                    draw_bg +: { color: theme.color_u_1 border_radius: 6.0 }
                    flow: Down
                    spacing: 6
                    user_pics := View{
                        visible: false
                        width: Fill
                        height: Fit
                        line_thumbs := PictureList{
                            Thumb := View{
                                width: 64 height: 48
                                pic := Image{width: Fill height: Fill fit: ImageFit.Smallest}
                            }
                        }
                    }
                    user_text := Line{}
                    user_stopped := Meta{visible: false text: "stopped"}
                    user_picks := Meta{
                        visible: false
                        width: Fill
                        text: ""
                        draw_text +: { text_style: theme.font_regular{font_size: 8.0} }
                    }
                }
            }
            EventRow := Row{
                padding: Inset{left: 14 right: 14 top: 3 bottom: 3}
                event_title := Meta{width: Fill}
                event_text := Line{
                    draw_text +: { text_style: theme.font_regular{font_size: 9.0} }
                }
                event_meta := Label{
                    visible: false
                    width: Fill
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_code{font_size: 7.5}
                    }
                }
            }
            AssistantRow := Row{
                padding: Inset{left: 14 right: 14 top: 6 bottom: 4}
                assistant_md := Markdown{
                    width: Fill
                    height: Fit
                    // Lays out what the list's selection asks of it.
                    selectable: true
                    body: ""
                }
            }
            // One call: a dot for how it went, the tool and its arguments cut to
            // the row's width (a click shows them whole, with the answer), and Stop
            // while it runs. Under it, on a line of its own, what it is doing and,
            // when the call counts its steps, a bar under that with how far it is.
            ToolRow := Row{
                padding: Inset{left: 14 right: 12 top: 0 bottom: 0}
                tool_head := View{
                    width: Fill
                    height: 24
                    flow: Right
                    spacing: 8
                    align: Align{y: 0.5}
                    cursor: MouseCursor.Hand
                    // Done calls stay quiet: a grey circle; a failed one is
                    // a red square, a running one breathes.
                    tool_dot := StatusDot{
                        width: 9
                        height: 9
                        draw_bg +: { color_success: theme.color_outline }
                    }
                    tool_title := Label{
                        width: Fill
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    tool_stop := ButtonFlat{
                        visible: false
                        text: "Stop"
                        height: 20
                        margin: 0
                        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
                    }
                }
                tool_status := View{
                    visible: false
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 2
                    margin: Inset{left: 17 bottom: 4}
                    tool_note := Label{
                        width: Fill
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.0}
                        }
                    }
                    tool_bar_box := View{
                        visible: false
                        width: Fill
                        height: Fit
                        tool_bar := ProgressBarFlat{
                            width: Fill
                            height: 12
                            show_percent: true
                            draw_bg +: { thickness: 2.0 }
                            draw_text +: {
                                color: theme.color_text_meta
                                text_style: theme.font_regular{font_size: 8.0}
                            }
                        }
                    }
                }
                tool_problem := Label{
                    visible: false
                    width: Fill
                    max_lines: 2
                    margin: Inset{left: 17 bottom: 3}
                    draw_text +: {
                        color: mix(theme.color_error, theme.color_text_meta, 0.35)
                        text_style: theme.font_regular{font_size: 8.0}
                    }
                }
                tool_detail := RoundedView{
                    visible: false
                    width: Fill
                    height: Fit
                    margin: Inset{left: 14 top: 2 bottom: 6}
                    padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                    draw_bg +: { color: theme.color_d_1 border_radius: 4.0 }
                    tool_detail_text := Label{
                        width: Fill
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_code{font_size: 7.5}
                        }
                    }
                }
            }
            TaskRow := Row{
                padding: Inset{left: 12 right: 12 top: 4 bottom: 4}
                task_tip := Tip{
                    RoundedView{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        cursor: MouseCursor.Default
                        padding: Inset{left: 8 right: 8 top: 8 bottom: 8}
                        draw_bg +: { color: theme.color_u_1 border_color: theme.color_outline_variant border_size: 1.0 border_radius: 6.0 }
                        task_pic := Image{visible: false width: 44 height: 44 fit: ImageFit.Smallest}
                        View{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 3
                            task_title := Label{
                                width: Fill
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                draw_text +: {
                                    color: theme.color_text
                                    text_style: theme.font_regular{font_size: 9.0}
                                }
                            }
                            task_bar_row := View{
                                width: Fill
                                height: Fit
                                task_bar := ProgressBarFlat{
                                    width: Fill
                                    height: 4
                                    value: -1.0
                                    draw_bg +: { thickness: 2.0 }
                                }
                            }
                            task_meta := Label{
                                width: Fill
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                draw_text +: {
                                    color: theme.color_text_meta
                                    text_style: theme.font_regular{font_size: 8.0}
                                }
                            }
                            task_problem := Label{
                                visible: false
                                width: Fill
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                draw_text +: {
                                    color: mix(theme.color_error, theme.color_text_meta, 0.35)
                                    text_style: theme.font_regular{font_size: 8.0}
                                }
                            }
                        }
                        task_buttons := View{
                            width: Fit
                            height: Fit
                            flow: Down
                            spacing: 3
                            task_stop := ButtonFlat{visible: false text: "Stop" width: 60 height: 24}
                            task_retry := ButtonFlat{visible: false text: "Retry" width: 60 height: 24}
                            task_act0 := ButtonFlat{visible: false width: 60 height: 24}
                            task_act1 := ButtonFlat{visible: false width: 60 height: 24}
                        }
                    }
                }
            }
            ConfirmRow := Row{
                padding: Inset{left: 12 right: 12 top: 6 bottom: 6}
                RoundedView{
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: Inset{left: 10 right: 10 top: 8 bottom: 8}
                    draw_bg +: { color: theme.color_u_1 border_color: theme.color_outline_variant border_size: 1.0 border_radius: 6.0 }
                    confirm_title := Line{
                        draw_text +: { text_style: theme.font_regular{font_size: 9.0} }
                    }
                    confirm_buttons := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        margin: Inset{top: 6}
                        run_button := Button{ text: "Run" }
                        Tip{
                            text: "Run this tool without asking again until the chat is cleared"
                            run_always := ButtonFlat{ text: "Always in this chat" }
                        }
                        deny_button := ButtonFlat{ text: "Cancel" }
                    }
                }
            }
            // A parked request: the tool's arguments as editable fields.
            RequestRow := Row{
                padding: Inset{left: 12 right: 12 top: 6 bottom: 6}
                RoundedView{
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 6
                    padding: Inset{left: 12 right: 12 top: 10 bottom: 10}
                    draw_bg +: { color: theme.color_u_1 border_color: theme.color_outline_variant border_size: 1.0 border_radius: 6.0 }
                    req_title := Line{
                        draw_text +: { text_style: theme.font_bold{font_size: 9.5} }
                    }
                    req_why := Line{
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    f0 := FieldSlot{}
                    f1 := FieldSlot{}
                    f2 := FieldSlot{}
                    f3 := FieldSlot{}
                    f4 := FieldSlot{}
                    f5 := FieldSlot{}
                    f6 := FieldSlot{}
                    f7 := FieldSlot{}
                    f8 := FieldSlot{}
                    f9 := FieldSlot{}
                    f10 := FieldSlot{}
                    f11 := FieldSlot{}
                    req_problem := Line{
                        visible: false
                        draw_text +: {
                            color: theme.color_error
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    req_buttons := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        margin: Inset{top: 4}
                        req_run := Button{ text: "Make it" }
                        Tip{
                            text: "Run this tool without asking again until the chat is cleared"
                            req_always := ButtonFlat{ text: "Always in this chat" }
                        }
                        req_cancel := ButtonFlat{ text: "Cancel" }
                    }
                }
            }
            SystemRow := Row{
                padding: Inset{left: 14 right: 14 top: 4 bottom: 4}
                system_text := Line{
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_italic{font_size: 8.5}
                    }
                }
            }
            // What is happening now, as the conversation's last line:
            // thinking, writing, running a tool, loading the model (with
            // its bar), or what went wrong.
            WorkRow := Row{
                padding: Inset{left: 14 right: 14 top: 6 bottom: 8}
                spacing: 4
                work_head := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 8
                    align: Align{y: 0.5}
                    work_dot := StatusDot{width: 10 height: 10 status: StatusKind.InProgress}
                    work_label := Label{
                        width: Fill
                        max_lines: 2
                        text: ""
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                }
                load_row := View{
                    visible: false
                    width: Fill
                    height: Fit
                    padding: Inset{left: 18}
                    load_bar := ProgressBarFlat{
                        width: Fill
                        height: 4
                        value: -1.0
                        draw_bg +: { thickness: 2.0 }
                    }
                }
            }

            // A turn's actions, under its answer.
            TurnRow := Row{
                padding: Inset{left: 14 right: 14 top: 2 bottom: 8}
                turn_buttons := View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 8
                    align: Align{y: 0.5}
                    turn_act0 := ButtonFlat{visible: false width: Fit height: 24}
                    turn_act1 := ButtonFlat{visible: false width: Fit height: 24}
                    turn_act2 := ButtonFlat{visible: false width: Fit height: 24}
                    turn_why := Label{
                        width: Fill
                        max_lines: 2
                        selectable: false
                        text: ""
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.0}
                        }
                    }
                }
            }
        }

        // One box: the pictures that go along, the line, and a bar with
        // attach, what the turn is about, and Send.
        composer := RoundedView{
            width: Fill
            height: Fit
            flow: Down
            spacing: 2
            margin: Inset{left: 10 right: 10 top: 4 bottom: 10}
            padding: Inset{left: 6 right: 6 top: 6 bottom: 5}
            draw_bg +: {
                color: theme.color_inset
                border_color: theme.color_outline_variant
                border_size: 1.0
                border_radius: 8.0
            }

            // What the person picked on the page to point the model at: each line
            // sent while they are here carries them. A chip goes with its x; hovering
            // one lets the host light the thing up on the page.
            picks_row := View{
                visible: false
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true}
                spacing: 4
                margin: Inset{bottom: 2}
                align: Align{y: 0.5}
                pick0 := PickChip{}
                pick1 := PickChip{}
                pick2 := PickChip{}
                pick3 := PickChip{}
                pick4 := PickChip{}
                pick5 := PickChip{}
                picks_more := Meta{
                    visible: false
                    text: ""
                    draw_text +: { text_style: theme.font_regular{font_size: 8.0} }
                }
            }

            // Tools the person allowed for this chat: they run without a card.
            allowed_row := View{
                visible: false
                width: Fill
                height: Fit
                flow: Right
                spacing: 6
                padding: Inset{left: 4 right: 0 bottom: 2}
                align: Align{y: 0.5}
                allowed_label := Label{
                    width: Fill
                    text: ""
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_regular{font_size: 8.0}
                    }
                }
                Tip{
                    text: "Ask again before these run"
                    allowed_clear := CloseButtonSm{}
                }
            }

            // The marks drawn on the page for the next line.
            marks_row := RoundedView{
                visible: false
                width: Fill
                height: Fit
                flow: Right
                spacing: 6
                margin: Inset{bottom: 2}
                padding: Inset{left: 8 right: 2 top: 2 bottom: 2}
                align: Align{y: 0.5}
                draw_bg +: { color: theme.color_u_1 border_radius: 5.0 }
                marks_label := Label{
                    width: Fill
                    max_lines: 1
                    text: ""
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: 8.0}
                    }
                }
                Tip{
                    text: "Clear the marks"
                    place: TopEnd
                    marks_clear := CloseButtonSm{}
                }
            }

            // Pictures that go with the next line.
            attach_row := View{
                visible: false
                width: Fill
                height: Fit
                flow: Down
                spacing: 2
                padding: Inset{left: 4 right: 0 bottom: 2}
                draft_thumbs := PictureList{Thumb := AttachPic{}}
                picture_notice := Meta{visible: false width: Fill}
                View{
                    width: Fill height: Fit
                    flow: Right spacing: 8 align: Align{y: 0.5}
                    attach_label := Meta{width: Fill}
                    Tip{
                        text: "Remove the pictures"
                        attach_clear := CloseButtonSm{}
                    }
                }
            }

            input := TextInputFlat{
                width: Fill
                height: Fit
                margin: 0
                padding: Inset{left: 4 right: 4 top: 3 bottom: 3}
                is_multiline: true
                submit_on_enter: true
                empty_text: "Ask for a change or a question"
                // No well of its own: the composer box is the field.
                draw_bg +: {
                    color: theme.color_u_hidden
                    color_hover: theme.color_u_hidden
                    color_focus: theme.color_u_hidden
                    color_down: theme.color_u_hidden
                    color_empty: theme.color_u_hidden
                    border_color: theme.color_u_hidden
                    border_color_hover: theme.color_u_hidden
                    border_color_focus: theme.color_u_hidden
                    border_color_down: theme.color_u_hidden
                    border_color_empty: theme.color_u_hidden
                }
                // The prompt is a hint, not text: the meta grey in every
                // state, never the typed colour (the composer is always focused).
                draw_text +: {
                    text_style: theme.font_regular{font_size: 9.5}
                    color_empty: mod.theme.color_text_meta
                    color_empty_hover: mod.theme.color_text_meta
                    color_empty_focus: mod.theme.color_text_meta
                }
            }

            composer_bar := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 4
                align: Align{y: 0.5}
                Tip{
                    text: "Attach a picture (or drop one on the panel)"
                    place: TopStart
                    attach_button := AttachButton{}
                }
                annotate_tip := Tip{
                    visible: false
                    text: "Annotate: draw on the page with a marker for the AI (Esc to stop)"
                    place: TopStart
                    annotate_button := AnnotateButton{}
                }
                annotate_on_tip := Tip{
                    visible: false
                    text: "Annotating: drag on the page to draw (Esc or press to stop; the marks stay)"
                    place: TopStart
                    annotate_on_button := AnnotateOnButton{}
                }
                // What the turn is about: the page and what is open on it.
                // It follows the page; it is not removed by hand.
                target_row := View{
                    visible: false
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 5
                    padding: Inset{left: 2 right: 4}
                    align: Align{y: 0.5}
                    target_kind := Meta{
                        text: "Editing ·"
                        draw_text +: { text_style: theme.font_regular{font_size: 8.0} }
                    }
                    target_label := Label{
                        width: Fill
                        max_lines: 1
                        text: ""
                        draw_text +: {
                            color: theme.color_text
                            text_style: theme.font_regular{font_size: 8.0}
                        }
                    }
                }
                bar_gap := View{width: Fill height: 1}
                mic_tip := Tip{
                    text: "Speak to the chat (the headset's microphone)"
                    place: TopEnd
                    visible: false
                    mic_button := MicButton{}
                }
                mic_live_tip := Tip{
                    text: "Stop listening and write what was said"
                    place: TopEnd
                    visible: false
                    mic_live_button := MicLiveButton{}
                }
                send_tip := Tip{
                    text: "Send (Enter; Shift+Enter for a new line)"
                    place: TopEnd
                    send_button := SendButton{}
                }
                stop_tip := Tip{
                    text: "Stop (Esc)"
                    place: TopEnd
                    stop_button := StopButton{visible: false}
                }
            }
        }
    }
}

/// The composer's microphone button, as the host sets it ([`AiChatPanel::set_voice`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VoiceUi {
    /// No microphone to offer: the button is not shown.
    #[default]
    Hidden,
    /// Ready: a press starts listening.
    Idle,
    /// Listening: the button is red and a press stops it.
    Listening,
    /// What was said is being turned into text: the button waits.
    Busy,
}

/// A button the host puts under a finished answer.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TurnButton {
    /// The id the host gets back on a press.
    pub id: String,
    /// The words on the button.
    pub label: String,
    /// Whether the person can press it.
    pub enabled: bool,
    /// Why the button is off, shown beside the buttons when filled in.
    pub why: String,
}

/// A turn's buttons and the tool call that locates its answer in the transcript.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TurnActions {
    /// The person's turn as counted by the host.
    pub turn: u64,
    /// The id of a tool call the turn made.
    pub call_id: String,
    /// The buttons in display order. The panel shows the first three.
    pub actions: Vec<TurnButton>,
}

/// What the panel tells its host.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum AiChatPanelAction {
    /// A native tool's whole JSON prefix, for the host's live preview.
    ToolCallPartial { call_id: String, name: String, partial_json: String },
    /// A finished task card's action button: the service's action id, for the host to carry out.
    TaskAction { call_id: String, action: String },
    /// The person pressed the button with this id under the turn's answer.
    TurnAction { turn: u64, id: String },
    /// Esc with an empty composer and no turn in flight: the host may hide
    /// the pane.
    Close,
    /// The person removed a chip of the composer's attachments (its id): the
    /// host drops it on the page (the next target it sets has it no more).
    AttachmentRemoved(String),
    /// The pointer is over a chip of the composer's attachments (its id), or
    /// left them (None): the host may light the thing up on the page.
    AttachmentHovered(Option<String>),
    /// The microphone button was pressed (shown only while the host offers one,
    /// [`AiChatPanel::set_voice`]): the host starts or stops listening.
    VoiceToggled,
    /// The person switched the marker on (true) or off: the host draws, the panel only asks.
    Annotate(bool),
    /// The person cleared the marks from the chip.
    MarksCleared,
    /// A line is waiting to go with the marks: the host reads the page's picture and answers
    /// with [`AiChatPanel::release_marks`].
    MarksWanted,
    #[default]
    None,
}

/// A line waiting for its marks. The text goes only after the host has every mark resolved and has
/// read the picture they are drawn in: it is never taken before [`Hold::begin_release`], however
/// long the host takes to resolve.
#[derive(Debug, Default, PartialEq)]
pub struct Hold {
    text: Option<String>,
    asked: bool,
    releasing: bool,
}

impl Hold {
    /// The person sent `text` while marks exist.
    pub fn take_line(&mut self, text: String) {
        *self = Hold { text: Some(text), asked: false, releasing: false };
    }

    pub fn is_waiting(&self) -> bool {
        self.text.is_some()
    }

    pub fn releasing(&self) -> bool {
        self.releasing
    }

    /// Whether the host should be asked for the marks' picture now: once, when the line waits and
    /// no mark is still being resolved.
    pub fn wants_picture(&mut self, resolving: usize) -> bool {
        if self.text.is_some() && !self.asked && !self.releasing && resolving == 0 {
            self.asked = true;
            return true;
        }
        false
    }

    /// The host answered with the picture (or none): false when no line waits.
    pub fn begin_release(&mut self) -> bool {
        if self.text.is_none() {
            return false;
        }
        self.releasing = true;
        true
    }

    /// The line to send now (its marks are in), or to give back.
    pub fn finish(&mut self) -> Option<String> {
        self.asked = false;
        self.releasing = false;
        self.text.take()
    }
}

/// What the panel shows of the host's marks: the Annotate button (when the host offers it), the
/// marker on, how many marks there are and how many the host is still resolving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MarksUi {
    pub offered: bool,
    pub on: bool,
    pub count: usize,
    pub resolving: usize,
}

/// What a row in the transcript list shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RowAt {
    /// An entry at this index in the conversation.
    Entry(usize),
    /// Buttons at this index in the host's turn actions.
    Turn(usize),
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiChatPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The host's initial model, by provider slug (`claude-cli`,
    /// `codex-cli`, `local`, …), until the person picks one: the saved pick
    /// wins over it. `MAKEPAD_AI_PROVIDER` overrides both.
    #[live]
    provider: String,
    /// The window's F10 overlay made this panel; any other is the app's own chat.
    #[live(false)]
    in_overlay: bool,
    /// Where the input that made this panel's pending actions came from: `Some(None)` the person,
    /// `Some(Some(who))` a program driving the app (sticky once seen), `None` none. Kept because
    /// the click arrives as an action after the input's origin marker has cleared; cleared after
    /// each actions event, so it never answers a later click.
    #[rust]
    press_origin: Option<Option<String>>,
    /// Pictures waiting for the next line.
    #[rust]
    attachments: PictureDraft<PictureLoad, Dropped>,
    /// Failed submissions whose originals are being read, in failure order.
    #[rust]
    returning: VecDeque<ReturningPictures>,
    /// The last picture refusal, shown with the draft until its pictures or model change.
    #[rust]
    picture_refusal: Option<String>,
    /// The microphone button's state, set by the host.
    #[rust]
    voice: VoiceUi,
    /// The buttons the host has set for each turn.
    #[rust]
    turn_actions: Vec<TurnActions>,
    /// Changes each time the host changes the buttons.
    #[rust]
    turn_actions_rev: u64,
    /// Transcript entries and button rows in display order.
    #[rust]
    rows: Vec<RowAt>,
    /// The conversation generation, the button revision and whether the chat was busy, used to
    /// build the rows.
    #[rust]
    rows_key: Option<(u64, u64, bool)>,
    /// One footer note per failed selection, until the next send.
    #[rust]
    load_notices: Vec<String>,
    /// Pictures the model asked to see again (`chat.picture`), being fitted to it on the task
    /// pool, by call id.
    #[rust]
    fitting: Vec<(String, DecodeTask<Result<Vec<u8>, String>>)>,
    /// The small copies of a restored chat's pictures, being made on the task pool.
    #[rust]
    thumbs_loading: Vec<(String, DecodeTask<Result<Arc<[u8]>, String>>)>,
    /// Slot revision and number of originals still being returned to this draft.
    #[rust]
    attach_shown: Option<(u64, usize)>,
    /// The ids of the attachment chips as drawn, by slot, and the one the pointer is over.
    #[rust]
    pick_ids: Vec<String>,
    #[rust]
    pick_hovered: Option<String>,
    /// The host's marks on the page, as it last said.
    #[rust]
    marks: MarksUi,
    /// A line waiting for its marks: it goes once the host has them resolved and their picture.
    #[rust]
    hold: Hold,
    /// The line being sent already has its marks.
    #[rust]
    marks_through: bool,
    /// The clipboard read a paste in the composer asked for (a copied
    /// picture is attached; text is the input's own paste).
    #[rust]
    paste_request: Option<u64>,
    /// Pictures pasted so far (names them apart).
    #[rust]
    pasted: usize,
    /// The small copies of the pictures the person sent, by file, for the transcript's lines
    /// and the task cards.
    #[rust]
    thumbs: HashMap<String, Arc<[u8]>>,
    /// The entry whose pictures each transcript row last loaded, and how many of them had
    /// their small copy then.
    #[rust]
    pics_filled: HashMap<WidgetUid, (usize, usize)>,
    /// Result pictures read once per path, including missing files.
    #[rust]
    result_pics: HashMap<String, Option<Arc<[u8]>>>,
    /// The picture each recycled task row last loaded (input/result keys are distinct).
    #[rust]
    task_pics_filled: HashMap<WidgetUid, String>,
    /// Avoid rebuilding labels and the stage-history tooltip on animation-only redraws.
    #[rust]
    task_rows_filled: HashMap<WidgetUid, (String, u64)>,
    /// Each call's row line ([`AiChatPanel::tool_line`]) by call id: its arguments read once.
    #[rust]
    tool_lines: HashMap<String, String>,
    /// Stage + ETA per live task, including calls scrolled out of view.
    #[rust]
    task_tracks: HashMap<String, ProgressTracker>,
    /// Stage + ETA of the model's current load (None unless loading).
    #[rust]
    load_track: ProgressTracker,
    /// The choices the provider menu lists, in its order.
    #[rust]
    provider_rows: Vec<ProviderRow>,
    /// The hub discovery answer the menu rows were built from (only the engine follows the hub).
    #[cfg(feature = "engine")]
    #[rust]
    hub_seen: u64,
    /// The provider menu shows these labels with this one picked.
    #[rust]
    pick_shown: Option<(Vec<String>, usize)>,
    /// A line for the status row until the next send (a picture that
    /// could not be read, a provider that is not installed).
    #[rust]
    notice: Option<String>,
    #[rust]
    engine: Option<EngineCore>,
    /// Embedded panels share the process registry. An external WM pane has its own bus mirror.
    #[rust(ServiceRegistry::process())]
    registry: ServiceRegistry,
    #[rust]
    bus: ServiceBus,
    /// The assistant's own `gen` service (pictures from the fleet), joined
    /// to the registry with the engine.
    #[cfg(feature = "gen")]
    #[rust]
    gen: Option<GenService>,
    #[rust]
    settings: Option<AiSettings>,
    /// What the person has set on each parked settings card, by call id,
    /// kept here so a card that scrolls away and back keeps its edits.
    #[rust]
    card_edits: HashMap<String, CardValues>,
    /// The call and slot each field editor was last filled for, by widget.
    #[rust]
    slot_filled: HashMap<WidgetUid, (String, usize)>,
    #[rust]
    drawn_generation: u64,
    /// The state generation last published as the bridge's transcript.
    #[rust]
    published: Option<u64>,
    #[rust]
    next_frame: NextFrame,
    /// The composer took the keyboard once it existed on screen — a
    /// focus set before the first draw lands on no area at all.
    #[rust]
    composer_focused: bool,
    /// The host's key for keeping this conversation on disk ([`Self::keep_chat`]);
    /// `None` keeps nothing.
    #[rust]
    chat_key: Option<String>,
    /// The conversation's pictures moved since the app's tools were shown them ([`Self::share_pictures`]).
    #[rust]
    pictures_moved: bool,
    /// The engine generation last written to the chat file.
    #[rust]
    chat_written: Option<u64>,
    /// What [`Self::publish_unsaved`] last told the platform.
    #[rust]
    unsaved_published: Option<String>,
}

impl AiChatPanel {
    /// The pointer over an attachment chip, or off them: the host is told once per change
    /// (`AttachmentHovered`), so it can light the thing up. Per event: a hit test per chip shown.
    fn hover_picks(&mut self, cx: &mut Cx, event: &Event) {
        if self.pick_ids.is_empty() && self.pick_hovered.is_none() {
            return;
        }
        let mut over = self.pick_hovered.clone();
        for (slot, id) in PICK_CHIPS.iter().enumerate().take(self.pick_ids.len()) {
            let area = self.view.view(cx, &[*id]).area();
            match event.hits(cx, area) {
                Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => over = Some(self.pick_ids[slot].clone()),
                Hit::FingerHoverOut(_) if over.as_ref() == Some(&self.pick_ids[slot]) => over = None,
                _ => {}
            }
        }
        if over.as_ref().is_some_and(|id| !self.pick_ids.contains(id)) {
            over = None;
        }
        if over != self.pick_hovered {
            self.pick_hovered = over.clone();
            cx.widget_action(self.widget_uid(), AiChatPanelAction::AttachmentHovered(over));
        }
    }

    /// A draft or picture not sent yet, or a reply still coming, is unsaved work
    /// (`restart::set_unsaved`, per panel): a restart that ends this app waits for it, the WM
    /// session's restart for a deploy among them (one ended two chats with the WM, 11-06).
    fn publish_unsaved(&mut self, cx: &mut Cx) {
        let replying = self.engine.as_ref().is_some_and(|e| matches!(e.state().status, Status::Thinking | Status::Streaming | Status::WaitingForTool));
        let typed = !self.view.text_input(cx, ids!(input)).text().trim().is_empty()
            || self.attachments.count != 0 || self.marks.count > 0;
        let what = if replying { Some("an AI chat reply still coming") } else if typed { Some("an AI chat message not sent") } else { None };
        if what != self.unsaved_published.as_deref() {
            self.unsaved_published = what.map(str::to_string);
            makepad_widgets::makepad_platform::restart::set_unsaved(&self.unsaved_key(), what);
        }
    }

    fn unsaved_key(&self) -> String {
        format!("aichat-{}", self.widget_uid().0)
    }

    /// The registry a host plugs in-process links into.
    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
    }

    /// Keep a hosted pane's mirrored WM services private to this panel.
    /// Call before starting the engine or receiving bus registrations.
    pub fn use_private_registry(&mut self) {
        debug_assert!(self.engine.is_none(), "use_private_registry must run before ensure_engine");
        self.registry = ServiceRegistry::new();
    }

    /// A studio `Custom` frame that may be a bus frame from the WM.
    pub fn on_custom(&mut self, json: &str) -> bool {
        self.bus.on_custom(&self.registry, json)
    }

    /// The chips over the composer: each opens `tool` of `app` as a
    /// settings card filled from `prefill`, with no model turn. At most
    /// [`MAX_CHIPS`] show.
    pub fn set_suggestions(&mut self, cx: &mut Cx, suggestions: Vec<Suggestion>) {
        self.ensure_engine().set_suggestions(suggestions);
        self.view.redraw(cx);
    }

    /// The context line over the composer ("Editing · DECK — ..."): while
    /// set, the turn focuses `target.app`'s service and `target.context`
    /// goes to the model with each line. Hosts keep it current by calling
    /// this again; `None` removes the line. The person does not remove it:
    /// it says what the page in front of them is.
    pub fn set_target(&mut self, cx: &mut Cx, target: Option<Target>) {
        self.ensure_engine().set_target(target);
        self.view.redraw(cx);
    }

    /// A line in the transcript from the host: what happened in the app
    /// (a render finished, an export started). The model is not told.
    pub fn announce(&mut self, cx: &mut Cx, text: &str) {
        self.ensure_engine().announce(text);
        self.view.redraw(cx);
    }

    /// Open a settings card for `tool` of `app` now (what a chip does).
    /// Returns the call id.
    pub fn propose(&mut self, cx: &mut Cx, app: &str, tool: &str, prefill: &str) -> String {
        let now = Self::now(cx);
        let id = self.ensure_engine().propose(app, tool, prefill, now);
        self.view.redraw(cx);
        id
    }

    pub fn state(&self) -> Option<&EngineState> {
        self.engine.as_ref().map(|e| e.state())
    }

    /// The transcript JSON for the bridge's `/ai/transcript`, rewritten when
    /// the engine's state moved: every host of the panel answers it (an
    /// overlay, Stage's chat column, the WM pane), so a hidden instance's
    /// conversation can be read.
    fn publish_transcript(&mut self, cx: &mut Cx) {
        let Some(state) = self.engine.as_ref().map(|e| e.state()) else { return };
        if self.published == Some(state.generation) {
            return;
        }
        self.published = Some(state.generation);
        cx.global::<crate::overlay::AiTranscript>().json = crate::overlay::transcript_json(state);
    }

    /// Keep this conversation on disk under `key` (the host's product, `fab`):
    /// it is written after every change once the turn settles, and comes
    /// back when the app starts again (a crash, a live-patch relaunch) until
    /// the person starts a new chat. The pictures the person sends are kept
    /// beside it (`<key>.pictures/`), as they were dropped.
    pub fn keep_chat(&mut self, key: &str) {
        let key: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect();
        if key.is_empty() || self.chat_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.chat_key = Some(key);
        self.chat_written = None;
        // An engine already up takes the saved chat now if nothing was said yet.
        self.keep_engine_chat();
    }

    fn chat_path(&self) -> Option<std::path::PathBuf> {
        let key = self.chat_key.as_ref()?;
        AiSettings::path().and_then(|settings| settings.parent().map(|dir| dir.join("chats").join(format!("{key}.json"))))
    }

    /// The engine keeps the conversation under the chat's key: what was saved comes back (if
    /// nothing was said yet), with its pictures.
    fn keep_engine_chat(&mut self) {
        let Some(path) = self.chat_path() else { return };
        let Some(engine) = self.engine.as_mut() else { return };
        crate::chat_store::keep(&path, engine, std::time::SystemTime::now());
        self.pictures_moved = true;
    }

    /// The pictures of the conversation moved (sent, restored, cleared): the app's tools see
    /// them now ([`ChatPictures`]), and the small copies of those the panel has none of yet
    /// are made on the task pool.
    fn share_pictures(&mut self, cx: &mut Cx) {
        self.pictures_moved = false;
        let Some(engine) = self.engine.as_ref() else { return };
        *cx.global::<ChatPictures>() = ChatPictures::of(engine);
        if !self.thumbs_loading.is_empty() {
            // The job under way covers what it was given; the rest on its return.
            self.pictures_moved = true;
            return;
        }
        let missing: Vec<SentPicture> = engine.pictures().iter().filter(|p| !self.thumbs.contains_key(&p.file)).cloned().collect();
        if missing.is_empty() {
            return;
        }
        for picture in missing {
            let file = picture.file.clone();
            let large = attach::large_decode(picture.width as usize, picture.height as usize);
            let pool = cx.task_pool();
            let task = cx.global::<DecodeQueue>().submit(pool, Class::Background, large, move || {
                attach::thumb(&picture.data()?, &picture.name)
            });
            self.thumbs_loading.push((file, task));
        }
    }

    /// Write the conversation when it moved and no turn is running. A temp
    /// file renamed over the old one: a crash mid-write leaves the last good copy.
    fn write_chat(&mut self) {
        let Some(path) = self.chat_path() else { return };
        let Some(engine) = self.engine.as_ref() else { return };
        let state = engine.state();
        if self.chat_written == Some(state.generation) || state.status.is_busy() || matches!(state.status, Status::Loading { .. }) {
            return;
        }
        self.chat_written = Some(state.generation);
        if let Err(error) = crate::chat_store::write(&path, engine) {
            log!("aichat: could not write {} ({error})", path.display());
        }
    }

    fn settings(&mut self) -> &AiSettings {
        if self.settings.is_none() {
            // The launch names the model outright (Claude Desktop, tests).
            self.settings = Some(match std::env::var(PROVIDER_ENV).ok().filter(|s| !s.trim().is_empty()) {
                Some(slug) => AiSettings::for_provider(&slug),
                None => AiSettings::load_with_provider(&self.provider),
            });
        }
        self.settings.as_ref().unwrap()
    }

    /// The engine comes up on first use — a local model is a long load
    /// and no host pays it before the person opens the pane.
    fn ensure_engine(&mut self) -> &mut EngineCore {
        if self.engine.is_none() {
            let lease_id = self.widget_uid().0;
            let settings = self.settings().clone();
            // A saved pick wins over the host default; only an unavailable
            // default Local choice needs the usual provider fallback.
            #[cfg(feature = "engine")]
            let settings = {
                let mut settings = settings;
                if let Some(choice) = fallback_choice(&settings.provider) {
                    settings.provider = choice;
                    settings.local_only = false;
                }
                settings
            };
            // The real models ride the `engine` feature; a build without a
            // runtime (the web page) says so and keeps the tool console.
            #[cfg(feature = "engine")]
            let (model, rows): (Box<dyn makepad_ai_services::Model>, Vec<ProviderRow>) = (
                match build_model(&settings.provider, settings.local_only) {
                    Ok(m) => m,
                    Err(reason) => Box::new(NoModelWithReason::new(reason)),
                },
                provider_rows(settings.local_only),
            );
            #[cfg(not(feature = "engine"))]
            let (model, rows): (Box<dyn makepad_ai_services::Model>, Vec<ProviderRow>) =
                (Box::new(NoModelWithReason::new("this build has no model runtime")), Vec::new());
            let mut core = EngineCore::new(self.registry.clone(), model, None, lease_id);
            // The state is the panel's window into the core; the core owns
            // it, so provider facts go in through the core.
            self.provider_rows = rows.clone();
            core.set_provider_facts(settings.provider.clone(), rows, settings.local_only);
            self.engine = Some(core);
            self.keep_engine_chat();
            #[cfg(feature = "gen")]
            {
                self.gen = GenService::open(&self.registry);
            }
        }
        self.engine.as_mut().unwrap()
    }

    /// The app as Claude Desktop names it: its own service's label (not
    /// the assistant's built-in ones), else the executable's name.
    fn app_title(&self, app_id: &str) -> String {
        self.registry
            .services()
            .into_iter()
            .find(|s| s.connected && !Self::own_service(&s.id))
            .map(|s| s.label)
            .unwrap_or_else(|| app_id.to_string())
    }

    fn own_service(id: &str) -> bool {
        #[cfg(feature = "gen")]
        if id == crate::gen::SERVICE_ID {
            return true;
        }
        let _ = id;
        false
    }

    /// Write this app's Claude Desktop extension and open it: Claude
    /// Desktop shows its install dialog.
    fn connect_claude_desktop(&mut self, cx: &mut Cx) {
        let written = std::env::current_exe()
            .map_err(|e| format!("cannot find this app's executable: {e}"))
            .and_then(|exe| {
                // Match the name published by the process's remote endpoint.
                let app_id = std::env::args_os()
                    .next()
                    .as_ref()
                    .and_then(|arg| Path::new(arg).file_name())
                    .and_then(|name| name.to_str())
                    .unwrap_or("makepad")
                    .to_string();
                let tools: Vec<ToolDef> = self.registry
                    .tool_definitions()
                    .into_iter()
                    .map(|def| {
                        let (service, tool) = split_name(&def.name);
                        ToolDef::new(api_name(service, tool), def.description, def.parameters, Risk::Read)
                    })
                    .collect();
                let dir = makepad_platform::home::app_state_home().join("mcpb");
                mcpb::write(&dir, &app_id, &self.app_title(&app_id), &exe, &tools)
            })
            .and_then(|path| mcpb::open_in_claude_desktop(&path).map(|_| path));
        match written {
            Ok(_) => self.note(cx, "The desktop assistant asks to install it: click Install, then type there".into()),
            Err(e) => self.note(cx, e),
        }
    }

    fn now(cx: &Cx) -> f64 {
        cx.seconds_since_app_start()
    }

    /// The microphone button: hidden, ready, listening or waiting ([`VoiceUi`]).
    pub fn set_voice(&mut self, cx: &mut Cx, voice: VoiceUi) {
        if self.voice != voice {
            self.voice = voice;
            self.view.redraw(cx);
        }
    }

    /// Set the buttons shown under finished answers.
    pub fn set_turn_actions(&mut self, cx: &mut Cx, actions: Vec<TurnActions>) {
        if self.turn_actions != actions {
            self.turn_actions = actions;
            self.turn_actions_rev += 1;
            self.view.redraw(cx);
        }
    }

    /// What was said, written after what is in the composer (a space between), and the composer
    /// focused: the person reads it and sends it, or says more.
    pub fn dictate(&mut self, cx: &mut Cx, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let input = self.view.text_input(cx, ids!(input));
        let before = input.text();
        let joined = if before.trim().is_empty() { text.to_string() } else { format!("{} {text}", before.trim_end()) };
        input.set_text(cx, &joined);
        input.take_key_focus(cx);
        self.view.redraw(cx);
    }

    /// A line the person must read, in the chat's notice style (a microphone that cannot be used).
    pub fn notify(&mut self, cx: &mut Cx, text: String) {
        self.note(cx, text);
    }

    /// A line as if typed and sent — the bridge's `/ai?say=`.
    pub fn say(&mut self, cx: &mut Cx, text: String) {
        self.send(cx, text);
    }

    /// Exactly `text` as the person's line, for a line the app writes for them: the composer's
    /// text, pictures and marks stay as they are, and the line carries none of them. It waits
    /// behind a running turn like a typed line. The line's id, whose fate the engine logs
    /// ([`EngineState::line_log`]); refused only when no model answers.
    pub fn send_exact(&mut self, cx: &mut Cx, text: &str) -> Result<u64, &'static str> {
        let engine = self.ensure_engine();
        if !engine.state().has_model() {
            return Err("No AI model is connected");
        }
        let id = engine.send_exact(text);
        self.view.redraw(cx);
        Ok(id)
    }

    /// The current view to send with the person's next ordinary line.
    pub fn set_next_line_picture(&mut self, picture: Option<ModelImage>) -> Result<(), String> {
        self.ensure_engine().set_next_line_picture(picture)
    }

    /// A picture for the person's next line, as if dropped on the panel.
    pub fn attach(&mut self, cx: &mut Cx, source: Source) {
        self.attach_batch(cx, std::iter::once(source));
    }

    fn attach_batch(&mut self, cx: &mut Cx, sources: impl IntoIterator<Item = Source>) {
        let items = sources.into_iter().map(|source| {
            let name = source.name();
            let label = name.clone();
            let handle = cx.task_pool().submit(Class::Visible, move || PreparedPicture::read(source, &label))
                .map(PictureLoad::Reading).map_err(PictureError::load);
            (name, handle)
        });
        self.attachments.select(items, &mut self.load_notices, log_picture_error);
        self.picture_refusal = None;
        self.view.redraw(cx);
    }

    fn show_draft_pictures(&self, cx: &mut Cx, slots_changed: bool) {
        let list = self.view.flat_list(cx, ids!(draft_thumbs));
        let mut list = slots_changed.then(|| list.borrow_mut()).flatten();
        let mut labels: Vec<_> = self.returning.iter().flat_map(|line| line.names.iter())
            .map(|name| format!("{name} (returning…)")).collect();
        for slot in self.attachments.slots() {
            let status = match &slot.state {
                SlotState::Reading(_) => "reading…".to_owned(),
                SlotState::Failed(_) => "couldn't be read".to_owned(),
                SlotState::Ready(picture) => format!("{}×{}", picture.width, picture.height),
            };
            labels.push(format!("{} ({status})", slot.name));
            let Some(item) = list.as_mut().and_then(|list| list.item(cx, LiveId(slot.id), id!(Thumb))) else { continue };
            let ready = matches!(slot.state, SlotState::Ready(_));
            item.view(cx, ids!(placeholder)).set_visible(cx, !ready);
            item.image(cx, ids!(pic)).set_visible(cx, ready);
            if let SlotState::Ready(picture) = &slot.state {
                let key = Path::new(&picture.file).with_extension("thumb.png");
                let png = picture.sent.thumb.as_ref().unwrap_or(&picture.sent.png);
                let _ = item.image(cx, ids!(pic)).load_image_from_data_async(cx, &key, png.clone());
            } else {
                item.label(cx, ids!(chip_name)).set_text(cx, &slot.name);
                item.label(cx, ids!(chip_state)).set_text(cx, &status);
            }
        }
        if let Some(mut list) = list {
            let before = list.items.len();
            list.items.retain_visible();
            if list.items.len() < before { cx.widget_tree_mark_dirty(list.widget_uid()); }
        }
        self.view.label(cx, ids!(attach_label)).set_text(cx, &labels.join(", "));
    }

    /// Answer every parked card as if clicked (the bridge's `/ai?confirm=`):
    /// `run`, `always` (run, and allow the tool for the rest of the chat) or
    /// `cancel`.
    ///
    /// Only `cancel` is taken: Run and Always are the person's press in the app, so a program
    /// driving the app cannot approve its own parked call.
    pub fn answer_cards(&mut self, cx: &mut Cx, how: &str) {
        if let Err(reason) = remote_answer_allowed(how) {
            log!("ai chat: {reason}");
            self.note(cx, reason);
            return;
        }
        let now = Self::now(cx);
        let parked: Vec<String> = self.ensure_engine().state().entries.iter().filter_map(|e| match e {
            Entry::Tool(t) if matches!(t.status, ToolStatus::Confirm) => Some(t.call_id.clone()),
            _ => None,
        }).collect();
        for call_id in parked {
            let engine = self.ensure_engine();
            match how {
                "always" => {
                    let _ = engine.allow_for_chat(&call_id, None, now);
                }
                "cancel" => engine.confirm(&call_id, false, now),
                _ => engine.confirm(&call_id, true, now),
            }
        }
        self.view.redraw(cx);
    }

    /// Switch the model; the conversation restarts.
    pub fn set_provider(&mut self, cx: &mut Cx, slug: &str) {
        let choice = ProviderChoice::from_slug(slug);
        let local_only = self.ensure_engine().state().local_only;
        let _ = (&choice, local_only);
        #[cfg(feature = "engine")]
        {
            let model: Box<dyn makepad_ai_services::Model> = match build_model(&choice, local_only) {
                Ok(m) => m,
                Err(reason) => Box::new(NoModelWithReason::new(reason)),
            };
            let settings = AiSettings { provider: choice.clone(), local_only };
            if let Err(reason) = settings.allows(&choice) {
                self.note(cx, reason);
                return;
            }
            let rows = provider_rows(local_only);
            self.provider_rows = rows.clone();
            let engine = self.engine.as_mut().unwrap();
            engine.set_model(model);
            self.picture_refusal = None;
            engine.set_provider_facts(choice, rows, local_only);
            if let Err(reason) = settings.save() {
                self.note(cx, format!("Could not save the provider choice: {reason}"));
            }
            self.settings = Some(settings);
        }
        self.view.redraw(cx);
    }

    fn note(&mut self, cx: &mut Cx, text: String) {
        self.notice = Some(text);
        self.view.redraw(cx);
    }

    fn poll_loading(&mut self, cx: &mut Cx) {
        let before = self.attachments.revision;
        self.attachments.poll_batches(|name, job| job.poll(cx, name), &mut self.load_notices, log_picture_error);
        if self.attachments.revision != before {
            self.picture_refusal = None;
            self.view.redraw(cx);
        }
        if self.hold.releasing() && self.attachments.reading == 0 {
            self.finish_release(cx);
        }
    }

    /// The host's marks changed: the Annotate button and the chip show them, and a held line
    /// goes on when its marks are resolved.
    pub fn set_marks(&mut self, cx: &mut Cx, marks: MarksUi) {
        if self.marks == marks {
            return;
        }
        self.marks = marks;
        self.view.widget(cx, ids!(annotate_tip)).set_visible(cx, marks.offered && !marks.on);
        self.view.widget(cx, ids!(annotate_on_tip)).set_visible(cx, marks.offered && marks.on);
        self.view.widget(cx, ids!(marks_row)).set_visible(cx, marks.count > 0);
        let words = match (marks.count, marks.resolving) {
            (1, 0) => "1 mark goes with the next line".to_string(),
            (n, 0) => format!("{n} marks go with the next line"),
            (n, r) => format!("{n} marks, {r} still being read"),
        };
        self.view.label(cx, ids!(marks_label)).set_text(cx, &words);
        if self.hold.is_waiting() {
            if marks.count == 0 && !self.hold.releasing() {
                // cleared while the line waited: it goes as it was typed
                self.finish_release(cx);
            } else {
                self.ask_marks(cx);
            }
        }
        self.view.redraw(cx);
    }

    /// Ask the host for the marks' picture once they are resolved.
    fn ask_marks(&mut self, cx: &mut Cx) {
        if self.hold.wants_picture(self.marks.resolving) {
            cx.widget_action(self.widget_uid(), AiChatPanelAction::MarksWanted);
        }
    }

    /// The host has the marks' picture (none: the marks go as text alone): the held line goes
    /// with it.
    pub fn release_marks(&mut self, cx: &mut Cx, picture: Option<Source>) {
        if !self.hold.begin_release() {
            return;
        }
        if let Some(source) = picture {
            self.attach(cx, source);
        }
        if self.attachments.reading == 0 {
            self.finish_release(cx);
        }
    }

    fn finish_release(&mut self, cx: &mut Cx) {
        if let Some(text) = self.hold.finish() {
            // This submit was requested earlier. The input may now contain a
            // failed line returned to the composer or words typed since.
            let input = self.view.text_input(cx, ids!(input));
            let draft = input.text();
            input.set_text(cx, "");
            self.marks_through = true;
            self.send(cx, text);
            if !draft.is_empty() { input.set_text(cx, &prepend_text(&input.text(), &draft)); }
        }
    }

    /// The person gave up on the held line: it is back in the composer.
    fn cancel_held(&mut self, cx: &mut Cx) {
        if let Some(text) = self.hold.finish() {
            let input = self.view.text_input(cx, ids!(input));
            input.set_text(cx, &prepend_text(&text, &input.text()));
            self.note(cx, "not sent: the marks were not ready".into());
        }
    }

    fn return_line(&mut self, cx: &mut Cx, id: u64, text: String, pictures: Vec<ReturnedPicture>) {
        let input = self.view.text_input(cx, ids!(input));
        if !text.is_empty() {
            input.set_text(cx, &prepend_text(&text, &input.text()));
        }
        if !pictures.is_empty() {
            let names: Vec<_> = pictures.iter().map(|p| p.original.name.clone()).collect();
            let originals: Vec<_> = pictures.into_iter().map(|p| {
                let thumb = self.thumbs.get(&p.original.file).cloned();
                (p, thumb)
            }).collect();
            match cx.task_pool().submit(Class::Visible, move || originals.into_iter().map(|(p, thumb)| returned_picture(p, thumb)).collect()) {
                Ok(read) => self.returning.push_back(ReturningPictures { line: id, names, read }),
                Err(e) => self.return_read_failed(id, &names, PictureError::load(e)),
            }
        }
        self.view.redraw(cx);
    }

    fn return_read_failed(&mut self, id: u64, names: &[String], error: PictureError) {
        let names = names.join(", ");
        let reason = error.report(&names, log_picture_error);
        self.ensure_engine().return_picture_errors(id, vec![format!("{names} ({reason})")]);
    }

    fn prepend_pictures(&mut self, id: u64, names: Vec<String>, results: Vec<Result<Dropped, PictureError>>) {
        let mut returned = Vec::new();
        let mut errors = Vec::new();
        for (name, result) in names.into_iter().zip(results) {
            match result {
                Ok(picture) => returned.push(picture),
                Err(why) => {
                    let reason = why.report(&name, log_picture_error);
                    errors.push(format!("{name} ({reason})"));
                }
            }
        }
        self.attachments.prepend_ready(returned.into_iter().map(|picture| (picture.sent.name.clone(), picture)));
        self.ensure_engine().return_picture_errors(id, errors);
    }

    /// Take what the task pool finished: pictures fitted for the model, and a restored chat's
    /// small copies.
    fn poll_pictures(&mut self, cx: &mut Cx) {
        // Returned pictures belong to the draft, even if a newer submission is
        // still waiting for its marks. Let that submission go with its own chips.
        while !self.hold.is_waiting() {
            let Some(result) = self.returning.front_mut().and_then(|line| line.read.try_take()) else { break };
            let line = self.returning.pop_front().unwrap();
            line.read.detach();
            match result {
                Ok(pictures) => self.prepend_pictures(line.line, line.names, pictures),
                Err(e) => self.return_read_failed(line.line, &line.names, PictureError::load(format!("{e:?}"))),
            }
            self.view.redraw(cx);
        }
        let mut i = 0;
        while i < self.fitting.len() {
            match self.fitting[i].1.try_take() {
                Some(result) => {
                    let (call_id, _) = self.fitting.remove(i);
                    let fitted = result.unwrap_or_else(|e| Err(format!("the picture could not be read ({e:?})")));
                    self.ensure_engine().picture_fitted(&call_id, fitted);
                    self.view.redraw(cx);
                }
                None => i += 1,
            }
        }
        self.thumbs_loading.retain(|(file, task)| {
            let Some(result) = task.try_take() else { return true };
            match result.and_then(|r| r) {
                Ok(thumb) => { self.thumbs.insert(file.clone(), thumb); }
                Err(e) => log!("aichat: the chat picture is not shown ({e})"),
            }
            self.task_rows_filled.clear();
            self.view.redraw(cx);
            false
        });
    }

    fn handle_drop(&mut self, cx: &mut Cx, event: &Event) {
        if !matches!(event, Event::Drag(_) | Event::Drop(_)) {
            return;
        }
        match event.drag_hits(cx, self.view.area()) {
            DragHit::Drag(drag) => {
                let accept = drag.items.iter().any(|item| Source::from_drag_item(item).is_some());
                if let Ok(mut response) = drag.response.try_lock() {
                    *response = if accept { DragResponse::Copy } else { DragResponse::None };
                }
            }
            DragHit::Drop(drop) => {
                let sources: Vec<_> = drop.items.iter().filter_map(Source::from_drag_item).collect();
                if sources.is_empty() {
                    self.note(cx, "drop a PNG, JPEG or WebP picture".into());
                }
                self.attach_batch(cx, sources);
            }
            _ => {}
        }
    }

    /// A line that cannot go yet stays in the composer. A chip or a bridge line that is not
    /// the draft goes after it: what is already in the composer is never replaced.
    fn keep_in_composer(&mut self, cx: &mut Cx, text: &str) {
        let input = self.view.text_input(cx, ids!(input));
        let draft = input.text();
        if draft != text {
            input.set_text(cx, &prepend_text(&draft, text));
        }
        input.take_key_focus(cx);
        self.view.redraw(cx);
    }

    fn send(&mut self, cx: &mut Cx, text: String) {
        let now = Self::now(cx);
        // A returned line's pictures are still being read: this line waits behind it.
        if !self.returning.is_empty() && !self.marks_through {
            self.keep_in_composer(cx, &text);
            return;
        }
        self.notice = None;
        // A pasted path to a picture is an attachment, not a line.
        if let Some(source) = Source::from_pasted_text(&text) {
            self.attach(cx, source);
            self.view.text_input(cx, ids!(input)).set_text(cx, "");
            return;
        }
        let console = text.trim().starts_with('/');
        let room = self.ensure_engine().picture_room();
        // Enter can arrive before the disabled Send button is drawn. A line waits while any
        // chip still reads; a console command carries no pictures and never waits.
        let ready = self.attachments.ready();
        if ready.is_none() && !console {
            self.keep_in_composer(cx, &text);
            return;
        }
        let ready = ready.unwrap_or_default();
        let count = pictures_that_go(room, ready.len());
        if !self.marks_through {
            self.load_notices.clear();
        }
        // Marks on the page go with the line: it waits until the host has them resolved and
        // their picture (the host's `MarksWanted`), and the person's text is never sent without.
        if self.marks.count > 0 && !self.marks_through && !console {
            self.hold.take_line(text);
            let input = self.view.text_input(cx, ids!(input));
            input.set_text(cx, "");
            input.take_key_focus(cx);
            self.ask_marks(cx);
            self.view.redraw(cx);
            return;
        }
        self.marks_through = false;
        let mut sent = false;
        if count > 0 && !console {
            let pictures = ready.into_iter().take(count).cloned().map(Dropped::into_new).collect();
            match self.ensure_engine().send_pictures(&text, pictures, now) {
                Ok(_) => {
                    self.attachments.sent(count, |picture| {
                        self.thumbs.insert(picture.file, picture.sent.thumb.unwrap_or(picture.sent.png));
                    });
                    self.picture_refusal = None;
                    sent = true;
                    self.share_pictures(cx);
                }
                Err(e) => {
                    self.picture_refusal = Some(format!("The pictures were not sent: {}. They stay attached.", e.trim_end_matches('.')));
                }
            }
        }
        if !sent {
            // Marks whose picture the model cannot take still go, as text.
            let line = if text.trim().is_empty() && self.marks.count > 0 { "What I marked on the page.".to_string() } else { text };
            self.ensure_engine().send(&line, now);
        }
        let input = self.view.text_input(cx, ids!(input));
        input.set_text(cx, "");
        // The widget drops the keyboard on submit; a chat composer keeps
        // it, so the next line can be typed straight away.
        input.take_key_focus(cx);
        self.view.redraw(cx);
    }

    /// Fill a settings card's row from the card and the person's edits.
    /// Each editor is filled from scratch only when it was last filled for
    /// another call or slot (the list recycles rows); after that the
    /// person's own typing and picks are the truth.
    fn draw_card(&mut self, cx: &mut Cx2d, row: &WidgetRef, call_id: &str, card: &Card, values: &CardValues) {
        row.label(cx, ids!(req_title)).set_text(cx, &card.title);
        let why = row.label(cx, ids!(req_why));
        why.set_visible(cx, !card.why.is_empty());
        why.set_text(cx, &card.why);
        let problem = row.label(cx, ids!(req_problem));
        problem.set_visible(cx, card.problem.is_some());
        if let Some(text) = &card.problem {
            problem.set_text(cx, &format!("Not run: {text}"));
        }
        row.button(cx, ids!(req_run)).set_text(cx, &card.confirm);
        let (mut pick_at, mut line_at) = (0, 0);
        for slot in 0..MAX_FIELDS {
            let id = slot_id(slot);
            let view = row.view(cx, &[id]);
            let Some(field) = card.fields.get(slot) else {
                view.set_visible(cx, false);
                continue;
            };
            view.set_visible(cx, true);
            row.label(cx, &[id, id!(label)]).set_text(cx, match field {
                Field::Pick { label, .. } | Field::Line { label, .. } => label,
            });
            let dropdown = row.drop_down(cx, &[id, id!(pick)]);
            let line = row.text_input(cx, &[id, id!(line)]);
            match field {
                Field::Pick { options, .. } => {
                    dropdown.set_visible(cx, true);
                    line.set_visible(cx, false);
                    let key = (call_id.to_string(), slot);
                    if self.slot_filled.get(&dropdown.widget_uid()) != Some(&key) {
                        self.slot_filled.insert(dropdown.widget_uid(), key);
                        dropdown.set_labels(cx, options.iter().map(|(label, _)| label.clone()).collect());
                    }
                    dropdown.set_selected_item(cx, values.picks.get(pick_at).copied().unwrap_or(0));
                    pick_at += 1;
                }
                Field::Line { hint, .. } => {
                    dropdown.set_visible(cx, false);
                    line.set_visible(cx, true);
                    let key = (call_id.to_string(), slot);
                    if self.slot_filled.get(&line.widget_uid()) != Some(&key) {
                        self.slot_filled.insert(line.widget_uid(), key);
                        line.set_empty_text(cx, hint.clone());
                    }
                    let want = values.lines.get(line_at).map(String::as_str).unwrap_or("");
                    if line.text() != want {
                        line.set_text(cx, want);
                    }
                    line_at += 1;
                }
            }
        }
    }

    /// Observe live tasks once per state change, even when their rows are off screen.
    fn sync_task_tracks(&mut self, state: &EngineState) {
        let mut running = HashSet::new();
        let now = std::time::Instant::now();
        for entry in &state.entries {
            let Entry::Tool(t) = entry else { continue };
            let Some(task) = &t.task else { continue };
            let (note, permille) = match &t.status {
                ToolStatus::Running { note, permille } | ToolStatus::NotResponding { note, permille } => (note, *permille),
                _ => continue,
            };
            running.insert(t.call_id.as_str());
            let stage = if task.stage.is_empty() { note.as_str() } else { &task.stage };
            // A step counter is progress within a stage, not a new ETA clock on every step.
            // It also distinguishes a known zero share from an indeterminate stage.
            let counter = stage.rsplit_once(' ').filter(|(_, count)| {
                count.split_once('/').is_some_and(|(step, total)| step.parse::<u64>().is_ok() && total.parse::<u64>().is_ok_and(|n| n > 0))
            });
            let phase = counter.map(|(phase, _)| phase.trim_end_matches([' ', '·'])).unwrap_or(stage);
            let fraction = if permille > 0 || counter.is_some() { permille as f64 / 1000.0 } else { -1.0 };
            self.task_tracks.entry(t.call_id.clone()).or_default().observe(phase, fraction, now);
        }
        self.task_tracks.retain(|call_id, _| running.contains(call_id.as_str()));
    }

    /// The small copy of the picture sent in this chat under `name`.
    fn named_thumb(&self, name: &str) -> Option<&Arc<[u8]>> {
        let file = &self.engine.as_ref()?.pictures().iter().find(|p| p.name == name)?.file;
        self.thumbs.get(file)
    }

    /// Fill one compact task card; only a changed picture is decoded for a recycled row.
    fn draw_task(&mut self, cx: &mut Cx2d, row: &WidgetRef, t: &ToolEntry) {
        let Some(task) = &t.task else { return };
        let uid = row.widget_uid();
        if self.task_rows_filled.get(&uid).is_some_and(|(call_id, generation)| call_id == &t.call_id && *generation == self.drawn_generation) {
            return;
        }
        let progress = self.task_tracks.get(&t.call_id).and_then(ProgressTracker::current);
        let running = matches!(t.status, ToolStatus::Running { .. } | ToolStatus::NotResponding { .. });
        let done_ok = matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. });
        let stopped = matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Cancelled, .. });
        let failed = matches!(t.status, ToolStatus::Done { .. }) && !done_ok && !stopped;
        let title = if task.source.is_empty() || (done_ok && !task.result.is_empty()) {
            task.title.clone()
        } else {
            format!("{} · {}", task.title, task.source)
        };
        row.label(cx, ids!(task_title)).set_text(cx, &title);
        // Running: "shape · 7/21 · 18% · a node · about 4 min left" (the time last, so a
        // narrow card cuts the least useful part).
        let mut time = String::new();
        let mut meta = match &t.status {
            ToolStatus::Running { note, permille } => {
                let stage = if task.stage.is_empty() { note } else { &task.stage };
                let mut line = if stage.is_empty() { progress.as_ref().map(|shown| shown.stage.clone()).unwrap_or_default() } else { stage.clone() };
                if *permille > 0 {
                    line.push_str(&format!(" · {}%", permille / 10));
                }
                if let Some(shown) = &progress {
                    time = shown.time_note();
                }
                line
            }
            ToolStatus::NotResponding { note, .. } => {
                let stage = if task.stage.is_empty() { note } else { &task.stage };
                if stage.is_empty() { "not responding".into() } else { format!("{stage} · not responding") }
            }
            ToolStatus::Done { outcome: ToolOutcome::Cancelled, .. } => "Stopped".into(),
            ToolStatus::Done { note, .. } if done_ok => if task.result.is_empty() { note.clone() } else { task.result.clone() },
            // A failure: the card's own reason when the service gave one.
            ToolStatus::Done { .. } if !task.result.is_empty() => task.result.clone(),
            ToolStatus::Done { outcome, note, text } => {
                let reason = if note.is_empty() { text } else { note };
                reason.lines().next().filter(|line| !line.is_empty()).unwrap_or(outcome.slug()).to_string()
            }
            ToolStatus::Confirm => String::new(),
        };
        if !task.host.is_empty() && (running || done_ok) {
            if !meta.is_empty() {
                meta.push_str(" · ");
            }
            meta.push_str(&task.host);
        }
        meta.push_str(&time);
        let meta_label = row.label(cx, ids!(task_meta));
        meta_label.set_visible(cx, !failed);
        meta_label.set_text(cx, if failed { "" } else { &meta });
        let problem = row.label(cx, ids!(task_problem));
        problem.set_visible(cx, failed);
        problem.set_text(cx, if failed { &meta } else { "" });
        let bar = row.progress_bar(cx, ids!(task_bar));
        row.view(cx, ids!(task_bar_row)).set_visible(cx, running);
        if running {
            let value = progress.and_then(|shown| shown.fraction).unwrap_or(-1.0);
            if bar.value() != value {
                bar.set_value(cx, value);
            }
        }
        row.button(cx, ids!(task_stop)).set_visible(cx, running);
        row.button(cx, ids!(task_retry)).set_visible(cx, failed);
        for (slot, id) in [id!(task_act0), id!(task_act1)].iter().enumerate() {
            let button = row.button(cx, &[*id]);
            let action = task.actions.get(slot).filter(|_| done_ok);
            button.set_visible(cx, action.is_some());
            button.set_text(cx, action.map(|action| action.label.as_str()).unwrap_or(""));
        }
        row.view(cx, ids!(task_buttons)).set_visible(cx, running || failed || (done_ok && !task.actions.is_empty()));

        let mut tip = task.source.clone();
        if !task.stages.is_empty() {
            if !tip.is_empty() {
                tip.push('\n');
            }
            tip.push_str(&task.stages.join(" → "));
        }
        if !task.host.is_empty() {
            if !tip.is_empty() {
                tip.push('\n');
            }
            tip.push_str("on ");
            tip.push_str(&task.host);
        }
        row.tip(cx, ids!(task_tip)).set_text(&tip);

        // The input: a picture sent in this chat by name, else a picture file (read once).
        if !task.picture.is_empty() && self.named_thumb(&task.picture).is_none() && !self.result_pics.contains_key(&task.picture) {
            let png = Path::new(&task.picture).is_file().then(|| std::fs::read(&task.picture).ok()).flatten().map(Arc::<[u8]>::from);
            self.result_pics.insert(task.picture.clone(), png);
        }
        // A missing result is remembered too, and falls back to the input picture.
        if done_ok && !task.result_picture.is_empty() && !self.result_pics.contains_key(&task.result_picture) {
            let png = std::fs::read(&task.result_picture).ok().map(Arc::<[u8]>::from);
            self.result_pics.insert(task.result_picture.clone(), png);
        }
        let result = if done_ok && !task.result_picture.is_empty() {
            self.result_pics.get(&task.result_picture).and_then(Option::as_ref)
        } else {
            None
        };
        let input = self.named_thumb(&task.picture).or_else(|| self.result_pics.get(&task.picture).and_then(Option::as_ref));
        let picture = result.map(|png| ("result:", task.result_picture.as_str(), png)).or_else(|| {
            input.filter(|_| !task.picture.is_empty()).map(|png| ("input:", task.picture.as_str(), png))
        });
        let image = row.image(cx, ids!(task_pic));
        image.set_visible(cx, picture.is_some());
        if let Some((prefix, key, png)) = picture {
            if self.task_pics_filled.get(&uid).and_then(|last| last.strip_prefix(prefix)) != Some(key) {
                let _ = image.load_image_from_data(cx, png);
                self.task_pics_filled.insert(uid, format!("{prefix}{key}"));
            }
        } else {
            self.task_pics_filled.remove(&uid);
        }
        self.task_rows_filled.insert(uid, (t.call_id.clone(), self.drawn_generation));
    }

    /// While the model loads, the status row reads "Loading weights · 42% ·
    /// about 12 s left" and a bar under it fills (a sweep when the stage
    /// reports no progress). Any other state hides the bar and clears the clock.
    /// While the model loads, the work line reads "Loading weights · 42% ·
    /// about 12 s left" and its bar fills (a sweep when the stage reports no
    /// progress): the line and the bar's value. Any other state keeps the
    /// plain line, no bar, and clears the clock.
    fn load_line(&mut self, state: &EngineState, plain: String) -> (String, Option<f64>) {
        match (&state.status, &self.notice) {
            (Status::Loading { phase, fraction }, None) => {
                let shown = self.load_track.observe(phase, if *fraction > 0.0 { *fraction } else { -1.0 }, std::time::Instant::now());
                (shown.line(), Some(shown.fraction.unwrap_or(-1.0)))
            }
            _ => {
                if self.load_track.current().is_some() {
                    self.load_track.reset();
                }
                (plain, None)
            }
        }
    }

    fn footer_line(state: &EngineState, notice: Option<&String>) -> (Option<String>, bool) {
        // Engine failures already have a transcript row and the failed status dot.
        match notice {
            Some(text) => (Some(text.clone()), true),
            None => (Self::work_line(state), matches!(state.status, Status::Error(_))),
        }
    }

    /// The line at the end of the conversation while something happens:
    /// `None` when idle.
    fn work_line(state: &EngineState) -> Option<String> {
        let rate = state.rate.map(|r| format!(" · {r:.0} tok/s")).unwrap_or_default();
        match &state.status {
            Status::Idle | Status::Error(_) => None,
            Status::Loading { phase, fraction } => Some(format!("Loading the model · {phase} {:.0}%", fraction * 100.0)),
            Status::Thinking => {
                let waited = match state.model_secs {
                    0 => String::new(),
                    secs if secs < 60 => format!(" {secs} s"),
                    secs => format!(" {} min {} s", secs / 60, secs % 60),
                };
                if !state.model_note.is_empty() {
                    Some(format!("{}…{waited}", state.model_note))
                } else if state.thinking.is_empty() {
                    Some(format!("Thinking…{waited}"))
                } else {
                    let tail: String = state.thinking.chars().rev().take(140).collect::<Vec<_>>().into_iter().rev().collect();
                    Some(format!("Thinking… {}", tail.split_whitespace().collect::<Vec<_>>().join(" ")))
                }
            }
            Status::Streaming => Some(format!("Writing{rate}")),
            Status::WaitingForTool => state.entries.iter().rev().find_map(|entry| match entry {
                Entry::Tool(t) => match t.status {
                    // The newest active call's own row (or card) says it runs, with its
                    // Stop and how far it is: no second line for it.
                    ToolStatus::Running { .. } | ToolStatus::NotResponding { .. } => Some(None),
                    ToolStatus::Confirm => Some(Some(format!("{} waits for your answer", t.tool))),
                    _ => None,
                },
                _ => None,
            }).flatten(),
        }
    }

    /// Place each turn's buttons after its answer when the transcript or the buttons change.
    fn update_rows(&mut self, state: &EngineState) {
        // busy is in the key: a turn's buttons go after its end, known once the chat is not busy,
        // and the status can settle without a new entry (a tool answered a frame later)
        let key = (state.generation, self.turn_actions_rev, state.status.is_busy());
        if self.rows_key == Some(key) {
            return;
        }
        let mut turns_after = HashMap::new();
        if !self.turn_actions.is_empty() {
            // Walking back finds the last matching call and the user line that follows it.
            let mut end = (!state.status.is_busy()).then_some(state.entries.len());
            let mut end_by_call = HashMap::new();
            for (at, entry) in state.entries.iter().enumerate().rev() {
                match entry {
                    Entry::User { .. } => end = Some(at),
                    Entry::Tool(tool) => { end_by_call.entry(tool.call_id.as_str()).or_insert(end); }
                    _ => {}
                }
            }
            for (k, turn) in self.turn_actions.iter().enumerate() {
                if !turn.actions.is_empty() {
                    if let Some(&Some(end)) = end_by_call.get(turn.call_id.as_str()) {
                        turns_after.entry(end - 1).or_insert_with(Vec::new).push(k);
                    }
                }
            }
        }
        self.rows.clear();
        for at in 0..state.entries.len() {
            self.rows.push(RowAt::Entry(at));
            if let Some(turns) = turns_after.remove(&at) {
                self.rows.extend(turns.into_iter().map(RowAt::Turn));
            }
        }
        self.rows_key = Some(key);
    }

    /// An entry as plain text, for a copy of a row that is not drawn.
    fn entry_text(entry: &Entry) -> String {
        match entry {
            Entry::User { text, .. } | Entry::Assistant { text, .. } | Entry::System { text } => text.clone(),
            Entry::Event(e) => e.text.clone(),
            Entry::Tool(t) => match &t.task {
                Some(task) => {
                    let detail = match &t.status {
                        ToolStatus::Done { note, .. } => if task.result.is_empty() { note } else { &task.result },
                        ToolStatus::Running { note, .. } | ToolStatus::NotResponding { note, .. } => if task.stage.is_empty() { note } else { &task.stage },
                        ToolStatus::Confirm => &task.stage,
                    };
                    format!("{} · {} — {detail}", task.title, task.source)
                }
                None => Self::tool_line(t),
            },
        }
    }

    /// A call in one line: the tool without its service's name (the context
    /// line already says where it runs), then its arguments ([`args_line`]);
    /// a row cuts it to its width.
    fn tool_line(t: &ToolEntry) -> String {
        let args = args_line(&t.args);
        if args.is_empty() { t.tool.clone() } else { format!("{}  {args}", t.tool) }
    }

    /// Open the platform's picture picker; the answer arrives as a
    /// `FileDialogAction` in `handle_actions`.
    fn pick_pictures(&mut self, cx: &mut Cx) {
        let dialog = FileDialog::new()
            .set_id(ATTACH_DIALOG)
            .set_title("Attach a picture".to_string())
            .set_multiple(true)
            .add_filter("Pictures".to_string(), vec!["png".into(), "jpg".into(), "jpeg".into(), "webp".into(), "heic".into()]);
        cx.open_select_file_dialog(dialog);
    }
}

/// The most of a call's arguments a row's line holds: more than any row is wide.
const ARGS_LINE_CHARS: usize = 240;

/// A call's arguments as plain `key: value` text on one line: `views: [front,
/// "35,0"], focus: {object: a/b}`. A string goes bare unless it holds what
/// separates the values; arguments that are no JSON go as they are.
fn args_line(args: &str) -> String {
    use makepad_strict_json::Value;
    fn value(v: &Value, out: &mut String) {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(i) => out.push_str(&i.to_string()),
            Value::F64(f) => out.push_str(&f.to_string()),
            Value::Str(s) if s.is_empty() || s.trim() != s || s.contains([',', ':', '[', ']', '{', '}', '"']) => {
                out.push('"');
                out.push_str(&s.replace('"', "\\\""));
                out.push('"');
            }
            Value::Str(s) => out.push_str(s),
            Value::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    value(item, out);
                }
                out.push(']');
            }
            Value::Obj(fields) => {
                out.push('{');
                pairs(fields, out);
                out.push('}');
            }
        }
    }
    fn pairs(fields: &[(String, Value)], out: &mut String) {
        for (i, (key, v)) in fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(key);
            out.push_str(": ");
            value(v, out);
        }
    }
    let mut out = String::new();
    match makepad_strict_json::parse(args.trim().as_bytes()) {
        Ok(Value::Obj(fields)) => pairs(&fields, &mut out),
        Ok(other) => value(&other, &mut out),
        Err(_) => out.push_str(args.trim()),
    }
    let mut line: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((at, _)) = line.char_indices().nth(ARGS_LINE_CHARS) {
        line.truncate(at);
        line.push('…');
    }
    line
}

impl Drop for AiChatPanel {
    fn drop(&mut self) {
        // The engine only enqueues Unsubscribe frames. Flush them while the
        // hosted bus and registry still exist; field destruction is too late.
        if let Some(engine) = self.engine.as_mut() {
            engine.shutdown();
        }
        self.bus.relay_down(&self.registry);
        // a closed panel holds no restart
        if self.unsaved_published.is_some() {
            makepad_widgets::makepad_platform::restart::set_unsaved(&self.unsaved_key(), None);
        }
    }
}

impl Widget for AiChatPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.bus.set_context(cx);
        if let Event::Custom(json) = event {
            self.on_custom(json);
        }
        cx.global::<DecodeQueue>().pump();
        self.handle_drop(cx, event);
        if self.attachments.reading != 0 {
            self.poll_loading(cx);
        }
        self.hover_picks(cx, event);
        // Esc in the composer: stop a running turn; with nothing running
        // and an empty composer, ask the host to hide the pane. An Esc
        // meant for the host (leaving a presentation, a menu) is not ours.
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::KeyV && ke.modifiers.is_primary() && cx.has_key_focus(self.view.text_input(cx, ids!(input)).area()) {
                self.paste_request = Some(cx.read_clipboard());
            }
        }
        // A copy takes what is selected, rows scrolled away included.
        if let Event::TextCopy(copy) = event {
            let list = self.view.portal_list(cx, ids!(transcript));
            if list.has_selection() && cx.has_key_focus(list.area()) && copy.response.borrow().is_none() {
                if let Some(engine) = &self.engine {
                    let state = engine.state();
                    let text = list.selected_text_with(&|i| match self.rows.get(i)? {
                        RowAt::Entry(at) => state.entries.get(*at).map(Self::entry_text),
                        RowAt::Turn(_) => None,
                    });
                    if !text.is_empty() {
                        *copy.response.borrow_mut() = Some(text);
                    }
                }
            }
        }
        if let Event::Clipboard(reply) = event {
            if self.paste_request == Some(reply.request) {
                self.paste_request = None;
                self.pasted += 1;
                self.attach_batch(cx, Source::from_clipboard(&reply.content, self.pasted));
            }
        }
        if let Event::KeyDown(ke) = event {
            let composing = cx.has_key_focus(self.view.text_input(cx, ids!(input)).area());
            if ke.key_code == KeyCode::Escape && composing {
                let busy = self.engine.as_ref().map(|e| e.state().status.is_busy()).unwrap_or(false);
                let empty = self.view.text_input(cx, ids!(input)).text().trim().is_empty();
                if self.hold.is_waiting() {
                    self.cancel_held(cx);
                    self.view.redraw(cx);
                } else if self.marks.on {
                    self.marks.on = false;
                    cx.widget_action(self.widget_uid(), AiChatPanelAction::Annotate(false));
                    self.view.widget(cx, ids!(annotate_tip)).set_visible(cx, self.marks.offered);
                    self.view.widget(cx, ids!(annotate_on_tip)).set_visible(cx, false);
                    self.view.redraw(cx);
                } else if busy {
                    let now = Self::now(cx);
                    self.ensure_engine().cancel(now);
                    self.view.redraw(cx);
                } else if empty {
                    cx.widget_action(self.widget_uid(), AiChatPanelAction::Close);
                }
            }
        }
        // The built-in services answer before the engine pumps, so a
        // finished picture lands in this same event.
        #[cfg(feature = "gen")]
        if let Some(gen) = self.gen.as_mut() {
            gen.handle_event(cx, event);
        }
        // An app started for Claude Desktop runs its engine from the start,
        // with the pane closed.
        // A panel the app placed itself is the app's one chat: the window's F10 slot makes
        // no second one (two engines would answer one window).
        if !self.in_overlay && !cx.global::<AiSlotRequests>().docked {
            cx.global::<AiSlotRequests>().docked = true;
        }
        if self.engine.is_none() && cx.global::<AiSlotRequests>().engine_at_start {
            self.ensure_engine();
        }
        // Discovery of the AI hub landed (or was asked again): the menu's
        // hub row follows it.
        #[cfg(feature = "engine")]
        if self.engine.is_some() && self.hub_seen != hub_generation() {
            self.hub_seen = hub_generation();
            let local_only = self.engine.as_ref().is_some_and(|e| e.state().local_only);
            let rows = hub_rows(local_only);
            // The hub's rows (the election's and one per other model) are
            // one run in the menu: replaced together.
            if let Some(at) = self.provider_rows.iter().position(|r| r.choice.is_hub()) {
                let end = at + self.provider_rows[at..].iter().take_while(|r| r.choice.is_hub()).count();
                if self.provider_rows[at..end] != rows[..] {
                    self.provider_rows.splice(at..end, rows);
                    let engine = self.engine.as_mut().unwrap();
                    let (provider, local_only) = (engine.state().provider.clone(), engine.state().local_only);
                    engine.set_provider_facts(provider, self.provider_rows.clone(), local_only);
                    self.view.redraw(cx);
                }
            }
        }
        if self.pictures_moved {
            self.share_pictures(cx);
        }
        self.poll_pictures(cx);
        // Drive the engine on every event; the bus relays what it sent.
        let now = Self::now(cx);
        let mut changed = false;
        let mut busy = false;
        let uid = self.widget_uid();
        let mut wanted = Vec::new();
        let mut keep = Vec::new();
        let mut returned = Vec::new();
        if let Some(engine) = self.engine.as_mut() {
            for ev in engine.pump(now) {
                match ev {
                    EngineEvent::Changed => changed = true,
                    EngineEvent::Confirm { .. } => changed = true,
                    EngineEvent::ToolCallPartial { call_id, name, partial_json } => {
                        cx.widget_action(uid, AiChatPanelAction::ToolCallPartial { call_id, name, partial_json });
                    }
                    EngineEvent::PictureWanted { call_id, picture, rect } => wanted.push((call_id, picture, rect)),
                    EngineEvent::KeepPicture(write) => keep.push(write),
                    EngineEvent::ReturnLine { id, text, pictures } => returned.push((id, text, pictures)),
                }
            }
            busy = engine.needs_pump() || engine.state().status.is_busy();
        }
        for (id, text, pictures) in returned {
            self.return_line(cx, id, text, pictures);
        }
        busy |= !self.returning.is_empty() && !self.hold.is_waiting();
        // The pool's picture jobs are polled on the frames they ask for.
        busy |= !self.thumbs_loading.is_empty();
        // A picture the person sent is written to its file on the task pool, and its end wakes
        // the app: what waits for the file (the app's picture tools) goes on then.
        for write in keep {
            let refused = write.clone();
            match cx.task_pool().submit(Class::Visible, move || {
                write.run();
                Cx::post_action(PictureKept);
            }) {
                Ok(handle) => handle.detach(),
                Err(e) => refused.abandon(format!("the task pool did not take its write ({e:?})")),
            }
        }
        // A picture the model asks to see again: read and fitted on the task pool.
        for (call_id, picture, rect) in wanted {
            let large = attach::large_decode(picture.width as usize, picture.height as usize);
            let pool = cx.task_pool();
            let task = cx.global::<DecodeQueue>().submit(pool, Class::Visible, large, move || pictures::fit_for_model(&picture, rect));
            self.fitting.push((call_id, task));
        }
        self.bus.relay_down(&self.registry);
        if changed {
            self.view.redraw(cx);
        }
        self.publish_transcript(cx);
        self.write_chat();
        if busy {
            self.next_frame = cx.new_next_frame();
        }
        // The confirmation sheet's rule: a click counts as the person's only when the input event
        // that made it was. Run and Always read their click later, from the actions, so the
        // origin of the event that made those actions is kept until then.
        let made = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        if !made.is_empty() {
            if !matches!(event, Event::Actions(_)) {
                self.press_origin = note_press(self.press_origin.take(), cx.injected_by());
            }
            cx.extend_actions(made);
        }
        self.widget_match_event(cx, event, scope);
        // A press's origin answers only the actions that follow it, never a later click.
        if matches!(event, Event::Actions(_)) { self.press_origin = None; }
        self.publish_unsaved(cx);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The engine exists once the panel is on screen.
        let room = self.ensure_engine().picture_room();
        let state = self.engine.as_ref().unwrap().state().clone();
        self.update_rows(&state);
        if self.drawn_generation != state.generation {
            self.sync_task_tracks(&state);
        }
        self.drawn_generation = state.generation;
        let mcp_row = self.view.view(cx, ids!(mcp_row));
        mcp_row.set_visible(cx, state.provider.slug() == CLAUDE_DESKTOP);

        // The header's dot: ready, working, or nothing answers.
        let silent = matches!(state.status, Status::Error(_)) || !state.has_model();
        let working = state.status.is_busy();
        let dot = if silent {
            StatusKind::Error
        } else if working {
            StatusKind::InProgress
        } else {
            StatusKind::Success
        };
        self.view.widget(cx, ids!(status_dot)).set_text(cx, dot.name());

        let (line, failed) = Self::footer_line(&state, self.notice.as_ref());
        let show_work = line.is_some() || failed;
        let (line, bar) = self.load_line(&state, line.unwrap_or_default());
        let work = show_work.then_some((line, bar, failed));

        // Send turns into Stop while a turn runs; with nothing typed, no
        // picture and no marks it is greyed. A greyed Send takes no press, so
        // this is the same test the press makes before sending.
        let busy = state.status.is_busy();
        self.view.button(cx, ids!(send_button)).set_visible(cx, !busy);
        self.view.button(cx, ids!(stop_button)).set_visible(cx, busy);
        let returning = self.returning.iter().map(|line| line.names.len()).sum::<usize>();
        let attached = self.attachments.count + returning;
        let count = pictures_that_go(room, attached);
        let draft = self.view.text_input(cx, ids!(input)).text();
        let typed = !draft.trim().is_empty() || count > 0 || self.marks.count > 0;
        let ready = returning == 0 && (draft.trim_start().starts_with('/') || self.attachments.reading == 0);
        self.view.widget(cx, ids!(send_button)).set_disabled(cx, !(typed && ready));
        self.view.widget(cx, ids!(mic_tip)).set_visible(cx, matches!(self.voice, VoiceUi::Idle | VoiceUi::Busy));
        self.view.widget(cx, ids!(mic_button)).set_disabled(cx, self.voice == VoiceUi::Busy);
        self.view.widget(cx, ids!(mic_live_tip)).set_visible(cx, self.voice == VoiceUi::Listening);

        let allowed: Vec<String> = self.engine.as_ref().map(|engine| engine.allowed().map(str::to_string).collect()).unwrap_or_default();
        self.view.view(cx, ids!(allowed_row)).set_visible(cx, !allowed.is_empty());
        if !allowed.is_empty() {
            self.view.label(cx, ids!(allowed_label)).set_text(cx, &format!("Runs without asking in this chat: {}", allowed.join(", ")));
        }

        let shown = (self.attachments.revision, returning);
        if self.attach_shown != Some(shown) {
            let slots_changed = self.attach_shown.is_none_or(|(revision, _)| revision != self.attachments.revision);
            self.attach_shown = Some(shown);
            self.view.view(cx, ids!(attach_row)).set_visible(cx, attached > 0);
            self.show_draft_pictures(cx, slots_changed);
        }
        let capacity = room_notice(room, attached);
        let notice = self.picture_refusal.as_deref().or(capacity.as_deref());
        let label = self.view.label(cx, ids!(picture_notice));
        label.set_visible(cx, notice.is_some());
        if let Some(text) = notice {
            label.set_text(cx, text);
        }

        // Nothing said yet: a hint and the host's starting points instead
        // of an empty list.
        let empty = state.entries.is_empty() && work.is_none();
        self.view.view(cx, ids!(empty_state)).set_visible(cx, empty);
        self.view.portal_list(cx, ids!(transcript)).set_visible(cx, !empty);
        if empty {
            let hint = match (&state.target, state.suggestions.is_empty()) {
                (_, false) => "Ask for a change or a question, or start from one of these:",
                (Some(_), true) => "Ask for a change or a question about what is open here.",
                (None, true) => "Ask for a change or a question.",
            };
            self.view.label(cx, ids!(empty_hint)).set_text(cx, hint);
            for i in 0..MAX_CHIPS {
                let chip = self.view.button(cx, &[LiveId::from_str(&format!("chip{i}"))]);
                match state.suggestions.get(i) {
                    Some(s) => {
                        chip.set_visible(cx, true);
                        chip.set_text(cx, &s.label);
                    }
                    None => chip.set_visible(cx, false),
                }
            }
        }
        self.view.view(cx, ids!(target_row)).set_visible(cx, state.target.is_some());
        self.view.view(cx, ids!(bar_gap)).set_visible(cx, state.target.is_none());
        if let Some(target) = &state.target {
            self.view.label(cx, ids!(target_label)).set_text(cx, &target.label);
        }
        let picks: &[Attachment] = state.target.as_ref().map_or(&[], |target| &target.attachments);
        self.view.view(cx, ids!(picks_row)).set_visible(cx, !picks.is_empty());
        self.pick_ids = picks.iter().take(PICK_CHIPS.len()).map(|a| a.id.clone()).collect();
        for (slot, id) in PICK_CHIPS.iter().enumerate() {
            let chip = self.view.view(cx, &[*id]);
            match picks.get(slot) {
                Some(a) => {
                    chip.set_visible(cx, true);
                    chip.label(cx, ids!(pick_kind)).set_text(cx, &a.kind);
                    chip.label(cx, ids!(pick_label)).set_text(cx, &a.label);
                }
                None => chip.set_visible(cx, false),
            }
        }
        // More than the chips hold: the rest by name (all of them still go with the line).
        let more = self.view.label(cx, ids!(picks_more));
        more.set_visible(cx, picks.len() > PICK_CHIPS.len());
        if picks.len() > PICK_CHIPS.len() {
            more.set_text(cx, &format!("+{}: {}", picks.len() - PICK_CHIPS.len(), picks[PICK_CHIPS.len()..].iter().map(|a| a.label.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if !self.provider_rows.is_empty() {
            // A choice with no row (Local in a build without local AI)
            // answers with no model, so the menu shows that row.
            let at = self
                .provider_rows
                .iter()
                .position(|r| r.choice == state.provider)
                .or_else(|| self.provider_rows.iter().position(|r| r.choice.slug() == "none"))
                .unwrap_or(0);
            let labels: Vec<String> = self
                .provider_rows
                .iter()
                .enumerate()
                .map(|(i, r)| match &r.unavailable {
                    Some(why) => format!("{} — {why}", r.label),
                    // The picked local row says where it runs once it knows
                    // ("AI hub · 149 (Qwen3.8 27B)"). The elected model can
                    // differ from this machine's weights in the base row.
                    None if i == at && r.choice.is_local() && state.provider_label.starts_with("AI hub · ") => state.provider_label.clone(),
                    None => r.label.clone(),
                })
                .collect();
            // A design re-run puts the template's labels back in the menu: what it shows is
            // checked, not only what was last set.
            let pick = self.view.drop_down(cx, ids!(provider_pick));
            if self.pick_shown.as_ref() != Some(&(labels.clone(), at)) || !pick.has_labels(&labels) {
                self.pick_shown = Some((labels.clone(), at));
                pick.set_labels(cx, labels);
                pick.set_disabled_items(cx, self.provider_rows.iter().map(|r| r.unavailable.is_some()).collect());
                pick.set_selected_item(cx, at);
            }
        }
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(list) = item.borrow::<FlatList>() {
                for item in self.attachments.slots().filter_map(|slot| list.items.get(&LiveId(slot.id))) {
                    item.widget.draw_all(cx, scope);
                }
                continue;
            }
            let Some(mut list) = item.borrow_mut::<PortalList>() else { continue };
            let total = self.rows.len();
            let work_index = total + self.load_notices.len();
            list.set_item_range(cx, 0, work_index + work.is_some() as usize);
            while let Some(index) = list.next_visible_item(cx) {
                if index >= total {
                    if let Some(note) = self.load_notices.get(index - total) {
                        let row = list.item(cx, index, id!(SystemRow));
                        row.label(cx, ids!(system_text)).set_text(cx, note);
                        row.draw_all(cx, &mut Scope::empty());
                        continue;
                    }
                    // The work line follows the last row (the list asks
                    // past its range to fill the view: those stay empty).
                    let Some((line, bar, failed)) = work.as_ref().filter(|_| index == work_index) else { continue };
                    let row = list.item(cx, index, id!(WorkRow));
                    let label = row.label(cx, ids!(work_label));
                    label.set_visible(cx, !line.is_empty());
                    label.set_text(cx, line);
                    row.widget(cx, ids!(work_dot)).set_text(cx, if *failed { StatusKind::Error } else { StatusKind::InProgress }.name());
                    row.view(cx, ids!(load_row)).set_visible(cx, bar.is_some());
                    if let Some(value) = bar {
                        row.progress_bar(cx, ids!(load_bar)).set_value(cx, *value);
                    }
                    row.draw_all(cx, &mut Scope::empty());
                    continue;
                }
                let at = match self.rows[index] {
                    RowAt::Entry(at) => at,
                    RowAt::Turn(k) => {
                        let row = list.item(cx, index, id!(TurnRow));
                        let actions = &self.turn_actions[k].actions;
                        for (slot, id) in TURN_BUTTONS.iter().enumerate() {
                            let button = row.button(cx, &[*id]);
                            button.set_visible(cx, slot < actions.len());
                            if let Some(action) = actions.get(slot) {
                                button.set_text(cx, &action.label);
                                button.set_enabled(cx, action.enabled);
                                if button.disabled(cx) == action.enabled {
                                    button.set_disabled(cx, !action.enabled);
                                }
                            }
                        }
                        let why = actions.iter().filter(|a| !a.why.is_empty()).map(|a| a.why.as_str()).collect::<Vec<_>>().join(" · ");
                        let label = row.label(cx, ids!(turn_why));
                        label.set_visible(cx, !why.is_empty());
                        label.set_text(cx, &why);
                        row.draw_all(cx, &mut Scope::empty());
                        continue;
                    }
                };
                let entry = &state.entries[at];
                let row = match entry {
                    Entry::User { text, pictures, attachments, delivery } => {
                        let mut row = list.item(cx, index, id!(UserRow));
                        row.label(cx, ids!(user_text)).set_text(cx, text);
                        let note = row.label(cx, ids!(user_stopped));
                        let label = delivery_note(delivery);
                        note.set_visible(cx, label.is_some());
                        if let Some(text) = label {
                            note.set_text(cx, &text);
                        }
                        let pointed = row.label(cx, ids!(user_picks));
                        pointed.set_visible(cx, !attachments.is_empty());
                        if !attachments.is_empty() {
                            pointed.set_text(cx, &format!("Pointing at {}", attachments.iter().map(|a| format!("{} {}", a.kind, a.label)).collect::<Vec<_>>().join(", ")));
                        }
                        // The line's pictures that have their small copy (a restored chat's
                        // come from the task pool a moment later).
                        let thumbs = pictures.iter().filter_map(|p| Some((p.file.as_str(), self.thumbs.get(&p.file)?)));
                        let count = thumbs.clone().count();
                        row.view(cx, ids!(user_pics)).set_visible(cx, count > 0);
                        if self.pics_filled.get(&row.widget_uid()) != Some(&(at, count)) {
                            self.pics_filled.insert(row.widget_uid(), (at, count));
                            let list = row.flat_list(cx, ids!(line_thumbs));
                            fill_picture_list(cx, list, thumbs);
                        }
                        let scope = &mut Scope::empty();
                        while let Some(item) = row.draw(cx, scope).step() {
                            if let Some(list) = item.borrow::<FlatList>() {
                                draw_picture_list(cx, scope, &list);
                            }
                        }
                        continue;
                    }
                    Entry::Event(event) => {
                        let row = list.item(cx, index, id!(EventRow));
                        row.label(cx, ids!(event_title)).set_text(cx, &format!("{} · {}", event.service_label, event.topic));
                        row.label(cx, ids!(event_text)).set_text(cx, &event.text);
                        let mut meta = Vec::new();
                        if event.dropped != 0 {
                            meta.push(format!("{} dropped", event.dropped));
                        }
                        if event.final_ {
                            meta.push("final".to_string());
                        }
                        if let Some(data) = &event.data {
                            meta.push(data.clone());
                        }
                        let meta_label = row.label(cx, ids!(event_meta));
                        meta_label.set_visible(cx, !meta.is_empty());
                        meta_label.set_text(cx, &meta.join("\n"));
                        row
                    }
                    // Streaming and finished text draw the same way, so the
                    // last line does not jump when the turn ends.
                    Entry::Assistant { text, .. } => {
                        let row = list.item(cx, index, id!(AssistantRow));
                        // No text, no row: a blank block above a card is a gap.
                        row.set_visible(cx, !text.trim().is_empty());
                        if let Some(mut md) = row.widget(cx, ids!(assistant_md)).borrow_mut::<Markdown>() {
                            md.set_text(cx, text);
                        }
                        row
                    }
                    Entry::Tool(ToolEntry { status: ToolStatus::Confirm, card: Some(card), call_id, .. }) => {
                        let row = list.item(cx, index, id!(RequestRow));
                        row.button(cx, ids!(req_always)).set_visible(cx, self.ensure_engine().can_allow_for_chat(call_id));
                        let values = self.card_edits.entry(call_id.clone()).or_insert_with(|| card.values()).clone();
                        self.draw_card(cx, &row, call_id, card, &values);
                        row
                    }
                    Entry::Tool(t) if matches!(t.status, ToolStatus::Confirm) => {
                        let row = list.item(cx, index, id!(ConfirmRow));
                        row.button(cx, ids!(run_always)).set_visible(cx, self.ensure_engine().can_allow_for_chat(&t.call_id));
                        row.label(cx, ids!(confirm_title)).set_text(cx, &format!("{} changes things outside the app. Run it?", Self::tool_line(t)));
                        row
                    }
                    Entry::Tool(t) if t.task.is_some() && !matches!(t.status, ToolStatus::Confirm) => {
                        let row = list.item(cx, index, id!(TaskRow));
                        self.draw_task(cx, &row, t);
                        row
                    }
                    Entry::Tool(t) => {
                        let row = list.item(cx, index, id!(ToolRow));
                        // the status line: what it is doing now (or a note it ended with), and how
                        // far, when it counts; a failure's reason in its own words below
                        let (kind, note, problem, permille, detail) = match &t.status {
                            ToolStatus::Running { note, permille } => (StatusKind::InProgress, note.clone(), String::new(), *permille, String::new()),
                            ToolStatus::NotResponding { note, permille } => {
                                let line = if note.is_empty() { "not responding".to_string() } else { format!("{note} · not responding") };
                                (StatusKind::Pending, line, String::new(), *permille, note.clone())
                            }
                            ToolStatus::Done { outcome, note, text } if outcome.is_ok() => {
                                let note = if note.is_empty() || note == "ok" { String::new() } else { note.clone() };
                                (StatusKind::Success, note, String::new(), 0, text.clone())
                            }
                            ToolStatus::Done { outcome, note, text } => {
                                let reason = if note.is_empty() { text.lines().next().unwrap_or("") } else { note.as_str() };
                                let problem = if reason.is_empty() { outcome.slug().to_string() } else { reason.to_string() };
                                (StatusKind::Error, String::new(), problem, 0, text.clone())
                            }
                            ToolStatus::Confirm => (StatusKind::Pending, String::new(), String::new(), 0, String::new()),
                        };
                        let running = matches!(t.status, ToolStatus::Running { .. } | ToolStatus::NotResponding { .. });
                        row.widget(cx, ids!(tool_dot)).set_text(cx, kind.name());
                        let line = self.tool_lines.entry(t.call_id.clone()).or_insert_with(|| Self::tool_line(t));
                        row.label(cx, ids!(tool_title)).set_text(cx, line);
                        row.button(cx, ids!(tool_stop)).set_visible(cx, running);
                        let counted = running && permille > 0;
                        row.view(cx, ids!(tool_status)).set_visible(cx, !note.is_empty() || counted);
                        row.label(cx, ids!(tool_note)).set_text(cx, &note);
                        row.view(cx, ids!(tool_bar_box)).set_visible(cx, counted);
                        let bar = row.progress_bar(cx, ids!(tool_bar));
                        let value = permille.min(1000) as f64 / 1000.0;
                        if counted && bar.value() != value {
                            bar.set_value(cx, value);
                        }
                        let problem_label = row.label(cx, ids!(tool_problem));
                        problem_label.set_visible(cx, !problem.is_empty() && !t.expanded);
                        problem_label.set_text(cx, &problem);
                        row.view(cx, ids!(tool_detail)).set_visible(cx, t.expanded);
                        if t.expanded {
                            let mut detail = if detail.is_empty() { t.args.clone() } else { format!("{}\n\n{}", t.args, detail) };
                            // The details stay a glance: a long answer is cut
                            // (the transcript route keeps it whole).
                            if let Some((at, _)) = detail.char_indices().nth(1200) {
                                detail.truncate(at);
                                detail.push_str(" …");
                            }
                            row.label(cx, ids!(tool_detail_text)).set_text(cx, &detail);
                        }
                        row
                    }
                    Entry::System { text } => {
                        let row = list.item(cx, index, id!(SystemRow));
                        row.label(cx, ids!(system_text)).set_text(cx, text);
                        row
                    }
                };
                row.draw_all(cx, &mut Scope::empty());
            }
        }
        // The composer takes the keyboard the first time it is on screen, not while the panel
        // is drawn collapsed or hidden: an app that starts with its chat closed has nothing
        // focused (a focused field blinks its caret, and keeps an idle app drawing).
        if !self.composer_focused {
            let shown = self.view.widget(cx, ids!(input)).area().rect(cx.cx).size;
            if shown.x > 1.0 && shown.y > 1.0 {
                self.composer_focused = true;
                self.view.text_input(cx, ids!(input)).take_key_focus(cx);
            }
        }
        DrawStep::done()
    }
}

/// What the remote `confirm` route may do: only cancel.
pub fn remote_answer_allowed(how: &str) -> Result<(), String> {
    if how == "cancel" {
        Ok(())
    } else {
        Err(format!("confirm {how} is refused: the person must press Run in the app; a remote confirm can only cancel"))
    }
}

/// The origin of the input since the last actions, after one more event from `now`: an injected
/// event stays injected until the actions that follow it are handled.
fn note_press(before: Option<Option<String>>, now: Option<String>) -> Option<Option<String>> {
    match before {
        Some(Some(injected)) => Some(Some(injected)),
        _ => Some(now),
    }
}

/// Run and Always count only from the person's own press: `press` is where the click's input
/// came from (see `AiChatPanel::press_origin`); an unknown origin is refused too.
fn judge_press(press: Option<Option<String>>, what: &str) -> Result<(), String> {
    match press {
        Some(None) => Ok(()),
        Some(Some(who)) => Err(format!("{what} refused: the press was injected by {who}; only the person's own press in the app answers it, and the call stays parked")),
        None => Err(format!("{what} refused: the press has no known origin; the call stays parked")),
    }
}

impl AiChatPanel {
    /// True when the press being answered came from the person. An injected press of Run or
    /// Always is refused and logged with its sender, and the row or card stays parked. Taking
    /// resets the mark, so one local press answers one click.
    fn take_local_press(press_origin: &mut Option<Option<String>>, notice: &mut Option<String>, what: &str) -> bool {
        match judge_press(press_origin.take(), what) {
            Ok(()) => true,
            Err(reason) => {
                log!("ai chat: {reason}");
                *notice = Some(format!("{what} needs your own press in the app"));
                false
            }
        }
    }

    /// The editors and buttons of a settings card row. True when the row
    /// was one and something happened.
    fn card_actions(&mut self, cx: &mut Cx, item: &WidgetRef, actions: &Actions, call_id: &str, now: f64) -> bool {
        let Some(card) = self.ensure_engine().state().tool(call_id).and_then(|t| t.card.clone()) else { return false };
        let mut changed = false;
        let edits = self.card_edits.entry(call_id.to_string()).or_insert_with(|| card.values());
        let (mut pick_at, mut line_at) = (0, 0);
        for (slot, field) in card.fields.iter().enumerate() {
            let id = slot_id(slot);
            match field {
                Field::Pick { .. } => {
                    if let Some(at) = item.drop_down(cx, &[id, id!(pick)]).changed(actions) {
                        if let Some(pick) = edits.picks.get_mut(pick_at) {
                            *pick = at;
                            changed = true;
                        }
                    }
                    pick_at += 1;
                }
                Field::Line { .. } => {
                    if let Some(text) = item.text_input(cx, &[id, id!(line)]).changed(actions) {
                        if let Some(line) = edits.lines.get_mut(line_at) {
                            *line = text;
                            changed = true;
                        }
                    }
                    line_at += 1;
                }
            }
        }
        if item.button(cx, ids!(req_run)).clicked(actions) {
            if !Self::take_local_press(&mut self.press_origin, &mut self.notice, "Run on the settings card") {
                return true;
            }
            let args = card.args(edits).to_json();
            // A refusal leaves the card parked with its reasons on it.
            if self.ensure_engine().confirm_with(call_id, true, Some(args), now).is_ok() {
                self.card_edits.remove(call_id);
            }
            return true;
        }
        if item.button(cx, ids!(req_always)).clicked(actions) {
            if !Self::take_local_press(&mut self.press_origin, &mut self.notice, "Always on the settings card") {
                return true;
            }
            let args = card.args(edits).to_json();
            match self.ensure_engine().allow_for_chat(call_id, Some(args), now) {
                Ok(()) => { self.card_edits.remove(call_id); }
                Err(reason) if !self.ensure_engine().can_allow_for_chat(call_id) => self.notice = Some(reason),
                Err(_) => {} // Argument errors are shown beside the editors.
            }
            return true;
        }
        if item.button(cx, ids!(req_cancel)).clicked(actions) {
            let _ = self.ensure_engine().confirm_with(call_id, false, None, now);
            self.card_edits.remove(call_id);
            return true;
        }
        changed
    }
}

impl WidgetMatchEvent for AiChatPanel {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope) {
        let input = self.view.text_input(cx, ids!(input));
        let returned = input.returned(actions).map(|(text, _)| text);
        if let Some(text) = returned {
            // Enter on an empty line with no picture sends nothing.
            if !text.trim().is_empty() || self.attachments.count != 0 || self.marks.count > 0 {
                self.send(cx, text);
            }
        } else if self.view.button(cx, ids!(send_button)).clicked(actions) {
            let text = input.text();
            if !text.trim().is_empty() || self.attachments.count != 0 || self.marks.count > 0 {
                self.send(cx, text);
            }
        }
        if input.changed(actions).is_some() {
            // Send greys and lights with the text.
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(stop_button)).clicked(actions) {
            let now = Self::now(cx);
            self.ensure_engine().cancel(now);
            self.view.redraw(cx);
        }
        let chip_clicked = (0..MAX_CHIPS).find(|i| self.view.button(cx, &[LiveId::from_str(&format!("chip{i}"))]).clicked(actions));
        if let Some(i) = chip_clicked {
            let chip = self.ensure_engine().state().suggestions.get(i).cloned();
            if let Some(chip) = chip {
                if chip.is_prompt() {
                    self.send(cx, chip.prefill);
                } else {
                    let now = Self::now(cx);
                    self.ensure_engine().propose(&chip.app, &chip.tool, &chip.prefill, now);
                }
                self.view.redraw(cx);
            }
        }
        if self.view.button(cx, ids!(attach_button)).clicked(actions) {
            self.pick_pictures(cx);
        }
        if self.view.button(cx, ids!(mic_button)).clicked(actions) || self.view.button(cx, ids!(mic_live_button)).clicked(actions) {
            cx.widget_action(self.widget_uid(), AiChatPanelAction::VoiceToggled);
        }
        for action in actions {
            match action.downcast_ref::<FileDialogAction>() {
                Some(FileDialogAction::FileSelected { id, paths }) if *id == ATTACH_DIALOG => {
                    self.attach_batch(cx, paths.iter().cloned().map(Source::Path));
                }
                Some(FileDialogAction::FileLoaded { id, files }) if *id == ATTACH_DIALOG => {
                    self.attach_batch(cx, files.iter().map(|file| Source::Bytes { name: file.name.clone(), bytes: file.bytes.clone() }));
                }
                _ => {}
            }
        }
        if self.view.button(cx, ids!(annotate_button)).clicked(actions) {
            self.marks.on = true;
            self.view.widget(cx, ids!(annotate_tip)).set_visible(cx, false);
            self.view.widget(cx, ids!(annotate_on_tip)).set_visible(cx, true);
            cx.widget_action(self.widget_uid(), AiChatPanelAction::Annotate(true));
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(annotate_on_button)).clicked(actions) {
            self.marks.on = false;
            self.view.widget(cx, ids!(annotate_tip)).set_visible(cx, true);
            self.view.widget(cx, ids!(annotate_on_tip)).set_visible(cx, false);
            cx.widget_action(self.widget_uid(), AiChatPanelAction::Annotate(false));
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(marks_clear)).clicked(actions) {
            cx.widget_action(self.widget_uid(), AiChatPanelAction::MarksCleared);
        }
        for (slot, id) in PICK_CHIPS.iter().enumerate() {
            if !self.view.view(cx, &[*id]).button(cx, ids!(pick_remove)).clicked(actions) {
                continue;
            }
            let Some(pick) = self.pick_ids.get(slot).cloned() else { continue };
            // Gone from the composer at once (its remove button's tip with it, on the actions
            // this sends); the host drops it on the page.
            self.view.view(cx, &[*id]).set_visible(cx, false);
            let target = self.ensure_engine().state().target.clone();
            if let Some(mut target) = target {
                target.attachments.retain(|a| a.id != pick);
                self.ensure_engine().set_target(Some(target));
            }
            if self.pick_hovered.as_ref() == Some(&pick) {
                self.pick_hovered = None;
                cx.widget_action(self.widget_uid(), AiChatPanelAction::AttachmentHovered(None));
            }
            cx.widget_action(self.widget_uid(), AiChatPanelAction::AttachmentRemoved(pick));
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(allowed_clear)).clicked(actions) {
            self.ensure_engine().revoke(None);
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(attach_clear)).clicked(actions) {
            self.attachments.clear(PictureLoad::cancel);
            for line in self.returning.drain(..) {
                line.read.cancel();
                line.read.detach();
            }
            self.picture_refusal = None;
            if self.hold.releasing() {
                self.finish_release(cx);
            }
            self.view.redraw(cx);
        }
        if self.attach_shown.is_some() {
            let list = self.view.flat_list(cx, ids!(draft_thumbs));
            for (id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(pic_remove)).clicked(actions) {
                    if let Some(SlotState::Reading(job)) = self.attachments.remove(id.0, &mut self.load_notices) {
                        job.cancel();
                    }
                    self.picture_refusal = None;
                    self.view.redraw(cx);
                }
            }
            if self.hold.releasing() && self.attachments.reading == 0 {
                self.finish_release(cx);
            }
        }
        if let Some(at) = self.view.drop_down(cx, ids!(provider_pick)).changed(actions) {
            if let Some(row) = self.provider_rows.get(at).cloned() {
                if let Some(why) = &row.unavailable {
                    self.note(cx, format!("{}: {why}", row.label));
                    // The menu goes back to the provider that answers.
                    self.pick_shown = None;
                } else {
                    self.set_provider(cx, &row.choice.slug());
                }
            }
        }
        if self.view.button(cx, ids!(mcp_connect)).clicked(actions) {
            self.connect_claude_desktop(cx);
        }
        if self.view.button(cx, ids!(new_chat)).clicked(actions) {
            let now = Self::now(cx);
            self.notice = None;
            self.load_notices.clear();
            self.ensure_engine().clear(now);
            self.thumbs.clear();
            self.share_pictures(cx);
            self.pics_filled.clear();
            self.result_pics.clear();
            self.task_pics_filled.clear();
            self.task_rows_filled.clear();
            self.tool_lines.clear();
            self.task_tracks.clear();
            self.view.redraw(cx);
        }
        // Turn buttons and tool cards in the transcript.
        let list = self.view.portal_list(cx, ids!(transcript));
        let items = list.items_with_actions(actions);
        if !items.is_empty() {
            let now = Self::now(cx);
            let mut cards = Vec::new();
            let mut seen_turn_rows = HashSet::new();
            for (index, item) in &items {
                match self.rows.get(*index) {
                    Some(RowAt::Entry(at)) => {
                        if let Some(Entry::Tool(t)) = self.state().and_then(|state| state.entries.get(*at)) {
                            cards.push((item, t.call_id.clone()));
                        }
                    }
                    // A change waiting to be drawn may have put another turn in this slot.
                    Some(RowAt::Turn(k)) if self.rows_key.is_some_and(|(_, rev, _)| rev == self.turn_actions_rev) => {
                        // The list can report a row once for each of its widget actions.
                        if !seen_turn_rows.insert(*k) {
                            continue;
                        }
                        let turn = &self.turn_actions[*k];
                        for (id, action) in TURN_BUTTONS.iter().zip(&turn.actions) {
                            if action.enabled && item.button(cx, &[*id]).clicked(actions) {
                                cx.widget_action(self.widget_uid(), AiChatPanelAction::TurnAction { turn: turn.turn, id: action.id.clone() });
                            }
                        }
                    }
                    _ => {}
                }
            }
            for (item, call_id) in cards {
                if self.card_actions(cx, item, actions, &call_id, now) {
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(tool_stop)).clicked(actions) || item.button(cx, ids!(task_stop)).clicked(actions) {
                    self.ensure_engine().stop_call(&call_id);
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(task_retry)).clicked(actions) {
                    let _ = self.ensure_engine().retry(&call_id, now);
                    self.view.redraw(cx);
                } else if let Some(slot) = [id!(task_act0), id!(task_act1)].iter().position(|id| item.button(cx, &[*id]).clicked(actions)) {
                    let action = self.ensure_engine().state().tool(&call_id)
                        .filter(|t| matches!(t.status, ToolStatus::Done { outcome: ToolOutcome::Ok, .. }))
                        .and_then(|t| t.task.as_ref())
                        .and_then(|task| task.actions.get(slot))
                        .map(|action| action.id.clone());
                    if let Some(action) = action {
                        cx.widget_action(self.widget_uid(), AiChatPanelAction::TaskAction { call_id, action });
                    }
                } else if item.button(cx, ids!(run_button)).clicked(actions) {
                    if Self::take_local_press(&mut self.press_origin, &mut self.notice, "Run") {
                        self.ensure_engine().confirm(&call_id, true, now);
                    }
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(run_always)).clicked(actions) {
                    if Self::take_local_press(&mut self.press_origin, &mut self.notice, "Always") {
                        if let Err(reason) = self.ensure_engine().allow_for_chat(&call_id, None, now) {
                            self.notice = Some(reason);
                        }
                    }
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(deny_button)).clicked(actions) {
                    self.ensure_engine().confirm(&call_id, false, now);
                    self.view.redraw(cx);
                // A press taken away (the list scrolled under it) toggles nothing.
                } else if item.view(cx, ids!(tool_head)).finger_up(actions).is_some_and(|e| !e.cancelled) {
                    self.ensure_engine().toggle_tool(&call_id);
                    self.view.redraw(cx);
                }
            }
        }
        let _ = scope;
    }
}

/// What a user row says about its line's fate, when it is not simply sent.
fn delivery_note(delivery: &LineDelivery) -> Option<String> {
    match delivery {
        LineDelivery::Sent => None,
        LineDelivery::Stopped => Some("stopped".into()),
        LineDelivery::Failed => Some("failed".into()),
        LineDelivery::NotSent(detail) if detail.is_empty() => Some("not sent".into()),
        LineDelivery::NotSent(detail) => Some(format!("not sent · {detail}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_reply_keeps_transcript_at_end_until_scrolled_up() {
        use makepad_ai_services::engine::{Model, ModelEvent, ToolDefinition};
        use makepad_widgets::{cx_draw::CxDraw, makepad_platform::event::{ScrollEvent, ScrollPhase}};
        use std::sync::Mutex;

        struct StreamModel(Arc<Mutex<Vec<ModelEvent>>>);
        impl Model for StreamModel {
            fn label(&self) -> String { "Stream".into() }
            fn configure(&mut self, _: &str, _: &[ToolDefinition]) -> Result<(), String> { Ok(()) }
            fn send_user(&mut self, _: &str, _: &str) {}
            fn send_tool_result(&mut self, _: &str, _: &str, _: bool) {}
            fn cancel(&mut self) {}
            fn reset(&mut self) {}
            fn poll(&mut self) -> Vec<ModelEvent> { std::mem::take(&mut *self.0.lock().unwrap()) }
        }

        makepad_widgets::test_cx::on_test_cx(|| {
            let mut pool = makepad_widgets::test_cx::checkout_test_cx(|vm| {
                makepad_widgets::script_mod(vm);
                crate::script_mod(vm);
            });
            let cx = &mut *pool;
            let mut panel = cx.with_vm(AiChatPanel::script_new_with_default);
            panel.use_private_registry();
            let events = Arc::new(Mutex::new(Vec::new()));
            let model = StreamModel(events.clone());
            panel.engine = Some(EngineCore::new(panel.registry.clone(), Box::new(model), None, 1));
            for turn in 0..4 {
                panel.send(cx, format!("turn {turn}"));
                events.lock().unwrap().extend([
                    ModelEvent::Delta("short reply".into()),
                    ModelEvent::TurnDone { tool_calls: 0 },
                ]);
                panel.ensure_engine().pump(turn as f64);
            }
            panel.send(cx, "last turn".into());
            events.lock().unwrap().push(ModelEvent::Delta("streaming start".into()));
            panel.ensure_engine().pump(4.0);
            assert!(matches!(panel.state().unwrap().status, Status::Streaming));

            let size = dvec2(360.0, 240.0);
            let pass = DrawPass::new(cx);
            pass.set_size(cx, size);
            let mut draw_list = DrawList2d::new(cx);
            let mut draw_panel = |cx: &mut Cx, panel: &mut AiChatPanel| -> (ActionsBuf, bool) {
                cx.new_draw_event = DrawEvent::default();
                let actions = cx.capture_actions(|cx| {
                    let event = DrawEvent::default();
                    let mut draw = CxDraw::new(cx, &event);
                    let mut frame = Cx2d::new(&mut draw);
                    frame.begin_pass(&pass, None);
                    draw_list.begin_always(&mut frame);
                    frame.begin_root_turtle(size, Layout::flow_down());
                    let _ = panel.draw_walk(&mut frame, &mut Scope::empty(), Walk::fixed(size.x, size.y));
                    frame.end_pass_sized_turtle();
                    draw_list.end(&mut frame);
                    frame.end_pass(&pass);
                });
                (actions, cx.new_draw_event.will_redraw())
            };

            let transcript = panel.view.portal_list(cx, ids!(transcript));
            assert!((0..4).any(|_| !draw_panel(cx, &mut panel).1), "initial transcript kept redrawing");
            assert!(transcript.borrow().unwrap().is_scrollable() && transcript.is_at_end());
            let range_before = transcript.borrow().unwrap().range_end();
            let reply_id = range_before - 2;
            let footer_id = range_before - 1;
            let reply_before = transcript.drawn_slot(reply_id).expect("streaming reply drawn").size;
            assert!(transcript.drawn_slot(footer_id).expect("WorkRow footer drawn").size > 0.0);

            events.lock().unwrap().push(ModelEvent::Delta("\n\nline 1\n\nline 2\n\nline 3\n\nline 4\n\nline 5\n\nline 6".into()));
            panel.ensure_engine().pump(4.1);
            let mut reached_end_count = 0;
            let settled = (0..4).any(|_| {
                let (actions, will_redraw) = draw_panel(cx, &mut panel);
                assert!(!transcript.scrolled(&actions));
                reached_end_count += usize::from(transcript.reached_end(&actions));
                !will_redraw
            });
            assert!(settled, "growing the reply kept the transcript redrawing");
            assert_eq!(transcript.borrow().unwrap().range_end(), range_before);
            assert!(transcript.drawn_slot(reply_id).expect("streaming reply drawn").size > reply_before);
            assert!(transcript.is_at_end());
            let footer = transcript.drawn_slot(footer_id).expect("WorkRow footer drawn");
            let viewport_h = transcript.area().rect(cx).size.y;
            assert!(footer.size > 0.0 && footer.start >= 0.0 && footer.start + footer.size <= viewport_h + 1.0);
            assert!(reached_end_count <= 1, "the footer re-entered more than once");

            let (idle, idle_redraw) = draw_panel(cx, &mut panel);
            assert!(!idle_redraw && transcript.is_at_end() && !transcript.scrolled(&idle) && !transcript.reached_end(&idle));
            let up = Event::Scroll(ScrollEvent {
                window_id: WindowId(1, 1),
                scroll: dvec2(0.0, -120.0),
                abs: transcript.area().rect(cx).center(),
                modifiers: KeyModifiers::default(),
                handled_x: Default::default(),
                handled_y: Default::default(),
                is_mouse: true,
                time: 4.2,
                phase: ScrollPhase::None,
            });
            panel.handle_event(cx, &up, &mut Scope::empty());
            let (away, _) = draw_panel(cx, &mut panel);
            assert!(!transcript.is_at_end() && !transcript.scrolled(&away));
        });
    }

    #[test]
    fn aichat_private_registry_does_not_populate_process_registry() {
        use makepad_ai_services::wire::{EndpointId, HostedUp, ServiceManifest, ServiceUp};

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            super::script_mod(vm);
            AiChatPanel::script_new_with_default(vm)
        });
        panel.use_private_registry();
        let id = "private_pane_mirror";
        let register = HostedUp {
            from: Some(EndpointId("w7".into())),
            msg: ServiceUp::Register { manifest: ServiceManifest::new(id, "Mirror", "A mirrored WM service."), port_tag: 7 },
        };
        assert!(panel.on_custom(&register.to_json()));
        let services = panel.registry().services();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, id);
        assert!(ServiceRegistry::process().services().iter().all(|service| service.id != id));
    }

    #[test]
    fn delivery_note_matches_every_line_fate() {
        assert_eq!(delivery_note(&LineDelivery::Sent), None, "a sent line has no note");
        assert_eq!(delivery_note(&LineDelivery::Stopped).as_deref(), Some("stopped"), "stopped note");
        assert_eq!(delivery_note(&LineDelivery::Failed).as_deref(), Some("failed"), "a line that acted then failed says failed, not not sent");
        assert_eq!(delivery_note(&LineDelivery::NotSent(String::new())).as_deref(), Some("not sent"), "plain not sent");
        assert_eq!(delivery_note(&LineDelivery::NotSent("could not return b.png".into())).as_deref(),
            Some("not sent · could not return b.png"), "not sent keeps its detail");
    }

    #[test]
    fn the_composer_offers_what_fits_and_explains_what_stays() {
        use PictureRoom::{Fits, NoPictures, Unknown};
        for (room, attached, go, notice) in [
            (Fits(4), 0, 0, None),
            (Fits(4), 3, 3, None),
            (Fits(4), 4, 4, None),
            (Fits(4), 6, 4, Some("This model takes 4 pictures with a line: the other 2 stay attached for the next.")),
            (Fits(1), 2, 1, Some("This model takes 1 picture with a line: the other 1 stays attached for the next.")),
            (Fits(0), 1, 0, Some("This model takes 0 pictures with a line: the other 1 stays attached for the next.")),
            (NoPictures, 0, 0, None),
            (NoPictures, 6, 0, Some("This model cannot look at pictures: they stay attached.")),
            (Unknown, 0, 0, None),
            (Unknown, 6, 6, None),
        ] {
            assert_eq!(pictures_that_go(room, attached), go, "send with {room:?}, {attached} attached");
            assert_eq!(room_notice(room, attached).as_deref(), notice, "notice with {room:?}, {attached} attached");
        }
        let state = EngineState { status: Status::Error("the recorded refusal".into()), ..Default::default() };
        assert_eq!(AiChatPanel::footer_line(&state, None), (None, true), "the failed state must not draw its transcript text again in the footer");
        let notice = String::from("the composer notice");
        assert_eq!(AiChatPanel::footer_line(&state, Some(&notice)), (Some(notice.clone()), true), "an unrelated composer notice must remain visible after a turn failed");
    }

    #[test]
    fn each_selection_reports_its_own_failed_reads_in_order() {
        let first = ["a.png", "x.png", "b.png", "c.png", "y.png", "d.png", "missing.png", "photo.heic"];
        let mut draft = PictureDraft::default();
        let mut completed = HashMap::new();
        let mut notes = Vec::new();
        let mut logs = Vec::new();
        let mut log = |name: &str, detail: &str| logs.push((name.to_owned(), detail.to_owned()));
        draft.select(first.into_iter().enumerate().map(|(i, name)| {
            (name.into(), if i == 4 { Err(PictureError::load(makepad_platform::thread::SubmitError::Closed)) } else { Ok(i) })
        }), &mut notes, &mut log);
        assert!(matches!(draft.slots().nth(4).unwrap().state, SlotState::Failed(_)), "a submission failure must mark its chip immediately");
        // Real read and decode failures finish out of order. The submission failure is already known.
        let failed = |source: Source| {
            let name = source.name();
            Err(attach::load_picture(source, &name).expect_err("the selected test picture must fail to load"))
        };
        let bytes: Arc<[u8]> = Arc::from(&b"\x89PNG\r\n\x1a\n"[..]);
        let decoder_error = makepad_image_decode::decode_rgba(&bytes).expect_err("the truncated PNG must reach the decoder and fail");
        let missing = std::env::temp_dir().join(format!("aichat-missing-picture-{}", std::process::id())).join("missing.png");
        let io_error = std::fs::metadata(&missing).expect_err("the missing selection must have an OS diagnostic");
        completed.insert(7, failed(Source::Path("photo.heic".into())));
        completed.insert(6, failed(Source::Path(missing)));
        completed.insert(1, failed(Source::Bytes { name: "x.png".into(), bytes }));
        completed.insert(5, Ok("d"));
        completed.insert(2, Ok("b"));
        draft.poll_batches(|_, i| completed.remove(i), &mut notes, &mut log);
        assert!(matches!(draft.slots().nth(1).unwrap().state, SlotState::Failed(_)), "a decode failure must mark its chip while the selection is open");
        assert!(matches!(draft.slots().nth(2).unwrap().state, SlotState::Ready("b")), "a success beside a pending picture must be visible immediately");
        assert!(notes.is_empty(), "an unfinished selection must not publish a partial note");

        let other = Source::Bytes { name: "z.png".into(), bytes: Arc::from(&b"plain text"[..]) };
        draft.select([(other.name(), failed(other).map(|_| 0usize)), ("removed.png".into(), failed(Source::Path("removed.heic".into())).map(|_| 0)),
            ("e.png".into(), Ok(8))].into_iter(), &mut notes, &mut log);
        let removed = draft.slots().find(|slot| slot.name == "removed.png").unwrap().id;
        draft.remove(removed, &mut notes);
        completed.insert(0, Ok("a"));
        draft.poll_batches(|_, i| completed.remove(i), &mut notes, &mut log);
        assert!(notes.is_empty(), "neither selection has settled yet");
        completed.insert(8, Ok("e"));
        completed.insert(3, Ok("c"));
        draft.poll_batches(|_, i| completed.remove(i), &mut notes, &mut log);
        assert_eq!(draft.ready().unwrap(), [&"a", &"b", &"c", &"d", &"e"], "the note must remove all failed chips and preserve success order");
        assert_eq!(notes, [
            concat!("4 of 8 couldn't be read: x.png (damaged or unreadable picture), y.png (couldn't be loaded, try again), ",
                "missing.png (not found), photo.heic (HEIC photos cannot be read here yet; save it as JPEG or PNG and drop that)"),
            "z.png couldn't be read (not a PNG, JPEG, WebP, GIF or BMP picture)",
        ], "each batch must show one note with only person-facing reasons in selection order, even when both finish on one frame");
        assert_eq!(logs, [
            ("y.png".into(), "task pool closed".into()),
            ("x.png".into(), format!("cannot decode ({decoder_error})")),
            ("missing.png".into(), io_error.to_string()),
        ], "the log must receive each rewritten diagnostic exactly once and nothing for the plain format reasons");
        draft.poll_batches(|_, _| panic!("a settled handle was polled again"), &mut notes, |_, _| {});
        assert_eq!(notes.len(), 2, "a settled selection must not publish its note twice");
    }

    #[test]
    fn a_partial_send_and_a_refusal_keep_the_unsent_pictures_in_order() {
        use makepad_ai_services::engine::{Model, ModelEvent, ToolDefinition};
        use makepad_platform::clipboard_read::{ClipboardContent, ClipboardFile, ClipboardImage};

        // The clipboard may also offer a preview bitmap of a selection of files.
        let files = ["a.png", "b.png", "skip.txt", "c.png", "d.png", "e.png", "f.png"];
        let clipboard = ClipboardContent {
            files: files.iter().map(|name| ClipboardFile {
                url: String::new(),
                path: std::env::temp_dir().join(name).to_string_lossy().into_owned(),
            }).collect(),
            image: Some(ClipboardImage::Png(Arc::new(vec![]))),
            ..Default::default()
        };
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut panel = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            super::script_mod(vm);
            AiChatPanel::script_new_with_default(vm)
        });
        panel.attachments.prepend_ready(Source::from_clipboard(&clipboard, 1).into_iter().enumerate().map(|(i, file)| {
            let name = file.name();
            let picture = attach::load(Source::Rgba {
                name: name.clone(), width: 1, height: 1, rgba: Arc::new(vec![i as u8, 0, 0, 255]),
            }).unwrap();
            (name, picture)
        }));
        let names = |panel: &AiChatPanel| panel.attachments.slots().filter_map(|s| match &s.state {
            SlotState::Ready(p) => Some(p.sent.name.clone()), _ => None,
        }).collect::<Vec<_>>();
        assert_eq!(names(&panel), ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png"]);
        let list = panel.view.flat_list(&cx, ids!(draft_thumbs));
        panel.show_draft_pictures(&mut cx, true);
        let list = list.borrow().expect("the composer has a picture list");
        assert_eq!(list.items.len(), 6, "all six have a thumbnail slot");
        assert!(!list.items[LiveId(6)].widget.widget(&cx, ids!(pic_remove)).is_empty(), "the sixth has its own remove button");
        drop(list);

        struct Four { failed: bool }
        impl Model for Four {
            fn label(&self) -> String { "Four".into() }
            fn picture_room(&self) -> PictureRoom { PictureRoom::Fits(4) }
            fn configure(&mut self, _: &str, _: &[ToolDefinition]) -> Result<(), String> { Ok(()) }
            fn admit_images(&self, images: &[ModelImage]) -> Result<(), String> {
                if images.len() == 4 { Ok(()) } else { Err("batch refused".into()) }
            }
            fn attach_images(&mut self, images: Vec<ModelImage>) -> Result<(), String> { self.admit_images(&images) }
            fn send_user(&mut self, _: &str, _: &str) { self.failed = true; }
            fn send_tool_result(&mut self, _: &str, _: &str, _: bool) {}
            fn cancel(&mut self) {}
            fn reset(&mut self) {}
            fn poll(&mut self) -> Vec<ModelEvent> {
                if std::mem::take(&mut self.failed) { vec![ModelEvent::Error("node refused".into())] } else { Vec::new() }
            }
        }

        panel.engine = Some(EngineCore::new(panel.registry.clone(), Box::new(Four { failed: false }), None, 1));
        panel.attach(&mut cx, Source::Rgba { name: "waiting.png".into(), width: 1, height: 1, rgba: Arc::new(vec![0; 4]) });
        let before = panel.state().unwrap().entries.len();
        panel.send(&mut cx, "compare".into());
        assert_eq!(panel.state().unwrap().entries.len(), before, "even when four earlier pictures fit, a later placeholder must hold Send");
        panel.send(&mut cx, "/help".into());
        assert!(panel.state().unwrap().entries.len() > before, "a console command carries no pictures and must not wait for a reading chip");
        assert!(matches!(panel.attachments.slots().last().unwrap().state, SlotState::Reading(_)), "a console command must leave the placeholder in the draft");
        let waiting = panel.attachments.slots().last().unwrap().id;
        let Some(SlotState::Reading(job)) = panel.attachments.remove(waiting, &mut panel.load_notices) else {
            panic!("the removed placeholder must return its outstanding job for cancellation");
        };
        job.cancel();
        panel.send(&mut cx, "compare".into());
        assert_eq!(names(&panel), ["e.png", "f.png"]);
        let engine = panel.engine.as_ref().unwrap();
        assert_eq!(engine.pictures().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a.png", "b.png", "c.png", "d.png"]);
        panel.send(&mut cx, "just the text if refused".into());
        assert_eq!(names(&panel), ["e.png", "f.png"]);
        assert_eq!(panel.picture_refusal.as_deref(), Some("The pictures were not sent: batch refused. They stay attached."));
        assert!(matches!(panel.state().unwrap().entries.last(), Some(Entry::User { text, pictures, .. }) if text == "just the text if refused" && pictures.is_empty()));

        let before = panel.state().unwrap().entries.len();
        panel.attach(&mut cx, Source::Rgba { name: "pending.png".into(), width: 1, height: 1, rgba: Arc::new(vec![0, 0, 0, 255]) });
        panel.send(&mut cx, "with the pending picture".into());
        assert_eq!(panel.state().unwrap().entries.len(), before, "Enter keeps the draft while a picture that would go is loading");
        assert_eq!(panel.view.text_input(&cx, ids!(input)).text(), "with the pending picture");

        let input = panel.view.text_input(&cx, ids!(input));
        input.set_text(&mut cx, "new text");
        let source = Source::Rgba { name: "new.png".into(), width: 1, height: 1, rgba: Arc::new(vec![42, 0, 0, 255]) };
        panel.attach(&mut cx, source.clone());
        let mut decoded = Some(attach::load(source).unwrap());
        panel.attachments.poll_batches(|name, _| (name == "new.png").then(|| Ok(decoded.take().unwrap())), &mut Vec::new(), |_, _| {});
        let event = panel.ensure_engine().pump(1.0).into_iter().find_map(|event| match event {
            EngineEvent::ReturnLine { id, text, pictures } => Some((id, text, pictures)),
            _ => None,
        }).expect("a refused picture line must reach the panel");
        panel.return_line(&mut cx, event.0, event.1, event.2);
        assert_eq!(input.text(), "compare\nnew text", "returning the line must prepend its text and preserve the new draft");
        let rows = panel.state().unwrap().entries.len();
        panel.send(&mut cx, "a chip's line".into());
        assert_eq!(input.text(), "compare\nnew text\na chip's line",
            "a line that cannot go while the returned pictures are read must wait behind the draft, never replace it");
        assert_eq!(panel.state().unwrap().entries.len(), rows, "nothing goes while the returned pictures are read");
        let pending = panel.returning.pop_front().expect("picture reads must run off the UI thread");
        let read = pending.read;
        let returned = std::thread::spawn(move || read.join()).join().unwrap().expect("the stored originals must be readable");
        panel.prepend_pictures(pending.line, pending.names, returned);
        assert_eq!(names(&panel), ["a.png", "b.png", "c.png", "d.png", "e.png", "f.png", "new.png"],
            "every returned chip must precede both the pictures left at Send and the newly attached picture");
        let SlotState::Ready(first) = &panel.attachments.slots().next().unwrap().state else { panic!("the first returned chip must be ready") };
        assert!(Arc::ptr_eq(first.sent.thumb.as_ref().unwrap(), panel.thumbs.get(&first.file).unwrap()),
            "a returned chip must reuse the thumbnail of the picture that was sent");

        let dir = std::env::temp_dir().join(format!("aichat-failed-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        panel.ensure_engine().keep_chat(dir.clone(), None).expect("bind the test chat picture store");
        for e in panel.ensure_engine().pump(2.0) {
            if let EngineEvent::KeepPicture(write) = e { write.run(); }
        }
        let originals = panel.attachments.slots().take(4).map(|s| match &s.state {
            SlotState::Ready(p) => p.clone().into_new(), _ => panic!("all four returned pictures must be ready"),
        }).collect();
        panel.ensure_engine().send_pictures("missing original", originals, 2.1).unwrap();
        let (id, pictures) = panel.ensure_engine().pump(2.2).into_iter().find_map(|e| match e {
            EngineEvent::ReturnLine { id, pictures, .. } => Some((id, pictures)),
            _ => None,
        }).expect("the next refused line must return its store references");
        std::fs::remove_file(dir.join(&pictures[1].original.file)).expect("remove just one saved original");
        let labels = pictures.iter().map(|p| p.original.name.clone()).collect();
        let results = pictures.into_iter().map(|p| returned_picture(p, None)).collect();
        panel.prepend_pictures(id, labels, results);
        assert_eq!(&names(&panel)[..3], ["a.png", "c.png", "d.png"], "a missing original must not block the other returned pictures");
        assert!(panel.state().unwrap().entries.iter().any(|e| matches!(e, Entry::User { text, delivery: LineDelivery::NotSent(detail), .. }
            if text.starts_with("missing original") && detail == "could not return b.png (not found)")),
            "the failed row must identify the missing original using only the person's reason");
        std::fs::remove_dir_all(dir).expect("remove the test picture store");

    }

    /// Run and Always answer only a local press; a remote `confirm` can only cancel.
    #[test]
    fn run_and_always_need_the_persons_own_press() {
        assert!(judge_press(Some(None), "Run").is_ok(), "a local press runs it");
        let injected = judge_press(Some(Some("agent LANE7".into())), "Always").unwrap_err();
        assert!(injected.contains("Always refused") && injected.contains("agent LANE7"), "{injected}");
        assert!(judge_press(None, "Run").is_err(), "a press of unknown origin is refused");
        let injected = note_press(Some(None), Some("agent".into()));
        assert_eq!(note_press(injected.clone(), None), injected, "a local event after an injected press can't launder it");
        assert_eq!(note_press(None, None), Some(None), "a local press alone is the person's");
        assert!(remote_answer_allowed("cancel").is_ok(), "remote cancel works");
        for how in ["run", "always"] {
            let refused = remote_answer_allowed(how).unwrap_err();
            assert!(refused.contains("person must press Run in the app"), "remote confirm {how}: {refused}");
        }
    }

    /// A call's row reads its arguments as `key: value` text, a string that holds a separator in
    /// quotes; a long one is cut with an ellipsis, short of what a row could hold.
    /// A line sent with marks still being resolved (the worker is behind a long build) waits: the
    /// host is not asked for the picture, and the text is not taken, until every mark is in; then
    /// it is asked once, and the text goes only when released. Giving up returns the text.
    #[test]
    fn a_line_waits_for_marks_still_resolving() {
        let mut hold = Hold::default();
        hold.take_line("what did I circle?".into());
        for _ in 0..5 {
            assert!(!hold.wants_picture(2), "asked for the picture with marks unresolved");
        }
        assert!(hold.is_waiting() && !hold.releasing());
        assert!(!hold.wants_picture(1));
        assert!(hold.wants_picture(0), "asked once they are all in");
        assert!(!hold.wants_picture(0), "and only once");
        assert!(hold.begin_release() && hold.releasing());
        assert!(!hold.wants_picture(0), "not again while the picture is read");
        assert_eq!(hold.finish().as_deref(), Some("what did I circle?"));
        assert!(!hold.is_waiting() && !hold.begin_release(), "nothing left to release");
        hold.take_line("again".into());
        assert_eq!(hold.finish().as_deref(), Some("again"), "given up: the text is back");
    }

    #[test]
    fn a_call_s_arguments_read_as_one_plain_line() {
        assert_eq!(args_line(r#"{"views":["front","35,0"],"focus":{"object":"a/b"},"ortho":true}"#), r#"views: [front, "35,0"], focus: {object: a/b}, ortho: true"#);
        assert_eq!(args_line("{}"), "");
        assert_eq!(args_line("not json\n at all"), "not json at all");
        let long = args_line(&format!(r#"{{"source":"{}"}}"#, "let a = 1\\n".repeat(100)));
        assert!(long.ends_with('…') && long.chars().count() == ARGS_LINE_CHARS + 1, "{long}");
    }
}
