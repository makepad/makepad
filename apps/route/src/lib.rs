//! route as a library: the map with its layers, the trip and turn-by-turn
//! navigation, the side panel, the assistant's tools — around one root
//! widget ([`RouteView`], view.rs) — and, for a host that links it, the
//! module ([`module`]): what the window manager seats in a tile
//! IN-PROCESS, one instance per isolate, without a `Window` or a process.
//! The standalone binary (src/main.rs, feature `standalone`) is the same
//! crate with a `Window` around `RouteView{}` and the AI port toward the
//! desktop assistant.
//!
//! Profiles, by feature:
//! - the default (`native`): the desktop with the in-app brain — the
//!   local dispatcher, voice in and out, the cloud escalation, image
//!   search — and the first-run test-map bake;
//! - the lib alone (no features): the same map and tools, no brain — what
//!   a host links; the prompt line says the assistant is not in this build;
//! - `demo`: the hosted demo (nav/api.rs), the site API instead of the
//!   native data plane.

pub use ::makepad_widgets;

#[cfg(all(feature = "demo", any(feature = "native", feature = "bake")))]
compile_error!("feature `demo` excludes `native` and `bake`");

#[cfg(not(feature = "demo"))]
pub mod ai;
pub mod assistant;
#[cfg(not(feature = "demo"))]
pub mod broker;
pub mod chrome;
#[cfg(feature = "native")]
pub mod claude_agent;
#[cfg(feature = "demo")]
pub mod clock;
#[cfg(feature = "native")]
pub mod ddg;
#[cfg(not(feature = "demo"))]
pub mod history;
#[cfg(not(feature = "demo"))]
pub mod layers;
#[cfg(feature = "native")]
pub mod local_agent;
#[cfg(not(feature = "demo"))]
pub mod maps_root;
#[cfg(not(feature = "demo"))]
pub mod module;
pub mod nav;
#[cfg(any(feature = "demo", test))]
pub mod nav_api;
pub mod overlays;
pub mod provisioner;
pub mod side_panel;
#[cfg(feature = "bake")]
pub mod testmap;
#[cfg(not(feature = "demo"))]
pub mod tools;
pub mod trip;
#[cfg(not(feature = "demo"))]
pub mod view;
#[cfg(feature = "native")]
pub mod voice;

pub use chrome::{
    ChatEntry, ChatState, EntryKind, ThemePreference, TiltShiftLayer, TranscriptList,
    AMSTERDAM_CENTER, LOCATION_FIX_TIMEOUT_SECONDS, THEME_STORAGE,
};
#[cfg(feature = "demo")]
pub(crate) use chrome::{
    location_error_status, show_location_status, LocationClick, LocationFix, LocationState,
};
#[cfg(not(feature = "demo"))]
pub use module::{RouteModule, ROUTE_MODULE};
#[cfg(not(feature = "demo"))]
pub use view::RouteView;
