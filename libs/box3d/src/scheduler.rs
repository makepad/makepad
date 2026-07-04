// Port of box3d/src/scheduler.h + scheduler.c
//
// The built-in task scheduler: a fixed ring of task slots claimed by
// background worker threads via CAS, a counting semaphore to wake workers,
// and a finish-task wait that HELPS by executing other pending tasks while
// waiting (load-bearing for the solver: the main thread never idles while
// workers drain a stage).
//
// Rust mapping (std only):
// - The C heap-allocated b3Scheduler becomes Arc<SchedulerShared> so worker
//   threads hold owned references; the owning `Scheduler` joins the threads
//   on destroy/Drop.
// - b3Semaphore -> Mutex<i32> + Condvar.
// - The C task/context pointers are published with plain writes followed by a
//   SEQ_CST status store; readers claim with a SEQ_CST CAS. The port keeps
//   that protocol with the fields in UnsafeCells: the release/acquire edge on
//   `status` orders the field writes before any post-claim read, so the
//   unsafe reads are race-free.
// - b3SchedulerEnqueueTask returns the task pointer; the port returns the
//   slot index. Deviation: the C world tracks world->taskCount to bound the
//   ring; the port counts enqueues on the scheduler itself (task_count),
//   reset by reset_scheduler, to avoid aliasing &mut World from task code.

use std::cell::UnsafeCell;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::b3_assert;
use crate::constants::{MAX_TASKS, MAX_WORKERS};
use crate::sync::AtomicIndex;

/// C: b3TaskCallback. Tasks reference stack data of the enqueueing scope via
/// raw pointers exactly like C; the enqueuer must finish every task before
/// that data goes out of scope.
pub type TaskCallback = unsafe fn(*mut ());

// enum b3SchedulerTaskStatus
const STATUS_FREE: i32 = 0;
const STATUS_PENDING: i32 = 1;
const STATUS_CLAIMED: i32 = 2;
const STATUS_COMPLETE: i32 = 3;

struct SchedulerTask {
    callback: UnsafeCell<Option<TaskCallback>>,
    task_context: UnsafeCell<*mut ()>,
    status: AtomicIndex,
}

impl Default for SchedulerTask {
    fn default() -> Self {
        SchedulerTask {
            callback: UnsafeCell::new(None),
            task_context: UnsafeCell::new(std::ptr::null_mut()),
            status: AtomicIndex::new(STATUS_FREE),
        }
    }
}

// C: b3Semaphore via Mutex + Condvar.
struct Semaphore {
    count: Mutex<i32>,
    cond: Condvar,
}

impl Semaphore {
    fn new(init_count: i32) -> Semaphore {
        Semaphore { count: Mutex::new(init_count), cond: Condvar::new() }
    }

    /// C: b3WaitSemaphore
    fn wait(&self) {
        let mut count = self.count.lock().unwrap();
        while *count == 0 {
            count = self.cond.wait(count).unwrap();
        }
        *count -= 1;
    }

    /// C: b3SignalSemaphore
    fn signal(&self) {
        let mut count = self.count.lock().unwrap();
        *count += 1;
        drop(count);
        self.cond.notify_one();
    }
}

struct SchedulerShared {
    tasks: Vec<SchedulerTask>,
    next_slot: AtomicIndex,
    /// Enqueues since the last reset (the port's home for the C
    /// world->taskCount ring budget).
    task_count: AtomicIndex,
    task_semaphore: Semaphore,
    shutdown: AtomicIndex,
}

// SAFETY: the UnsafeCell task fields are published/consumed under the status
// protocol documented at the top of the file; everything else is atomics,
// the semaphore, or immutable after construction.
unsafe impl Sync for SchedulerShared {}
unsafe impl Send for SchedulerShared {}

/// C: b3Scheduler. Owned by the world; worker threads run until destroy.
pub struct Scheduler {
    shared: Arc<SchedulerShared>,
    threads: Vec<JoinHandle<()>>,
    /// total workers including the main thread
    worker_count: i32,
}

// Try to claim and execute one pending task.
// Returns true if work was performed, false otherwise.
// C: b3SchedulerExecuteOne
fn scheduler_execute_one(shared: &SchedulerShared) -> bool {
    let task_count = shared.next_slot.load();
    for t in 0..task_count {
        let task = &shared.tasks[t as usize];
        if task.status.load() != STATUS_PENDING {
            continue;
        }

        if !task.status.compare_exchange(STATUS_PENDING, STATUS_CLAIMED) {
            continue;
        }

        // SAFETY: the enqueue wrote callback/context before the SeqCst
        // status store to PENDING; winning the CAS gives this thread
        // exclusive claim and a happens-before edge to those writes.
        unsafe {
            let callback = (*task.callback.get()).unwrap();
            let context = *task.task_context.get();
            callback(context);
        }

        task.status.store(STATUS_COMPLETE);
        return true;
    }

    false
}

// Background worker thread entry point.
// C: b3SchedulerWorkerMain
fn scheduler_worker_main(shared: Arc<SchedulerShared>) {
    loop {
        shared.task_semaphore.wait();

        if shared.shutdown.load() != 0 {
            break;
        }

        // Claim and execute all available work
        while scheduler_execute_one(&shared) {}
    }
}

/// C: b3CreateScheduler. Background threads use worker indices
/// 1..worker_count-1; the calling (main) thread is worker 0.
pub fn create_scheduler(worker_count: i32) -> Scheduler {
    b3_assert!(0 < worker_count && worker_count <= MAX_WORKERS as i32);

    let mut tasks = Vec::with_capacity(MAX_TASKS);
    for _ in 0..MAX_TASKS {
        tasks.push(SchedulerTask::default());
    }

    let shared = Arc::new(SchedulerShared {
        tasks,
        next_slot: AtomicIndex::new(0),
        task_count: AtomicIndex::new(0),
        task_semaphore: Semaphore::new(0),
        shutdown: AtomicIndex::new(0),
    });

    let thread_count = worker_count - 1;
    let mut threads = Vec::with_capacity(thread_count as usize);
    for i in 0..thread_count {
        let worker_shared = Arc::clone(&shared);
        threads.push(
            std::thread::Builder::new()
                .name(format!("box3d_worker_{:02}", i + 1))
                .spawn(move || scheduler_worker_main(worker_shared))
                .expect("failed to spawn box3d worker thread"),
        );
    }

    Scheduler { shared, threads, worker_count }
}

/// C: b3DestroyScheduler (also runs on Drop).
pub fn destroy_scheduler(scheduler: &mut Scheduler) {
    if scheduler.threads.is_empty() {
        return;
    }

    scheduler.shared.shutdown.store(1);

    // Wake all background threads so they see the shutdown flag
    for _ in 0..scheduler.threads.len() {
        scheduler.shared.task_semaphore.signal();
    }

    for thread in scheduler.threads.drain(..) {
        let _ = thread.join();
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        destroy_scheduler(self);
    }
}

impl Scheduler {
    /// Total workers including the main thread.
    #[inline]
    pub fn worker_count(&self) -> i32 {
        self.worker_count
    }
}

/// C: b3ResetScheduler — recycles the task ring between phases/steps.
/// Only call when no tasks are pending or running.
pub fn reset_scheduler(scheduler: &Scheduler) {
    scheduler.shared.next_slot.store(0);
    scheduler.shared.task_count.store(0);
}

/// Enqueues since the last reset (the port's task-ring budget; C tracks this
/// on the world as world->taskCount).
pub fn scheduler_task_count(scheduler: &Scheduler) -> i32 {
    scheduler.shared.task_count.load()
}

/// C: b3SchedulerEnqueueTask. Returns the task slot to pass to
/// scheduler_finish_task.
///
/// # Safety
/// `task_context` must stay valid (and any data it points to must remain
/// safe to access under `callback`'s own contract) until
/// scheduler_finish_task returns for the returned slot.
pub unsafe fn scheduler_enqueue_task(
    scheduler: &Scheduler,
    task: TaskCallback,
    task_context: *mut (),
    _name: &str,
) -> i32 {
    let shared = &scheduler.shared;

    let slot = shared.next_slot.fetch_add(1);
    b3_assert!(slot < MAX_TASKS as i32);
    shared.task_count.fetch_add(1);

    let scheduler_task = &shared.tasks[slot as usize];
    // SAFETY: the slot was exclusively claimed by the fetch_add above and its
    // status is not PENDING (fresh ring position after reset), so no worker
    // reads these cells until the status store below publishes them.
    unsafe {
        *scheduler_task.callback.get() = Some(task);
        *scheduler_task.task_context.get() = task_context;
    }

    // Memory fence: status must be published after callback and context are written
    scheduler_task.status.store(STATUS_PENDING);

    // One wake per enqueue is enough: at most one worker picks up each task.
    shared.task_semaphore.signal();

    slot
}

/// C: b3SchedulerFinishTask. Blocks until the task completes; the calling
/// thread helps execute any available work while waiting so it never idles
/// while background threads are busy on other tasks from the same phase.
pub fn scheduler_finish_task(scheduler: &Scheduler, slot: i32) {
    let shared = &scheduler.shared;
    let wait_task = &shared.tasks[slot as usize];

    while wait_task.status.load() != STATUS_COMPLETE {
        if !scheduler_execute_one(shared) {
            std::thread::yield_now();
        }
    }
}

// ---------------------------------------------------------------------------
// External task system hooks (C: b3WorldDef enqueueTask/finishTask/userTaskContext)
// ---------------------------------------------------------------------------

/// C: b3EnqueueTaskCallback (types.h). Provided by the user to run Box3D tasks
/// on their own job system. The user's system must run `task(task_context)`
/// exactly once, on any thread. Returning null tells Box3D the task was
/// executed inline (synchronously) and finish will not be called for it;
/// otherwise the returned pointer is passed back to the finish callback.
///
/// # Safety contract (same as C)
/// - `task_context` points at stack data of `world_step` and is only valid
///   until the matching finish returns (or, for a null return, until the
///   enqueue call returns).
/// - The finish callback must BLOCK until the task has completed. Box3D holds
///   its stack across every fork/join; a job system that cannot park a job's
///   stack must not call `world_step` from inside a job (see the C types.h
///   discussion).
pub type EnqueueTaskCallback =
    unsafe fn(task: TaskCallback, task_context: *mut (), user_context: *mut (), name: &str) -> *mut ();

/// C: b3FinishTaskCallback. Must block until the user task has completed.
pub type FinishTaskCallback = unsafe fn(user_task: *mut (), user_context: *mut ());

/// Handle returned by TaskSystem::enqueue (C: the `void*` user task pointer,
/// where NULL means the task already ran inline and needs no finish).
#[derive(Clone, Copy, Debug)]
pub enum TaskHandle {
    /// The task was executed inline at the enqueue site (C: NULL return, or
    /// the serial path, or the task ring budget was exhausted).
    Inline,
    /// Built-in scheduler task slot.
    Internal(i32),
    /// User task pointer from an external task system.
    External(*mut ()),
}

/// The world's task dispatch (C: world->enqueueTaskFcn/finishTaskFcn/
/// userTaskContext + world->scheduler, selected at world creation).
pub enum TaskSystemKind {
    /// worker_count == 1 and no user callbacks: every task runs inline at the
    /// enqueue site (C: b3DefaultAddTaskFcn / b3DefaultFinishTaskFcn).
    Serial,
    /// The built-in scheduler (C: b3SchedulerEnqueueTask/b3SchedulerFinishTask).
    Internal(Scheduler),
    /// User-provided job system (C: def->enqueueTask/finishTask).
    External {
        enqueue: EnqueueTaskCallback,
        finish: FinishTaskCallback,
        user_context: *mut (),
    },
}

pub struct TaskSystem {
    pub kind: TaskSystemKind,
    /// C: world->taskCount — enqueues since the last reset, used to bound the
    /// task ring (B3_MAX_TASKS). The internal scheduler keeps its own count;
    /// this counter serves the serial and external paths.
    task_count: AtomicIndex,
}

impl Default for TaskSystem {
    fn default() -> Self {
        TaskSystem::serial()
    }
}

impl TaskSystem {
    pub fn serial() -> TaskSystem {
        TaskSystem { kind: TaskSystemKind::Serial, task_count: AtomicIndex::new(0) }
    }

    pub fn internal(worker_count: i32) -> TaskSystem {
        TaskSystem {
            kind: TaskSystemKind::Internal(create_scheduler(worker_count)),
            task_count: AtomicIndex::new(0),
        }
    }

    /// # Safety
    /// The callbacks must uphold the EnqueueTaskCallback/FinishTaskCallback
    /// contracts; `user_context` must stay valid for the world's lifetime.
    pub fn external(
        enqueue: EnqueueTaskCallback,
        finish: FinishTaskCallback,
        user_context: *mut (),
    ) -> TaskSystem {
        TaskSystem {
            kind: TaskSystemKind::External { enqueue, finish, user_context },
            task_count: AtomicIndex::new(0),
        }
    }

    /// True when tasks may run on other threads (internal or external system).
    #[inline]
    pub fn is_parallel(&self) -> bool {
        !matches!(self.kind, TaskSystemKind::Serial)
    }

    /// C: world->taskCount (enqueues since the last reset).
    pub fn task_count(&self) -> i32 {
        match &self.kind {
            TaskSystemKind::Internal(scheduler) => scheduler_task_count(scheduler),
            _ => self.task_count.load(),
        }
    }

    /// C call-site pattern:
    ///   if (world->taskCount < B3_MAX_TASKS) { task = enqueueTaskFcn(...); taskCount += 1; }
    ///   else { run inline }
    /// The budget check is centralized here; when the ring budget is exhausted
    /// or the system is Serial, the task runs inline (same execution position)
    /// and TaskHandle::Inline is returned.
    ///
    /// # Safety
    /// `task_context` (and everything it references) must stay valid until
    /// `finish` returns for the returned handle. `task` may run concurrently
    /// on another thread under its own data-access contract.
    pub unsafe fn enqueue(&self, task: TaskCallback, task_context: *mut (), name: &str) -> TaskHandle {
        match &self.kind {
            TaskSystemKind::Serial => {
                self.task_count.fetch_add(1);
                // SAFETY: forwarded caller contract; inline execution.
                unsafe { task(task_context) };
                TaskHandle::Inline
            }

            TaskSystemKind::Internal(scheduler) => {
                if scheduler_task_count(scheduler) < MAX_TASKS as i32 {
                    // SAFETY: forwarded caller contract.
                    let slot = unsafe { scheduler_enqueue_task(scheduler, task, task_context, name) };
                    TaskHandle::Internal(slot)
                } else {
                    // SAFETY: forwarded caller contract; inline execution.
                    unsafe { task(task_context) };
                    TaskHandle::Inline
                }
            }

            TaskSystemKind::External { enqueue, user_context, .. } => {
                if self.task_count.load() < MAX_TASKS as i32 {
                    self.task_count.fetch_add(1);
                    // SAFETY: forwarded caller contract; the user system runs
                    // the task exactly once per the EnqueueTaskCallback contract.
                    let user_task = unsafe { enqueue(task, task_context, *user_context, name) };
                    if user_task.is_null() {
                        // C: NULL means the user executed the task inline.
                        TaskHandle::Inline
                    } else {
                        TaskHandle::External(user_task)
                    }
                } else {
                    // SAFETY: forwarded caller contract; inline execution.
                    unsafe { task(task_context) };
                    TaskHandle::Inline
                }
            }
        }
    }

    /// C: world->finishTaskFcn(userTask, userTaskContext). Blocks until the
    /// task completes. No-op for TaskHandle::Inline (C skips NULL user tasks).
    pub fn finish(&self, handle: TaskHandle) {
        match handle {
            TaskHandle::Inline => {}

            TaskHandle::Internal(slot) => match &self.kind {
                TaskSystemKind::Internal(scheduler) => scheduler_finish_task(scheduler, slot),
                _ => unreachable!("internal task handle without internal scheduler"),
            },

            TaskHandle::External(user_task) => match &self.kind {
                TaskSystemKind::External { finish, user_context, .. } => {
                    // SAFETY: handle came from this system's enqueue; the user
                    // finish blocks until the task completes (contract).
                    unsafe { finish(user_task, *user_context) };
                }
                _ => unreachable!("external task handle without external system"),
            },
        }
    }

    /// C: b3ResetScheduler + world->taskCount = 0. Recycle the task ring
    /// between phases/steps. Only call when no tasks are pending or running.
    pub fn reset(&self) {
        if let TaskSystemKind::Internal(scheduler) = &self.kind {
            reset_scheduler(scheduler);
        }
        self.task_count.store(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncSlice;

    // World wiring: worker_count > 1 spawns the scheduler at create, the step
    // path is unaffected (stages still run on worker 0 until they are
    // parallelized), and destroy joins the background threads.
    #[test]
    fn world_with_workers_creates_and_destroys() {
        let mut def = crate::types::default_world_def();
        def.worker_count = 4;
        let mut world = crate::physics_world::create_world(&def);
        assert_eq!(world.worker_count, 4);
        assert!(matches!(world.task_system.kind, TaskSystemKind::Internal(_)));
        assert_eq!(world.task_contexts.len(), 4);

        crate::physics_world::world_step(&mut world, 1.0 / 60.0, 4);
        crate::physics_world::world_step(&mut world, 1.0 / 60.0, 4);

        crate::physics_world::destroy_world(world);
    }

    // All tasks must run even with zero background threads: the finish-task
    // help loop is the only executor. This proves the C help-while-waiting
    // behavior was ported.
    #[test]
    fn finish_task_executes_pending_work() {
        let scheduler = create_scheduler(1); // 0 background threads
        reset_scheduler(&scheduler);

        let mut flags = [0u32; 8];
        let view = SyncSlice::new(&mut flags);

        struct Ctx<'a> {
            view: &'a SyncSlice<'a, u32>,
            index: usize,
        }

        unsafe fn task(context: *mut ()) {
            let ctx = unsafe { &*(context as *const Ctx) };
            unsafe { *ctx.view.get_mut(ctx.index) += 1 };
        }

        let contexts: Vec<Ctx> = (0..8).map(|index| Ctx { view: &view, index }).collect();
        let mut slots = Vec::new();
        for ctx in &contexts {
            // SAFETY: contexts outlive the finish loop below.
            let slot = unsafe {
                scheduler_enqueue_task(&scheduler, task, ctx as *const Ctx as *mut (), "test")
            };
            slots.push(slot);
        }

        for slot in slots {
            scheduler_finish_task(&scheduler, slot);
        }

        drop(view);
        assert_eq!(flags, [1u32; 8]);
    }
}
