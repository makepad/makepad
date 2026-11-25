//! A value computed once, by the first thread that asks, whose other askers wait off the CPU.
//!
//! [`ComputeOnce`] is [`std::sync::OnceLock`] for values whose initialiser is long work on a
//! host's CPU pool: the initialiser runs with no lock held, so it may give the pool a turn, and
//! the threads waiting for it wait through [`crate::blocking::wait_off_cpu`], which holds no CPU,
//! so a pool whose permits they would hold can still run the initialiser.

use crate::blocking;
use std::sync::{Condvar, Mutex, OnceLock};

/// A value computed once ([`ComputeOnce::get_or_init`]). Ready reads are one atomic load.
pub struct ComputeOnce<T> {
    value: OnceLock<T>,
    /// True while a thread runs the initialiser.
    building: Mutex<bool>,
    done: Condvar,
}

impl<T> Default for ComputeOnce<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ComputeOnce<T> {
    pub const fn new() -> Self {
        Self { value: OnceLock::new(), building: Mutex::new(false), done: Condvar::new() }
    }

    /// The value, once computed. One atomic load; never waits.
    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }

    /// The value, computing it with `f` if no thread has. While another thread computes it, this
    /// one waits off the CPU ([`blocking::wait_off_cpu`], named by `T`'s type). `f` runs with no
    /// lock held. If `f` panics, its waiters wake and the next asker computes it with its own
    /// `f`, as with `OnceLock`. A UI thread uses [`ComputeOnce::get`]: it must not wait for a
    /// value another thread computes.
    pub fn get_or_init(&self, f: impl FnOnce() -> T) -> &T {
        if let Some(value) = self.value.get() {
            return value;
        }
        let mut f = Some(f);
        loop {
            let mut building = self.building.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(value) = self.value.get() {
                return value;
            }
            if !*building {
                *building = true;
                drop(building);
                // Clears `building` and wakes the waiters after the value is set, or on unwind.
                struct Built<'a, T>(&'a ComputeOnce<T>);
                impl<T> Drop for Built<'_, T> {
                    fn drop(&mut self) {
                        *self.0.building.lock().unwrap_or_else(|e| e.into_inner()) = false;
                        self.0.done.notify_all();
                    }
                }
                let built = Built(self);
                let value = f.take().expect("a thread computes the value at most once per call")();
                // Only the thread that set `building` sets the value.
                let _ = self.value.set(value);
                drop(built);
                return self.value.get().expect("the value was set above");
            }
            drop(building);
            blocking::wait_off_cpu(std::any::type_name::<T>(), || {
                let mut building = self.building.lock().unwrap_or_else(|e| e.into_inner());
                while *building {
                    building = self.done.wait(building).unwrap_or_else(|e| e.into_inner());
                }
            });
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ComputeOnce<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ComputeOnce").field(&self.value.get()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::ComputeOnce;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// Hook calls per waited-for type name: each test waits on its own type, so tests running in
    /// parallel count apart.
    static WAITS: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());

    fn waits(what: &'static str) -> usize {
        WAITS.lock().unwrap().iter().find(|(name, _)| *name == what).map_or(0, |(_, n)| *n)
    }

    fn install_counting_hook() {
        crate::blocking::install(|wait, what| {
            {
                let mut waits = WAITS.lock().unwrap();
                match waits.iter_mut().find(|(name, _)| *name == what) {
                    Some((_, n)) => *n += 1,
                    None => waits.push((what, 1)),
                }
            }
            wait();
        });
    }

    /// Spins until `n` waiters of `T` called the hook (they then wait on the cell).
    fn until_waiting<T>(n: usize) {
        while waits(std::any::type_name::<T>()) < n {
            thread::yield_now();
        }
    }

    #[test]
    fn one_builder_under_waiters_and_each_waiter_through_the_hook() {
        #[derive(Debug, PartialEq)]
        struct Shared(usize);
        install_counting_hook();
        const WAITERS: usize = 4;
        let cell = Arc::new(ComputeOnce::<Shared>::new());
        let builds = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(AtomicUsize::new(0));
        let builder = {
            let (cell, builds, started) = (cell.clone(), builds.clone(), started.clone());
            thread::spawn(move || {
                cell.get_or_init(|| {
                    builds.fetch_add(1, SeqCst);
                    started.store(1, SeqCst);
                    // Every waiter waits before the value exists.
                    until_waiting::<Shared>(WAITERS);
                    Shared(7)
                }).0
            })
        };
        while started.load(SeqCst) == 0 {
            thread::yield_now();
        }
        let waiters: Vec<_> = (0..WAITERS).map(|_| {
            let (cell, builds) = (cell.clone(), builds.clone());
            thread::spawn(move || cell.get_or_init(|| { builds.fetch_add(1, SeqCst); Shared(0) }).0)
        }).collect();
        assert_eq!(builder.join().unwrap(), 7, "the builder returns its value");
        for waiter in waiters {
            assert_eq!(waiter.join().unwrap(), 7, "a waiter returns the builder's value");
        }
        assert_eq!(builds.load(SeqCst), 1, "only the first asker computes the value");
        assert_eq!(waits(std::any::type_name::<Shared>()), WAITERS, "each waiter waits through the host's hook, named by the type");
        assert_eq!(cell.get(), Some(&Shared(7)), "a ready read returns the value");
    }

    #[test]
    fn a_panicking_initialiser_wakes_its_waiter_which_computes_it() {
        #[derive(Debug, PartialEq)]
        struct Retried(u32);
        install_counting_hook();
        let cell = Arc::new(ComputeOnce::<Retried>::new());
        let started = Arc::new(AtomicUsize::new(0));
        let builder = {
            let (cell, started) = (cell.clone(), started.clone());
            thread::spawn(move || {
                cell.get_or_init(|| {
                    started.store(1, SeqCst);
                    until_waiting::<Retried>(1);
                    panic!("the initialiser fails");
                });
            })
        };
        while started.load(SeqCst) == 0 {
            thread::yield_now();
        }
        let waiter = {
            let cell = cell.clone();
            thread::spawn(move || cell.get_or_init(|| Retried(2)).0)
        };
        assert!(builder.join().is_err(), "the initialiser's panic reaches its caller");
        assert_eq!(waiter.join().unwrap(), 2, "the waiter wakes and computes the value itself");
        assert_eq!(cell.get(), Some(&Retried(2)), "the value is the retry's");
        assert_eq!(cell.get_or_init(|| Retried(3)), &Retried(2), "a later asker reads it");
    }
}
