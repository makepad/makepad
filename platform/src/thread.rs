use {
    std::cell::RefCell,
    std::sync::{
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
        makepad_live_id::LiveId,
        cx::Cx,
        cx_api::*,
        event::{Signal, Event}
    }
};

pub struct ToUIReceiver<T> {
    sender: Sender<T>,
    pub receiver: Receiver<T>,
    signal: Signal
}

pub struct ToUISender<T> {
    sender: Sender<T>,
    signal: Signal
}

impl<T> Clone for ToUISender<T> {
    fn clone(&self) -> Self {
        Self {sender: self.sender.clone(), signal: self.signal.clone()}
    }
}

unsafe impl<T> Send for ToUISender<T> {}

impl<T> Default for ToUIReceiver<T> {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            signal: LiveId::unique().into()
        }
    }
}

impl<T> ToUIReceiver<T> {
    pub fn sender(&self) -> ToUISender<T> {
        ToUISender {
            sender: self.sender.clone(),
            signal: self.signal.clone()
        }
    }
    
    
    pub fn try_recv(&self, event: &Event) -> Result<T, TryRecvError> {
        if let Event::Signal(se) = event {
            if se.signals.get(&self.signal).is_some() {
                return self.receiver.try_recv()
            }
        }
        Err(TryRecvError::Empty)
    }
}

impl<T> ToUISender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T >> {
        let res = self.sender.send(t);
        Cx::post_signal(self.signal);
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

unsafe impl<T> Send for FromUIReceiver<T> {}

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

pub struct ThreadPool {
    task_queue: Arc<Mutex<RefCell<Vec<Box<dyn FnOnce() + Send + 'static>>>>>,
    sender: Arc<RefCell<Option<Sender<()>>>>,
}

impl Drop for ThreadPool{
    fn drop(&mut self){
        self.sender.borrow_mut().take();
    }
}

impl ThreadPool {
    pub fn new(cx: &mut Cx, num_threads: usize) -> Self {
        let (sender, receiver) = channel::<()>();
        let task_queue = Arc::new(Mutex::new(RefCell::new(Vec::<Box<dyn FnOnce() + Send + 'static>>::new())));
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..num_threads {
            let receiver = receiver.clone();
            let task_queue = task_queue.clone();
            cx.spawn_thread(move || loop {
                let task = if let Ok(receiver) = receiver.lock() {
                    match receiver.recv() {
                        Err(_) => return, // thread closed
                        Ok(_)=>{
                            if let Some(task) = task_queue.lock().unwrap().borrow_mut().pop(){
                                task
                            }
                            else{ // the task queue got cleared
                                continue;
                            }
                        }
                    }
                }
                else {
                    panic!();
                };
                task();
            })
        }
        let sender = Arc::new(RefCell::new(Some(sender)));
        cx.thread_pool_senders.push(sender.clone());
        Self{
            task_queue,
            sender
        }
    }
    
    pub fn clear_queue(&self){
        self.task_queue.lock().unwrap().borrow_mut().clear();
    }
    
    pub fn execute<F>(&self, task: F) where F: FnOnce() + Send + 'static {
        let sender = self.sender.borrow_mut();
        if let Some(tps) = sender.as_ref(){
            self.task_queue.lock().unwrap().borrow_mut().insert(0,Box::new(task));
            tps.send(()).unwrap();
        }
    }

    pub fn execute_first<F>(&self, task: F) where F: FnOnce() + Send + 'static {
        let sender = self.sender.borrow_mut();
        if let Some(tps) = sender.as_ref(){
            self.task_queue.lock().unwrap().borrow_mut().push(Box::new(task));
            tps.send(()).unwrap();
        }
    }
}
