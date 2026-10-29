//! One persistent descriptor writer per hosted app. Complete batches retain
//! their ordering; a full queue returns ownership for the UI to retry.
use crate::{
    os::shared_framebuf::ExportedHostSwapchain,
    thread::{to_ui_bounded, TaskHandle, ThreadOptions, ThreadSpawner, ToUIReceiver},
};
use crate::makepad_network::mpsc::{sync_channel, SyncSender, TrySendError};
use std::{
    num::NonZeroUsize,
};

pub struct SentSwapchain {
    pub tag: u64,
    pub message: makepad_studio_protocol::StudioToApp,
}

pub struct NativeDescriptors {
    pub event: makepad_studio_protocol::NativeAppEvent,
    pub fds: Vec<(makepad_studio_protocol::PresentableImageId, std::os::fd::OwnedFd)>,
}

enum Export { Swapchain(ExportedHostSwapchain), Native(NativeDescriptors) }

pub struct LinuxSwapchainSender {
    requests: SyncSender<(u64, Export)>,
    replies: ToUIReceiver<Result<SentSwapchain, String>>,
    interrupt: makepad_hal::hosted_ipc::HostEndpoint,
    failed: Option<String>,
    worker: TaskHandle<()>,
    incoming: ToUIReceiver<(makepad_studio_protocol::PresentableImageId, std::os::fd::OwnedFd)>,
    reader: TaskHandle<()>,
}

impl LinuxSwapchainSender {
    pub fn start(
        spawner: &ThreadSpawner,
        endpoint: makepad_hal::hosted_ipc::HostEndpoint,
    ) -> Result<Self, String> {
        let interrupt = endpoint.try_clone().map_err(|e| e.to_string())?;
        let (requests, receive) = sync_channel::<(u64, Export)>(2);
        let (publish, replies) = to_ui_bounded(NonZeroUsize::new(2).unwrap());
        let read_endpoint = endpoint.try_clone().map_err(|e| e.to_string())?;
        let (in_tx, incoming) = to_ui_bounded(NonZeroUsize::new(2).unwrap());
        let reader = spawner.spawn_worker(ThreadOptions { name: Some("gpu-aux-inbox".into()), ..Default::default() }, move || {
            while let Ok(fd) = read_endpoint.recv() { if in_tx.send(fd).is_err() { break; } }
        }).map_err(|e| format!("start GPU descriptor reader: {e:?}"))?;
        let worker = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("gpu-aux-outbox".into()),
                    ..Default::default()
                },
                move || {
                    while let Ok((tag, export)) = receive.recv() {
                        let result = match export {
                            Export::Swapchain(export) => export.send(&endpoint).map(makepad_studio_protocol::StudioToApp::Swapchain),
                            Export::Native(export) => (|| {
                                for fd in export.fds { endpoint.send(fd)?; }
                                Ok(makepad_studio_protocol::StudioToApp::NativeApp(export.event))
                            })(),
                        }.map(|message| SentSwapchain { tag, message }).map_err(|error| error.to_string());
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
            worker, incoming, reader,
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
        match self.requests.try_send((tag, Export::Swapchain(batch))) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full((_, Export::Swapchain(batch)))) => Err(batch),
            Err(TrySendError::Disconnected((_, Export::Swapchain(batch)))) => {
                self.failed = Some("GPU descriptor writer disconnected".into());
                Err(batch)
            }
            _ => unreachable!(),
        }
    }

    pub fn try_send_native(&mut self, batch: NativeDescriptors) -> Result<(), NativeDescriptors> {
        if self.failed.is_some() { return Err(batch); }
        match self.requests.try_send((u64::MAX, Export::Native(batch))) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full((_, Export::Native(batch)))) => Err(batch),
            Err(TrySendError::Disconnected((_, Export::Native(batch)))) => {
                self.failed = Some("GPU descriptor writer disconnected".into()); Err(batch)
            }
            _ => unreachable!(),
        }
    }

    pub fn receive(&mut self) -> Vec<(makepad_studio_protocol::PresentableImageId, std::os::fd::OwnedFd)> {
        let _ = self.reader.try_take();
        let mut out = Vec::new();
        while let Ok(fd) = self.incoming.try_recv() { out.push(fd); }
        out
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
