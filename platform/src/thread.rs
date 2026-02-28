use {
    crate::{cx::Cx, cx_api::*},
    std::sync::{
        mpsc::{channel, Sender},
        Arc, Mutex,
    },
};

pub use makepad_network::{
    FromUIReceiver, FromUISender, SignalFromUI, SignalToUI, ToUIReceiver, ToUISender,
};

pub struct RevThreadPool {
    tasks: Arc<Mutex<Vec<Box<dyn FnOnce() + Send + 'static>>>>,
}

impl RevThreadPool {
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let tasks: Arc<Mutex<Vec<Box<dyn FnOnce() + Send + 'static>>>> = Default::default();

        for _ in 0..num_threads {
            let tasks = tasks.clone();
            cx.spawn_thread(move || loop {
                let task = if let Ok(mut tasks) = tasks.lock() {
                    tasks.pop()
                } else {
                    panic!();
                };
                if let Some(task) = task {
                    task();
                }
            })
        }
        Self { tasks }
    }

    pub fn execute<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.tasks.lock().unwrap().insert(0, Box::new(task));
    }

    pub fn execute_rev<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.tasks.lock().unwrap().push(Box::new(task));
    }
}

pub struct TagThreadPool<T: Clone + Send + 'static + PartialEq> {
    tasks: Arc<Mutex<Vec<(T, Box<dyn FnOnce(T) + Send + 'static>)>>>,
}

impl<T> TagThreadPool<T>
where
    T: Clone + Send + 'static + PartialEq,
{
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let tasks: Arc<Mutex<Vec<(T, Box<dyn FnOnce(T) + Send + 'static>)>>> = Default::default();

        for _ in 0..num_threads {
            let tasks = tasks.clone();
            cx.spawn_thread(move || loop {
                let task = if let Ok(mut tasks) = tasks.lock() {
                    tasks.pop()
                } else {
                    panic!()
                };
                if let Some((tag, task)) = task {
                    task(tag);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            })
        }
        Self { tasks }
    }

    pub fn execute<F>(&self, tag: T, task: F)
    where
        F: FnOnce(T) + Send + 'static,
    {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.retain(|v| v.0 != tag);
            tasks.insert(0, (tag, Box::new(task)));
        }
    }

    pub fn execute_rev<F>(&self, tag: T, task: F)
    where
        F: FnOnce(T) + Send + 'static,
    {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.retain(|v| v.0 != tag);
            tasks.push((tag, Box::new(task)));
        }
    }
}

pub struct MessageThreadPool<T: Clone + Send + 'static> {
    sender: Sender<Box<dyn FnOnce(Option<T>) + Send + 'static>>,
    msg_senders: Vec<Sender<T>>,
}

impl<T> MessageThreadPool<T>
where
    T: Clone + Send + 'static,
{
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let (sender, receiver) = channel::<Box<dyn FnOnce(Option<T>) + Send + 'static>>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut msg_senders = Vec::new();
        for _ in 0..num_threads {
            let receiver = receiver.clone();
            let (msg_send, msg_recv) = channel::<T>();
            msg_senders.push(msg_send);
            cx.spawn_thread(move || loop {
                let task = if let Ok(receiver) = receiver.lock() {
                    match receiver.recv() {
                        Ok(task) => task,
                        Err(_) => return,
                    }
                } else {
                    panic!();
                };
                let mut msg_out = None;
                while let Ok(msg) = msg_recv.try_recv() {
                    msg_out = Some(msg);
                }
                task(msg_out);
            })
        }
        Self {
            sender,
            msg_senders,
        }
    }

    pub fn send_msg(&self, msg: T) {
        for sender in &self.msg_senders {
            sender.send(msg.clone()).unwrap();
        }
    }

    pub fn execute<F>(&self, task: F)
    where
        F: FnOnce(Option<T>) + Send + 'static,
    {
        self.sender.send(Box::new(task)).unwrap();
    }
}
