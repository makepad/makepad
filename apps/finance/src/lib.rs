//! finance as a library: the ledger, CSV import, reports and charts over a
//! SQLite file, and — for a host that links it — the module ([`module`]):
//! what the phone window manager (and other hosts) seats in a tile
//! IN-PROCESS, one instance per isolate, without a `Window` or a process.
//! The standalone binary (`src/main.rs`, feature `standalone`) is the same
//! crate with a `Window` around `Finance{}`.
//!
//! One root widget ([`view::Finance`]) owns everything: the ledger, the
//! native/demo backend, reports, charts, the demo seed. It is a desktop
//! app when it is wide and a phone app when it is narrow — the same
//! screens, the same data, one code path; what changes with width is the
//! chrome and the density, keyed off the widget's own rect in
//! `draw_walk`, never a window event (see [`view::Layout`]).
//!
//! The database path is a value on the view, set by whoever seats it
//! (see [`view::Finance::set_db_path`]): the standalone window keeps
//! `local/finance/finance.db` from a checkout, the module keeps the
//! makepad home's `finance/finance.db`. Either way a first run with an
//! empty database seeds a generated demo household, so the app is never
//! empty.

#![allow(dead_code)] // ledger, import and report surface built ahead of the views that use it

pub use makepad_widgets;

pub mod ai;
pub mod chart;
pub mod csv;
pub mod date;
#[cfg(all(not(target_arch = "wasm32"), not(feature = "demo")))]
mod db;
pub mod import;
pub mod model;
pub mod module;
pub mod money;
pub mod report;
mod runtime;
mod seed;
pub mod theme;
pub mod view;

pub use module::{FinanceModule, FINANCE_MODULE};
