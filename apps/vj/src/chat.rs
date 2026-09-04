//! The GEN drawer's chat: a THIN client of the Asset Server's chat broker.
//!
//! The mechanics — the session, the worker thread on a channel, the
//! transcript with its tool chips and rate meter, cancel and clear — are the
//! shared component in [`makepad_chat_ui`], the exact flow the asset
//! UI and the game sandbox run. This file is what makes it the VJ's chat:
//!
//! - it opens the session as `("gen", "vj")` — the generation namespace this
//!   app already publishes into, and the vj client profile, so the broker
//!   assembles the performer context (see `libs/asset/chat/context/vj.md`)
//!   and its tool surface;
//! - the vj profile parks NOTHING back on the client, so any stray parked
//!   call is answered honestly by [`NoClientTools`] instead of timing out.
//!
//! The app never talks to a fleet box: the server picks the serving node,
//! executes catalog and operation tools with its own credentials, and
//! streams the turn back over `/v1/chat/sessions/*`.

use makepad_chat_ui::{ChatFeed, FeedConfig, NoClientTools};
use makepad_asset_client::{ApiEndpoints, ChatAttachment};
use makepad_widgets::Cx;
use std::path::PathBuf;

/// The transcript and its rate meter are the shared component's; this app
/// only reads them.
pub use makepad_chat_ui::{ChatData, ChatRole};

/// The VJ's handle on the chat: the shared feed once the asset server
/// session is up. No app-owned tool state — the vj profile has none.
#[derive(Default)]
pub struct ChatBridge {
    feed: Option<ChatFeed>,
}

impl ChatBridge {
    /// The store session is up: open the chat on its broker. The session
    /// itself is created lazily, on the first turn.
    pub fn connect(
        &mut self,
        cx: &Cx,
        endpoints: ApiEndpoints,
        token: Option<String>,
        cache: PathBuf,
    ) {
        ChatData::set_status("Asset server connected · opening Qwen on the first message");
        self.feed = Some(ChatFeed::start(
            FeedConfig::new(endpoints, token, cache, "gen", "vj"),
            Box::new(NoClientTools),
            cx.thread_spawner(),
        ));
    }

    /// The session died: drop the feed (its worker retires the broker
    /// session) so the next `connect` opens a fresh one.
    pub fn disconnect(&mut self) {
        if self.feed.take().is_some() {
            ChatData::set_status("asset server lost — the chat reopens with the session");
        }
    }

    pub fn send(&self, text: String, attachments: Vec<ChatAttachment>) {
        match &self.feed {
            Some(feed) => {
                // The app owns the user's bubble (see `ChatFeed::send`):
                // exactly one push per message, from here.
                ChatData::push(ChatRole::User, &text);
                feed.send(text, attachments);
            }
            None => {
                ChatData::push(ChatRole::User, &text);
                ChatData::push(
                    ChatRole::System,
                    "The asset server session is not up yet — the chat opens as soon as it is.",
                );
            }
        }
    }

    /// Stop button: end the reply in flight.
    pub fn cancel(&self) {
        if let Some(feed) = &self.feed {
            feed.cancel();
        }
    }

    /// Clear: wipe the transcript AND retire the session — the next message
    /// starts a conversation the model has no memory of.
    pub fn clear(&self) {
        match &self.feed {
            Some(feed) => feed.clear(),
            None => ChatData::clear(),
        }
    }

    /// True once the store session was handed over (a second connect is a
    /// second session on the broker, so the host asks first).
    pub fn is_linked(&self) -> bool {
        self.feed.is_some()
    }

    pub fn take_dirty(&self) -> bool {
        self.feed.as_ref().is_some_and(|feed| feed.take_dirty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One send, one user bubble (the law the asset UI pinned when both
    /// halves pushed it and every message appeared twice) — and before the
    /// session is up the chat says so instead of swallowing the message.
    /// One test, not two: the transcript is a process-global and parallel
    /// tests would race on it.
    #[test]
    fn a_send_puts_one_bubble_up_and_says_when_the_session_is_down() {
        let bridge = ChatBridge::default();
        assert!(!bridge.is_linked());
        ChatData::clear();
        bridge.send("play something darker".into(), Vec::new());
        let data = makepad_chat_ui::CHAT.read().unwrap();
        let users: Vec<&str> = data
            .messages
            .iter()
            .filter(|m| m.role == ChatRole::User)
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(users, vec!["play something darker"], "{:?}", data.messages);
        assert!(
            data.messages
                .iter()
                .any(|m| m.role == ChatRole::System && m.text.contains("not up yet")),
            "{:?}",
            data.messages
        );
        drop(data);
        ChatData::clear();
    }
}
