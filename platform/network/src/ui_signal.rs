use std::{
    fmt,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{
            channel, sync_channel, Receiver, RecvError, SendError, Sender, SyncSender,
            TryRecvError, TrySendError,
        },
        Arc, Mutex,
    },
};

type UiWake = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct UiWaker {
    wake: UiWake,
}

impl fmt::Debug for UiWaker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UiWaker").finish_non_exhaustive()
    }
}

impl UiWaker {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self { wake: Arc::new(wake) }
    }

    pub fn wake(&self) {
        (self.wake)();
    }
}

static UI_WAKER: Mutex<Option<UiWaker>> = Mutex::new(None);

/// Installs the process event-loop wake callback. Platform initialization may
/// replace it when ownership moves to a new event loop.
pub fn install_ui_waker(waker: Option<UiWaker>) {
    if let Ok(mut slot) = UI_WAKER.lock() {
        *slot = waker;
    }
}

/// Wake the event loop and nothing else, for a worker whose result the next
/// beat picks up by itself (a render worker whose pass is already dirty).
pub fn wake_ui_loop() {
    let waker = UI_WAKER.lock().ok().and_then(|slot| slot.clone());
    if let Some(waker) = waker {
        waker.wake();
    }
}

#[derive(Clone, Debug, Default)]
pub struct SignalToUI(Arc<AtomicBool>);

static UI_SIGNAL: AtomicBool = AtomicBool::new(false);
static INTERNAL_SIGNAL: AtomicBool = AtomicBool::new(false);
static ACTION_SIGNAL: AtomicBool = AtomicBool::new(false);

impl SignalToUI {
    /// An app-facing channel has something for the UI thread. The next beat
    /// dispatches `Event::Signal` to the whole widget tree, so raise this only
    /// for data an app polls on that event: a `ToUISender`, a task result, a
    /// texture readback. Makepad's own queues raise `set_internal_signal`.
    pub fn set_ui_signal() {
        if !UI_SIGNAL.swap(true, Ordering::AcqRel) {
            wake_ui_loop();
        }
    }

    /// One of makepad's own queues has something for the UI thread: media
    /// device changes, the script pump, a termination request. The next beat
    /// services those without waking every widget with an `Event::Signal`.
    pub fn set_internal_signal() {
        if !INTERNAL_SIGNAL.swap(true, Ordering::AcqRel) {
            wake_ui_loop();
        }
    }

    pub fn set_action_signal() {
        if !ACTION_SIGNAL.swap(true, Ordering::AcqRel) {
            wake_ui_loop();
        }
    }

    /// Whether any of the UI thread's signals is raised (without clearing).
    pub fn any_pending() -> bool {
        UI_SIGNAL.load(Ordering::Acquire)
            || INTERNAL_SIGNAL.load(Ordering::Acquire)
            || ACTION_SIGNAL.load(Ordering::Acquire)
    }

    pub fn check_and_clear_ui_signal() -> bool {
        UI_SIGNAL.swap(false, Ordering::AcqRel)
    }

    pub fn check_and_clear_internal_signal() -> bool {
        INTERNAL_SIGNAL.swap(false, Ordering::AcqRel)
    }

    pub fn check_and_clear_action_signal() -> bool {
        ACTION_SIGNAL.swap(false, Ordering::AcqRel)
    }

    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn check_and_clear(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }

    pub fn set(&self) {
        self.0.store(true, Ordering::Release);
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
        self.0.swap(false, Ordering::AcqRel)
    }

    pub fn set(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
enum ChannelSender<T> {
    Unbounded(Sender<T>),
    Bounded(SyncSender<T>),
}

impl<T> Clone for ChannelSender<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Unbounded(sender) => Self::Unbounded(sender.clone()),
            Self::Bounded(sender) => Self::Bounded(sender.clone()),
        }
    }
}

#[derive(Debug)]
pub struct ToUIReceiver<T> {
    sender: ChannelSender<T>,
    pub receiver: Receiver<T>,
}

#[derive(Debug)]
pub struct ToUISender<T> {
    sender: ChannelSender<T>,
}

impl<T> Clone for ToUISender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<T> Default for ToUIReceiver<T> {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender: ChannelSender::Unbounded(sender),
            receiver,
        }
    }
}

pub fn to_ui_bounded<T>(capacity: NonZeroUsize) -> (ToUISender<T>, ToUIReceiver<T>) {
    let (sender, receiver) = sync_channel(capacity.get());
    let sender = ChannelSender::Bounded(sender);
    (
        ToUISender {
            sender: sender.clone(),
        },
        ToUIReceiver { sender, receiver },
    )
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
                Ok(last) => store_last = Some(last),
                Err(TryRecvError::Empty) => return store_last.ok_or(TryRecvError::Empty),
                Err(TryRecvError::Disconnected) => return Err(TryRecvError::Disconnected),
            }
        }
    }
}

impl<T> ToUISender<T> {
    pub fn from_sender(sender: Sender<T>) -> Self {
        Self {
            sender: ChannelSender::Unbounded(sender),
        }
    }

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let result = match &self.sender {
            ChannelSender::Unbounded(sender) => sender.send(value),
            ChannelSender::Bounded(sender) => sender.send(value),
        };
        if result.is_ok() {
            SignalToUI::set_ui_signal();
        }
        result
    }

    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let result = match &self.sender {
            ChannelSender::Unbounded(sender) => sender.send(value).map_err(|error| {
                TrySendError::Disconnected(error.0)
            }),
            ChannelSender::Bounded(sender) => sender.try_send(value),
        };
        if result.is_ok() {
            SignalToUI::set_ui_signal();
        }
        result
    }
}

pub struct ToUIOneshotSender<T> {
    sender: Option<SyncSender<T>>,
}

pub struct ToUIOneshotReceiver<T> {
    receiver: Receiver<T>,
}

pub fn to_ui_oneshot<T>() -> (ToUIOneshotSender<T>, ToUIOneshotReceiver<T>) {
    let (sender, receiver) = sync_channel(1);
    (
        ToUIOneshotSender {
            sender: Some(sender),
        },
        ToUIOneshotReceiver { receiver },
    )
}

impl<T> ToUIOneshotSender<T> {
    pub fn send(mut self, value: T) -> Result<(), SendError<T>> {
        let result = self.sender.take().unwrap().send(value);
        if result.is_ok() {
            SignalToUI::set_ui_signal();
        }
        result
    }
}

impl<T> ToUIOneshotReceiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv()
    }
}

pub struct FromUIReceiver<T> {
    receiver: Receiver<T>,
}

pub struct FromUISender<T> {
    receiver: Option<Receiver<T>>,
    sender: Sender<T>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiverAlreadyTaken;

impl fmt::Display for ReceiverAlreadyTaken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FromUI receiver was already taken")
    }
}

impl std::error::Error for ReceiverAlreadyTaken {}

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

    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        self.sender.send(value)
    }

    pub fn sender(&self) -> FromUISender<T> {
        FromUISender {
            sender: self.sender.clone(),
            receiver: None,
        }
    }

    pub fn receiver(&mut self) -> Result<FromUIReceiver<T>, ReceiverAlreadyTaken> {
        self.receiver
            .take()
            .map(|receiver| FromUIReceiver { receiver })
            .ok_or(ReceiverAlreadyTaken)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_and_ui_signals_are_independent() {
        SignalToUI::check_and_clear_ui_signal();
        SignalToUI::check_and_clear_internal_signal();
        SignalToUI::set_internal_signal();
        assert!(!SignalToUI::check_and_clear_ui_signal());
        assert!(SignalToUI::check_and_clear_internal_signal());
        assert!(!SignalToUI::check_and_clear_internal_signal());
        SignalToUI::set_ui_signal();
        assert!(!SignalToUI::check_and_clear_internal_signal());
        assert!(SignalToUI::check_and_clear_ui_signal());
    }

    #[test]
    fn oneshot_delivers_once() {
        let (sender, receiver) = to_ui_oneshot();
        sender.send(12).unwrap();
        assert_eq!(receiver.try_recv().unwrap(), 12);
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn from_ui_receiver_is_fallible() {
        let mut sender = FromUISender::<()>::default();
        assert!(sender.receiver().is_ok());
        assert!(sender.receiver().is_err());
    }
}
