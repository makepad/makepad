use crate::mpsc::{
    channel, sync_channel, Receiver, RecvError, SendError, Sender, SyncSender, TryRecvError,
    TrySendError,
};
use std::{
    fmt,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
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

/// `MAKEPAD_FRAME_TRACE=1` or the `frame_sources` trace topic: who raised the app
/// signal, by source line, for the platform's frame-source report (`take_signal_sources`).
fn signal_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_FRAME_TRACE").is_some_and(|v| v != "0" && !v.is_empty()))
        || makepad_error_log::trace_enabled("frame_sources")
}

type SignalSources = std::collections::HashMap<&'static std::panic::Location<'static>, u32>;
static SIGNAL_SOURCES: Mutex<Option<SignalSources>> = Mutex::new(None);

#[track_caller]
fn note_signal_source() {
    if !signal_trace_on() { return; }
    let at = std::panic::Location::caller();
    if let Ok(mut sources) = SIGNAL_SOURCES.lock() {
        *sources.get_or_insert_with(Default::default).entry(at).or_default() += 1;
    }
}

/// The app-signal raises counted since the last call (`MAKEPAD_FRAME_TRACE=1` only).
pub fn take_signal_sources() -> Vec<(&'static std::panic::Location<'static>, u32)> {
    SIGNAL_SOURCES.lock().ok().and_then(|mut sources| sources.take()).map(|m| m.into_iter().collect()).unwrap_or_default()
}

/// The UI thread's signals (`UI`, `INTERNAL`, `ACTION`, `HOOKS`) and when each was raised. The process
/// has one (`SIGNALS`), which every sender to the UI raises.
struct UiSignals {
    raised: [AtomicBool; 4],
    #[cfg(not(target_arch = "wasm32"))]
    raised_ns: [std::sync::atomic::AtomicU64; 4],
}

const UI: usize = 0;
const INTERNAL: usize = 1;
const ACTION: usize = 2;
const HOOKS: usize = 3;

static SIGNALS: UiSignals = UiSignals::new();

impl UiSignals {
    const fn new() -> Self {
        Self {
            raised: [const { AtomicBool::new(false) }; 4],
            #[cfg(not(target_arch = "wasm32"))]
            raised_ns: [const { std::sync::atomic::AtomicU64::new(0) }; 4],
        }
    }

    /// Raises one; true when it was down (the event loop needs a wake).
    fn raise(&self, which: usize) -> bool {
        if self.raised[which].swap(true, Ordering::AcqRel) {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.raised_ns[which].store(signal_clock_ns(), Ordering::Relaxed);
        true
    }

    fn check_and_clear(&self, which: usize) -> bool {
        self.raised[which].swap(false, Ordering::AcqRel)
    }

    fn any_pending(&self) -> bool {
        self.raised.iter().any(|raised| raised.load(Ordering::Acquire))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn ages_ms(&self) -> [Option<u64>; 4] {
        let now = signal_clock_ns();
        std::array::from_fn(|i| {
            self.raised[i]
                .load(Ordering::Acquire)
                .then(|| now.saturating_sub(self.raised_ns[i].load(Ordering::Relaxed)) / 1_000_000)
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn signal_clock_ns() -> u64 {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    BASE.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
}

impl SignalToUI {
    /// An app-facing channel has something for the UI thread. The next beat
    /// dispatches `Event::Signal` to the whole widget tree, so raise this only
    /// for data an app polls on that event: a `ToUISender`, a task result, a
    /// texture readback. Makepad's own queues raise `set_internal_signal`.
    #[track_caller]
    pub fn set_ui_signal() {
        note_signal_source();
        if SIGNALS.raise(UI) {
            wake_ui_loop();
        }
    }

    /// One of makepad's own queues has something for the UI thread: media
    /// device changes, the script pump, a termination request. The next beat
    /// services those without waking every widget with an `Event::Signal`.
    pub fn set_internal_signal() {
        if SIGNALS.raise(INTERNAL) {
            wake_ui_loop();
        }
    }

    pub fn set_action_signal() {
        if SIGNALS.raise(ACTION) {
            wake_ui_loop();
        }
    }

    /// Whether any of the UI thread's signals is raised (without clearing).
    pub fn any_pending() -> bool {
        SIGNALS.any_pending()
    }

    /// Lets an inspector see a wake the event loop has slept through.
    pub fn pending_signal_ages_ms() -> [Option<u64>; 4] {
        #[cfg(not(target_arch = "wasm32"))]
        {
            SIGNALS.ages_ms()
        }
        #[cfg(target_arch = "wasm32")]
        { [None; 4] }
    }

    pub fn check_and_clear_ui_signal() -> bool {
        SIGNALS.check_and_clear(UI)
    }

    pub fn check_and_clear_internal_signal() -> bool {
        SIGNALS.check_and_clear(INTERNAL)
    }

    pub fn check_and_clear_action_signal() -> bool {
        SIGNALS.check_and_clear(ACTION)
    }

    /// A signal hook's own signal was raised (`set_for_hooks`): the loop runs the hooks.
    pub fn check_and_clear_hook_signal() -> bool {
        SIGNALS.check_and_clear(HOOKS)
    }

    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn check_and_clear(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }

    #[track_caller]
    pub fn set(&self) {
        self.0.store(true, Ordering::Release);
        Self::set_ui_signal();
    }

    /// Raises this signal for a signal hook that checks it (`Cx::signal_hooks`, a subsystem's
    /// own service on the UI thread): the next beat runs the hooks, and no widget hears an
    /// `Event::Signal`. For wakes no app polls on that event, such as a stream sink's picture
    /// encoded or its room free again, a few times each frame.
    #[track_caller]
    pub fn set_for_hooks(&self) {
        self.0.store(true, Ordering::Release);
        note_signal_source();
        if SIGNALS.raise(HOOKS) {
            wake_ui_loop();
        }
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

    #[track_caller]
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

    #[track_caller]
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
    #[track_caller]
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
    fn the_ui_internal_and_hook_signals_are_independent() {
        // Signals of its own: the process's are raised by every test that sends to the UI.
        let signals = UiSignals::new();
        assert!(signals.raise(INTERNAL));
        assert!(!signals.check_and_clear(UI));
        assert!(signals.check_and_clear(INTERNAL));
        assert!(!signals.check_and_clear(INTERNAL));
        assert!(signals.raise(UI));
        assert!(!signals.raise(UI), "a raised signal wakes the loop once");
        assert!(!signals.check_and_clear(INTERNAL));
        assert!(signals.check_and_clear(UI));
        assert!(signals.raise(HOOKS));
        assert!(!signals.check_and_clear(UI), "a hook signal is no UI signal: the app hears no Event::Signal");
        assert!(signals.check_and_clear(HOOKS));
        assert!(!signals.any_pending());
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
