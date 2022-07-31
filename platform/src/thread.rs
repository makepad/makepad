use {
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

pub struct ThreadPool<T: Clone + Send + 'static> {
    sender: Sender<Box<dyn FnOnce(Option<T>) + Send + 'static>>,
    msg_senders: Vec<Sender<T>>
}

impl<T> ThreadPool<T> where T : Clone + Send + 'static{
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
                        Err(_) => return
                    }
                }
                else {
                    panic!();
                };
                let mut msg_out = None;
                while let Ok(msg) = msg_recv.try_recv(){
                    msg_out = Some(msg);
                }
                task(msg_out);
            })
        }
        Self{
            sender,
            msg_senders
        }
    }
    
    pub fn send_msg(&self, msg: T){
        for sender in &self.msg_senders{
            sender.send(msg.clone()).unwrap();
        }
    }
    
    pub fn execute<F>(&self, task: F) where F: FnOnce(Option<T>) + Send + 'static {
        self.sender.send(Box::new(task)).unwrap();
    }
}
