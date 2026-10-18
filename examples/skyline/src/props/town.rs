//! Town: the props layer, four rows of houses on the shared site (level along a quay or climbing a
//! hillside), roofs by the town's style, lit windows in rooms, shop floors, street and quay lamps
//! washing the walls. Owns the town group's shared helpers (the page's `townSite`, `townGround`,
//! `townTd`, `townKit`). Zones: `z.town`, `z.town_x`.
//!
//! Port of the page's `pkTown` (stage C2, group 1): emit instances and parts through [`Ctx`].
//! Stub: emits nothing yet.
use super::kit::Ctx;

pub fn build(_c: &mut Ctx) {}
