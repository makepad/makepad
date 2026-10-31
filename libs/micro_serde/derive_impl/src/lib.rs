//! The expansion logic of makepad-micro-serde-derive as a plain library (see
//! makepad-script-derive-impl for the pattern).

mod derive_bin;
mod derive_json;
mod derive_ron;

pub use derive_bin::{derive_de_bin_impl, derive_ser_bin_impl};
pub use derive_json::{derive_de_json_impl, derive_ser_json_impl};
pub use derive_ron::{derive_de_ron_impl, derive_ser_ron_impl};
