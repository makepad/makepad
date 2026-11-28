//! Device limits published by the HAL backend; readers need no device or HAL lock.

/// The largest single GPU buffer the device allocates, in bytes (Metal's
/// `maxBufferLength`); 0 while unknown. A mesh past it cannot be drawn in
/// one buffer.
#[relax_api::relax_api]
static MAX_GPU_BUFFER_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn max_gpu_buffer_bytes() -> u64 {
    MAX_GPU_BUFFER_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set by the backend once it knows its device.
pub fn set_max_gpu_buffer_bytes(bytes: u64) {
    MAX_GPU_BUFFER_BYTES.store(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// Device memory the backend's pressure watch keeps free for the UI, or zero while inactive.
#[relax_api::relax_api]
static GPU_UI_RESERVE_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn gpu_ui_reserve_bytes() -> u64 {
    GPU_UI_RESERVE_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Publish the reserve enforced by the active backend.
pub fn set_gpu_ui_reserve_bytes(bytes: u64) {
    GPU_UI_RESERVE_BYTES.store(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// The memory the device works in, in bytes (Metal's
/// `recommendedMaxWorkingSetSize`, else half the machine's memory); 0
/// while unknown. Everything a mesh holds at once on its way to the GPU
/// must fit in it.
#[relax_api::relax_api]
static GPU_WORKING_SET_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn gpu_working_set_bytes() -> u64 {
    GPU_WORKING_SET_BYTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// `MAKEPAD_GPU_WORKING_SET=<bytes>` runs as on a device with no more
/// memory than that (a smaller device's limits, tried on this one).
pub fn set_gpu_working_set_bytes(bytes: u64) {
    let emulated = relax_api::env::var("MAKEPAD_GPU_WORKING_SET")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let bytes = match emulated {
        Some(e) if bytes == 0 || e < bytes => e,
        _ => bytes,
    };
    GPU_WORKING_SET_BYTES.store(bytes, std::sync::atomic::Ordering::Relaxed);
}
