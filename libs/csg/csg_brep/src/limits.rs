//! A documented ceiling on allocations whose size comes from caller input, so an
//! absurd count fails with an error on every machine instead of depending on how much
//! memory the operating system is willing to promise.
//!
//! Upstream: no counterpart. Upstream sizes these arrays from the caller's argument
//! (a degree, count, index or voxel count) and fails softly when `malloc` /
//! `smos_Calloc` returns `NULL`. A fallible Rust reservation is not equivalent: with
//! memory overcommit a reservation of tens of GiB succeeds, and the zero-fill that
//! follows then pages it all in (issue #505). Each such site therefore refuses a
//! request above [`MAX_ALLOC_BYTES`] up front and reports it as it reports a failed
//! reservation.
//!
//! | Rust                 | Upstream                                     |
//! |----------------------|----------------------------------------------|
//! | [`MAX_ALLOC_BYTES`]  | none (the machine's memory)                  |
//! | [`fits`]             | none                                         |
//! | [`try_filled`]       | `malloc`/`smos_Calloc` + fill, `NULL` on failure |
//! | [`try_reserve_total`] | `smos_Realloc` growing an array, `NULL` on failure |
//!
//! Deviations: a request above [`MAX_ALLOC_BYTES`] fails even where the machine could
//! have backed it. No upstream test or caller comes near the bound (the largest arrays
//! the difftest fixtures build are a few MiB).

/// Largest single allocation, in bytes, that a caller-sized array may take (1 GiB).
pub const MAX_ALLOC_BYTES: usize = 1 << 30;

/// Whether `len` values of `T` stay within [`MAX_ALLOC_BYTES`].
#[must_use]
pub const fn fits<T>(len: usize) -> bool {
    let size = if size_of::<T>() == 0 {
        1
    } else {
        size_of::<T>()
    };
    match len.checked_mul(size) {
        Some(bytes) => bytes <= MAX_ALLOC_BYTES,
        None => false,
    }
}

/// `len` copies of `value`, or `None` when that exceeds [`MAX_ALLOC_BYTES`] or cannot
/// be reserved.
#[must_use]
pub fn try_filled<T: Clone>(len: usize, value: T) -> Option<Vec<T>> {
    if !fits::<T>(len) {
        return None;
    }
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, value);
    Some(v)
}

/// Makes room in `v` for a total of `len` values; `false` (and `v` unchanged) when
/// that exceeds [`MAX_ALLOC_BYTES`] or cannot be reserved.
#[must_use]
pub fn try_reserve_total<T>(v: &mut Vec<T>, len: usize) -> bool {
    fits::<T>(len) && v.try_reserve_exact(len.saturating_sub(v.len())).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound() {
        assert!(fits::<f64>(MAX_ALLOC_BYTES / 8));
        assert!(!fits::<f64>(MAX_ALLOC_BYTES / 8 + 1));
        assert!(!fits::<f64>(usize::MAX));
        assert!(fits::<()>(MAX_ALLOC_BYTES));
        assert!(!fits::<()>(MAX_ALLOC_BYTES + 1));
        assert_eq!(try_filled(3, 1.5), Some(vec![1.5; 3]));
        assert_eq!(try_filled(1 << 31, 0.0_f64), None);
        let mut v = vec![1_u8];
        assert!(try_reserve_total(&mut v, 10));
        assert!(v.capacity() >= 10);
        assert!(!try_reserve_total(&mut v, MAX_ALLOC_BYTES + 1));
        assert_eq!(v, [1]);
    }
}

/// What unwinds a kernel call on purpose (see [`probe`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unwound {
    /// The stack armed by [`arm`] is nearly used up.
    Stack,
    /// The flag armed by [`arm`] was set.
    Cancelled,
}

thread_local! {
    /// The lowest address the stack may reach, 0 when nothing is armed.
    static STACK_FLOOR: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
    static CANCEL: core::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
        const { core::cell::RefCell::new(None) };
}

/// Disarms the thread's limits when dropped.
pub struct Armed {
    floor: usize,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Drop for Armed {
    fn drop(&mut self) {
        STACK_FLOOR.with(|f| f.set(self.floor));
        CANCEL.with(|c| *c.borrow_mut() = self.cancel.take());
    }
}

fn stack_address() -> usize {
    let marker = 0u8;
    core::ptr::from_ref(&marker) as usize
}

/// Arms [`probe`] on this thread: it unwinds once the stack grew `budget` bytes below
/// here, or once `cancel` is set.
#[must_use]
pub fn arm(budget: usize, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Armed {
    let floor = stack_address().saturating_sub(budget);
    Armed {
        floor: STACK_FLOOR.with(|f| f.replace(floor)),
        cancel: CANCEL.with(|c| c.borrow_mut().replace(cancel)),
    }
}

/// Called on every recursive path of the kernel and in its hot evaluators. Unwinds with
/// [`Unwound`] (no panic message) past the armed stack floor or after a cancel; does
/// nothing when nothing is armed.
#[inline]
pub fn probe() {
    let floor = STACK_FLOOR.with(core::cell::Cell::get);
    if floor == 0 {
        return;
    }
    if stack_address() < floor {
        std::panic::resume_unwind(Box::new(Unwound::Stack));
    }
    let cancelled = CANCEL.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
    });
    if cancelled {
        std::panic::resume_unwind(Box::new(Unwound::Cancelled));
    }
}
