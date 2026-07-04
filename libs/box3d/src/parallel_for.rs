// Port of box3d/src/parallel_for.h/.c
//
// The C version partitions [0, item_count) into blocks claimed by worker
// threads via an atomic counter. The Rust port is single-threaded: the callback
// runs once over the whole range with worker_index 0, which matches the C
// behavior with one worker (same visitation order).

/// C: b3ParallelForCallback(start, end, workerIndex, context) — context is
/// captured by the closure.
pub fn parallel_for(callback: &mut dyn FnMut(i32, i32, i32), item_count: i32, _min_block_size: i32) {
    if item_count <= 0 {
        return;
    }
    callback(0, item_count, 0);
}
