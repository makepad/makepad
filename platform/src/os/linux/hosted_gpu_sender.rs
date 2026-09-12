//! One persistent descriptor writer per hosted app. Complete batches retain
//! their ordering; a full queue returns ownership for the UI to retry.
use crate::{
    os::shared_framebuf::{aux_chan, ExportedHostSwapchain, SharedSwapchain},
    thread::{to_ui_bounded, TaskHandle, ThreadOptions, ThreadSpawner, ToUIReceiver},
};
use std::{
    num::NonZeroUsize,
    sync::mpsc::{sync_channel, SyncSender, TrySendError},
};

pub struct SentSwapchain {
    pub tag: u64,
    pub swapchain: SharedSwapchain,
}

pub struct LinuxSwapchainSender {
    requests: SyncSender<(u64, ExportedHostSwapchain)>,
    replies: ToUIReceiver<Result<SentSwapchain, String>>,
    interrupt: aux_chan::HostEndpoint,
    failed: Option<String>,
    worker: TaskHandle<()>,
}

impl LinuxSwapchainSender {
    pub fn start(
        spawner: &ThreadSpawner,
        endpoint: aux_chan::HostEndpoint,
    ) -> Result<Self, String> {
        let interrupt = endpoint.try_clone().map_err(|e| e.to_string())?;
        let (requests, receive) = sync_channel::<(u64, ExportedHostSwapchain)>(2);
        let (publish, replies) = to_ui_bounded(NonZeroUsize::new(2).unwrap());
        let worker = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("gpu-aux-outbox".into()),
                    ..Default::default()
                },
                move || {
                    while let Ok((tag, export)) = receive.recv() {
                        let result = export
                            .send(&endpoint)
                            .map(|swapchain| SentSwapchain { tag, swapchain })
                            .map_err(|error| error.to_string());
                        let failed = result.is_err();
                        if publish.send(result).is_err() || failed {
                            break;
                        }
                    }
                },
            )
            .map_err(|e| format!("start GPU descriptor writer: {e:?}"))?;
        Ok(Self {
            requests,
            replies,
            interrupt,
            failed: None,
            worker,
        })
    }

    /// On pressure or disconnection the caller retains exactly this batch;
    /// never export another one or discard its handles as a retry strategy.
    pub fn try_send(
        &mut self,
        tag: u64,
        batch: ExportedHostSwapchain,
    ) -> Result<(), ExportedHostSwapchain> {
        if self.failed.is_some() {
            return Err(batch);
        }
        match self.requests.try_send((tag, batch)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full((_, batch))) => Err(batch),
            Err(TrySendError::Disconnected((_, batch))) => {
                self.failed = Some("GPU descriptor writer disconnected".into());
                Err(batch)
            }
        }
    }

    pub fn poll(&mut self) -> Vec<SentSwapchain> {
        let mut completed = Vec::new();
        while let Ok(result) = self.replies.try_recv() {
            match result {
                Ok(batch) => completed.push(batch),
                Err(error) => self.failed = Some(error),
            }
        }
        let _ = self.worker.try_take();
        completed
    }

    pub fn error(&self) -> Option<&str> {
        self.failed.as_deref()
    }
}

impl Drop for LinuxSwapchainSender {
    fn drop(&mut self) {
        let _ = self.interrupt.shutdown();
    }
}
