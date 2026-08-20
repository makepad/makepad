//! Shared asset preview widgets.
//!
//! One pool of views for catalog content, instantiated by every host that
//! shows an asset: the asset UI's Create viewer and Library preview today,
//! the VJ and DJ surfaces and the sandbox next. Before this crate each app
//! grew its own copy — the asset UI's Library well drew a 512×128 waveform
//! strip stretched into a tall panel while the DJ had a real wave view three
//! directories away.
//!
//! # Seams
//!
//! Widgets here take TYPED content in and hand ACTIONS back out:
//!
//! - in: decoded textures, blob bytes, and manifest facts ([`PreviewContent`])
//! - out: what the user did to them ([`AudioAction`]), for the host to
//!   carry out against whatever it owns
//!
//! Nothing in here reaches into an app's state, owns an audio device, opens
//! a file or talks to a server. A preview is HANDED content; it does not go
//! and get it. That is what lets the same widget sit in an app that streams
//! from the asset store and in one that plays local decks.
//!
//! # Two faces
//!
//! - **The widget layer — this crate.** Depends on the widget library and
//!   NOTHING else: no asset-server client, no content contract, no
//!   renderer. `apps/sandbox` is a nested cargo workspace that cannot
//!   depend on `makepad-asset-client` at all, and it must be able to adopt
//!   these widgets with its own blob supply, so that dependency can never
//!   appear here.
//! - **The client-wired face — the host.** Resolving an asset id to its
//!   head revision, streaming a blob, decoding it into a texture: that is
//!   the asset UI's `store_content` / IO worker today, the VJ's media lane,
//!   and the sandbox's own fetcher. Each hands the result to the same
//!   widgets.
//!
//! # Styling
//!
//! Every widget registers a `set_type_default()` base with theme-driven
//! draw props, so a host restyles by overriding those props in its own
//! `script_mod` — never by forking the widget.

use makepad_widgets::*;

pub mod audio_view;
pub mod preview;

pub use audio_view::{AudioAction, AudioView};
pub use preview::{ContentPreview, PreviewContent};

/// Register every shared preview widget. A host calls this once, after
/// `makepad_widgets::script_mod`, before its own UI module.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::audio_view::script_mod(vm);
    crate::preview::script_mod(vm);
}
