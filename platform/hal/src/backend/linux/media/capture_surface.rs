//! Completed Linux capture buffers. The HAL owns the mapped allocation until every recorder
//! lease is released. Handles in ScreenSurface are session-scoped tickets, never raw pointers.
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
