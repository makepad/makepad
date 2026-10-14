//! Completed Linux capture buffers. The HAL owns the mapped allocation until every recorder
//! lease is released. Handles in ScreenSurface are session-scoped tickets, never raw pointers.
use makepad_relax_api::relax_api;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

pub trait Mapping: Send + Sync {
    fn bytes(&self) -> &[u8];
}
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub bgra: bool,
    pub mapping: Arc<dyn Mapping>,
}
impl Surface {
    pub fn bytes(&self) -> &[u8] {
        self.mapping.bytes()
    }
    pub fn stride(&self) -> usize {
        self.width as usize * 4
    }
}
fn surfaces() -> &'static Mutex<HashMap<(u32, u64), Arc<Surface>>> {
    static SURFACES: OnceLock<Mutex<HashMap<(u32, u64), Arc<Surface>>>> = OnceLock::new();
    SURFACES.get_or_init(Default::default)
}
pub(crate) fn publish(session: u32, surface: Arc<Surface>) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    super::lock(surfaces()).insert((session, id), surface);
    id
}
pub fn take(session: u32, handle: u64) -> Option<Arc<Surface>> {
    super::lock(surfaces()).remove(&(session, handle))
}
pub(crate) fn close_session(session: u32) {
    let retired: Vec<_> = {
        let mut entries = super::lock(surfaces());
        let keys: Vec<_> = entries.keys().filter(|k| k.0 == session).copied().collect();
        keys.into_iter()
            .filter_map(|key| entries.remove(&key))
            .collect()
    };
    drop(retired);
}

/// A recorder's lease of a completed capture buffer (`ScreenSurface`'s handle), the way JIT-
/// compiled code (makepad run) holds one: the buffer and the table it came from are this crate's.
#[relax_api(handle)]
pub struct CaptureLease {
    surface: Option<Arc<Surface>>,
}

/// Takes the buffer `ScreenSurface` named `handle` from `session`'s table; an empty lease when
/// it is gone (the session closed).
#[relax_api]
pub fn take_lease(session: u32, handle: u64) -> CaptureLease {
    CaptureLease { surface: take(session, handle) }
}

#[relax_api]
impl CaptureLease {
    /// A buffer was taken.
    #[relax_api]
    pub fn found(&self) -> bool { self.surface.is_some() }
    #[relax_api]
    pub fn width(&self) -> u32 { self.surface.as_ref().map_or(0, |s| s.width) }
    #[relax_api]
    pub fn height(&self) -> u32 { self.surface.as_ref().map_or(0, |s| s.height) }
    /// BGRA (true) or RGBA (false).
    #[relax_api]
    pub fn bgra(&self) -> bool { self.surface.as_ref().is_some_and(|s| s.bgra) }
    /// Rows of `width * 4` bytes, top row first.
    #[relax_api]
    pub fn bytes(&self) -> &[u8] { self.surface.as_ref().map_or(&[], |s| s.bytes()) }
}
