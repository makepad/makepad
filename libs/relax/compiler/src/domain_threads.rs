//! Runtime-owned Relax std TLS. Drain JIT destructors before retiring a domain thread; OS key
//! destructor ordering otherwise lets the domain block disappear before a later key uses it.
use std::ffi::c_void;

pub(crate) struct ThreadData {
    pub registrations: Vec<(u64, u64)>,
    values: Vec<(u64, Vec<usize>)>,
}

pub(crate) unsafe fn data(block: u64) -> &'static mut ThreadData {
    let slot = (block as *mut u64).add(1);
    if *slot == 0 {
        // the thread's record lives as long as the thread, whatever phase it is in when it is
        // first asked for: the kept heap (the settle swap destroys the compiler generation)
        let _kept = crate::zone_alloc::kept();
        *slot = Box::into_raw(Box::new(ThreadData {
            registrations: Vec::new(),
            values: Vec::new(),
        })) as u64;
    }
    &mut *(*slot as *mut ThreadData)
}

pub extern "C" fn key_create(dtor: usize) -> u32 {
    crate::domain::tls_key_create(crate::domain::current(), dtor)
}
pub extern "C" fn key_delete(key: u32) {
    crate::domain::tls_key_delete(crate::domain::current(), key);
}
pub extern "C" fn key_get(key: u32) -> *mut c_void {
    let base = crate::domain::current();
    unsafe {
        data(crate::jit::tls_block())
            .values
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
        values[key as usize] = value as usize;
    }
}

/// The TLS block is installed while callbacks run, and the registration stays live even if a
/// destructor blocks forever. No arbitrary destructor-round limit drops app data.
pub(crate) unsafe fn finish(block: u64) -> Vec<(u64, u64)> {
    let slot = (block as *mut u64).add(1);
    if *slot == 0 {
        return Vec::new();
    }
    let p = *slot as *mut ThreadData;
    loop {
        // Complete a round before revisiting keys set by a destructor. LocalKey marks itself
        // destroyed while it drops; a later destructor must see that marker, not a fresh key.
        let pending: Vec<(u64, u32)> = (*p)
            .values
            .iter()
            .flat_map(|(base, values)| {
                values
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| **v != 0)
                    .map(move |(key, _)| (*base, key as u32))
            })
            .collect();
        if pending.is_empty() {
            break;
        }
        for (key_base, key) in pending {
            // End the borrow before JIT runs: a destructor can initialise another TLS key.
            let value = (*p)
                .values
                .iter_mut()
                .find(|v| v.0 == key_base)
                .map(|v| std::mem::take(&mut v.1[key as usize]))
                .unwrap_or(0);
            if value == 0 {
                continue;
            }
            if let Some(dtor) = crate::domain::tls_destructor(key_base, key) {
                crate::domain::enter(key_base);
                let f: extern "C" fn(*mut c_void) = std::mem::transmute(dtor);
                f(value as *mut c_void);
            }
        }
    }
    *slot = 0;
    let registrations = std::mem::take(&mut (*p).registrations);
    drop(Box::from_raw(p));
    crate::zone_alloc::domain::enter(std::ptr::null());
    registrations
}
