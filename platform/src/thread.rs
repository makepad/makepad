use {
    std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{
            channel,
            Sender,
            Receiver,
            RecvError,
            TryRecvError,
            SendError,
        },
        Arc,
        Mutex
    },
    crate::{
        cx::Cx,
        cx_api::*,
    }
};

#[derive(Clone, Debug, Default)]
pub struct Signal(Arc<AtomicBool>);

pub (crate) static UI_SIGNAL: AtomicBool = AtomicBool::new(false);

impl Signal {
    pub fn set_ui_signal() {
        UI_SIGNAL.store(true, Ordering::SeqCst)
    }
    
    pub (crate) fn check_and_clear_ui_signal() -> bool {
        UI_SIGNAL.swap(false, Ordering::SeqCst)
    }
    
    pub fn new() -> Self {
        Self (Arc::new(AtomicBool::new(false)))
    }
    
    pub fn check_and_clear(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
    
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
        Self::set_ui_signal();
    }
}

pub struct ToUIReceiver<T> {
    sender: Sender<T>,
    pub receiver: Receiver<T>,
}

pub struct ToUISender<T> {
    sender: Sender<T>,
}

impl<T> Clone for ToUISender<T> {
    fn clone(&self) -> Self {
        Self {sender: self.sender.clone()}
    }
}

unsafe impl<T: Send> Send for ToUISender<T> {}

impl<T> Default for ToUIReceiver<T> {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
        }
    }
}

impl<T> ToUIReceiver<T> {
    pub fn sender(&self) -> ToUISender<T> {
        ToUISender {
            sender: self.sender.clone(),
        }
    }
    
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv()
    }
    
    pub fn try_recv_flush(&self) -> Result<T, TryRecvError> {
        let mut store_last = None;
        loop {
            match self.receiver.try_recv() {
                Ok(last) => {
                    store_last = Some(last);
                },
                Err(TryRecvError::Empty) => {
                    if let Some(last) = store_last {
                        return Ok(last)
                    }
                    else {
                        return Err(TryRecvError::Empty)
                    }
                },
                Err(TryRecvError::Disconnected) => {
                    return Err(TryRecvError::Disconnected)
                }
            }
        }
    }
}

impl<T> ToUISender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T >> {
        let res = self.sender.send(t);
        Signal::set_ui_signal();
        res
    }
}

pub struct FromUIReceiver<T> {
    receiver: Receiver<T>,
}

pub struct FromUISender<T> {
    receiver: Option<Receiver<T >>,
    sender: Sender<T>,
}

unsafe impl<T: Send> Send for FromUIReceiver<T> {}

impl<T> Default for FromUISender<T> {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver: Some(receiver),
        }
    }
}

impl<T> FromUISender<T> {
    pub fn new_channel(&mut self) {
        let (sender, receiver) = channel();
        self.sender = sender;
        self.receiver = Some(receiver)
    }
    
    pub fn send(&self, t: T) -> Result<(), SendError<T >> {
        self.sender.send(t)
    }
    
    pub fn sender(&self) -> FromUISender<T> {
        FromUISender {
            sender: self.sender.clone(),
            receiver: None
        }
    }
    
    pub fn receiver(&mut self) -> FromUIReceiver<T> {
        FromUIReceiver {
            receiver: self.receiver.take().unwrap()
        }
    }
}

impl<T> FromUIReceiver<T> {
    pub fn recv(&self) -> Result<T, RecvError> {
        self.receiver.recv()
    }
    
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv()
    }
}

pub struct RevThreadPool {
    tasks: Arc<Mutex<Vec<Box<dyn FnOnce() + Send + 'static >> >>,
}

impl RevThreadPool {
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let tasks: Arc<Mutex<Vec<Box<dyn FnOnce() + Send + 'static >> >> = Default::default();
        
        for _ in 0..num_threads {
            let tasks = tasks.clone();
            cx.spawn_thread(move || loop {
                let task = if let Ok(mut tasks) = tasks.lock() {
                    tasks.pop()
                }
                else {
                    panic!();
                };
                if let Some(task) = task {
                    task();
                }
            })
        }
        Self {
            tasks
        }
    }
    
    pub fn execute<F>(&self, task: F) where F: FnOnce() + Send + 'static {
        self.tasks.lock().unwrap().insert(0, Box::new(task));
    }
    
    pub fn execute_rev<F>(&self, task: F) where F: FnOnce() + Send + 'static {
        self.tasks.lock().unwrap().push(Box::new(task));
    }
}

pub struct TagThreadPool<T: Clone + Send + 'static + PartialEq> {
    tasks: Arc<Mutex<Vec<(T, Box<dyn FnOnce(T) + Send + 'static >) >> >,
}

impl<T> TagThreadPool<T>where T: Clone + Send + 'static + PartialEq {
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let tasks: Arc<Mutex<Vec<(T, Box<dyn FnOnce(T) + Send + 'static >) >> > = Default::default();
        
        for _ in 0..num_threads {
            let tasks = tasks.clone();
            cx.spawn_thread(move || loop {
                let task = if let Ok(mut tasks) = tasks.lock() {
                    tasks.pop()
                }
                else {
                    panic!();
                };
                if let Some((tag, task)) = task {
                    task(tag);
                }
            })
        }
        Self {
            tasks
        }
    }
    
    pub fn execute<F>(&self, tag: T, task: F) where F: FnOnce(T) + Send + 'static {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.retain( | v | v.0 != tag);
            tasks.insert(0, (tag, Box::new(task)));
        }
    }
    
    pub fn execute_rev<F>(&self, tag: T, task: F) where F: FnOnce(T) + Send + 'static {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.retain( | v | v.0 != tag);
            tasks.push((tag, Box::new(task)));
        }
    }
}



pub struct MessageThreadPool<T: Clone + Send + 'static> {
    sender: Sender<Box<dyn FnOnce(Option<T>) + Send + 'static >>,
    msg_senders: Vec<Sender<T >>
}

impl<T> MessageThreadPool<T> where T: Clone + Send + 'static {
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let (sender, receiver) = channel::<Box<dyn FnOnce(Option<T>) + Send + 'static >> ();
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
                        Err(_) => return
                    }
                }
                else {
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
            msg_senders
        }
    }
    
    pub fn send_msg(&self, msg: T) {
        for sender in &self.msg_senders {
            sender.send(msg.clone()).unwrap();
        }
    }
    
    pub fn execute<F>(&self, task: F) where F: FnOnce(Option<T>) + Send + 'static {
        self.sender.send(Box::new(task)).unwrap();
    }
}
