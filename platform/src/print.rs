//! Submit a finished PDF to the operating system's print UI and receive its outcome.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// A PDF and the job title shown by the system print UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintJob {
    pub pdf: Arc<[u8]>,
    pub title: String,
}

/// Identifies one call to [`crate::Cx::print_pdf`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrintId(pub u64);

impl PrintId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The outcome reported by the system print operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrintResult {
    /// The system completed the print operation successfully.
    Printed,
    Cancelled,
    /// This backend has no PDF print UI.
    Unsupported,
    Failed(String),
}

/// Completion of a PDF print request, delivered as [`crate::Event::PrintResult`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintResultEvent {
    pub id: PrintId,
    pub result: PrintResult,
}
