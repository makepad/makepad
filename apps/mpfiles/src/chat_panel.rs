//! The chat panel: the transcript widget, the state it draws from, and the
//! panel's own shape.
//!
//! The panel lives on the right of the window and slides out over nothing —
//! it is a column in the body row, so opening it narrows the folder view
//! rather than covering it. Everything it shows comes from [`ChatState`],
//! which the shell hands down as the event scope; the transcript itself is a
//! `PortalList` so a long conversation costs the same as a short one.

use makepad_widgets::*;

/// Who said one line of the transcript.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChatVoice {
    /// The person, as they typed it.
    User,
    /// The model.
    #[default]
    Assistant,
    /// A tool it ran — "looked at ~/local/maps — 12 entries". Dim, because it
    /// is what happened rather than what was said.
    Tool,
    /// The app itself: loading, errors, interruptions.
    Info,
}

#[derive(Clone, Debug)]
pub struct ChatLine {
    pub voice: ChatVoice,
    pub text: String,
}

/// Everything the panel draws. Handed to the widget tree as the event scope.
#[derive(Default)]
pub struct ChatState {
    pub lines: Vec<ChatLine>,
    /// The answer being written right now, shown as a live last row.
    pub pending: String,
}

/// The most lines kept. A file question is a short conversation, and a
/// transcript that grows without limit is a leak with a scrollbar.
const MAX_LINES: usize = 400;

impl ChatState {
    pub fn push(&mut self, voice: ChatVoice, text: impl Into<String>) {
        self.lines.push(ChatLine {
            voice,
            text: text.into(),
        });
        if self.lines.len() > MAX_LINES {
            let cut = self.lines.len() - MAX_LINES;
            self.lines.drain(..cut);
        }
    }

    /// Turn whatever has streamed in so far into a real line.
    pub fn commit_pending(&mut self) -> bool {
        let text = std::mem::take(&mut self.pending);
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let text = text.to_string();
        self.push(ChatVoice::Assistant, text);
        true
    }

    /// How many rows the list draws: the lines, plus the one being written.
    pub fn row_count(&self) -> usize {
        self.lines.len() + usize::from(!self.pending.trim().is_empty())
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.MpfTranscriptBase = #(MpfTranscript::register_widget(vm))

    let ChatLine = View{
        width: Fill
        height: Fit
        padding: Inset{left: 14 right: 12 top: 3 bottom: 3}
        line_label := Label{
            width: Fill
            height: Fit
            draw_text +: {
                color: mod.mpf.fg
                text_style: theme.font_regular{font_size: 9.5}
            }
        }
    }

    let ChatButton = RectView{
        width: Fit
        height: 26
        padding: Inset{left: 12 right: 12}
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: mod.mpf.bg_light
            border_color: mod.mpf.muted
            border_size: 1.0
        }
        chat_button_label := Label{
            draw_text +: {
                color: mod.mpf.fg_bright
                text_style: theme.font_regular{font_size: 9.0}
            }
        }
    }

    mod.widgets.MpfChatPanel = SolidView{
        visible: false
        width: 340
        height: Fill
        flow: Down
        draw_bg +: {color: mod.mpf.bg_dark}

        chat_header := SolidView{
            width: Fill
            height: 36
            flow: Right
            padding: Inset{left: 16 right: 10}
            align: Align{y: 0.5}
            draw_bg +: {color: mod.mpf.bg_light}
            Label{
                width: Fill
                text: "Ask about these files"
                draw_text +: {
                    color: mod.mpf.fg_bright
                    text_style: theme.font_bold{font_size: 10.0}
                }
            }
            chat_close := View{
                width: 20
                height: 20
                align: Align{x: 0.5 y: 0.5}
                cursor: MouseCursor.Hand
                Icon{
                    icon_walk: Walk{width: 10 height: 10}
                    draw_icon +: {
                        svg: crate_resource("self://resources/icons/close.svg")
                        color: mod.mpf.fg_dim
                    }
                }
            }
        }

        chat_status := Label{
            width: Fill
            height: Fit
            padding: Inset{left: 14 right: 12 top: 6 bottom: 4}
            max_lines: 3
            text: "The model loads the first time you open this."
            draw_text +: {
                color: mod.mpf.fg_dim
                text_style: theme.font_regular{font_size: 8.5}
            }
        }

        chat_transcript := mod.widgets.MpfTranscriptBase{
            width: Fill
            height: Fill
            chat_list := PortalList{
                width: Fill
                height: Fill
                UserLine := ChatLine{
                    padding: Inset{left: 14 right: 12 top: 9 bottom: 3}
                    line_label +: {
                        draw_text +: {
                            color: mod.mpf.fg_bright
                            text_style: theme.font_bold{font_size: 9.5}
                        }
                    }
                }
                AssistantLine := ChatLine{}
                ToolLine := ChatLine{
                    padding: Inset{left: 22 right: 12 top: 2 bottom: 2}
                    line_label +: {
                        draw_text +: {
                            color: mod.mpf.fg_dim
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                }
                InfoLine := ChatLine{
                    padding: Inset{left: 14 right: 12 top: 2 bottom: 2}
                    line_label +: {
                        draw_text +: {
                            color: mod.mpf.accent
                            text_style: theme.font_regular{font_size: 8.5}
                        }
                    }
                }
            }
        }

        chat_about := SolidView{
            width: Fill
            height: Fit
            padding: Inset{left: 14 right: 12 top: 5 bottom: 5}
            draw_bg +: {color: mod.mpf.bg}
            chat_about_label := Label{
                width: Fill
                height: Fit
                max_lines: 2
                text: "about: this folder"
                draw_text +: {
                    color: mod.mpf.fg
                    text_style: theme.font_regular{font_size: 8.5}
                }
            }
        }

        chat_input_row := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 6
            padding: Inset{left: 12 right: 12 top: 8 bottom: 4}
            align: Align{y: 0.5}
            chat_input_box := View{
                width: Fill
                height: 28
                chat_input := MpfInput{
                    empty_text: "what is this?"
                }
            }
            chat_send := ChatButton{
                chat_button_label +: {text: "Ask"}
            }
            chat_stop := ChatButton{
                visible: false
                chat_button_label +: {
                    text: "Stop"
                    draw_text +: {color: mod.mpf.accent}
                }
            }
        }

        chat_hint := Label{
            width: Fill
            height: Fit
            padding: Inset{left: 14 right: 12 bottom: 8}
            max_lines: 2
            text: "Reads only — it can look at your files and never change them."
            draw_text +: {
                color: mod.mpf.fg_dim
                text_style: theme.font_regular{font_size: 8.0}
            }
        }
    }
}

/// The transcript: a `PortalList` over the [`ChatState`] in the scope.
#[derive(Script, ScriptHook, Widget)]
pub struct MpfTranscript {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
}

impl Widget for MpfTranscript {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<PortalList>() else {
                continue;
            };
            let Some(chat) = scope.data.get_mut::<ChatState>() else {
                continue;
            };
            let total = chat.row_count();
            list.set_item_range(cx, 0, total);
            while let Some(index) = list.next_visible_item(cx) {
                // The list fills its viewport past the range it was given;
                // the rows past the end have nothing to draw.
                if index >= total {
                    continue;
                }
                let (voice, text) = match chat.lines.get(index) {
                    Some(line) => (line.voice, line.text.clone()),
                    None => (ChatVoice::Assistant, chat.pending.trim_end().to_string()),
                };
                let template = match voice {
                    ChatVoice::User => id!(UserLine),
                    ChatVoice::Assistant => id!(AssistantLine),
                    ChatVoice::Tool => id!(ToolLine),
                    ChatVoice::Info => id!(InfoLine),
                };
                let item = list.item(cx, index, template);
                item.label(cx, ids!(line_label)).set_text(cx, &text);
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pending_answer_is_a_row_until_it_is_committed() {
        let mut chat = ChatState::default();
        assert_eq!(chat.row_count(), 0);
        chat.push(ChatVoice::User, "what is this?");
        assert_eq!(chat.row_count(), 1);
        chat.pending.push_str("It is ");
        assert_eq!(chat.row_count(), 2);
        assert!(chat.commit_pending());
        assert_eq!(chat.row_count(), 2);
        assert_eq!(chat.lines[1].text, "It is");
        // Committing nothing adds nothing.
        assert!(!chat.commit_pending());
        assert_eq!(chat.row_count(), 2);
    }

    #[test]
    fn the_transcript_stops_growing() {
        let mut chat = ChatState::default();
        for i in 0..MAX_LINES + 50 {
            chat.push(ChatVoice::Info, format!("line {i}"));
        }
        assert_eq!(chat.lines.len(), MAX_LINES);
        // The oldest went, the newest stayed.
        assert_eq!(chat.lines.last().unwrap().text, format!("line {}", MAX_LINES + 49));
    }
}
