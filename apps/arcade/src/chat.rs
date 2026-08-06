//! The conversation: what the player said, what the AI is saying back, and
//! what the running game is complaining about.
//!
//! The model is a process-global so the list widget can read it during draw
//! without threading a scope through (the same shape gamemaker uses). It holds
//! only presentation state — every edit the AI proposes goes through the
//! intent log in [`crate::authoring`], never from here.

use makepad_widgets::*;

pub static CHAT: std::sync::RwLock<ChatData> = std::sync::RwLock::new(ChatData {
    messages: Vec::new(),
    streaming_text: String::new(),
    activity: String::new(),
    is_streaming: false,
    last_delta: None,
});

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ChatRole {
    User,
    Assistant,
    /// Engine-side trouble (a failed eval, a missing sound). Shown differently
    /// because the player did not say it and the AI did not either.
    System,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

pub struct ChatData {
    pub messages: Vec<ChatMessage>,
    pub streaming_text: String,
    /// What the agent is doing right now, e.g. "Changing the game". Pinned
    /// under the streaming reply so a long silent tool call still moves.
    pub activity: String,
    pub is_streaming: bool,
    pub last_delta: Option<std::time::Instant>,
}

impl ChatData {
    pub fn push(role: ChatRole, text: impl Into<String>) {
        if let Ok(mut data) = CHAT.write() {
            data.messages.push(ChatMessage {
                role,
                text: text.into(),
            });
        }
    }

    pub fn begin_stream() {
        if let Ok(mut data) = CHAT.write() {
            data.streaming_text.clear();
            data.is_streaming = true;
        }
    }

    pub fn push_delta(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            data.streaming_text.push_str(text);
            data.last_delta = Some(std::time::Instant::now());
        }
    }

    /// Land the streamed reply as a message. Returns the number of items the
    /// list should scroll to.
    pub fn end_stream() -> usize {
        let Ok(mut data) = CHAT.write() else { return 0 };
        let text = std::mem::take(&mut data.streaming_text);
        if !text.trim().is_empty() {
            data.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                text,
            });
        }
        data.is_streaming = false;
        data.activity.clear();
        data.messages.len()
    }

    pub fn set_activity(text: &str) {
        if let Ok(mut data) = CHAT.write() {
            data.activity = text.to_string();
        }
    }

    pub fn item_count() -> usize {
        match CHAT.read() {
            Ok(data) => data.messages.len() + data.is_streaming as usize,
            Err(_) => 0,
        }
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ArcadeChatBase = #(ArcadeChat::register_widget(vm))
    mod.widgets.ArcadeChat = set_type_default() do mod.widgets.ArcadeChatBase {
        width: Fill
        height: Fill

        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: false
            auto_tail: true
            smooth_tail: true
            selectable: true

            User := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 40 right: 4}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x2a3a5a
                    radius: 8.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xe8eef8
                    draw_text.text_style: theme.font_regular{font_size: 13}
                }
            }

            Assistant := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 4 right: 40}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x232330
                    radius: 8.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xd8dee8
                    draw_text.text_style: theme.font_regular{font_size: 13}
                }
            }

            System := RoundedView {
                width: Fill
                height: Fit
                margin: Inset{top: 6 bottom: 6 left: 4 right: 4}
                padding: Inset{left: 12 top: 8 right: 12 bottom: 8}
                show_bg: true
                draw_bg +: {
                    color: #x3a2a24
                    radius: 8.0
                }
                body := Label {
                    width: Fill
                    height: Fit
                    text: ""
                    draw_text.color: #xe8c9a0
                    draw_text.text_style: theme.font_regular{font_size: 12}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ArcadeChat {
    #[deref]
    view: View,
}

impl Widget for ArcadeChat {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Ok(data) = CHAT.read() else {
            return DrawStep::done();
        };
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let portal = item.as_portal_list();
            let Some(mut list) = portal.borrow_mut() else {
                continue;
            };
            let msg_count = data.messages.len();
            list.set_item_range(cx, 0, msg_count + data.is_streaming as usize);

            while let Some(item_id) = list.next_visible_item(cx) {
                // The streaming reply is a virtual item past the end.
                if data.is_streaming && item_id == msg_count {
                    let mut text = data.streaming_text.clone();
                    if !data.activity.is_empty() {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(&data.activity);
                    }
                    let widget = list.item(cx, item_id, id!(Assistant));
                    widget.label(cx, ids!(body)).set_text(cx, &text);
                    widget.draw_all_unscoped(cx);
                    continue;
                }
                let Some(msg) = data.messages.get(item_id) else {
                    continue;
                };
                let template = match msg.role {
                    ChatRole::User => id!(User),
                    ChatRole::Assistant => id!(Assistant),
                    ChatRole::System => id!(System),
                };
                let widget = list.item(cx, item_id, template);
                widget.label(cx, ids!(body)).set_text(cx, &msg.text);
                widget.draw_all_unscoped(cx);
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

    /// The chat model is a process-global; tests must not interleave.
    pub static CHAT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear() {
        let mut data = CHAT.write().unwrap();
        data.messages.clear();
        data.streaming_text.clear();
        data.activity.clear();
        data.is_streaming = false;
    }

    #[test]
    fn a_turn_streams_then_lands_as_one_message() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::push(ChatRole::User, "make it rain");
        ChatData::begin_stream();
        assert_eq!(ChatData::item_count(), 2, "the streaming bubble counts");

        ChatData::push_delta("Adding ");
        ChatData::push_delta("rain!");
        assert_eq!(CHAT.read().unwrap().streaming_text, "Adding rain!");

        let count = ChatData::end_stream();
        assert_eq!(count, 2);
        let data = CHAT.read().unwrap();
        assert!(!data.is_streaming);
        assert_eq!(data.messages[1].role, ChatRole::Assistant);
        assert_eq!(data.messages[1].text, "Adding rain!");
        drop(data);
        clear();
    }

    #[test]
    fn an_empty_reply_does_not_leave_a_blank_bubble() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::begin_stream();
        ChatData::push_delta("   \n ");
        ChatData::end_stream();
        assert!(
            CHAT.read().unwrap().messages.is_empty(),
            "whitespace-only replies are dropped"
        );
        clear();
    }

    #[test]
    fn errors_are_injected_as_system_messages() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::push(ChatRole::System, "game.splash:4: unknown verb 'wobble'");
        let data = CHAT.read().unwrap();
        assert_eq!(data.messages[0].role, ChatRole::System);
        assert!(data.messages[0].text.contains("wobble"));
        drop(data);
        clear();
    }

    #[test]
    fn activity_rides_along_with_the_streaming_reply() {
        let _guard = CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        ChatData::begin_stream();
        ChatData::set_activity("Changing the game");
        assert_eq!(CHAT.read().unwrap().activity, "Changing the game");
        // Completing the turn clears it: a finished reply must not keep
        // claiming the AI is still working.
        ChatData::push_delta("Done!");
        ChatData::end_stream();
        assert!(CHAT.read().unwrap().activity.is_empty());
        clear();
    }
}
