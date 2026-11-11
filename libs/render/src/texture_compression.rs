//! Adapt the caller's task pool to the texture encoder's chunk executor.

use makepad_draw::makepad_platform::thread::TaskPool;

/// Run all chunks through the pool, returning after every chunk finishes.
/// Call from a worker, as required by [`TaskPool::fan_out`].
pub fn pool_executor(pool: &TaskPool) -> impl Fn(usize, &(dyn Fn(usize) + Sync)) + '_ {
    // One chunk is the caller's own item: there is nothing to share, and a
    // fan-out of one still costs a helper hand-off and a budget checkpoint
    // (about 12 ms each, measured over 234 single-mip encodes of one model).
    move |chunks, work| if chunks == 1 { work(0) } else { pool.fan_out(chunks, work) }
}
