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

#[derive(Clone, Debug)]
pub struct Spawner {
    task_sender: Sender<Arc<Task>>,
}

impl Spawner {
    pub fn spawn(&self, future: impl Future<Output = ()> + 'static) -> Result<(), SpawnError> {
        if self.task_sender.send(Arc::new(Task {
            inner: Mutex::new(TaskInner {
                future: Some(Box::pin(future)),
                task_sender: self.task_sender.clone(),
            }),
        })).is_err() {
            return Err(SpawnError::shutdown());
        }
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
        use {std::task::Context, crate::task};

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
        self.inner
            .lock()
            .unwrap()
            .task_sender
            .send(self.clone())
            .unwrap();
    }
}

struct TaskInner {
    future: Option<Pin<Box<dyn Future<Output = ()> + 'static>>>,
    task_sender: Sender<Arc<Task>>,
}

pub fn new_executor_and_spawner() -> (Executor, Spawner) {
    use std::sync::mpsc;

    let (task_sender, task_receiver) = mpsc::channel();
    (Executor { task_receiver }, Spawner { task_sender })
}
