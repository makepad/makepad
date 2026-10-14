//! Hosted Vulkan bootstrap runs descriptor I/O on one persistent worker.
//! UI requests and completed batches are bounded; the UI never receives an
//! auxiliary descriptor synchronously. This carries handles, not pixel data.
//! The UI connects the channel: one attempt per poll until the host listens,
//! which for a warm instance is its adoption, however long after startup.
use crate::{
    os::shared_framebuf::aux_chan,
    thread::{to_ui_bounded, TaskHandle, ThreadOptions, ThreadSpawner, ToUIReceiver},
};
use makepad_hal::{dmabuf::DmaBuf, hosted_ipc::{self, ClientEndpoint}};
use makepad_studio_protocol::SharedSwapchain;
use crate::makepad_network::mpsc::{sync_channel, SyncSender, TrySendError};
use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Clone, Copy)]
pub(crate) struct SwapchainRequest {
    pub swapchain: SharedSwapchain,
}

pub(crate) struct LoadedSwapchain {
    pub request: SwapchainRequest,
    /// The images' import descriptors, in the swapchain's order.
    pub images: Vec<DmaBuf>,
}

// Shuts the worker's stream down: merely dropping a cloned file descriptor
// would leave the worker's recvmsg blocked.
struct Interrupt(ClientEndpoint);
impl Drop for Interrupt {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

enum Reply {
    Loaded(LoadedSwapchain),
    Failed(String),
}

pub(crate) struct GpuInbox {
    path: PathBuf,
    /// Hands the connected endpoint to the worker; None once connected.
    connect: Option<SyncSender<ClientEndpoint>>,
    requests: SyncSender<SwapchainRequest>,
    replies: Option<ToUIReceiver<Reply>>,
    pending: VecDeque<SwapchainRequest>,
    interrupt: Option<Interrupt>,
    stop: Arc<AtomicBool>,
    failed: Option<String>,
    reported: bool,
    backlogged: bool,
    _worker: TaskHandle<()>,
}

impl GpuInbox {
    pub fn start(spawner: &ThreadSpawner) -> Result<Self, String> {
        let path = aux_chan::client_path().map_err(|error| error.to_string())?;
        let (connect, connected) = sync_channel::<ClientEndpoint>(1);
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
                        // Dropping the inbox before it connected ends the wait.
                        let Ok(endpoint) = connected.recv() else { return Ok(()) };
                        while !worker_stop.load(Ordering::Acquire) {
                            let request = match receive.recv_timeout(Duration::from_millis(100)) {
                                Ok(request) => request,
                                Err(crate::makepad_network::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(crate::makepad_network::mpsc::RecvTimeoutError::Disconnected) => {
                                    return Ok(())
                                }
                            };
                            let images = request
                                .swapchain
                                .presentable_images
                                .iter()
                                .map(|image| {
                                    hosted_ipc::recv_image(&endpoint, *image,
                                        request.swapchain.alloc_width, request.swapchain.alloc_height)
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
            path,
            connect: Some(connect),
            requests,
            replies: Some(replies),
            pending: VecDeque::new(),
            interrupt: None,
            stop,
            failed: None,
            reported: false,
            backlogged: false,
            _worker: worker,
        })
    }

    pub fn enqueue(&mut self, request: SwapchainRequest) -> Result<(), String> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        self.pending.push_back(request);
        self.flush();
        if let Some(error) = &self.failed {
            Err(error.clone())
        } else {
            Ok(())
        }
    }

    fn fail(&mut self, error: String) {
        self.failed = Some(error);
        self.stop.store(true, Ordering::Release);
        self.pending.clear();
        // An unconnected worker stops waiting; a connected one leaves recvmsg.
        self.connect.take();
        self.interrupt.take();
        // Wake a blocked publisher.
        self.replies.take();
    }

    fn flush(&mut self) {
        while let Some(request) = self.pending.pop_front() {
            match self.requests.try_send(request) {
                Ok(()) => {}
                Err(TrySendError::Full(request)) => {
                    if !std::mem::replace(&mut self.backlogged, true) {
                        crate::warning!("GPU descriptor worker busy; retaining requests for the next turn");
                    }
                    self.pending.push_front(request);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.fail("GPU descriptor worker disconnected".into());
                    break;
                }
            }
        }
        if self.pending.is_empty() { self.backlogged = false; }
    }

    /// One connect attempt while the host has no listener for this child yet.
    fn try_connect(&mut self) -> Result<(), String> {
        let Some(connect) = &self.connect else { return Ok(()) };
        let Some(endpoint) = ClientEndpoint::try_connect(&self.path)
            .map_err(|error| format!("auxiliary connect: {error}"))? else { return Ok(()) };
        // The host publishes metadata only after sending its complete
        // FD batch. Missing bytes therefore mean broken transport,
        // not a slow GPU. Bound that failure so descriptor backpressure
        // cannot indefinitely hide subsequent cancellation or exit.
        endpoint.set_read_timeout(Some(Duration::from_secs(1))).map_err(|error| error.to_string())?;
        let interrupt = Interrupt(endpoint.try_clone().map_err(|error| error.to_string())?);
        connect.send(endpoint).map_err(|_| "GPU descriptor worker disconnected".to_string())?;
        self.connect = None;
        self.interrupt = Some(interrupt);
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Vec<LoadedSwapchain>, String> {
        if self.failed.is_none() {
            if let Err(error) = self.try_connect() {
                self.fail(error);
            }
        }
        let mut loaded = Vec::new();
        while let Some(reply) = self
            .replies
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            match reply {
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
