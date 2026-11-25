//! A list sharded by an unsafe associated fn of a subtrait that takes the supertrait's associated type
//! (`NonNull<Self::Target>`), implemented for a type whose parameter is bounded `'static`, and called from
//! a method of an impl whose self type names `L::Target`. Comparing that impl item with the trait's
//! needs the `S: 'static` the impl header implies.
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// # Safety
/// `as_raw` points at a live target.
pub unsafe trait Link {
    type Handle;
    type Target;
    fn as_raw(handle: &Self::Handle) -> NonNull<Self::Target>;
}

/// # Safety
/// An item's shard id never changes.
pub unsafe trait ShardedItem: Link {
    /// # Safety
    /// `target` points at a live item.
    unsafe fn shard_id(target: NonNull<Self::Target>) -> usize;
}

pub struct Header {
    id: usize,
}

pub struct Task<S: 'static> {
    raw: NonNull<Header>,
    _p: PhantomData<S>,
}

unsafe impl<S> Link for Task<S> {
    type Handle = Task<S>;
    type Target = Header;
    fn as_raw(handle: &Task<S>) -> NonNull<Header> {
        handle.raw
    }
}

unsafe impl<S> ShardedItem for Task<S> {
    unsafe fn shard_id(target: NonNull<Self::Target>) -> usize {
        unsafe { target.as_ref().id }
    }
}

pub struct ShardedList<L, T> {
    shards: Box<[Mutex<Vec<NonNull<T>>>]>,
    _p: PhantomData<L>,
}

impl<L, T> ShardedList<L, T> {
    pub fn new(n: usize) -> Self {
        ShardedList { shards: (0..n).map(|_| Mutex::new(Vec::new())).collect(), _p: PhantomData }
    }
}

impl<L: ShardedItem> ShardedList<L, L::Target> {
    pub fn push(&self, val: &L::Handle) -> usize {
        let id = unsafe { L::shard_id(L::as_raw(val)) } % self.shards.len();
        self.shards[id].lock().unwrap().push(L::as_raw(val));
        id
    }
}

pub struct Scheduler;

pub struct Owned<S: 'static> {
    list: ShardedList<Task<S>, <Task<S> as Link>::Target>,
}

impl<S: 'static> Owned<S> {
    fn bind(&self, task: Task<S>) -> usize {
        self.list.push(&task)
    }
}

pub fn run() -> (usize, usize) {
    let owned: Owned<Arc<Scheduler>> = Owned { list: ShardedList::new(4) };
    let headers: &'static [Header; 2] = Box::leak(Box::new([Header { id: 6 }, Header { id: 3 }]));
    let task = |h: &'static Header| Task::<Arc<Scheduler>> { raw: NonNull::from(h), _p: PhantomData };
    (owned.bind(task(&headers[0])), owned.bind(task(&headers[1])))
}
