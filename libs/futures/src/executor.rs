use std::{
    error, fmt,
    future::Future,
    pin::Pin,
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
    task::Wake,
};

#[derive(Debug)]
pub struct Executor {
    task_receiver: Receiver<Arc<Task>>,
}

impl Executor {
    pub fn run(&self) {
        while let Ok(task) = self.task_receiver.recv() {
            task.run();
        }
    }

    pub fn run_until_stalled(&self) {
        while let Ok(task) = self.task_receiver.try_recv() {
            task.run();
        }
    }
}

/// Called when a task is queued (spawned or woken), from whichever thread did it: the owner of
/// the [`Executor`] wakes and runs it.
pub type OnQueued = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct Spawner {
    task_sender: Sender<Arc<Task>>,
    on_queued: OnQueued,
}

impl fmt::Debug for Spawner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spawner").finish_non_exhaustive()
    }
}

impl Spawner {
    pub fn spawn(&self, future: impl Future<Output = ()> + 'static) -> Result<(), SpawnError> {
        if self
            .task_sender
            .send(Arc::new(Task {
                inner: Mutex::new(TaskInner {
                    future: Some(Box::pin(future)),
                    task_sender: self.task_sender.clone(),
                    on_queued: self.on_queued.clone(),
                }),
            }))
            .is_err()
        {
            return Err(SpawnError::shutdown());
        }
        (self.on_queued)();
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SpawnError {
    _priv: (),
}

impl SpawnError {
    pub fn shutdown() -> Self {
        Self { _priv: () }
    }
}

impl error::Error for SpawnError {}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "executor is shutdown")
    }
}

struct Task {
    inner: Mutex<TaskInner>,
}

impl Task {
    fn run(self: Arc<Task>) {
        use {crate::task, std::task::Context};

        let future = self.inner.lock().unwrap().future.take();
        if let Some(mut future) = future {
            let waker = task::waker(self.clone());
            let mut cx = Context::from_waker(&waker);
            if future.as_mut().poll(&mut cx).is_pending() {
                self.inner.lock().unwrap().future = Some(future);
            }
        }
    }
}

impl Wake for Task {
    fn wake(self: Arc<Task>) {
        let on_queued = {
            let inner = self.inner.lock().unwrap();
            inner.task_sender.send(self.clone()).unwrap();
            inner.on_queued.clone()
        };
        on_queued();
    }
}

struct TaskInner {
    future: Option<Pin<Box<dyn Future<Output = ()> + 'static>>>,
    task_sender: Sender<Arc<Task>>,
    on_queued: OnQueued,
}

/// `on_queued` runs whenever a task is queued, so a sleeping owner wakes to run it.
pub fn new_executor_and_spawner(on_queued: OnQueued) -> (Executor, Spawner) {
    use std::sync::mpsc;

    let (task_sender, task_receiver) = mpsc::channel();
    (Executor { task_receiver }, Spawner { task_sender, on_queued })
}
