//! The devhub control panel: every node the master knows on one time axis (its leases as thin
//! bars, its load as a soft area), what is stuck first ("needs a look", worst first, each with
//! its detail and the commands that act on it), the fleet's queue and blockers, and the
//! landing train. Client-only: it reads the running master's control socket (`panel`, `land`,
//! `inbox`, `bucket`, `job`, `lanes`) and never writes, serves or takes a lease.
//! `DEVHUB_PANEL_DEMO=1` shows a synthetic fleet instead.
//!
//! The views are widgets (`DevhubPanel`, and its `FleetTimeline`, `NeedsList`, `DetailPane`,
//! `FleetQueue`, `LandStrip`): register them with [`script_mod`] to embed the panel elsewhere.

pub mod demo;
pub mod detail;
pub mod float;
pub mod fmt;
pub mod glance;
pub mod model;
pub mod needs;
pub mod needs_list;
pub mod panel;
pub mod queue;
pub mod source;
pub mod strip;
pub mod timeline;

use makepad_widgets::*;

/// Registers the panel's widgets.
pub fn script_mod(vm: &mut ScriptVm) {
    timeline::script_mod(vm);
    needs_list::script_mod(vm);
    detail::script_mod(vm);
    queue::script_mod(vm);
    strip::script_mod(vm);
    panel::script_mod(vm);
}
