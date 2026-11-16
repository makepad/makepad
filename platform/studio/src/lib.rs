pub use cursor::MouseCursor;
pub use keyboard::{
    CharOffset, FullTextState, ImeAction, ImeActionEvent, KeyCode, KeyEvent, TextInputEvent,
};
pub use makepad_error_log::LogLevel;
pub use mouse::{KeyModifiers, MouseButton};
pub use shared_framebuf::*;
pub use studio::*;
pub use relay::*;

pub mod cursor;
pub mod hub_protocol;
pub mod keyboard;
pub mod mouse;
pub mod relay;
pub mod shared_framebuf;
pub mod studio;
pub mod native_app;
pub use native_app::*;

/// The hosted-protocol versions this build speaks, oldest and newest. Bump the newest on any
/// change to the hosted wire's types (a new message or field goes at the end, so an older
/// version's peer still decodes everything it knows); raise the oldest only to drop one. A WM's
/// hub admits a client at the version it negotiated on the WM socket (`proto=<v>` in its `/app`
/// URL); a client that states none is admitted only on this build's [`WIRE`]. A host sends a
/// client only messages of the version it was admitted at.
pub const HOSTED_VERSIONS: (u32, u32) = (1, 2);

/// The hosted wire's fingerprint: every source file whose types travel
/// between a host and its `--stdin-loop` children (binary, by ordinal), and
/// `KeyCode`'s. A child states it when it connects (`wire=` in its
/// `/app?...` URL); a host compares it with its own and does not talk to a
/// child built from other sources (it rebuilds it, or says so). Any edit of
/// these files changes it: a child built before the edit is stale.
pub const WIRE: u64 = {
    use makepad_key_code::fingerprint;
    let mut hash = makepad_key_code::SOURCE_FINGERPRINT;
    hash = fingerprint(hash, include_bytes!("lib.rs"));
    hash = fingerprint(hash, include_bytes!("cursor.rs"));
    hash = fingerprint(hash, include_bytes!("hub_protocol.rs"));
    hash = fingerprint(hash, include_bytes!("keyboard.rs"));
    hash = fingerprint(hash, include_bytes!("mouse.rs"));
    hash = fingerprint(hash, include_bytes!("relay.rs"));
    hash = fingerprint(hash, include_bytes!("shared_framebuf.rs"));
    hash = fingerprint(hash, include_bytes!("studio.rs"));
    hash = fingerprint(hash, include_bytes!("native_app.rs"));
    hash
};
