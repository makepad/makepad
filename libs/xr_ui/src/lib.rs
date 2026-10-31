//! Headset UI for apps that stream their XR view (`makepad-xr-stream`): the same look and the
//! same gestures in every app.
//!
//! - [`XrWristMenu`]: a menu card on the left wrist. The app writes its rows (buttons, toggles,
//!   text) when what they show changes; the menu lays them out, hit-tests the right hand's aim
//!   or fingertip, lights the hover and reports presses.
//! - [`XrPanel`] on a [`Card`]: a floating card (beside a hand, facing the head, pinned in the
//!   room) with the same rows.
//! - [`input`]: aim, trigger and grip the same for controllers and tracked hands; button edges.
//! - [`haptic`]: the standard ticks (hover, press, grab, snap, limit).
//! - [`edit`]: grab-to-edit, one gesture one edit one undo step, through the app's [`EditSink`].
//!
//! Every panel's texture is redrawn only when its rows or hover changed; nothing allocates per
//! frame once the rows are written. See the crate's README for adding XR to an app.

pub mod card;
pub mod edit;
pub mod haptic;
pub mod input;
pub mod menu;
pub mod panel;

pub use card::Card;
pub use edit::{EditSink, GrabEdits};
pub use haptic::{Haptic, Haptics};
pub use menu::{MenuOpen, MenuStep, XrWristMenu};
pub use panel::{CellKind, DrawXrCard, Height, Rows, Tone, Width, XrPanel};

use makepad_widgets::*;

/// The card shader, the panel and the menu (an app with headset UI registers them).
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    panel::script_mod(vm);
    menu::script_mod(vm)
}
