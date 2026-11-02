use core::cell::Cell;
use core::panic::Location;

// Tokens distinguish recursive acquisitions and guards dropped out of order. Only the
// eight newest sites are retained; the count remains exact even when older sites expire.
struct HeldLocks {
    count: Cell<u32>,
    next: Cell<u64>,
    len: Cell<usize>,
    sites: [Cell<Option<(u64, &'static Location<'static>)>>; 8],
}

thread_local! {
    static HELD_LOCKS: HeldLocks = const { HeldLocks {
        count: Cell::new(0),
        next: Cell::new(0),
        len: Cell::new(0),
        sites: [const { Cell::new(None) }; 8],
    } };
}

pub(crate) struct HeldLock(u64);

impl HeldLock {
    #[inline]
    #[track_caller]
    pub(crate) fn new() -> Self {
        let site = Location::caller();
        HELD_LOCKS.with(|held| {
            let id = held.next.get().wrapping_add(1);
            held.next.set(id);
            held.count.set(held.count.get() + 1);
            let mut len = held.len.get();
            if len == held.sites.len() {
                for i in 1..len {
                    held.sites[i - 1].set(held.sites[i].get());
                }
                len -= 1;
            }
            held.sites[len].set(Some((id, site)));
            held.len.set(len + 1);
            Self(id)
        })
    }
}

impl Drop for HeldLock {
    #[inline]
    fn drop(&mut self) {
        HELD_LOCKS.with(|held| {
            held.count.set(held.count.get() - 1);
            let len = held.len.get();
            for i in (0..len).rev() {
                if held.sites[i].get().map(|entry| entry.0) == Some(self.0) {
                    for j in i + 1..len {
                        held.sites[j - 1].set(held.sites[j].get());
                    }
                    held.sites[len - 1].set(None);
                    held.len.set(len - 1);
                    break;
                }
            }
        });
    }
}

/// The current thread's held-lock count and newest retained acquisition site.
/// With more than eight simultaneous acquisitions, older sites can be forgotten;
/// the count is still exact and a missing site never means that parking is safe.
#[inline]
pub fn held_lock() -> (u32, Option<&'static Location<'static>>) {
    HELD_LOCKS.with(|held| {
        let len = held.len.get();
        let site = if len == 0 { None } else { held.sites[len - 1].get().map(|entry| entry.1) };
        (held.count.get(), site)
    })
}
