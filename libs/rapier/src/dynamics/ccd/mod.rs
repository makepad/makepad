// TODO: not sure why it complains about PredictedImpacts being unused,
//       making it private or pub(crate) triggers a different error.
#[allow(unused_imports)]
pub use self::ccd_solver::{CCDSolver, PredictedImpacts};
pub use self::toi_entry::TOIEntry;

mod ccd_solver;
mod toi_entry;
