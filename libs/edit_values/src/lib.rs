//! The editable-values layer: one model of "a value a person can tune" for
//! every document kind (a Splash scene, a score, a parametric part).
//!
//! - [`ValueSite`]: one value: its id (a stable path the document resolves
//!   again on every read, so bindings survive edits around it), kind, unit,
//!   range, step, where it is written and its spatial handle.
//! - [`ValueSource`]: a document's sites, resolved by id, and written back as
//!   exact byte [`Splice`]s.
//! - [`Edit`] and [`EditSink`]: the one edit format and its contract: every
//!   edit of one gesture is one undo step; a live edit (`done: false`) shows
//!   without committing; the final one commits and re-evaluates.
//! - [`anchor`]: handles in 3D and the drag math the view and the headset
//!   share.
//!
//! Panels, inline code controls, lenses and handles are drawn by their own
//! crates; none of them define value kinds, splices or undo rules of their own.

pub mod anchor;
pub mod edit;
pub mod site;

pub use anchor::{Anchor, DragPointer, Grip};
pub use edit::{apply_splices, normalize_splices, Change, Edit, EditSink, Splice};
pub use site::{Live, Range, SiteId, Taper, Value, ValueKind, ValueSite, ValueSource};
