// rejects: E0603
mod zv { macro_rules! inner { () => {} } use inner as z; }
pub use zv::z;
