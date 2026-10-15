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

use crate::attach::{self, Attachment, Source};
use crate::bus::ServiceBus;
#[cfg(feature = "gen")]
use crate::gen::GenService;
use crate::settings::AiSettings;
#[cfg(feature = "engine")]
use makepad_ai_services::engine::models::{build_model, fallback_choice, hub_generation, hub_row, provider_rows};
use makepad_ai_services::engine::NoModelWithReason;
use makepad_ai_services::engine::{EngineCore, EngineEvent, ModelImage, ServiceRegistry};
use makepad_ai_services::mcp::mcpb;
use makepad_ai_services::provider::{CLAUDE_DESKTOP, PROVIDER_ENV};
use makepad_ai_services::card::{Card, CardValues, Field, MAX_FIELDS};
use makepad_ai_services::state::*;
use makepad_ai_services::progress_eta::ProgressTracker;
use makepad_ai_services::wire::{api_name, split_name, Risk, ToolDef};
use makepad_widgets::makepad_platform::file_dialogs::{FileDialog, FileDialogAction};
use std::collections::HashMap;
use std::path::Path;
use makepad_widgets::ai_slot::AiSlotRequests;
use makepad_widgets::*;

/// Starting points the empty conversation shows.
pub const MAX_CHIPS: usize = 4;

/// The picture picker's dialog id.
const ATTACH_DIALOG: LiveId = live_id!(ai_chat_attach);

fn slot_id(slot: usize) -> LiveId {
    LiveId::from_str(&format!("f{slot}"))
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

            UserRow := Row{
                padding: Inset{left: 12 right: 12 top: 12 bottom: 4}
                RoundedView{
                    width: Fill
                    height: Fit
                    padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                    draw_bg +: { color: theme.color_u_1 border_radius: 6.0 }
                    user_text := Line{}
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
                    body: ""
                }
            }
            // One line per call: a dot for how it went, what it did, and
            // a click for the arguments and the answer.
            ToolRow := Row{
                padding: Inset{left: 14 right: 12 top: 0 bottom: 0}
                tool_head := View{
                    width: Fill
                    height: 20
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
                        draw_text +: {
                            color: theme.color_text_meta
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                    tool_note := Meta{
                        draw_text +: { text_style: theme.font_regular{font_size: 8.0} }
                    }
                    tool_stop := ButtonFlat{ text: "Stop" visible: false }
                }
                tool_problem := Label{
                    visible: false
                    width: Fill
                    max_lines: 2
                    margin: Inset{left: 14 bottom: 3}
                    draw_text +: {
                        color: mix(theme.color_error, theme.color_text_meta, 0.35)
                        text_style: theme.font_regular{font_size: 8.0}
                    }
                }
                tool_bar := SolidView{
                    width: Fill
                    height: 2
                    margin: Inset{left: 14 bottom: 3}
                    draw_bg +: { color: theme.color_primary }
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

            // The element picked on the page, under the page's context: the
            // turn is about it until the person clears it here or on the page.
            selection_row := RoundedView{
                visible: false
                width: Fill
                height: Fit
                flow: Right
                spacing: 6
                margin: Inset{bottom: 2}
                padding: Inset{left: 8 right: 2 top: 2 bottom: 2}
                align: Align{y: 0.5}
                draw_bg +: { color: theme.color_u_1 border_radius: 5.0 }
                selection_kind := Meta{
                    text: "Selected ·"
                    draw_text +: { text_style: theme.font_regular{font_size: 8.0} }
                }
                selection_label := Label{
                    width: Fill
                    max_lines: 1
                    text: ""
                    draw_text +: {
                        color: theme.color_text
                        text_style: theme.font_regular{font_size: 8.0}
                    }
                }
                Tip{
                    text: "Clear the selection"
                    place: TopEnd
                    selection_clear := CloseButtonSm{}
                }
            }

            // Pictures that go with the next line.
            attach_row := View{
                visible: false
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                padding: Inset{left: 4 right: 0 bottom: 2}
                align: Align{y: 0.5}
                attach_thumb := Image{width: 44 height: 32 fit: ImageFit.Smallest}
                attach_label := Label{
                    width: Fill
                    text: ""
                    draw_text +: {
                        color: theme.color_text_meta
                        text_style: theme.font_regular{font_size: 8.5}
                    }
                }
                Tip{
                    text: "Remove the pictures"
                    attach_clear := CloseButtonSm{}
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

/// What the panel tells its host.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum AiChatPanelAction {
    /// A native tool's whole JSON prefix, for the host's live preview.
    ToolCallPartial { call_id: String, name: String, partial_json: String },
    /// Esc with an empty composer and no turn in flight: the host may hide
    /// the pane.
    Close,
    /// The person cleared the selected element under the context line: the
    /// host deselects it on the page (the next target it sets has none).
    SelectionCleared,
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct AiChatPanel {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The host's model for this panel, by provider slug (`claude-cli`,
    /// `codex-cli`, `local`, …); empty keeps the person's saved choice.
    /// `MAKEPAD_AI_PROVIDER` overrides both.
    #[live]
    provider: String,
    /// Pictures waiting for the next line.
    #[rust]
    attachments: Vec<Attachment>,
    /// Dropped pictures being decoded on the task pool.
    #[rust]
    loading: Vec<TaskHandle<Result<Attachment, String>>>,
    /// The attachment chip shows this many pictures.
    #[rust]
    attach_shown: Option<(usize, usize)>,
    /// Stage + ETA of the model's current load (None unless loading).
    #[rust]
    load_track: ProgressTracker,
    /// The choices the provider menu lists, in its order.
    #[rust]
    provider_rows: Vec<ProviderRow>,
    /// The hub discovery answer the menu rows were built from.
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
    /// The process's registry: the panel shows the services every port of this process opened.
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
}

impl AiChatPanel {
    /// The registry a host plugs in-process links into.
    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
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

    fn settings(&mut self) -> &AiSettings {
        if self.settings.is_none() {
            self.settings = Some(AiSettings::load());
        }
        self.settings.as_ref().unwrap()
    }

    /// The engine comes up on first use — a local model is a long load
    /// and no host pays it before the person opens the pane.
    fn ensure_engine(&mut self) -> &mut EngineCore {
        if self.engine.is_none() {
            let lease_id = self.widget_uid().0;
            let mut settings = self.settings().clone();
            // A host (or the environment) naming its model picks it for
            // this panel: that is the person's explicit choice for the app.
            let host = std::env::var(PROVIDER_ENV).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| self.provider.clone());
            if !host.trim().is_empty() {
                settings.provider = ProviderChoice::from_slug(&host);
                settings.local_only = settings.provider.is_local() || settings.provider.slug() == "none";
            } else {
                // The saved (or default) choice is not built into this app
                // (Local without `localai`): start on the first provider
                // that can answer here. Only this panel's copy changes, so
                // a `localai` build still starts on Local; the lock guards
                // a local model this build does not have.
                #[cfg(feature = "engine")]
                if let Some(choice) = fallback_choice(&settings.provider) {
                    settings.provider = choice;
                    settings.local_only = false;
                }
            }
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
                let dir = makepad_platform::home::makepad_home().join("mcpb");
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

    /// A line as if typed and sent — the bridge's `/ai?say=`.
    pub fn say(&mut self, cx: &mut Cx, text: String) {
        self.send(cx, text);
    }

    /// Images the model sees with its next input — for a host's tool that
    /// captured something while a tool round is in flight (its results
    /// carry the pictures). `Err` when the model cannot see images.
    pub fn attach_for_model(&mut self, images: Vec<ModelImage>) -> Result<(), String> {
        self.ensure_engine().model_mut().attach_images(images)
    }

    /// A picture for the person's next line, as if dropped on the panel.
    pub fn attach(&mut self, cx: &mut Cx, source: Source) {
        match cx.task_pool().submit(Lane::Light, move || attach::load(source)) {
            Ok(handle) => self.loading.push(handle),
            Err(e) => self.note(cx, format!("could not read the picture ({e:?})")),
        }
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
            let rows = self.provider_rows.clone();
            let engine = self.engine.as_mut().unwrap();
            engine.set_model(model);
            engine.set_provider_facts(choice, rows, local_only);
        }
        self.view.redraw(cx);
    }

    fn note(&mut self, cx: &mut Cx, text: String) {
        self.notice = Some(text);
        self.view.redraw(cx);
    }

    fn poll_loading(&mut self, cx: &mut Cx) {
        let mut i = 0;
        while i < self.loading.len() {
            match self.loading[i].try_take() {
                Some(result) => {
                    self.loading.remove(i).detach();
                    match result {
                        Ok(Ok(picture)) => {
                            self.attachments.push(picture);
                            // At most four pictures ride with one line.
                            if self.attachments.len() > 4 {
                                self.attachments.remove(0);
                            }
                        }
                        Ok(Err(e)) => self.note(cx, e),
                        Err(e) => self.note(cx, format!("could not read the picture ({e:?})")),
                    }
                    self.view.redraw(cx);
                }
                None => i += 1,
            }
        }
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
                let sources: Vec<Source> = drop.items.iter().filter_map(Source::from_drag_item).collect();
                if sources.is_empty() {
                    self.note(cx, "drop a PNG, JPEG or WebP picture".into());
                }
                for source in sources.into_iter().take(4) {
                    self.attach(cx, source);
                }
            }
            _ => {}
        }
    }

    fn send(&mut self, cx: &mut Cx, text: String) {
        let now = Self::now(cx);
        self.notice = None;
        // A pasted path to a picture is an attachment, not a line.
        if let Some(source) = Source::from_pasted_text(&text) {
            self.attach(cx, source);
            self.view.text_input(cx, ids!(input)).set_text(cx, "");
            return;
        }
        let mut text = text;
        if !self.attachments.is_empty() && !text.trim().starts_with('/') {
            let pictures = std::mem::take(&mut self.attachments);
            let names: Vec<String> = pictures.iter().map(|p| p.name.clone()).collect();
            let images = pictures.into_iter().map(|p| ModelImage { label: format!("reference: {}", p.name), png: p.png }).collect();
            match self.ensure_engine().model_mut().attach_images(images) {
                Ok(()) => {
                    if text.trim().is_empty() {
                        text = "Use the attached picture as the reference.".into();
                    }
                    text.push_str(&format!("\n[attached: {}]", names.join(", ")));
                }
                Err(e) => self.note(cx, format!("the pictures were not sent: {e}")),
            }
            self.attach_shown = None;
        }
        self.ensure_engine().send(&text, now);
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

    /// The line at the end of the conversation while something happens:
    /// `None` when idle.
    fn work_line(state: &EngineState) -> Option<String> {
        let rate = state.rate.map(|r| format!(" · {r:.0} tok/s")).unwrap_or_default();
        match &state.status {
            Status::Idle | Status::Error(_) => None,
            Status::Loading { phase, fraction } => Some(format!("Loading the model · {phase} {:.0}%", fraction * 100.0)),
            Status::Thinking => {
                if state.thinking.is_empty() {
                    Some("Thinking…".to_string())
                } else {
                    let tail: String = state.thinking.chars().rev().take(140).collect::<Vec<_>>().into_iter().rev().collect();
                    Some(format!("Thinking… {}", tail.split_whitespace().collect::<Vec<_>>().join(" ")))
                }
            }
            Status::Streaming => Some(format!("Writing{rate}")),
            Status::WaitingForTool => {
                let running = state.entries.iter().rev().find_map(|entry| match entry {
                    Entry::Tool(t) if matches!(t.status, ToolStatus::Running { .. } | ToolStatus::NotResponding { .. }) => Some(t.tool.clone()),
                    _ => None,
                });
                Some(match running {
                    Some(tool) => format!("Running {tool}…"),
                    None => "Waiting for the app…".to_string(),
                })
            }
        }
    }

    /// A tool card's one line: the call without the service's name (the
    /// context line already says where it runs), its arguments as plain
    /// `key: value` text.
    fn tool_line(t: &ToolEntry) -> String {
        let prefix = format!("{} · ", t.service_label);
        let title = t.title.strip_prefix(prefix.as_str()).unwrap_or(&t.title);
        let line = title.replace("\":\"", ": ").replace("\":", ": ").replace("\",\"", ", ").replace('"', "");
        // The engine cuts long arguments: no dangling separator at the cut.
        line.trim_end_matches([',', ' ', '{', '[', ':']).to_string()
    }

    /// Open the platform's picture picker; the answer arrives as a
    /// `FileDialogAction` in `handle_actions`.
    fn pick_pictures(&mut self, cx: &mut Cx) {
        let dialog = FileDialog::new()
            .set_id(ATTACH_DIALOG)
            .set_title("Attach a picture".to_string())
            .set_multiple(true)
            .add_filter("Pictures".to_string(), vec!["png".into(), "jpg".into(), "jpeg".into(), "webp".into()]);
        cx.open_select_file_dialog(dialog);
    }
}

impl Drop for AiChatPanel {
    fn drop(&mut self) {
        // The engine only enqueues Unsubscribe frames. Flush them while the
        // hosted bus and registry still exist; field destruction is too late.
        if let Some(engine) = self.engine.as_mut() {
            engine.shutdown();
        }
        self.bus.relay_down(&self.registry);
    }
}

impl Widget for AiChatPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.bus.set_context(cx);
        if let Event::Custom(json) = event {
            self.on_custom(json);
        }
        self.handle_drop(cx, event);
        if !self.loading.is_empty() {
            self.poll_loading(cx);
        }
        // Esc in the composer: stop a running turn; with nothing running
        // and an empty composer, ask the host to hide the pane. An Esc
        // meant for the host (leaving a presentation, a menu) is not ours.
        if let Event::KeyDown(ke) = event {
            let composing = cx.has_key_focus(self.view.text_input(cx, ids!(input)).area());
            if ke.key_code == KeyCode::Escape && composing {
                let busy = self.engine.as_ref().map(|e| e.state().status.is_busy()).unwrap_or(false);
                let empty = self.view.text_input(cx, ids!(input)).text().trim().is_empty();
                if busy {
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
        if self.engine.is_none() && cx.global::<AiSlotRequests>().engine_at_start {
            self.ensure_engine();
        }
        // Discovery of the AI hub landed (or was asked again): the menu's
        // hub row follows it.
        #[cfg(feature = "engine")]
        if self.engine.is_some() && self.hub_seen != hub_generation() {
            self.hub_seen = hub_generation();
            let row = hub_row();
            if let Some(at) = self.provider_rows.iter().position(|r| r.choice.is_local()) {
                if self.provider_rows[at] != row {
                    self.provider_rows[at] = row;
                    let engine = self.engine.as_mut().unwrap();
                    let (provider, local_only) = (engine.state().provider.clone(), engine.state().local_only);
                    engine.set_provider_facts(provider, self.provider_rows.clone(), local_only);
                    self.view.redraw(cx);
                }
            }
        }
        // Drive the engine on every event; the bus relays what it sent.
        let now = Self::now(cx);
        let mut changed = false;
        let mut busy = false;
        let uid = self.widget_uid();
        if let Some(engine) = self.engine.as_mut() {
            for ev in engine.pump(now) {
                match ev {
                    EngineEvent::Changed => changed = true,
                    EngineEvent::Confirm { .. } => changed = true,
                    EngineEvent::ToolCallPartial { call_id, name, partial_json } => {
                        cx.widget_action(uid, AiChatPanelAction::ToolCallPartial { call_id, name, partial_json });
                    }
                }
            }
            busy = engine.needs_pump()
                || engine.state().status.is_busy()
                || matches!(engine.state().status, Status::Loading { .. });
        }
        self.bus.relay_down(&self.registry);
        if changed {
            self.view.redraw(cx);
        }
        self.publish_transcript(cx);
        if busy {
            self.next_frame = cx.new_next_frame();
        }
        self.view.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The engine exists once the panel is on screen.
        let _ = self.ensure_engine();
        let state = self.engine.as_ref().unwrap().state().clone();
        self.drawn_generation = state.generation;
        let mcp_row = self.view.view(cx, ids!(mcp_row));
        mcp_row.set_visible(cx, state.provider.slug() == CLAUDE_DESKTOP);

        // The header's dot: ready, working, or nothing answers.
        let silent = matches!(state.status, Status::Error(_)) || state.provider_label == "No model";
        let working = state.status.is_busy() || matches!(state.status, Status::Loading { .. });
        let dot = if silent {
            StatusKind::Error
        } else if working {
            StatusKind::InProgress
        } else {
            StatusKind::Success
        };
        self.view.widget(cx, ids!(status_dot)).set_text(cx, dot.name());

        // The line at the end of the conversation: what happens now, or
        // what went wrong.
        let (line, failed) = match (&self.notice, &state.status) {
            (Some(notice), _) => (Some(notice.clone()), true),
            (None, Status::Error(e)) => (Some(e.clone()), true),
            (None, _) => (Self::work_line(&state), false),
        };
        let work = match line {
            Some(line) => {
                let (line, bar) = self.load_line(&state, line);
                Some((line, bar, failed))
            }
            None => {
                let _ = self.load_line(&state, String::new());
                None
            }
        };

        // Send turns into Stop while a turn runs; with nothing typed and no
        // picture it is greyed.
        let busy = state.status.is_busy();
        self.view.button(cx, ids!(send_button)).set_visible(cx, !busy);
        self.view.button(cx, ids!(stop_button)).set_visible(cx, busy);
        let typed = !self.view.text_input(cx, ids!(input)).text().trim().is_empty() || !self.attachments.is_empty();
        self.view.widget(cx, ids!(send_button)).set_disabled(cx, !typed);

        let shown = (self.attachments.len(), self.loading.len());
        if self.attach_shown != Some(shown) {
            self.attach_shown = Some(shown);
            self.view.view(cx, ids!(attach_row)).set_visible(cx, shown.0 + shown.1 > 0);
            let mut names: Vec<String> = self.attachments.iter().map(|a| format!("{} ({}×{})", a.name, a.width, a.height)).collect();
            if !self.loading.is_empty() {
                names.push("reading…".into());
            }
            self.view.label(cx, ids!(attach_label)).set_text(cx, &names.join(", "));
            if let Some(thumb) = self.attachments.last().and_then(|a| a.thumb.clone()) {
                let _ = self.view.image(cx, ids!(attach_thumb)).load_png_from_data(cx, &thumb);
            }
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
        let selection = state.target.as_ref().and_then(|target| target.selection.as_deref());
        self.view.view(cx, ids!(selection_row)).set_visible(cx, selection.is_some());
        if let Some(selection) = selection {
            self.view.label(cx, ids!(selection_label)).set_text(cx, selection);
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
            if self.pick_shown.as_ref() != Some(&(labels.clone(), at)) {
                self.pick_shown = Some((labels.clone(), at));
                let pick = self.view.drop_down(cx, ids!(provider_pick));
                pick.set_labels(cx, labels);
                pick.set_disabled_items(cx, self.provider_rows.iter().map(|r| r.unavailable.is_some()).collect());
                pick.set_selected_item(cx, at);
            }
        }
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else { continue };
            let total = state.entries.len();
            list.set_item_range(cx, 0, total + work.is_some() as usize);
            while let Some(index) = list.next_visible_item(cx) {
                if index >= total {
                    // The work line follows the last entry (the list asks
                    // past its range to fill the view: those stay empty).
                    let Some((line, bar, failed)) = work.as_ref().filter(|_| index == total) else { continue };
                    let row = list.item(cx, index, id!(WorkRow));
                    row.label(cx, ids!(work_label)).set_text(cx, line);
                    row.widget(cx, ids!(work_dot)).set_text(cx, if *failed { StatusKind::Error } else { StatusKind::InProgress }.name());
                    row.view(cx, ids!(load_row)).set_visible(cx, bar.is_some());
                    if let Some(value) = bar {
                        row.progress_bar(cx, ids!(load_bar)).set_value(cx, *value);
                    }
                    row.draw_all(cx, &mut Scope::empty());
                    continue;
                }
                let entry = &state.entries[index];
                let row = match entry {
                    Entry::User { text } => {
                        let row = list.item(cx, index, id!(UserRow));
                        row.label(cx, ids!(user_text)).set_text(cx, text);
                        row
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
                        let values = self.card_edits.entry(call_id.clone()).or_insert_with(|| card.values()).clone();
                        self.draw_card(cx, &row, call_id, card, &values);
                        row
                    }
                    Entry::Tool(t) if matches!(t.status, ToolStatus::Confirm) => {
                        let row = list.item(cx, index, id!(ConfirmRow));
                        row.label(cx, ids!(confirm_title)).set_text(cx, &format!("{} changes things outside the app. Run it?", Self::tool_line(t)));
                        row
                    }
                    Entry::Tool(t) => {
                        let row = list.item(cx, index, id!(ToolRow));
                        let (kind, note, problem, permille, detail) = match &t.status {
                            ToolStatus::Running { note, permille } => {
                                let note = if note.is_empty() && *permille > 0 { format!("{}%", permille / 10) } else { note.clone() };
                                (StatusKind::InProgress, note, String::new(), *permille, String::new())
                            }
                            ToolStatus::NotResponding { note, permille } => {
                                (StatusKind::Pending, "not responding".into(), String::new(), *permille, note.clone())
                            }
                            ToolStatus::Done { outcome, note, text } if outcome.is_ok() => {
                                let note = if note.is_empty() || note == "ok" { String::new() } else { note.clone() };
                                (StatusKind::Success, note, String::new(), 1000, text.clone())
                            }
                            ToolStatus::Done { outcome, note, text } => {
                                let problem = if note.is_empty() { text.lines().next().unwrap_or("").to_string() } else { note.clone() };
                                (StatusKind::Error, outcome.slug().to_string(), problem, 1000, text.clone())
                            }
                            ToolStatus::Confirm => (StatusKind::Pending, String::new(), String::new(), 0, String::new()),
                        };
                        row.widget(cx, ids!(tool_dot)).set_text(cx, kind.name());
                        row.label(cx, ids!(tool_title)).set_text(cx, &Self::tool_line(t));
                        row.label(cx, ids!(tool_note)).set_text(cx, &note);
                        row.button(cx, ids!(tool_stop)).set_visible(cx, matches!(t.status, ToolStatus::Running { .. } | ToolStatus::NotResponding { .. }));
                        let problem_label = row.label(cx, ids!(tool_problem));
                        problem_label.set_visible(cx, !problem.is_empty() && !t.expanded);
                        problem_label.set_text(cx, &problem);
                        let bar_visible = matches!(t.status, ToolStatus::Running { .. });
                        row.view(cx, ids!(tool_bar)).set_visible(cx, bar_visible);
                        if bar_visible {
                            // The bar is a fraction of the row's inner width
                            // (the row's rect is last frame's; the first
                            // frame of a card draws it at a token width).
                            let inner = (row.area().rect(cx).size.x - 40.0).max(24.0);
                            let px = inner * (permille as f64 / 1000.0).max(0.05);
                            let mut bar = row.view(cx, ids!(tool_bar));
                            script_apply_eval!(cx, bar, { width: #(px) });
                        }
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
        if !self.composer_focused {
            self.composer_focused = true;
            self.view.text_input(cx, ids!(input)).take_key_focus(cx);
        }
        DrawStep::done()
    }
}

impl AiChatPanel {
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
            let args = card.args(edits).to_json();
            // A refusal leaves the card parked with its reasons on it.
            if self.ensure_engine().confirm_with(call_id, true, Some(args), now).is_ok() {
                self.card_edits.remove(call_id);
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
            if !text.trim().is_empty() || !self.attachments.is_empty() {
                self.send(cx, text);
            }
        } else if self.view.button(cx, ids!(send_button)).clicked(actions) {
            let text = input.text();
            if !text.trim().is_empty() || !self.attachments.is_empty() {
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
        for action in actions {
            match action.downcast_ref::<FileDialogAction>() {
                Some(FileDialogAction::FileSelected { id, paths }) if *id == ATTACH_DIALOG => {
                    for path in paths.iter().take(4) {
                        self.attach(cx, Source::Path(path.clone()));
                    }
                }
                Some(FileDialogAction::FileLoaded { id, files }) if *id == ATTACH_DIALOG => {
                    for file in files.iter().take(4) {
                        self.attach(cx, Source::Bytes { name: file.name.clone(), bytes: file.bytes.clone() });
                    }
                }
                _ => {}
            }
        }
        if self.view.button(cx, ids!(selection_clear)).clicked(actions) {
            // Shown cleared at once; the host deselects on the page.
            let target = self.ensure_engine().state().target.clone();
            if let Some(mut target) = target {
                target.selection = None;
                self.ensure_engine().set_target(Some(target));
            }
            cx.widget_action(self.widget_uid(), AiChatPanelAction::SelectionCleared);
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(attach_clear)).clicked(actions) {
            self.attachments.clear();
            self.attach_shown = None;
            self.view.redraw(cx);
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
            self.ensure_engine().clear(now);
            self.view.redraw(cx);
        }
        // Cards: confirm buttons and click-to-expand.
        let list = self.view.portal_list(cx, ids!(transcript));
        let items = list.items_with_actions(actions);
        if !items.is_empty() {
            let now = Self::now(cx);
            let call_ids: Vec<Option<String>> = {
                let state = self.ensure_engine().state();
                items
                    .iter()
                    .map(|(index, _)| match state.entries.get(*index) {
                        Some(Entry::Tool(t)) => Some(t.call_id.clone()),
                        _ => None,
                    })
                    .collect()
            };
            for ((_, item), call_id) in items.iter().zip(call_ids) {
                let Some(call_id) = call_id else { continue };
                if self.card_actions(cx, item, actions, &call_id, now) {
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(tool_stop)).clicked(actions) {
                    self.ensure_engine().stop_call(&call_id);
                    self.view.redraw(cx);
                } else if item.button(cx, ids!(run_button)).clicked(actions) {
                    self.ensure_engine().confirm(&call_id, true, now);
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
