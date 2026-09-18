//! The super-app engine as one Rust `dylib`: every crate the host and the
//! tile apps share, re-exported so rustc links each of them into this one
//! `.so` and every downstream crate binds it here (`IncludedFromDylib`).
//!
//! A `--extern` crate that is never named is never loaded, hence the
//! re-exports: each one pulls its crate (and that crate's rlib closure)
//! into this dylib.

pub use libloading;
pub use makepad_ai_services;
pub use makepad_app_module;
pub use makepad_base64;
pub use makepad_civil_time;
pub use makepad_converse;
pub use makepad_diskmap;
pub use makepad_geodata;
pub use makepad_html;
pub use makepad_image_tiles;
pub use makepad_map_nav;
pub use makepad_micro_serde;
pub use makepad_network;
pub use makepad_search;
pub use makepad_sqlite;
pub use makepad_strict_json;
pub use makepad_studio_protocol;
pub use makepad_widgets;
pub use makepad_wm_api;
pub use makepad_wm_theme;
