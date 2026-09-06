//! The chat pane every Makepad app puts on screen: ONE component.
//!
//! Two apps show a Qwen chat — the game sandbox and the asset UI — and both
//! want the same machinery:
//!
//! - a session on the Asset Server's chat broker (`/v1/chat/sessions`,
//!   send, events), never a provider call of their own ([`feed`]);
//! - a worker thread on a channel pumping that event stream into
//!   presentation state, so the UI thread never waits on HTTP ([`feed`]);
//! - a transcript with tool chips and a rate meter that reads the serving
//!   box's own token counts, `· thinking` included ([`transcript`]);
//! - Escape to cancel the reply in flight and Clear to start over;
//! - errors shown as system lines in the app's voice, never a raw provider
//!   string dropped into a bubble.
//!
//! What is NOT shared is the personality: [`feed::FeedConfig`] carries the
//! namespace and the declared client profile, and the broker picks the
//! taught context and the tool surface from those. The sandbox opens
//! `("sandbox", "game")` and executes `world.*`; the asset UI opens
//! `("gen", "gen")` and executes the fleet generate tools. Same mechanics,
//! different chat.

use makepad_widgets::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod feed;
#[cfg(target_arch = "wasm32")]
#[path = "portable/feed.rs"]
pub mod feed;
pub mod list;
pub mod transcript;

pub use feed::{ChatFeed, ClientTools, FeedConfig, NoClientTools};
pub use list::{AssetChatList, ThinkingDots};
pub use transcript::{ChatData, ChatMessage, ChatRole, RateMeter, CHAT};

/// Register the shared chat widgets. A host calls this once, after
/// `makepad_widgets::script_mod`, before its own UI module.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::list::script_mod(vm);
}
