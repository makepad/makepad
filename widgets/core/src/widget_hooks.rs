//! Where the optional widget families plug into the core.
//!
//! The core never names a family: a `Window` hosts the design overlay, the
//! voice wave and the AI chat slot, and the widget tree dumps a dock's tabs,
//! but each of those lives in a family crate an app may leave out. A family
//! installs its hooks on the `Cx` when it registers (the same way the tweaker
//! fills `Cx::tweak_callback` and the aichat crate fills `Cx::ai_callback`);
//! a hook nobody installed is `None`, and the core then does what it did
//! when the family was compiled out.
//!
//! Registration order is the other half: the `Window`'s DSL names
//! `VoiceWave`, `Tweaker` and `AiChatSlot`, so those three register where
//! the window expects them ([`WindowFamilies`]). Without the family the core
//! puts an empty hidden view under the name instead.

use crate::{
    makepad_draw::*,
    makepad_micro_serde::*,
    view::View,
    widget::WidgetRef,
};
use std::any::TypeId;

/// Answers whether a family took an event before the window's own dispatch.
pub type WindowIntercept = fn(&mut Cx, &Event, &mut View, WindowId) -> bool;

/// The design overlay (`makepad-widgets-tweaker`).
#[derive(Clone, Copy)]
pub struct TweakerHooks {
    /// Shift+F10 and the pointer while the overlay is on.
    pub window_intercept: WindowIntercept,
    /// The overlay's panel covers this point (a window drag query answers
    /// Client there, not Caption).
    pub panel_owns_pointer: fn(Vec2d) -> bool,
    /// Installed as `Cx::tweak_callback` by `set_ui_root`.
    pub tweak_callback: fn(&mut Cx, &str, &[(String, String)]) -> Result<String, String>,
    /// The widget type the tree tab reports as an inspector.
    pub inspector_type: TypeId,
    /// The design delta since `after_generation`, for the chat's Studio
    /// feedback; `None` while nothing changed.
    pub feedback_snapshot: fn(u64) -> Option<TweakFeedbackSnapshot>,
}

/// The AI chat slot (`makepad-widgets-ai`).
#[derive(Clone, Copy)]
pub struct AiHooks {
    /// F10 and the pointer over its open pane.
    pub window_intercept: WindowIntercept,
}

/// The voice input wave (`makepad-widgets-voice`).
#[derive(Clone, Copy)]
pub struct VoiceHooks {
    /// The window's actions, for the wave in its caption bar to turn into
    /// injected text and Enter.
    pub window_actions: fn(&mut Cx, &Actions, &mut View, &mut Scope),
}

/// The hooks the families installed on this `Cx`.
#[derive(Default)]
pub struct WidgetHooks {
    pub tweaker: Option<TweakerHooks>,
    pub ai: Option<AiHooks>,
    pub voice: Option<VoiceHooks>,
    /// A dock's tabs and tab headers for the widget tree's dumps
    /// (`makepad-widgets-data`); `None` for a widget that is not a dock.
    pub dock_dump: Option<fn(&WidgetRef, &Cx) -> Option<DockCompactDump>>,
}

impl WidgetHooks {
    /// The installed hooks, for a caller holding `&mut Cx`.
    pub fn of(cx: &mut Cx) -> &mut WidgetHooks {
        cx.global::<WidgetHooks>()
    }

    /// The installed hooks, if a family installed any.
    pub fn get(cx: &Cx) -> Option<&WidgetHooks> {
        cx.get_global_ref::<WidgetHooks>()
    }

    pub fn tweaker(cx: &Cx) -> Option<TweakerHooks> {
        Self::get(cx).and_then(|hooks| hooks.tweaker)
    }

    pub fn ai(cx: &Cx) -> Option<AiHooks> {
        Self::get(cx).and_then(|hooks| hooks.ai)
    }

    pub fn voice(cx: &Cx) -> Option<VoiceHooks> {
        Self::get(cx).and_then(|hooks| hooks.voice)
    }

    /// The dock dump of `widget`, when the data family is there and the
    /// widget is a dock.
    pub fn dock_dump(cx: &Cx, widget: &WidgetRef) -> Option<DockCompactDump> {
        (Self::get(cx)?.dock_dump?)(widget, cx)
    }

    /// Whether `type_id` is the design overlay's widget type.
    pub fn is_inspector(cx: &Cx, type_id: Option<TypeId>) -> bool {
        match (Self::tweaker(cx), type_id) {
            (Some(tweaker), Some(type_id)) => tweaker.inspector_type == type_id,
            _ => false,
        }
    }
}

/// The families that register at a fixed place inside the core's
/// `widgets_mod`, because the `Window` names them. Each is the family's
/// registration, which also installs its hooks; `None` puts an empty
/// hidden view under the name.
#[derive(Clone, Copy, Default)]
pub struct WindowFamilies {
    /// Registers `VoiceWave`.
    pub voice: Option<fn(&mut ScriptVm)>,
    /// Registers `Tweaker`.
    pub tweaker: Option<fn(&mut ScriptVm)>,
    /// Registers `AiChatSlot`.
    pub ai: Option<fn(&mut ScriptVm)>,
}

/// A dock's layout, as the widget tree's dumps print it.
#[derive(Clone, Debug, Default)]
pub struct DockCompactDump {
    pub tabs: Vec<DockCompactTabsInfo>,
    pub tab_headers: Vec<DockCompactTabInfo>,
}

#[derive(Clone, Debug)]
pub struct DockCompactTabsInfo {
    pub tabs_id: LiveId,
    pub selected_tab_id: Option<LiveId>,
    pub tab_count: usize,
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct DockCompactTabInfo {
    pub tabs_id: LiveId,
    pub tab_id: LiveId,
    pub is_active: bool,
    pub title: String,
    pub rect: Rect,
}

/// What the world asks of the slot, outside the widget tree: the bridge's
/// `/ai?on=` and `/ai?say=`, the overlay's Escape. A `Cx` global; the
/// slot takes `open`, the overlay takes `say`.
#[derive(Default)]
pub struct AiSlotRequests {
    /// `Some(true)` open, `Some(false)` close; taken by the slot.
    pub open: Option<bool>,
    /// Lines to send as if typed, in order; taken by the overlay.
    pub say: Vec<String>,
    /// The slot's state as it last reported it, for whoever asks.
    pub is_open: bool,
    /// The standalone window whose F10 pane most recently received input.
    pub current_window: Option<usize>,
    /// Ask that window to intercept a drag over its application content.
    pub select_region: Option<usize>,
    /// Completed region in window-local layout coordinates; consumed by chat.
    pub selected_region: Option<AiSelectedRegion>,
    pub region_cancelled: bool,
    /// Enabled by the shared chat module only for a Studio feedback launch.
    pub feedback_enabled: bool,
    /// The chat's engine runs from startup, the pane closed: the app was
    /// started for Claude Desktop, which drives its tools without F10.
    pub engine_at_start: bool,
    pub feedback_selecting: bool,
    pub feedback_busy: bool,
    pub feedback_focus: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct AiSelectedRegion {
    pub window_id: usize,
    pub window_size: Vec2d,
    pub rect: Rect,
}

/// One live design edit the tweaker recorded, as its diff and the chat's
/// Studio feedback carry it.
#[derive(Clone, Debug, SerJson)]
pub struct TweakDiffEntry {
    pub seq: u64,
    pub path: String,
    pub prop: String,
    pub old: String,
    pub new: String,
    /// Where the widget's source object was constructed — `file:line` of
    /// the literal to edit. For a widget built from a template (a tab, a
    /// list item) this is the TEMPLATE's `:=` site, not the instance: that
    /// is what the AI rewrites so every instance follows.
    pub origin: String,
    /// How many other live widgets share that source object and received
    /// the same edit (0 for an ordinary, one-off widget).
    pub siblings: u32,
    /// "this" — specialise this instance (origin = its own site) — or
    /// "all" — every widget of the type (origin = the type's definition).
    pub scope: String,
}

/// A complete current design delta. Empty entries mean all edits were undone.
/// No source files are written by the tweaker or this export.
pub struct TweakFeedbackSnapshot {
    pub generation: u64,
    pub entries: Result<Vec<TweakDiffEntry>, String>,
}
