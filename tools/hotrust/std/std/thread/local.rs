//! Thread-local storage over pthread keys. `thread_local! { static K: T = e; }` is lowered
//! by HotRust's frontend to `static K: LocalKey<T> = LocalKey::new(<fn returning e>);`
//! (coord.md F-OS2). Each key lazily creates one pthread key; a thread's value lives in a
//! heap slot whose destructor runs at thread exit through the key destructor.

use alloc::boxed::Box;
use core::cell::{Cell, RefCell};
use core::error::Error;
use core::ffi::c_void;
use core::fmt;
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering::{AcqRel, Acquire};

use crate::sys;

pub struct LocalKey<T: 'static> {
    key: AtomicUsize, // pthread key + 1; 0 = not created yet
    init: fn() -> T,
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub struct AccessError;

impl fmt::Debug for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AccessError").finish()
    }
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt("already destroyed", f)
    }
}

impl Error for AccessError {}

/// Header first so one non-generic key destructor can drop any slot type.
#[repr(C)]
struct Slot<T> {
    drop_slot: unsafe fn(*mut u8),
    key: sys::pthread_key_t,
    value: T,
}

#[repr(C)]
struct SlotHeader {
    drop_slot: unsafe fn(*mut u8),
    key: sys::pthread_key_t,
}

/// Stored in the key while a slot is being destroyed (and after): access fails.
const DESTROYED: usize = 1;

unsafe fn drop_slot<T>(p: *mut u8) {
    drop(Box::from_raw(p as *mut Slot<T>));
}

extern "C" fn key_dtor(p: *mut c_void) {
    if p as usize == DESTROYED {
        return; // second destructor round: leave the key empty
    }
    unsafe {
        // pthread cleared the key before calling us: mark it destroyed while the value
        // drops (access from other destructors then fails with AccessError); pthread
        // calls us once more with DESTROYED and then leaves the key empty.
        let header = &*(p as *const SlotHeader);
        let drop_fn = header.drop_slot;
        sys::pthread_setspecific(header.key, DESTROYED as *const c_void);
        drop_fn(p as *mut u8);
    }
}

impl<T: 'static> LocalKey<T> {
    pub const fn new(init: fn() -> T) -> LocalKey<T> {
        LocalKey { key: AtomicUsize::new(0), init }
    }

    fn os_key(&self) -> sys::pthread_key_t {
        let k = self.key.load(Acquire);
        if k != 0 {
            return k - 1;
        }
        let mut new_key: sys::pthread_key_t = 0;
        let r = unsafe { sys::pthread_key_create(&mut new_key as *mut sys::pthread_key_t, key_dtor) };
        if r != 0 {
            panic!("failed to allocate a thread-local storage key");
        }
        match self.key.compare_exchange(0, new_key + 1, AcqRel, Acquire) {
            Ok(_) => new_key,
            Err(other) => {
                unsafe {
                    sys::pthread_key_delete(new_key);
                }
                other - 1
            }
        }
    }

    fn slot(&'static self) -> Option<*const T> {
        let key = self.os_key();
        let p = unsafe { sys::pthread_getspecific(key) } as usize;
        if p == DESTROYED {
            return None;
        }
        if p != 0 {
            return Some(unsafe { &(*(p as *const Slot<T>)).value as *const T });
        }
        let value = (self.init)();
        // init may have initialized this key itself (recursive init): real std then panics
        // ("recursive initialization") only for const-less lazy keys; keep the first value.
        let again = unsafe { sys::pthread_getspecific(key) } as usize;
        if again != 0 && again != DESTROYED {
            drop(value);
            return Some(unsafe { &(*(again as *const Slot<T>)).value as *const T });
        }
        let slot = Box::into_raw(Box::new(Slot { drop_slot: drop_slot::<T>, key, value }));
        unsafe {
            sys::pthread_setspecific(key, slot as *const c_void);
            Some(&(*slot).value as *const T)
        }
    }

    pub fn with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> R {
        match self.try_with(f) {
            Ok(r) => r,
            Err(_) => panic!("cannot access a Thread Local Storage value during or after destruction: AccessError"),
        }
    }

    pub fn try_with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> Result<R, AccessError> {
        match self.slot() {
            Some(p) => Ok(f(unsafe { &*p })),
            None => Err(AccessError),
        }
    }
}

impl<T: 'static> LocalKey<Cell<T>> {
    pub fn set(&'static self, value: T) {
        self.with(|cell| cell.set(value))
    }
    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(|cell| cell.get())
    }
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(|cell| cell.take())
    }
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

impl<T: 'static> LocalKey<RefCell<T>> {
    pub fn with_borrow<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> R {
        self.with(|cell| f(&cell.borrow()))
    }
    pub fn with_borrow_mut<F: FnOnce(&mut T) -> R, R>(&'static self, f: F) -> R {
        self.with(|cell| f(&mut cell.borrow_mut()))
    }
    pub fn set(&'static self, value: T) {
        self.with(|cell| {
            *cell.borrow_mut() = value;
        })
    }
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(|cell| cell.take())
    }
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

impl<T: 'static> fmt::Debug for LocalKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("LocalKey").finish_non_exhaustive()
    }
}
