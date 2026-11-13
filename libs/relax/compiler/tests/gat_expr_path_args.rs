// An expression path through a generic associated type with its arguments left out
// (`Self::RwLock::new(value)` for `type RwLock<T> = RwLock<T>`, eyeball 0.8's Lock impls):
// rustc infers them (seg.infer_args) like the arguments of `RwLock::new(value)`.
use std::ops::Deref;
use std::sync::{Arc, RwLock, RwLockReadGuard};

pub trait Lock {
    type RwLock<T>;
    type RwLockReadGuard<'a, T>: Deref<Target = T>
    where
        T: 'a;
    type Shared<T>: Deref<Target = T>;
    type Buf<const N: usize>;

    fn new_rwlock<T>(value: T) -> Self::RwLock<T>;
    fn read_noblock<T>(lock: &Self::RwLock<T>) -> Self::RwLockReadGuard<'_, T>;
    fn new_shared<T>(value: T) -> Self::Shared<T>;
    fn shared_read_count<T>(shared: &Self::Shared<T>) -> usize;
    fn new_buf<const N: usize>() -> Self::Buf<N>;
}

pub enum SyncLock {}

impl Lock for SyncLock {
    type RwLock<T> = RwLock<T>;
    type RwLockReadGuard<'a, T> = RwLockReadGuard<'a, T>
    where
        T: 'a;
    type Shared<T> = Arc<T>;
    type Buf<const N: usize> = Vec<[u8; N]>;

    fn new_rwlock<T>(value: T) -> Self::RwLock<T> {
        Self::RwLock::new(value)
    }
    fn read_noblock<T>(lock: &Self::RwLock<T>) -> Self::RwLockReadGuard<'_, T> {
        lock.try_read().unwrap()
    }
    fn new_shared<T>(value: T) -> Self::Shared<T> {
        Self::Shared::new(value)
    }
    fn shared_read_count<T>(shared: &Self::Shared<T>) -> usize {
        Self::Shared::strong_count(shared)
    }
    fn new_buf<const N: usize>() -> Self::Buf<N> {
        <Self as Lock>::Buf::with_capacity(2)
    }
}

struct Observable<T, L: Lock = SyncLock> {
    state: L::Shared<L::RwLock<T>>,
}

impl<T, L: Lock> Observable<T, L> {
    fn new(value: T) -> Self {
        Self { state: L::new_shared(L::new_rwlock(value)) }
    }
}

#[test]
fn gat_args_inferred_in_expression_paths() {
    let lock = SyncLock::new_rwlock(5u32);
    assert_eq!(*SyncLock::read_noblock(&lock), 5);
    let shared = SyncLock::new_shared(String::from("x"));
    let other = shared.clone();
    assert_eq!(SyncLock::shared_read_count(&shared), 2);
    drop(other);
    assert_eq!(<SyncLock as Lock>::shared_read_count(&shared), 1);
    let o: Observable<Vec<u8>> = Observable::new(vec![1, 2, 3]);
    assert_eq!(SyncLock::read_noblock(&*o.state).len(), 3);
    let b = SyncLock::new_buf::<4>();
    assert!(b.capacity() >= 2 && b.is_empty());
}
