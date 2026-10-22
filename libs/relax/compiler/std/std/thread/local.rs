//! Thread-local storage. `thread_local!` (Relax's frontend, base/thread_local_mac.rs) makes each
//! key a `const LocalKey<T>` whose fn returns the calling thread's value, as std's expansion does:
//! a `const` initialiser of a type without drop glue is a `#[thread_local]` static (the compiler
//! places it at a fixed offset from the thread pointer where the platform has static TLS);
//! every other key lazily creates one domain-local runtime key (`LazyKey`), and a thread's value
//! lives in a heap slot whose destructor runs before the runtime deregisters its domain thread.

use alloc::alloc::{handle_alloc_error, GlobalAlloc, Layout, System};
use core::cell::{Cell, RefCell};
use core::error::Error;
use core::ffi::c_void;
use core::fmt;
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering::{AcqRel, Acquire};

use crate::sys;

/// A thread-local's handle: the fn the expansion made for this key returns the calling thread's
/// value (None: destroyed), as std's `LocalKey { inner }` (library/std/src/thread/local.rs). The key
/// is a `const`, so a `with` reaches that fn directly (a constant).
pub struct LocalKey<T: 'static> {
    inner: fn() -> Option<*const T>,
}

/// The lazily created runtime key of a thread-local that needs one (lazy initialisation or
/// drop glue): its `get` is what `LocalKey`'s fn calls. One per key (a `static` in that fn).
pub struct LazyKey<T: 'static> {
    key: AtomicUsize, // runtime key + 1; 0 = not created yet
    _t: core::marker::PhantomData<*const T>,
}

// the key word is shared by all threads; the values are each thread's own
unsafe impl<T: 'static> Sync for LazyKey<T> {}

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
    key: sys::Key,
    value: T,
}

#[repr(C)]
struct SlotHeader {
    drop_slot: unsafe fn(*mut u8),
    key: sys::Key,
}

/// Stored in the key while a slot is being destroyed (and after): access fails.
const DESTROYED: usize = 1;

/// A slot is the thread's storage of the value, which std keeps in the thread's own TLS block
/// (sys/thread_local/native): it comes from System, never from the program's global allocator
/// (a `#[global_allocator]` may use thread-locals itself).
unsafe fn drop_slot<T>(p: *mut u8) {
    core::ptr::drop_in_place(p as *mut Slot<T>);
    System.dealloc(p, Layout::new::<Slot<T>>());
}

extern "C" fn key_dtor(p: *mut c_void) {
    unsafe {
        // std's `destroy` (sys/thread_local/native/lazy.rs): the key is marked destroyed for good
        // before the value drops, so access from this and every later destructor fails with
        // AccessError and nothing re-initialises it. The runtime calls this once per key, last
        // registered first; a late key's (no drop glue) only after every other destructor ran.
        let header = &*(p as *const SlotHeader);
        let drop_fn = header.drop_slot;
        sys::key_set(header.key, DESTROYED as *const c_void);
        drop_fn(p as *mut u8);
    }
}

/// The calling thread's value of `key`: read inline from the table the runtime keeps in the
/// thread's block (word 1 points at its record, whose first three words are the domain the table
/// belongs to, the table and its length -- domain_threads.rs ThreadData), the host call only
/// when the thread runs another domain, has no record yet or has not set that key.
#[cfg(not(target_family = "wasm"))]
#[inline(always)]
fn key_value(key: sys::Key) -> usize {
    unsafe {
        let block = core::intrinsics_rt::tls_block();
        let record = *block.add(1) as *const usize;
        if !record.is_null() && *record == *block && (key as usize) < *record.add(2) {
            return *(*record.add(1) as *const usize).add(key as usize);
        }
    }
    sys::key_get(key) as usize
}

/// The web targets: the runtime's per-thread record holds host words (native pointers, 8 bytes),
/// which a wasm program cannot read in place; the host call answers.
#[cfg(target_family = "wasm")]
#[inline(always)]
fn key_value(key: sys::Key) -> usize {
    sys::key_get(key) as usize
}

impl<T: 'static> LazyKey<T> {
    pub const fn new() -> LazyKey<T> {
        LazyKey { key: AtomicUsize::new(0), _t: core::marker::PhantomData }
    }

    fn os_key(&self) -> sys::Key {
        let k = self.key.load(Acquire);
        if k != 0 {
            return (k - 1) as sys::Key;
        }
        // rustc never destroys a thread-local whose type has no drop glue (std's native TLS
        // registers no destructor for it): its value stays readable in every other destructor
        let new_key = match sys::key_create(key_dtor, !core::mem::needs_drop::<T>()) {
            Some(k) => k,
            None => panic!("failed to allocate a thread-local storage key"),
        };
        match self.key.compare_exchange(0, new_key as usize + 1, AcqRel, Acquire) {
            Ok(_) => new_key,
            Err(other) => {
                sys::key_delete(new_key);
                (other - 1) as sys::Key
            }
        }
    }

    /// The calling thread's value, initialised by `init` on its first access; None once it is
    /// being destroyed.
    pub fn get(&'static self, init: fn() -> T) -> Option<*const T> {
        let key = self.os_key();
        let p = key_value(key);
        if p == DESTROYED {
            return None;
        }
        if p != 0 {
            return Some(unsafe { &(*(p as *const Slot<T>)).value as *const T });
        }
        let value = init();
        // init may have initialized this key itself (init-on-init): as std's lazy storage,
        // the outer value replaces the inner one, which is dropped after the replacement
        // (so its destructor already sees the new value).
        let again = sys::key_get(key) as usize;
        if again == DESTROYED {
            drop(value);
            return None;
        }
        if again != 0 {
            unsafe {
                let slot = again as *mut Slot<T>;
                let old = core::mem::replace(&mut (*slot).value, value);
                drop(old);
                return Some(&(*slot).value as *const T);
            }
        }
        let layout = Layout::new::<Slot<T>>();
        let slot = unsafe { System.alloc(layout) } as *mut Slot<T>;
        if slot.is_null() {
            handle_alloc_error(layout);
        }
        unsafe {
            slot.write(Slot { drop_slot: drop_slot::<T>, key, value });
            sys::key_set(key, slot as *const c_void);
            Some(&(*slot).value as *const T)
        }
    }
}

impl<T: 'static> LocalKey<T> {
    /// `inner` returns the calling thread's value (None: destroyed); the expansion's.
    pub const fn new(inner: fn() -> Option<*const T>) -> LocalKey<T> {
        LocalKey { inner }
    }

    pub fn with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> R {
        match self.try_with(f) {
            Ok(r) => r,
            Err(_) => panic!("cannot access a Thread Local Storage value during or after destruction: AccessError"),
        }
    }

    #[inline]
    pub fn try_with<F: FnOnce(&T) -> R, R>(&'static self, f: F) -> Result<R, AccessError> {
        match (self.inner)() {
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
