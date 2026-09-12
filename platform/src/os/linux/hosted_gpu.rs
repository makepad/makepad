//! Hosted Vulkan bootstrap runs descriptor I/O on one persistent worker.
//! UI requests and completed batches are bounded; the UI never receives an
//! auxiliary descriptor synchronously. This carries handles, not pixel data.
use crate::{
    os::shared_framebuf::{
        aux_chan, shared_presentable_image_recv_fds_from_aux_chan, LinuxPresentableImage,
    },
    thread::{to_ui_bounded, TaskHandle, ThreadOptions, ThreadSpawner, ToUIReceiver},
};
use makepad_studio_protocol::SharedSwapchain;
use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, SyncSender, TrySendError},
        Arc,
    },
    time::Duration,
};

const QUEUED_REQUESTS: usize = 32;

#[derive(Clone, Copy)]
pub(crate) struct SwapchainRequest {
    pub transition: Option<(u64, u64)>,
    pub renderer_generation: u64,
    pub window_id: Option<crate::window::WindowId>,
    pub swapchain: SharedSwapchain,
}

pub(crate) struct LoadedSwapchain {
    pub request: SwapchainRequest,
    pub images: Vec<LinuxPresentableImage>,
}

// Closing an unconsumed Connected message also interrupts its worker. Merely
// dropping a cloned file descriptor would leave the other recvmsg blocked.
struct Interrupt(aux_chan::ClientEndpoint);
impl Drop for Interrupt {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

enum Reply {
    Connected(Interrupt),
    Loaded(LoadedSwapchain),
    Failed(String),
}

pub(crate) struct GpuInbox {
    requests: SyncSender<SwapchainRequest>,
    replies: Option<ToUIReceiver<Reply>>,
    pending: VecDeque<SwapchainRequest>,
    interrupt: Option<Interrupt>,
    stop: Arc<AtomicBool>,
    failed: Option<String>,
    reported: bool,
    _worker: TaskHandle<()>,
}

impl GpuInbox {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        let (requests, receive) = sync_channel::<SwapchainRequest>(4);
        let (publish, replies) = to_ui_bounded(NonZeroUsize::new(4).unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("gpu-aux-inbox".into()),
                    ..Default::default()
                },
                move || {
                    let run = || -> Result<(), String> {
                        let endpoint =
                            aux_chan::ClientEndpoint::connect_from_studio_env_cancellable(
                                &worker_stop,
                            )
                            .map_err(|error| error.to_string())?;
                        // The host publishes metadata only after sending its complete
                        // FD batch. Missing bytes therefore mean broken transport,
                        // not a slow GPU. Bound that failure so descriptor backpressure
                        // cannot indefinitely hide subsequent cancellation or exit.
                        endpoint
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .map_err(|error| error.to_string())?;
                        let interrupt =
                            Interrupt(endpoint.try_clone().map_err(|error| error.to_string())?);
                        publish
                            .send(Reply::Connected(interrupt))
                            .map_err(|_| "GPU inbox closed".to_string())?;
                        while !worker_stop.load(Ordering::Acquire) {
                            let request = match receive.recv_timeout(Duration::from_millis(100)) {
                                Ok(request) => request,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                    return Ok(())
                                }
                            };
                            let images = request
                                .swapchain
                                .presentable_images
                                .iter()
                                .map(|image| {
                                    shared_presentable_image_recv_fds_from_aux_chan(
                                        *image, &endpoint,
                                    )
                                })
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|error| error.to_string())?;
                            publish
                                .send(Reply::Loaded(LoadedSwapchain { request, images }))
                                .map_err(|_| "GPU inbox closed".to_string())?;
                        }
                        Ok(())
                    };
                    if let Err(error) = run() {
                        let _ = publish.send(Reply::Failed(error));
                    }
                },
            )
            .map_err(|error| format!("start GPU descriptor worker: {error:?}"))?;
        Ok(Self {
            requests,
            replies: Some(replies),
            pending: VecDeque::new(),
            interrupt: None,
            stop,
            failed: None,
            reported: false,
            _worker: worker,
        })
    }

    pub fn enqueue(&mut self, request: SwapchainRequest) -> Result<(), String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        if self.pending.len() == QUEUED_REQUESTS {
            return Err("GPU descriptor queue is full; retain the host command for retry".into());
        }
        self.pending.push_back(request);
        self.flush();
        if let Some(error) = &self.failed {
            Err(error.clone())
        } else {
            Ok(())
        }
    }

    pub fn has_capacity(&self) -> bool {
        // A failed transport must still admit control traffic (especially
        // Cancel/Kill). Only ordinary bounded queue pressure pauses admission.
        self.failed.is_some() || self.pending.len() < QUEUED_REQUESTS
    }

    fn fail(&mut self, error: String) {
        self.failed = Some(error);
        self.stop.store(true, Ordering::Release);
        self.pending.clear();
        self.interrupt.take();
        // Wake a blocked publisher and drop an unconsumed Connected guard,
        // which may be the only UI-side handle able to interrupt recvmsg.
        self.replies.take();
    }

    fn flush(&mut self) {
        while let Some(request) = self.pending.pop_front() {
            match self.requests.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => {
                    self.pending.push_front(request);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.fail("GPU descriptor worker disconnected".into());
                    break;
                }
            }
        }
    }

    pub fn poll(&mut self) -> Result<Vec<LoadedSwapchain>, String> {
        let mut loaded = Vec::new();
        while let Some(reply) = self
            .replies
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            match reply {
                Reply::Connected(interrupt) => {
                    if self.failed.is_some() {
                        drop(interrupt);
                    } else {
                        self.interrupt = Some(interrupt);
                    }
                }
                Reply::Loaded(batch) => loaded.push(batch),
                Reply::Failed(error) => self.fail(error),
            }
        }
        self.flush();
        if let Some(error) = &self.failed {
            if !self.reported {
                self.reported = true;
                return Err(error.clone());
            }
        }
        Ok(loaded)
    }
}

impl Drop for GpuInbox {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.interrupt.take();
    }
}
