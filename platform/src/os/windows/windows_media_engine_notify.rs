//! IMFMediaEngineNotify callback via Makepad's `implement_com!` helper.

use {
    std::sync::Mutex,
    windows::{
        core::ComObject,
        Win32::Media::MediaFoundation::{IMFMediaEngineNotify, IMFMediaEngineNotify_Impl},
    },
};

pub(crate) struct MediaEngineNotifyState {
    pub events: Mutex<Vec<u32>>,
}

crate::implement_com! {
    for_struct: MediaEngineNotifyState,
    identity: IMFMediaEngineNotify,
    wrapper_struct: MediaEngineNotifyState_Impl,
    interface_count: 1,
    interfaces: {
        0: IMFMediaEngineNotify
    }
}

impl IMFMediaEngineNotify_Impl for MediaEngineNotifyState_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, _param2: u32) -> windows::core::Result<()> {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
        Ok(())
    }
}

pub(crate) fn new_media_engine_notify() -> ComObject<MediaEngineNotifyState> {
    ComObject::new(MediaEngineNotifyState {
        events: Mutex::new(Vec::new()),
    })
}

pub(crate) fn drain_notify_events(state: &MediaEngineNotifyState) -> Vec<u32> {
    state
        .events
        .lock()
        .map(|mut events| std::mem::take(&mut *events))
        .unwrap_or_default()
}
