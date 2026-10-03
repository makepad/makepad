//! MediaEngine callback through a metadata-generated COM vtable.

use {
    std::sync::{Arc, Mutex},
    windows::Win32::Media::MediaFoundation::{IMFMediaEngineNotify, IMFMediaEngineNotifyImpl},
};

pub(crate) struct MediaEngineNotifyState {
    pub events: Mutex<Vec<u32>>,
}

struct SharedMediaEngineNotify(Arc<MediaEngineNotifyState>);
impl IMFMediaEngineNotifyImpl for SharedMediaEngineNotify {
    fn EventNotify(
        &self,
        event: u32,
        _param1: usize,
        _param2: u32,
    ) -> Result<(), windows::core::HRESULT> {
        if let Ok(mut events) = self.0.events.lock() {
            events.push(event);
        }
        Ok(())
    }
}

pub(crate) fn new_media_engine_notify() -> (IMFMediaEngineNotify, Arc<MediaEngineNotifyState>) {
    let state = Arc::new(MediaEngineNotifyState {
        events: Mutex::new(Vec::new()),
    });
    (
        IMFMediaEngineNotify::implement(Box::new(SharedMediaEngineNotify(state.clone()))),
        state,
    )
}

pub(crate) fn drain_notify_events(state: &MediaEngineNotifyState) -> Vec<u32> {
    state
        .events
        .lock()
        .map(|mut events| std::mem::take(&mut *events))
        .unwrap_or_default()
}
