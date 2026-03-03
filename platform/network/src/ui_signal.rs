use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{channel, Receiver, RecvError, SendError, Sender, TryRecvError},
    Arc,
};

#[derive(Clone, Debug, Default)]
pub struct SignalToUI(Arc<AtomicBool>);

static UI_SIGNAL: AtomicBool = AtomicBool::new(false);
static ACTION_SIGNAL: AtomicBool = AtomicBool::new(false);

impl SignalToUI {
    pub fn set_ui_signal() {
        UI_SIGNAL.store(true, Ordering::SeqCst)
    }

    pub fn set_action_signal() {
        ACTION_SIGNAL.store(true, Ordering::SeqCst)
    }

    pub fn check_and_clear_ui_signal() -> bool {
        UI_SIGNAL.swap(false, Ordering::SeqCst)
    }

    pub fn check_and_clear_action_signal() -> bool {
        ACTION_SIGNAL.swap(false, Ordering::SeqCst)
    }

    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn check_and_clear(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }

    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
        Self::set_ui_signal();
    }
}

#[derive(Clone, Debug, Default)]
pub struct SignalFromUI(Arc<AtomicBool>);

impl SignalFromUI {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn check_and_clear(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }

    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub struct ToUIReceiver<T> {
    sender: Sender<T>,
    pub receiver: Receiver<T>,
}

#[derive(Debug)]
pub struct ToUISender<T> {
    sender: Sender<T>,
}

impl<T> Clone for ToUISender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

unsafe impl<T: Send> Send for ToUISender<T> {}

impl<T> Default for ToUIReceiver<T> {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self { sender, receiver }
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
                }
                Err(TryRecvError::Empty) => {
                    if let Some(last) = store_last {
                        return Ok(last);
                    } else {
                        return Err(TryRecvError::Empty);
                    }
                }
                Err(TryRecvError::Disconnected) => return Err(TryRecvError::Disconnected),
            }
        }
    }
}

impl<T> ToUISender<T> {
    pub fn from_sender(sender: Sender<T>) -> Self {
        Self { sender }
    }

    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        let res = self.sender.send(t);
        SignalToUI::set_ui_signal();
        res
    }
}

pub struct FromUIReceiver<T> {
    receiver: Receiver<T>,
}

pub struct FromUISender<T> {
    receiver: Option<Receiver<T>>,
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

    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        self.sender.send(t)
    }

    pub fn sender(&self) -> FromUISender<T> {
        FromUISender {
            sender: self.sender.clone(),
            receiver: None,
        }
    }

    pub fn receiver(&mut self) -> FromUIReceiver<T> {
        FromUIReceiver {
            receiver: self.receiver.take().unwrap(),
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

impl<T> std::ops::Deref for FromUIReceiver<T> {
    type Target = Receiver<T>;
    fn deref(&self) -> &Receiver<T> {
        &self.receiver
    }
}
