use {
    crate::task::{Spawn, SpawnError},
    std::{
        future::Future,
        pin::Pin,
        sync::{
            mpsc::{Receiver, Sender},
            Arc, Mutex,
        },
        task::Wake
    }
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

impl Spawn for Spawner {
    fn spawn(&self, future: impl Future<Output = ()> + 'static) -> Result<(), SpawnError> {
        if let Err(_) = self.task_sender.send(Arc::new(Task {
            inner: Mutex::new(TaskInner {
                future: Some(Box::pin(future)),
                task_sender: self.task_sender.clone(),
            }),
        })) {
            return Err(SpawnError::shutdown());
        }
        Ok(())
    }
}

struct Task {
    inner: Mutex<TaskInner>,
}

impl Task {
    fn run(self: Arc<Task>) {
        use std::task::Context;

        let future = self.inner.lock().unwrap().future.take();
        if let Some(mut future) = future {
            let waker = super::task::waker(self.clone());
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
