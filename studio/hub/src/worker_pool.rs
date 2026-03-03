use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

type Job = Box<dyn FnOnce() + Send + 'static>;

enum WorkerMessage {
    Job(Job),
    Shutdown,
}

pub struct WorkerPool {
    sender: Sender<WorkerMessage>,
    workers: Vec<JoinHandle<()>>,
    worker_count: usize,
}

impl WorkerPool {
    pub fn new(worker_count: usize) -> Self {
        let worker_count = worker_count.max(1);
        let (sender, receiver) = mpsc::channel::<WorkerMessage>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let receiver = Arc::clone(&receiver);
            workers.push(thread::spawn(move || loop {
                let message = {
                    let Ok(receiver) = receiver.lock() else {
                        break;
                    };
                    receiver.recv()
                };
                match message {
                    Ok(WorkerMessage::Job(job)) => job(),
                    Ok(WorkerMessage::Shutdown) | Err(_) => break,
                }
            }));
        }

        Self {
            sender,
            workers,
            worker_count,
        }
    }

    pub fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let _ = self.sender.send(WorkerMessage::Job(Box::new(job)));
    }

    pub fn worker_count(&self) -> usize {
        self.worker_count
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for _ in 0..self.workers.len() {
            let _ = self.sender.send(WorkerMessage::Shutdown);
        }
        while let Some(worker) = self.workers.pop() {
            let _ = worker.join();
        }
    }
}
