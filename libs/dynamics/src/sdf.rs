//! Replaces the C's SDF plugin callbacks.

pub trait Sdf: Send + Sync {
    fn distance(&self, point: &[f64; 3]) -> f64;
    fn gradient(&self, gradient: &mut [f64; 3], point: &[f64; 3]);
}

#[derive(Clone)]
pub struct SdfPlugin(pub std::sync::Arc<dyn Sdf>);

impl std::fmt::Debug for SdfPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SdfPlugin")
    }
}

impl PartialEq for SdfPlugin {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}
