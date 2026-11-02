//! Shell diagnostics copied by their owners; inspection never waits for a shell turn.

use makepad_remote::json_str;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

#[derive(Clone, Default)]
pub(crate) struct Window {
    pub pixels: [u32; 2],
    pub hidden: bool,
    pub starved: Option<Instant>,
    pub pending_swapchain: bool,
}

#[derive(Clone)]
pub(crate) struct Snapshot {
    pub backend: &'static str,
    pub reason: String,
    pub wm_socket: Option<String>,
    pub protocol: Option<u32>,
    pub admitted: Option<u64>,
    pub refused: Option<String>,
    pub hub: &'static str,
    // A retiring reader must not mark a replacement connection broken.
    #[cfg(target_os = "linux")]
    pub hub_id: usize,
    pub app_frame_shown: Option<bool>,
    /// When the first app frame went to the WM (`crate::clock_ns`).
    pub app_frame_at: Option<u64>,
    pub detached: bool,
    pub windows: HashMap<usize, Window>,
}

impl Snapshot {
    pub(crate) fn new(backend: &'static str, reason: String) -> Self {
        Self {
            backend, reason,
            wm_socket: None, protocol: None, admitted: None, refused: None,
            hub: "none",
            #[cfg(target_os = "linux")]
            hub_id: 0,
            app_frame_shown: None, app_frame_at: None, detached: false,
            windows: HashMap::new(),
        }
    }
}

pub(crate) fn snapshot() -> MutexGuard<'static, Snapshot> {
    static SHELL: OnceLock<Mutex<Snapshot>> = OnceLock::new();
    SHELL.get_or_init(|| Mutex::new(Snapshot::new(std::env::consts::OS, String::new())))
        .lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn line() -> String {
    let s = snapshot().clone();
    let number = |n: Option<u64>| n.map_or_else(|| "null".into(), |n| n.to_string());
    let string = |s: Option<&str>| s.map_or_else(|| "null".into(), json_str);
    let mut windows: Vec<_> = s.windows.iter().collect();
    windows.sort_unstable_by_key(|(id, _)| **id);
    let windows = windows.into_iter().map(|(id, w)| format!(
        "{{\"window\":{id},\"pixels\":[{},{}],\"hidden\":{},\"starved_ms\":{},\"pending_swapchain\":{}}}",
        w.pixels[0], w.pixels[1], w.hidden,
        number(w.starved.map(|at| at.elapsed().as_millis() as u64)), w.pending_swapchain,
    )).collect::<Vec<_>>().join(",");
    format!(
        "{{\"backend\":{},\"reason\":{},\"wm_socket\":{},\"protocol\":{},\"admitted\":{},\"refused\":{},\"hub\":{},\"app_frame_shown\":{},\"app_frame_ago_ms\":{},\"detached\":{},\"windows\":[{}]}}",
        json_str(s.backend), json_str(&s.reason), string(s.wm_socket.as_deref()),
        number(s.protocol.map(u64::from)), number(s.admitted), string(s.refused.as_deref()),
        json_str(s.hub), s.app_frame_shown.map_or_else(|| "null".into(), |b| b.to_string()),
        number(s.app_frame_at.map(|at| crate::clock_ns().saturating_sub(at) / 1_000_000)), s.detached, windows,
    )
}
