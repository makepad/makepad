//! B-rep healing and stitching: closing gaps, merging coincident topology and
//! repairing tolerances of imported models.
//!
//! Upstream: `SmHealData`, `SmHealProbArray`, `SmStitch`, `SmGap` in `source/SMLib`, and
//! `SM_API/SmApiHeal`.

pub mod face_props;
pub mod gap;
pub mod heal_data;
pub mod stitch;
