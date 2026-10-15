//! Rust-held references to script values -- ScriptObjectRef, ScriptArrayRef, ScriptHandleRef
//! (and ScriptFnRef through its object ref) -- as root counts per heap slot.
//!
//! A ref is `{roots, value}`. Its Clone calls [`script_root_retain`] and its Drop calls
//! [`script_root_release`]: plain non-generic fns over a pointer, a kind and a slot index, so
//! app code built by another compiler than the VM (Rapid-compiled apps against the rustc-built
//! VM in the makepad loader, local/plans/loader-core.md section 8) calls them through the C ABI.
//! The counts are one `u32` per slot index (no hashing). A rooted value is alive, so its slot
//! index is not reused while it is counted.
//!
//! The table outlives its heap while refs exist (a ref may be dropped after its VM, as with the
//! `Rc`'d root maps before): `owners` counts the heap plus every live ref, and the last one
//! frees it. Not `Send`/`Sync`, as the refs never were.
use makepad_relax_api::relax_api;

use std::cell::{Cell, UnsafeCell};

/// What a root count is for.
pub const ROOT_OBJECT: u8 = 0;
pub const ROOT_ARRAY: u8 = 1;
pub const ROOT_HANDLE: u8 = 2;

#[relax_api(handle)]
pub struct ScriptRoots {
    /// the owning heap (until it drops) plus every live ref
    owners: Cell<usize>,
    /// per kind: count per slot index
    counts: [UnsafeCell<Vec<u32>>; 3],
    /// per kind: slots with a non-zero count
    rooted: [Cell<usize>; 3],
}

impl ScriptRoots {
    fn new() -> *const ScriptRoots {
        Box::into_raw(Box::new(ScriptRoots {
            owners: Cell::new(1),
            counts: Default::default(),
            rooted: Default::default(),
        }))
    }

    #[inline]
    fn add(&self, kind: u8, index: u32) {
        // SAFETY: single-threaded (the refs are !Send), and no reference into the vec outlives
        // a call
        let v = unsafe { &mut *self.counts[kind as usize].get() };
        let i = index as usize;
        if i >= v.len() {
            v.resize(i + 1, 0);
        }
        if v[i] == 0 {
            self.rooted[kind as usize].set(self.rooted[kind as usize].get() + 1);
        }
        v[i] += 1;
    }

    #[inline]
    fn remove(&self, kind: u8, index: u32) {
        let v = unsafe { &mut *self.counts[kind as usize].get() };
        let i = index as usize;
        match v.get_mut(i) {
            Some(c) if *c >= 1 => {
                *c -= 1;
                if *c == 0 {
                    self.rooted[kind as usize].set(self.rooted[kind as usize].get() - 1);
                }
            }
            _ => eprintln!("script root released more often than retained (kind {}, slot {})", kind, index),
        }
    }

    /// One more owner (a ref); its last release frees the table.
    #[inline]
    fn own(&self) {
        self.owners.set(self.owners.get() + 1);
    }

    #[inline]
    unsafe fn disown(this: *const ScriptRoots) {
        let n = (*this).owners.get() - 1;
        (*this).owners.set(n);
        if n == 0 {
            drop(Box::from_raw(this as *mut ScriptRoots));
        }
    }

    /// The rooted slot indices of `kind`, ascending.
    pub(crate) fn rooted_slots(&self, kind: u8) -> impl Iterator<Item = u32> + '_ {
        let v = unsafe { &*self.counts[kind as usize].get() };
        v.iter().enumerate().filter(|(_, c)| **c > 0).map(|(i, _)| i as u32)
    }

    /// How many slots of `kind` are rooted.
    pub(crate) fn rooted_len(&self, kind: u8) -> usize {
        self.rooted[kind as usize].get()
    }

    pub(crate) fn bytes(&self) -> usize {
        self.counts.iter().map(|c| unsafe { (*c.get()).capacity() } * 4).sum()
    }
}

/// A new ref to slot `index` of `kind` (the ref owns the table until its release).
#[inline(always)]
pub fn script_root_retain(roots: *const ScriptRoots, kind: u8, index: u32) {
    script_root(roots, kind, index, false)
}

/// A ref to slot `index` of `kind` is gone.
#[inline(always)]
pub fn script_root_release(roots: *const ScriptRoots, kind: u8, index: u32) {
    script_root(roots, kind, index, true)
}

/// A new ref to slot `index` of `kind` (`release` false: the ref owns the table until its
/// release), or a ref to it is gone (`release` true).
#[inline(never)]
#[relax_api]
pub fn script_root(roots: *const ScriptRoots, kind: u8, index: u32, release: bool) {
    if roots.is_null() {
        return;
    }
    if !release {
        // SAFETY: a non-null `roots` comes from a live ref or heap (an owner)
        unsafe {
            (*roots).add(kind, index);
            (*roots).own();
        }
        return;
    }
    unsafe {
        (*roots).remove(kind, index);
        ScriptRoots::disown(roots);
    }
}

/// The heap's ownership of its root table.
#[relax_api(plain)]
pub struct ScriptRootsOwner(*const ScriptRoots);

impl Default for ScriptRootsOwner {
    fn default() -> Self {
        Self(ScriptRoots::new())
    }
}

impl Drop for ScriptRootsOwner {
    fn drop(&mut self) {
        unsafe { ScriptRoots::disown(self.0) }
    }
}

impl ScriptRootsOwner {
    #[inline]
    pub(crate) fn ptr(&self) -> *const ScriptRoots {
        self.0
    }

    #[inline]
    pub(crate) fn get(&self) -> &ScriptRoots {
        unsafe { &*self.0 }
    }

    /// A root the heap itself holds (its module root): counted, owns nothing.
    pub(crate) fn add_static(&self, kind: u8, index: u32) {
        self.get().add(kind, index);
    }
}
