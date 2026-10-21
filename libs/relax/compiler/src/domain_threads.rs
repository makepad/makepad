//! Runtime-owned Relax std TLS. Drain JIT destructors before retiring a domain thread; OS key
//! destructor ordering otherwise lets the domain block disappear before a later key uses it.
use std::ffi::c_void;

/// The thread's record (word 1 of its thread-local block). The first three words are read by
/// Relax std's `LocalKey` (std/thread/local.rs) as an inline fast path -- the layout is shared:
/// the values of the domain `cache_base` (its key table: `cache_vals`, `cache_len` entries).
/// The table moves only in `key_set`, which refreshes the cache; any other domain's access
/// goes through `key_get`.
#[repr(C)]
pub(crate) struct ThreadData {
    cache_base: u64,
    cache_vals: *const usize,
    cache_len: usize,
    pub registrations: Vec<(u64, u64)>,
    values: Vec<(u64, Vec<usize>)>,
    /// (domain, key) in the order this thread first set each key: destructors run last-first, as
    /// std's (library/std/src/sys/thread_local/destructors/list.rs: registered at a key's first
    /// initialisation on the thread, run by popping the list). A key's value never returns to 0
    /// (its destructor leaves LocalKey's DESTROYED marker), so each key is registered once.
    order: Vec<(u64, u32)>,
}

impl ThreadData {
    fn refresh(&mut self, base: u64) {
        match self.values.iter().find(|v| v.0 == base) {
            Some(v) => {
                self.cache_base = base;
                self.cache_vals = v.1.as_ptr();
                self.cache_len = v.1.len();
            }
            None => self.cache_len = 0,
        }
    }
}

pub(crate) unsafe fn data(block: u64) -> &'static mut ThreadData {
    let slot = (block as *mut u64).add(1);
    if *slot == 0 {
        // the thread's record lives as long as the thread, whatever phase it is in when it is
        // first asked for: the kept heap (the settle swap destroys the compiler generation)
        let _kept = crate::zone_alloc::kept();
        *slot = Box::into_raw(Box::new(ThreadData {
            cache_base: 0,
            cache_vals: std::ptr::null(),
            cache_len: 0,
            registrations: Vec::new(),
            values: Vec::new(),
            order: Vec::new(),
        })) as u64;
    }
    &mut *(*slot as *mut ThreadData)
}

/// `late`: the key's type has no drop glue. As rustc's thread-locals of such types (never
/// destroyed), its value stays readable in every other key's destructor; its `dtor` only frees
/// the slot, after the last of those ran (`finish`).
pub extern "C" fn key_create(dtor: usize, late: u32) -> u32 {
    crate::domain::tls_key_create(crate::domain::current(), dtor, late != 0)
}
pub extern "C" fn key_delete(key: u32) {
    crate::domain::tls_key_delete(crate::domain::current(), key);
}
pub extern "C" fn key_get(key: u32) -> *mut c_void {
    let base = crate::domain::current();
    unsafe {
        let t = data(crate::jit::tls_block());
        // the next reads of this domain's keys take the inline path
        if t.cache_base != base || t.cache_len == 0 {
            t.refresh(base);
        }
        t.values
            .iter()
            .find(|v| v.0 == base)
            .and_then(|v| v.1.get(key as usize))
            .copied()
            .unwrap_or(0) as *mut c_void
    }
}
pub extern "C" fn key_set(key: u32, value: *const c_void) {
    unsafe {
        let _kept = crate::zone_alloc::kept();
        let base = crate::domain::current();
        let t = data(crate::jit::tls_block());
        let i = match t.values.iter().position(|v| v.0 == base) {
            Some(i) => i,
            None => {
                t.values.push((base, Vec::new()));
                t.values.len() - 1
            }
        };
        let values = &mut t.values[i].1;
        if values.len() <= key as usize {
            values.resize(key as usize + 1, 0);
        }
        if values[key as usize] == 0 && value as usize != 0 {
            t.order.push((base, key));
        }
        values[key as usize] = value as usize;
        t.refresh(base);
    }
}

/// The TLS block is installed while callbacks run, and the registration stays live even if a
/// destructor blocks forever. No arbitrary destructor limit drops app data.
pub(crate) unsafe fn finish(block: u64) -> Vec<(u64, u64)> {
    let slot = (block as *mut u64).add(1);
    if *slot == 0 {
        return Vec::new();
    }
    let p = *slot as *mut ThreadData;
    // std's destructor list (destructors/list.rs `run`): keys with drop glue, last-registered
    // first; a destructor that initialises another key registers it and it runs next. LocalKey's
    // destructor marks its key DESTROYED for good (native/lazy.rs `destroy`: later access fails
    // with AccessError, nothing re-initialises it). Late keys (no drop glue: std registers no
    // destructor, the value is never destroyed) stay set and readable meanwhile.
    while let Some((key_base, key)) = (*p).order.pop() {
        let Some((dtor, false)) = crate::domain::tls_destructor(key_base, key) else { continue };
        // End the borrow before JIT runs: the destructor can initialise another TLS key.
        let value = (*p)
            .values
            .iter()
            .find(|v| v.0 == key_base)
            .and_then(|v| v.1.get(key as usize).copied())
            .unwrap_or(0);
        if value == 0 {
            continue;
        }
        crate::domain::enter(key_base);
        let f: extern "C" fn(*mut c_void) = std::mem::transmute(dtor);
        f(value as *mut c_void);
    }
    // No program code runs any more: the late keys' slots are freed (their destructor drops no
    // value, so it touches no other key).
    let late: Vec<(u64, usize, usize)> = (*p)
        .values
        .iter()
        .flat_map(|(base, values)| {
            values
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
                .map(move |(key, v)| (*base, key, *v))
        })
        .filter_map(|(base, key, v)| match crate::domain::tls_destructor(base, key as u32) {
            Some((dtor, true)) => Some((base, dtor, v)),
            _ => None,
        })
        .collect();
    for (key_base, dtor, value) in late {
        crate::domain::enter(key_base);
        let f: extern "C" fn(*mut c_void) = std::mem::transmute(dtor);
        f(value as *mut c_void);
    }
    *slot = 0;
    let registrations = std::mem::take(&mut (*p).registrations);
    drop(Box::from_raw(p));
    crate::zone_alloc::domain::enter(std::ptr::null());
    registrations
}
