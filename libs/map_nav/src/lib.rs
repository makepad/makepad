//! Offline map navigation primitives shared by the import tooling (which
//! builds the artifacts) and the app/widget side (which queries them).
//!
//! - `search`: the `region.search` place/POI/street index format, builder
//!   and query engine (prefix autocomplete + category synonyms + proximity).
//! - `graph`: the `region.graph` routing graph format, builder from OSM
//!   ways/nodes/restrictions, nearest-edge snapping and A* routing per
//!   travel mode (car / bike / foot).
//! - `nav`: maneuver generation and the turn-by-turn `NavSession` state
//!   machine (map-matching, progress, off-route detection).
//!
//! No UI or makepad dependencies; everything is unit-testable.

pub mod fmt;
pub mod geo;
pub mod graph;
pub mod nav;
pub mod search;
