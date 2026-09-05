//! The viewer's decode pool: a baked library is read-only, so serving the
//! grid is nothing but hardware decodes, ranked by how much screen each one
//! would paint.
//!
//! There is no per-view decode pool. Each requested decode is a heavy job on
//! the runtime pool and sends its result back over a [`ToUISender`] (which
//! raises the UI signal, so the widget drains on `Event::Signal`). A pan
//! that leaves work behind calls [`StoreHandle::wants`] with the whole
//! current truth, cancelling queued decodes that are no longer visible.

use crate::library::{display_frame, ItemId, Library};
use crate::tape::{full_frame, read_frame, FullFrame, Planes};
use makepad_widgets::makepad_platform::thread::{
    CancellationToken, Lane, TaskHandle, TaskPool, ToUIReceiver, ToUISender,
};
use std::collections::HashMap;

pub enum StoreEvent {
    /// A sealed shard's page at one level, hardware decoded from tape.
    Page { shard: i64, level: usize, planes: Planes },
    /// A picture at full resolution, mipmapped.
    Full { item: ItemId, px: u32, finest: bool, frame: FullFrame },
    /// A page decode that did not come back; without this the widget's
    /// "already asked" mark would stand for ever.
    PageFailed { shard: i64, level: usize },
    /// A full-frame decode that did not come back; same contract.
    FullFailed { item: ItemId },
}

#[derive(Clone, Copy)]
enum Work {
    DecodePage { shard: i64, level: usize },
    DecodeFull { item: ItemId, px: u32 },
}

struct Pending {
    work: Work,
    cancel: CancellationToken,
    task: TaskHandle<()>,
}

pub struct StoreHandle {
    pool: TaskPool,
    pending: Vec<Pending>,
    pub events: ToUIReceiver<StoreEvent>,
    pub library: Library,
}

impl StoreHandle {
    pub fn need_page(&mut self, shard: i64, level: usize, _priority: u64) -> bool {
        self.submit(Work::DecodePage { shard, level })
    }

    pub fn need_full(&mut self, item: ItemId, px: u32, _priority: u64) -> bool {
        self.submit(Work::DecodeFull { item, px })
    }

    fn submit(&mut self, work: Work) -> bool {
        let cancel = CancellationToken::new();
        let job_cancel = cancel.clone();
        let library = self.library.clone();
        let events: ToUISender<StoreEvent> = self.events.sender();
        let Ok(task) = self.pool.submit(Lane::Heavy, move || {
            if job_cancel.is_cancelled() {
                return;
            }
            let event = decode(&library, work);
            if !job_cancel.is_cancelled() {
                let _ = events.send(event);
            }
        }) else {
            return false;
        };
        self.pending.push(Pending { work, cancel, task });
        true
    }

    pub fn reap_finished(&mut self) {
        self.pending.retain_mut(|pending| pending.task.try_take().is_none());
    }

    /// The whole current want: everything queued and not named here is
    /// dropped. Returns (pages, fulls) that were dropped, so the caller can
    /// strike its "already asked" marks for exactly those.
    pub fn wants(
        &mut self,
        pages: &HashMap<(i64, usize), u64>,
        fulls: &HashMap<ItemId, u64>,
    ) -> (Vec<(i64, usize)>, Vec<ItemId>) {
        let mut dropped_pages = Vec::new();
        let mut dropped_fulls = Vec::new();
        self.pending.retain_mut(|pending| {
            if pending.task.try_take().is_some() {
                return false;
            }
            let wanted = match pending.work {
                Work::DecodePage { shard, level } => pages.contains_key(&(shard, level)),
                Work::DecodeFull { item, .. } => fulls.contains_key(&item),
            };
            if !wanted {
                pending.cancel.cancel();
                pending.task.cancel();
                match pending.work {
                    Work::DecodePage { shard, level } => dropped_pages.push((shard, level)),
                    Work::DecodeFull { item, .. } => dropped_fulls.push(item),
                }
            }
            wanted
        });
        (dropped_pages, dropped_fulls)
    }

    pub fn shutdown(&mut self) {
        for pending in self.pending.drain(..) {
            pending.cancel.cancel();
            pending.task.cancel();
        }
    }
}

impl Drop for StoreHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Attach a baked library to the runtime decode pool.
pub fn spawn(library: Library, pool: TaskPool) -> StoreHandle {
    let events = ToUIReceiver::default();
    StoreHandle { pool, pending: Vec::new(), events, library }
}

fn decode(library: &Library, work: Work) -> StoreEvent {
    match work {
        Work::DecodePage { shard, level } => match read_frame(&library.tape_path(shard, level)) {
            Ok(planes) => StoreEvent::Page { shard, level, planes },
            Err(_) => StoreEvent::PageFailed { shard, level },
        },
        Work::DecodeFull { item, px } => match display_frame(library, item, px) {
            Ok((planes, finest)) => {
                let got = planes.width.max(planes.height);
                StoreEvent::Full { item, px: got, finest, frame: full_frame(&planes) }
            }
            Err(_) => StoreEvent::FullFailed { item },
        }
    }
}
