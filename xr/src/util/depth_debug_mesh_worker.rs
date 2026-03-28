use crate::depth_debug_mesh::{build_tsdf_snapshot_debug_mesh_chunks, DebugDepthMeshChunk};
use makepad_widgets::makepad_platform::{SparseTsdGridReadSnapshot, TsdfPublishedSnapshot};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

#[derive(Default)]
struct DepthDebugMeshWorkerMailbox {
    version: u64,
    shutdown: bool,
    pending_snapshot: Option<Arc<TsdfPublishedSnapshot>>,
}

pub(crate) struct XrDepthDebugMeshWorkerResult {
    pub(crate) generation: u64,
    pub(crate) update_sequence: u64,
    pub(crate) snapshot_grid: Arc<SparseTsdGridReadSnapshot>,
    pub(crate) chunks: Vec<DebugDepthMeshChunk>,
}

pub(crate) struct XrDepthDebugMeshWorker {
    mailbox: Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    latest_result: Arc<Mutex<Option<XrDepthDebugMeshWorkerResult>>>,
    join_handle: Option<JoinHandle<()>>,
}

impl XrDepthDebugMeshWorker {
    pub(crate) fn new() -> Self {
        let mailbox = Arc::new((
            Mutex::new(DepthDebugMeshWorkerMailbox::default()),
            Condvar::new(),
        ));
        let latest_result = Arc::new(Mutex::new(None));
        let mailbox_thread = mailbox.clone();
        let result_thread = latest_result.clone();
        let join_handle = thread::Builder::new()
            .name("makepad-xr-depth-debug-mesh".to_string())
            .spawn(move || depth_debug_mesh_worker_loop(mailbox_thread, result_thread))
            .ok();

        Self {
            mailbox,
            latest_result,
            join_handle,
        }
    }

    pub(crate) fn request_snapshot(&mut self, snapshot: Arc<TsdfPublishedSnapshot>) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.pending_snapshot = Some(snapshot);
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
    }

    pub(crate) fn take_latest_result(&mut self) -> Option<XrDepthDebugMeshWorkerResult> {
        self.latest_result
            .lock()
            .ok()
            .and_then(|mut result| result.take())
    }
}

impl Drop for XrDepthDebugMeshWorker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.mailbox;
        if let Ok(mut mailbox) = lock.lock() {
            mailbox.shutdown = true;
            mailbox.version = mailbox.version.saturating_add(1);
            wake.notify_one();
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn depth_debug_mesh_worker_loop(
    mailbox: Arc<(Mutex<DepthDebugMeshWorkerMailbox>, Condvar)>,
    latest_result: Arc<Mutex<Option<XrDepthDebugMeshWorkerResult>>>,
) {
    let mut seen_version = 0u64;
    loop {
        let snapshot = {
            let (lock, wake) = &*mailbox;
            let mut guard = match lock.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            while !guard.shutdown && guard.version == seen_version {
                guard = match wake.wait(guard) {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
            }
            if guard.shutdown {
                return;
            }
            seen_version = guard.version;
            guard.pending_snapshot.take()
        };

        let Some(snapshot) = snapshot else {
            continue;
        };
        let result = XrDepthDebugMeshWorkerResult {
            generation: snapshot.generation,
            update_sequence: snapshot.update_sequence,
            snapshot_grid: snapshot.grid.clone(),
            chunks: build_tsdf_snapshot_debug_mesh_chunks(snapshot.as_ref()),
        };
        if let Ok(mut latest_result) = latest_result.lock() {
            *latest_result = Some(result);
        }
    }
}
