// Port of box3d/src/parallel_for.h + parallel_for.c
//
// Divide [0, item_count) into blocks and process them with cooperative
// claiming: up to worker_count tasks are enqueued, and each task loops,
// atomically claiming the next unclaimed block until the range is drained.
// Blocks the caller until all work is complete, so the shared state (and
// anything `context` points to) lives on the caller's stack exactly like C.
//
// worker_index is the TASK identity (0..task_count-1), stable across all
// invocations from the same task, so it is safe to index per-worker state
// (world.task_contexts[worker_index]); at most one live task per index.
//
// With worker_count <= 1 or no scheduler the callback runs inline as a single
// (0, item_count, 0) call — identical to the serial port this replaces, so
// single-threaded behavior (and its float determinism) is unchanged.

use crate::b3_assert;
use crate::constants::MAX_WORKERS;
use crate::scheduler::{TaskCallback, TaskHandle, TaskSystem};
use crate::sync::AtomicIndex;

/// C: b3ParallelForCallback(startIndex, endIndex, workerIndex, context).
pub type ParallelForCallback = unsafe fn(start_index: i32, end_index: i32, worker_index: i32, context: *mut ());

// Shared state for one parallel_for invocation. Workers race on next_block to
// claim work, so a slow chunk can't strand the other threads.
// C: b3ParallelForShared
struct ParallelForShared {
    next_block: AtomicIndex,
    block_count: i32,
    block_size: i32,
    item_count: i32,
    callback: ParallelForCallback,
    context: *mut (),
}

// C: b3ParallelForTask
struct ParallelForTask {
    shared: *const ParallelForShared,
    worker_index: i32,
}

// C: b3ParallelForTrampoline
unsafe fn parallel_for_trampoline(task_context: *mut ()) {
    // SAFETY: the task and shared structs live on the parallel_for stack
    // frame, which blocks in scheduler_finish_task until every task
    // completes; the scheduler's status protocol publishes the writes.
    let task = unsafe { &*(task_context as *const ParallelForTask) };
    let shared = unsafe { &*task.shared };
    let worker_index = task.worker_index;
    let context = shared.context;
    let callback = shared.callback;

    let block_count = shared.block_count;
    let block_size = shared.block_size;
    let item_count = shared.item_count;

    loop {
        let block_index = shared.next_block.fetch_add(1);
        if block_index >= block_count {
            break;
        }

        let start = block_index * block_size;
        let mut end = start + block_size;
        if end > item_count {
            end = item_count;
        }

        unsafe { callback(start, end, worker_index, context) };
    }
}

/// C: b3ParallelFor(world, callback, itemCount, minRange, context, name).
/// The port passes the task system and worker count instead of the world so
/// tasks may reference world data through `context` without aliasing.
///
/// # Safety
/// `callback` invocations may run concurrently on task-system threads
/// with distinct (start, end) ranges and distinct worker_index values; the
/// callback must only touch data that is safe under that partitioning
/// (disjoint per-index element access, per-worker_index state, atomics).
/// `context` must stay valid until this function returns (it does not return
/// until all work completes).
pub unsafe fn parallel_for(
    task_system: &TaskSystem,
    worker_count: i32,
    callback: ParallelForCallback,
    item_count: i32,
    min_range: i32,
    context: *mut (),
    name: &str,
) {
    if item_count <= 0 {
        return;
    }

    b3_assert!(min_range > 0);
    b3_assert!(0 < worker_count && worker_count <= MAX_WORKERS as i32);

    // Serial fast path: identical to the pre-threading port (a single
    // whole-range invocation on worker 0). C reaches the same result through
    // b3DefaultAddTaskFcn running the trampoline inline.
    if worker_count <= 1 || !task_system.is_parallel() {
        unsafe { callback(0, item_count, 0, context) };
        return;
    }

    // Target multiple blocks per worker to reduce thread stalls.
    // Block size grows once items exceed max_block_count * min_range
    // so the block count stays bounded and per-block sync overhead stays low.
    let blocks_per_worker = 4;
    let max_block_count = blocks_per_worker * worker_count;

    let block_size;
    let block_count;
    if item_count <= min_range * max_block_count {
        block_size = min_range;
        block_count = (item_count + block_size - 1) / block_size;
    } else {
        block_size = (item_count + max_block_count - 1) / max_block_count;
        block_count = (item_count + block_size - 1) / block_size;
    }
    b3_assert!(block_count >= 1);
    b3_assert!(block_size * block_count >= item_count);

    // No point enqueueing more tasks than blocks.
    let task_count = if worker_count < block_count { worker_count } else { block_count };

    let shared = ParallelForShared {
        next_block: AtomicIndex::new(0),
        block_count,
        block_size,
        item_count,
        callback,
        context,
    };

    let mut tasks: Vec<ParallelForTask> = Vec::with_capacity(task_count as usize);
    for i in 0..task_count {
        tasks.push(ParallelForTask { shared: &shared as *const ParallelForShared, worker_index: i });
    }

    // C: handles[i] == NULL marks tasks that ran inline (task ring budget
    // exhausted, or an external system that executed synchronously). The
    // budget check and inline fallback live inside TaskSystem::enqueue.
    let mut handles: [TaskHandle; MAX_WORKERS] = [TaskHandle::Inline; MAX_WORKERS];
    for i in 0..task_count as usize {
        // SAFETY: `tasks` and `shared` outlive the finish loop below, and
        // the trampoline honors the block partitioning contract.
        handles[i] = unsafe {
            task_system.enqueue(
                parallel_for_trampoline as TaskCallback,
                &tasks[i] as *const ParallelForTask as *mut (),
                name,
            )
        };
    }

    for handle in handles.iter().take(task_count as usize) {
        task_system.finish(*handle);
    }
}

/// Safe serial wrapper (the pre-threading port's API): one whole-range call on
/// worker 0. Use in code paths that are inherently single-threaded.
pub fn parallel_for_serial(callback: &mut dyn FnMut(i32, i32, i32), item_count: i32) {
    if item_count <= 0 {
        return;
    }
    callback(0, item_count, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncSlice;

    // 4 workers claim blocks over 10_000 items; every slot must be touched
    // exactly once (block disjointness), independent of execution order.
    #[test]
    fn parallel_for_touches_each_item_once() {
        let task_system = TaskSystem::internal(4);

        struct Ctx<'a> {
            view: SyncSlice<'a, u32>,
        }

        unsafe fn body(start: i32, end: i32, _worker_index: i32, context: *mut ()) {
            let ctx = unsafe { &*(context as *const Ctx) };
            for i in start..end {
                // SAFETY: blocks are disjoint, so index i is visited by
                // exactly one worker.
                unsafe { *ctx.view.get_mut(i as usize) += 1 };
            }
        }

        for round in 0..8 {
            task_system.reset();

            let mut counts = vec![0u32; 10_000];
            let item_count = counts.len() as i32;
            {
                let ctx = Ctx { view: SyncSlice::new(&mut counts) };
                // SAFETY: ctx outlives the call; body honors disjoint blocks.
                unsafe {
                    parallel_for(
                        &task_system,
                        4,
                        body,
                        item_count,
                        16,
                        &ctx as *const Ctx as *mut (),
                        "test",
                    );
                }
            }

            assert!(
                counts.iter().all(|&c| c == 1),
                "round {}: every item must be processed exactly once",
                round
            );
        }
    }

    // The serial path must behave exactly like the pre-threading port:
    // one whole-range call with worker index 0.
    #[test]
    fn parallel_for_serial_path_single_call() {
        unsafe fn body(start: i32, end: i32, worker_index: i32, context: *mut ()) {
            let calls = unsafe { &mut *(context as *mut Vec<(i32, i32, i32)>) };
            calls.push((start, end, worker_index));
        }

        let mut calls: Vec<(i32, i32, i32)> = Vec::new();
        unsafe {
            parallel_for(
                &TaskSystem::serial(),
                1,
                body,
                100,
                8,
                &mut calls as *mut _ as *mut (),
                "test",
            );
        }
        assert_eq!(calls, vec![(0, 100, 0)]);
    }
}
