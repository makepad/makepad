//! The viewer's decode pool: a baked library is read-only, so serving the
//! grid is nothing but hardware decodes, ranked by how much screen each one
//! would paint.
//!
//! There is no coordinator thread. The widget pushes wants straight onto a
//! shared priority queue; a handful of workers pop the biggest first, decode
//! on VideoToolbox, and send the result back over a [`ToUISender`] (which
//! raises the UI signal, so the widget drains on `Event::Signal`). A pan
//! that leaves work behind calls [`StoreHandle::wants`] with the whole
//! current truth: queued decodes not named are dropped, the rest take their
//! fresh weights.

use crate::library::{display_frame, ItemId, Library};
use crate::tape::{full_frame, read_frame, FullFrame, Planes};
use makepad_widgets::makepad_platform::thread::{
    ThreadOptions, ThreadSpawner, ToUIReceiver, ToUISender,
};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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

enum Work {
    DecodePage { shard: i64, level: usize },
    DecodeFull { item: ItemId, px: u32 },
}

struct QueueState {
    /// (priority, work): the pool serves the biggest first, newest on a tie.
    immediate: Vec<(u64, Work)>,
    closed: bool,
}

struct WorkQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
}

impl WorkQueue {
    fn new() -> WorkQueue {
        WorkQueue { state: Mutex::new(QueueState { immediate: Vec::new(), closed: false }), cv: Condvar::new() }
    }

    fn push(&self, work: Work, priority: u64) {
        self.state.lock().unwrap().immediate.push((priority, work));
        self.cv.notify_one();
    }

    fn pop(&self) -> Option<Work> {
        let mut s = self.state.lock().unwrap();
        loop {
            if s.closed {
                return None;
            }
            // A linear scan: the queue holds at most a screenful of asks
            // between two wants messages, and a decode costs six orders of
            // magnitude more than walking it.
            let mut best: Option<(usize, u64)> = None;
            for (i, (pri, _)) in s.immediate.iter().enumerate() {
                if best.map_or(true, |(_, bp)| *pri >= bp) {
                    best = Some((i, *pri));
                }
            }
            if let Some((i, _)) = best {
                return Some(s.immediate.swap_remove(i).1);
            }
            s = self.cv.wait_timeout(s, Duration::from_millis(100)).unwrap().0;
        }
    }

    /// Make the queue match what is on screen right now: queued decodes not
    /// in the want maps are dropped, the rest take their fresh weights.
    /// Returns what was dropped so the caller can strike its own claims.
    fn retarget(
        &self,
        pages: &HashMap<(i64, usize), u64>,
        fulls: &HashMap<ItemId, u64>,
    ) -> (Vec<(i64, usize)>, Vec<ItemId>) {
        let mut s = self.state.lock().unwrap();
        let mut dropped_pages = Vec::new();
        let mut dropped_fulls = Vec::new();
        s.immediate.retain_mut(|(pri, w)| match w {
            Work::DecodePage { shard, level } => match pages.get(&(*shard, *level)) {
                Some(p) => {
                    *pri = *p;
                    true
                }
                None => {
                    dropped_pages.push((*shard, *level));
                    false
                }
            },
            Work::DecodeFull { item, .. } => match fulls.get(item) {
                Some(p) => {
                    *pri = *p;
                    true
                }
                None => {
                    dropped_fulls.push(*item);
                    false
                }
            },
        });
        (dropped_pages, dropped_fulls)
    }

    fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        s.immediate.clear();
        drop(s);
        self.cv.notify_all();
    }
}

pub struct StoreHandle {
    queue: Arc<WorkQueue>,
    pub events: ToUIReceiver<StoreEvent>,
    pub library: Library,
}

impl StoreHandle {
    pub fn need_page(&self, shard: i64, level: usize, priority: u64) {
        self.queue.push(Work::DecodePage { shard, level }, priority);
    }

    pub fn need_full(&self, item: ItemId, px: u32, priority: u64) {
        self.queue.push(Work::DecodeFull { item, px }, priority);
    }

    /// The whole current want: everything queued and not named here is
    /// dropped. Returns (pages, fulls) that were dropped, so the caller can
    /// strike its "already asked" marks for exactly those.
    pub fn wants(
        &self,
        pages: &HashMap<(i64, usize), u64>,
        fulls: &HashMap<ItemId, u64>,
    ) -> (Vec<(i64, usize)>, Vec<ItemId>) {
        self.queue.retarget(pages, fulls)
    }

    /// A read-only store has nothing to flush: closing the queue is the
    /// whole shutdown, and the workers exit on their next pop.
    pub fn shutdown(&self) {
        self.queue.close();
    }
}

/// Bring up the decode pool over a baked library.
pub fn spawn(library: Library, spawner: &ThreadSpawner) -> StoreHandle {
    let queue = Arc::new(WorkQueue::new());
    let events = ToUIReceiver::default();
    let workers = spawner.worker_count(2, 8).get();
    for i in 0..workers {
        let queue = queue.clone();
        let events: ToUISender<StoreEvent> = events.sender();
        let library = library.clone();
        if let Ok(handle) = spawner.spawn_worker(
            ThreadOptions { name: Some(format!("tiles-decode-{i}").into()), ..Default::default() },
            move || worker(library, queue, events),
        ) {
            handle.detach();
        }
    }
    StoreHandle { queue, events, library }
}

fn worker(library: Library, queue: Arc<WorkQueue>, events: ToUISender<StoreEvent>) {
    while let Some(work) = queue.pop() {
        let event = match work {
            Work::DecodePage { shard, level } => match read_frame(&library.tape_path(shard, level)) {
                Ok(planes) => StoreEvent::Page { shard, level, planes },
                Err(_) => StoreEvent::PageFailed { shard, level },
            },
            Work::DecodeFull { item, px } => match display_frame(&library, item, px) {
                Ok((planes, finest)) => {
                    let got = planes.width.max(planes.height);
                    StoreEvent::Full { item, px: got, finest, frame: full_frame(&planes) }
                }
                Err(_) => StoreEvent::FullFailed { item },
            },
        };
        if events.send(event).is_err() {
            break;
        }
    }
}
