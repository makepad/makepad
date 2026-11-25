use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize};

std::cfg_select! {
    any(target_vendor = "apple", target_os = "vxworks") => { type PthreadKey = std::ffi::c_ulong; }
    any(target_os = "redox", target_os = "cygwin") => { type PthreadKey = usize; }
    any(target_os = "android", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd",
        target_os = "openbsd", target_os = "haiku", target_os = "hurd", target_os = "nto",
        target_os = "nuttx") => { type PthreadKey = c_int; }
    _ => { type PthreadKey = std::ffi::c_uint; }
}

type Destructor = unsafe extern "C" fn(*mut c_void);

extern "C" {
    fn pthread_key_create(key: *mut PthreadKey, destructor: Option<Destructor>) -> c_int;
    fn pthread_key_delete(key: PthreadKey) -> c_int;
    fn pthread_getspecific(key: PthreadKey) -> *mut c_void;
    fn pthread_setspecific(key: PthreadKey, value: *const c_void) -> c_int;
    #[cfg_attr(target_os = "netbsd", link_name = "__sigfillset14")]
    fn sigfillset(set: *mut SigSet) -> c_int;
    #[cfg_attr(all(target_os = "macos", target_arch = "x86"), link_name = "pthread_sigmask$UNIX2003")]
    fn pthread_sigmask(how: c_int, set: *const SigSet, old: *mut SigSet) -> c_int;
}

std::cfg_select! {
    target_os = "android" => { type SigSet = std::ffi::c_ulong; }
    target_vendor = "apple" => { type SigSet = u32; }
    // Room for libc's sigset_t (1024 bits on glibc). libc reads and writes its native size.
    _ => { type SigSet = [u64; 16]; }
}
const SIG_SETMASK: c_int = std::cfg_select! {
    all(target_os = "linux", any(target_arch = "sparc", target_arch = "sparc64")) => 4,
    any(target_vendor = "apple", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd",
        target_os = "openbsd", target_os = "haiku", target_os = "solaris", target_os = "illumos",
        target_os = "hurd", target_os = "vxworks", target_arch = "mips", target_arch = "mips64",
        target_arch = "mips32r6", target_arch = "mips64r6") => 3,
    any(target_os = "cygwin", target_os = "rtems", target_os = "horizon") => 0,
    _ => 2,
};

/// Protects header publication and in-place replacement from an interrupting handler.
struct BlockSignals(SigSet);

impl BlockSignals {
    fn new() -> Self {
        let mut all = SigSet::default();
        let mut old = SigSet::default();
        unsafe {
            check_pthread(sigfillset(&mut all));
            check_pthread(pthread_sigmask(SIG_SETMASK, &all, &mut old));
        }
        Self(old)
    }
}

impl Drop for BlockSignals {
    fn drop(&mut self) {
        check_pthread(unsafe { pthread_sigmask(SIG_SETMASK, &self.0, ptr::null_mut()) });
    }
}

fn check_pthread(result: c_int) {
    if result != 0 {
        std::process::abort();
    }
}

fn create_key(destructor: Option<Destructor>) -> PthreadKey {
    let mut key = 0;
    check_pthread(unsafe { pthread_key_create(&mut key, destructor) });
    key
}

static KEY: AtomicUsize = AtomicUsize::new(usize::MAX);

#[inline]
fn table_key() -> PthreadKey {
    match KEY.load(Acquire) {
        usize::MAX => install_key(),
        key => key as PthreadKey,
    }
}

#[cold]
fn install_key() -> PthreadKey {
    let _signals = BlockSignals::new();
    let installed = KEY.load(Acquire);
    if installed != usize::MAX {
        return installed as PthreadKey;
    }
    let mut key = create_key(Some(destroy_table));
    if key as usize == usize::MAX {
        let sentinel = key;
        key = create_key(Some(destroy_table));
        check_pthread(unsafe { pthread_key_delete(sentinel) });
    }
    match KEY.compare_exchange(usize::MAX, key as usize, Release, Acquire) {
        Ok(_) => key,
        Err(installed) => {
            check_pthread(unsafe { pthread_key_delete(key) });
            installed as PthreadKey
        }
    }
}

#[cfg(test)]
static LIVE_ALLOCATIONS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Tables and chunks need zeroed pointers. Value boxes are written in full.
fn allocate(layout: Layout, zeroed: bool) -> *mut u8 {
    let p = unsafe {
        if zeroed {
            System.alloc_zeroed(layout)
        } else {
            System.alloc(layout)
        }
    };
    if p.is_null() {
        std::process::abort();
    }
    #[cfg(test)]
    LIVE_ALLOCATIONS.fetch_add(1, Relaxed);
    p
}

unsafe fn free(p: *mut u8, layout: Layout) {
    System.dealloc(p, layout);
    #[cfg(test)]
    LIVE_ALLOCATIONS.fetch_sub(1, Relaxed);
}

const DIRECTORY_LEN: usize = 32;
const FIRST_CHUNK: u64 = 64;
static NEXT: AtomicU64 = AtomicU64::new(1);

/// One process-wide id per declaration. Its type fixes the layout of every value in the slot.
pub struct Slot<T> {
    id: AtomicU64,
    invariant: PhantomData<fn(T) -> T>,
}

impl<T> Slot<T> {
    pub const fn new() -> Self {
        Self { id: AtomicU64::new(0), invariant: PhantomData }
    }

    #[inline]
    fn id(&self) -> u64 {
        match self.id.load(Relaxed) {
            0 => self.assign(),
            id => id,
        }
    }

    #[cold]
    fn assign(&self) -> u64 {
        let n = NEXT.fetch_add(1, Relaxed) + FIRST_CHUNK;
        let chunk = n.ilog2() - FIRST_CHUNK.ilog2();
        assert!((chunk as usize) < DIRECTORY_LEN, "thread-local directory exhausted");
        let offset = n - (FIRST_CHUNK << chunk);
        // Chunk occupies bits 32..37. The largest chunks need more than 32 offset bits.
        let id = ((offset >> 32) << 37) | (u64::from(chunk) << 32) | u64::from(offset as u32);
        match self.id.compare_exchange(0, id, Relaxed, Relaxed) {
            Ok(_) => id,
            Err(installed) => installed,
        }
    }
}

type Entry = AtomicPtr<Node>;
const DESTROYED: *mut Node = ptr::without_provenance_mut(1);

/// List links name stable entries so teardown can mark the key before calling its drop glue.
struct Node {
    next: *mut Entry,
    drop_fn: unsafe fn(*mut Node),
}

#[repr(C)]
struct Value<T> {
    node: Node,
    value: T,
}

unsafe fn drop_value<T>(node: *mut Node) {
    let value = node.cast::<Value<T>>();
    ptr::drop_in_place(value);
    free(value.cast(), Layout::new::<Value<T>>());
}

struct Table {
    directory: [AtomicPtr<Entry>; DIRECTORY_LEN],
    drops: AtomicPtr<Entry>,
    retained: AtomicPtr<Entry>,
    rounds: Cell<u8>,
}

fn chunk_layout(chunk: usize) -> Layout {
    let len = usize::try_from(FIRST_CHUNK << chunk).expect("thread-local chunk exceeds address space");
    Layout::array::<Entry>(len).expect("thread-local chunk exceeds address space")
}

impl Table {
    #[inline]
    fn current() -> *mut Self {
        let key = table_key();
        let table = unsafe { pthread_getspecific(key) }.cast::<Self>();
        if table.is_null() {
            Self::install(key)
        } else {
            table
        }
    }

    #[cold]
    fn install(key: PthreadKey) -> *mut Self {
        // pthread has no compare-exchange for first publication of a thread's header.
        let _signals = BlockSignals::new();
        let existing = unsafe { pthread_getspecific(key) }.cast::<Self>();
        if !existing.is_null() {
            return existing;
        }
        let table = allocate(Layout::new::<Self>(), true).cast::<Self>();
        // Zero initializes every pointer and the round counter.
        check_pthread(unsafe { pthread_setspecific(key, table.cast()) });
        table
    }

    #[inline]
    fn entry(&self, id: u64) -> &Entry {
        let chunk = ((id >> 32) as usize) & (DIRECTORY_LEN - 1);
        let offset = ((id >> 37) << 32) | u64::from(id as u32);
        let cell = &self.directory[chunk];
        let mut entries = cell.load(Acquire);
        if entries.is_null() {
            entries = Self::install_chunk(cell, chunk);
        }
        // The chunk layout fits usize and the assigned offset lies inside it.
        unsafe { &*entries.add(offset as usize) }
    }

    #[cold]
    fn install_chunk(cell: &AtomicPtr<Entry>, chunk: usize) -> *mut Entry {
        let layout = chunk_layout(chunk);
        let entries = allocate(layout, true).cast();
        match cell.compare_exchange(ptr::null_mut(), entries, Release, Acquire) {
            Ok(_) => entries,
            Err(installed) => {
                unsafe { free(entries.cast(), layout) };
                installed
            }
        }
    }

    unsafe fn push(head: &AtomicPtr<Entry>, entry: &Entry, node: *mut Node) {
        // Only this registration writes next, and only until the node joins the list.
        let entry = ptr::from_ref(entry).cast_mut();
        let mut next = head.load(Acquire);
        loop {
            (*node).next = next;
            match head.compare_exchange(next, entry, Release, Acquire) {
                Ok(_) => return,
                Err(new_head) => next = new_head,
            }
        }
    }

    unsafe fn drain(head: &AtomicPtr<Entry>) {
        loop {
            let first = head.load(Acquire);
            if first.is_null() {
                return;
            }
            let node = (*first).load(Acquire);
            if head.compare_exchange(first, (*node).next, Release, Acquire).is_ok() {
                (*first).store(DESTROYED, Release);
                ((*node).drop_fn)(node);
            }
        }
    }
}

/// pthread clears the key before each call. Keep plain values alive through the later destructors.
/// The C boundary aborts if a value's destructor unwinds.
unsafe extern "C" fn destroy_table(p: *mut c_void) {
    let key = table_key();
    check_pthread(pthread_setspecific(key, p));
    let table = &*p.cast::<Table>();
    let round = table.rounds.get() + 1;
    table.rounds.set(round);
    loop {
        Table::drain(&table.drops);
        if round < 4 {
            return;
        }
        let _signals = BlockSignals::new();
        if table.drops.load(Acquire).is_null() {
            check_pthread(pthread_setspecific(key, ptr::null()));
            break;
        }
    }
    Table::drain(&table.retained);
    for (chunk, cell) in table.directory.iter().enumerate() {
        let entries = cell.load(Relaxed);
        if !entries.is_null() {
            free(entries.cast(), chunk_layout(chunk));
        }
    }
    free(p.cast(), Layout::new::<Table>());
}

/// A thread-local key declared by the thread_local macro.
pub struct LocalKey<T: 'static> {
    slot: &'static Slot<T>,
    init: fn() -> T,
}

impl<T> fmt::Debug for LocalKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalKey").finish_non_exhaustive()
    }
}

/// The key's destructor has started on this thread.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessError;

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("already destroyed")
    }
}

impl std::error::Error for AccessError {}

#[cold]
#[track_caller]
fn panic_access_error() -> ! {
    panic!("cannot access a Thread Local Storage value during or after destruction: {AccessError:?}")
}

impl<T: 'static> LocalKey<T> {
    #[doc(hidden)]
    pub const fn __new(slot: &'static Slot<T>, init: fn() -> T) -> Self {
        Self { slot, init }
    }

    #[inline]
    fn value(&'static self, init: Option<&mut Option<T>>) -> Result<*const T, AccessError> {
        let table = unsafe { &*Table::current() };
        let entry = table.entry(self.slot.id());
        let mut node = entry.load(Acquire);
        if node == DESTROYED {
            return Err(AccessError);
        }
        if node.is_null() {
            node = self.initialize(table, entry, init);
        }
        Ok(unsafe { &raw const (*node.cast::<Value<T>>()).value })
    }

    #[cold]
    fn initialize(&self, table: &Table, entry: &Entry, init: Option<&mut Option<T>>) -> *mut Node {
        let value = init.and_then(Option::take).unwrap_or_else(self.init);
        let layout = Layout::new::<Value<T>>();
        let new = allocate(layout, false).cast::<Value<T>>();
        unsafe {
            new.write(Value { node: Node { next: ptr::null_mut(), drop_fn: drop_value::<T> }, value });
            match entry.compare_exchange(ptr::null_mut(), new.cast(), Release, Acquire) {
                Ok(_) => {
                    let head = if mem::needs_drop::<T>() { &table.drops } else { &table.retained };
                    Table::push(head, entry, new.cast());
                    new.cast()
                }
                Err(existing) => {
                    // A recursive initializer finished first. Its with scope has already ended.
                    let value = (&raw const (*new).value).read();
                    free(new.cast(), layout);
                    let old = {
                        let _signals = BlockSignals::new();
                        (&raw mut (*existing.cast::<Value<T>>()).value).replace(value)
                    };
                    drop(old);
                    existing
                }
            }
        }
    }

    /// Borrows this thread's value, initializing it on first access.
    pub fn with<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.try_with(f).unwrap_or_else(|_| panic_access_error())
    }

    /// Borrows the value or returns an error if its destruction has started.
    #[inline]
    pub fn try_with<F, R>(&'static self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&T) -> R,
    {
        let value = self.value(None)?;
        Ok(f(unsafe { &*value }))
    }

    fn initialize_with<F, R>(&'static self, init: T, f: F) -> R
    where
        F: FnOnce(Option<T>, &T) -> R,
    {
        let mut init = Some(init);
        let value = self.value(Some(&mut init)).unwrap_or_else(|_| panic_access_error());
        f(init, unsafe { &*value })
    }
}

impl<T: 'static> LocalKey<Cell<T>> {
    /// Sets the value, skipping the declared initializer on first access.
    pub fn set(&'static self, value: T) {
        self.initialize_with(Cell::new(value), |value, cell| {
            if let Some(value) = value {
                cell.set(value.into_inner());
            }
        });
    }

    /// Copies the current value out of the cell.
    pub fn get(&'static self) -> T
    where
        T: Copy,
    {
        self.with(Cell::get)
    }

    /// Replaces the value with its default and returns the previous one.
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(Cell::take)
    }

    /// Stores a new value and returns the previous one.
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }

    /// Applies a function to the current value and stores its result.
    pub fn update(&'static self, f: impl FnOnce(T) -> T)
    where
        T: Copy,
    {
        self.with(|cell| cell.update(f));
    }
}

impl<T: 'static> LocalKey<RefCell<T>> {
    /// Borrows the contents of this thread's cell.
    pub fn with_borrow<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        self.with(|cell| f(&cell.borrow()))
    }

    /// Mutably borrows the contents of this thread's cell.
    pub fn with_borrow_mut<F, R>(&'static self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        self.with(|cell| f(&mut cell.borrow_mut()))
    }

    /// Sets the value, skipping the declared initializer on first access.
    pub fn set(&'static self, value: T) {
        self.initialize_with(RefCell::new(value), |value, cell| {
            if let Some(value) = value {
                *cell.borrow_mut() = value.into_inner();
            }
        });
    }

    /// Replaces the value with its default and returns the previous one.
    pub fn take(&'static self) -> T
    where
        T: Default,
    {
        self.with(RefCell::take)
    }

    /// Stores a new value and returns the previous one.
    pub fn replace(&'static self, value: T) -> T {
        self.with(|cell| cell.replace(value))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::__table_thread_local as tls;
    use std::sync::Mutex;
    use std::thread;

    fn methods() {
        static INITS: AtomicUsize = AtomicUsize::new(0);
        tls! {
            /// A fresh count for each worker.
            pub(super) static LAZY: Cell<i32> = {
                INITS.fetch_add(1, Relaxed);
                Cell::new(7)
            };
            static CELL: Cell<i32> = panic!("Cell::set ran the initializer");
            static BORROWED: RefCell<String> = panic!("RefCell::set ran the initializer")
        }
        let worker = || {
            assert_eq!(LAZY.get(), 7, "each thread starts with the initializer's value");
            LAZY.update(|v| v + 2);
            assert_eq!(LAZY.try_with(Cell::get), Ok(9), "update changes the initialized value");
            assert_eq!(LAZY.with(Cell::get), 9, "with sees the same value as try_with");

            CELL.set(3);
            assert_eq!(CELL.get(), 3, "Cell::set supplies the initial value");
            CELL.set(5);
            assert_eq!(CELL.replace(8), 5, "Cell::replace returns the last assigned value");
            assert_eq!(CELL.take(), 8, "Cell::take returns the replacement");
            assert_eq!(CELL.get(), 0, "Cell::take leaves the default");

            BORROWED.set("first".into());
            assert_eq!(BORROWED.with_borrow(String::len), 5, "RefCell::set supplies the initial value");
            BORROWED.set("next".into());
            BORROWED.with_borrow_mut(|s| s.push('!'));
            assert_eq!(BORROWED.replace("last".into()), "next!", "replace returns the mutated borrow");
            assert_eq!(BORROWED.take(), "last", "RefCell::take returns the replacement");
            assert!(BORROWED.with_borrow(String::is_empty), "RefCell::take leaves the default");
        };
        let a = thread::spawn(worker);
        let b = thread::spawn(worker);
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(INITS.load(Relaxed), 2, "lazy initialization runs once per thread");
        assert_eq!(format!("{LAZY:?}"), "LocalKey { .. }", "formatting a key does not access its value");
    }

    fn recursive_initialization() {
        struct Generation(u32);
        static DROPS: Mutex<Vec<(u32, Result<u32, AccessError>)>> = Mutex::new(Vec::new());
        impl Drop for Generation {
            fn drop(&mut self) {
                let visible = RECURSIVE.try_with(|v| v.0);
                DROPS.lock().unwrap().push((self.0, visible));
            }
        }
        tls! {
            static IN_INIT: Cell<bool> = const { Cell::new(false) };
            static RECURSIVE: Generation = {
                if IN_INIT.replace(true) {
                    Generation(1)
                } else {
                    assert_eq!(RECURSIVE.with(|v| v.0), 1, "the recursive access sees the inner value");
                    Generation(2)
                }
            };
        }
        thread::spawn(|| {
            assert_eq!(RECURSIVE.with(|v| v.0), 2, "the initializer that finishes last wins");
            assert_eq!(
                *DROPS.lock().unwrap(),
                [(1, Ok(2))],
                "the inner drop sees the replacement immediately"
            );
        })
        .join()
        .unwrap();
        assert_eq!(
            *DROPS.lock().unwrap(),
            [(1, Ok(2)), (2, Err(AccessError))],
            "the replacement drops once at exit and cannot access itself"
        );
    }

    fn thread_exit() {
        struct OnDrop(fn());
        impl Drop for OnDrop {
            fn drop(&mut self) {
                (self.0)();
            }
        }
        static EVENTS: Mutex<Vec<(&str, Result<u32, AccessError>)>> = Mutex::new(Vec::new());
        tls! {
            static FIRST: OnDrop = OnDrop(|| {
                let seen = SECOND.try_with(|_| 0);
                EVENTS.lock().unwrap().push(("first", seen));
            });
            static SECOND: OnDrop = OnDrop(|| {
                let own = SECOND.try_with(|_| 0);
                EVENTS.lock().unwrap().push(("second", own));
                DURING_DROP.with(|_| ());
            });
            static DURING_DROP: OnDrop = OnDrop(|| {
                EVENTS.lock().unwrap().push(("during drop", PLAIN.try_with(Cell::get)));
            });
            static PLAIN: Cell<u32> = const { Cell::new(0) };
        }

        struct Late {
            key: PthreadKey,
            round: u8,
            plain: Option<Result<u32, AccessError>>,
            destroyed: Option<Result<(), AccessError>>,
        }
        unsafe extern "C" fn observe(p: *mut c_void) {
            let state = &mut *p.cast::<Late>();
            state.round += 1;
            if state.round < 3 {
                // Observe on a later pass regardless of pthread key order.
                check_pthread(pthread_setspecific(state.key, p));
            } else {
                state.plain = Some(PLAIN.try_with(Cell::get));
                state.destroyed = Some(SECOND.try_with(|_| ()));
            }
        }
        let key = create_key(Some(observe));
        let mut late = Late { key, round: 0, plain: None, destroyed: None };
        thread::scope(|scope| {
            scope
                .spawn(|| {
                    FIRST.with(|_| ());
                    SECOND.with(|_| ());
                    PLAIN.set(37);
                    check_pthread(unsafe { pthread_setspecific(key, ptr::from_mut(&mut late).cast()) });
                })
                .join()
                .unwrap();
        });
        check_pthread(unsafe { pthread_key_delete(key) });
        assert_eq!(
            *EVENTS.lock().unwrap(),
            [("second", Err(AccessError)), ("during drop", Ok(37)), ("first", Err(AccessError))],
            "drops run newest first, including new registrations, and the destroyed slot stays closed"
        );
        assert_eq!(late.plain, Some(Ok(37)), "a later pthread destructor reads the original plain value");
        assert_eq!(
            late.destroyed,
            Some(Err(AccessError)),
            "a later pthread destructor cannot revive the dropped key"
        );
    }

    fn many_statics() {
        static INITS: AtomicUsize = AtomicUsize::new(0);
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        struct Counted(Cell<usize>);
        impl Counted {
            fn new() -> Self {
                INITS.fetch_add(1, Relaxed);
                LIVE.fetch_add(1, Relaxed);
                Self(Cell::new(0))
            }
        }
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Relaxed);
                LIVE.fetch_sub(1, Relaxed);
            }
        }
        macro_rules! keys {
            (row $($col:ident),+) => {{
                $(tls! { static $col: Counted = Counted::new(); })+
                [$(&$col),+]
            }};
            ($cols:tt; $($row:ident),+) => {{
                $(const $row: [&LocalKey<Counted>; 10] = keys! $cols;)+
                [$($row),+]
            }};
        }
        const KEYS: [[&LocalKey<Counted>; 10]; 20] = keys! {
            (row A, B, C, D, E, F, G, H, I, J);
            R0, R1, R2, R3, R4, R5, R6, R7, R8, R9,
            R10, R11, R12, R13, R14, R15, R16, R17, R18, R19
        };
        fn probe_key() -> PthreadKey {
            let key = create_key(None);
            check_pthread(unsafe { pthread_key_delete(key) });
            key
        }
        let before = probe_key();
        let workers: Vec<_> = (1..=50)
            .map(|worker| {
                thread::spawn(move || {
                    KEYS[0][0].with(|first| {
                        first.0.set(worker);
                        for key in KEYS.iter().flatten().skip(1) {
                            key.with(|v| {
                                assert_eq!(
                                    v.0.replace(worker),
                                    0,
                                    "every declaration has its own per-thread value"
                                );
                            });
                        }
                        assert_eq!(first.0.get(), worker, "a borrowed value survives table growth");
                    });
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let after = probe_key();
        assert_eq!(before, after, "200 statics on 50 threads consume no further pthread keys");
        assert_eq!(INITS.load(Relaxed), 200 * 50, "each static initializes on every worker");
        assert_eq!(DROPS.load(Relaxed), INITS.load(Relaxed), "each initialization has a matching drop");
        assert_eq!(LIVE.load(Relaxed), 0, "no counted values survive thread exit");
    }

    // Keep the process-wide allocation count and pthread key index isolated from other probes.
    #[test]
    fn pthread_table_lifecycle() {
        methods();
        recursive_initialization();
        thread_exit();
        many_statics();
        assert_eq!(LIVE_ALLOCATIONS.load(Relaxed), 0, "all value, chunk and header allocations are freed");
    }
}
