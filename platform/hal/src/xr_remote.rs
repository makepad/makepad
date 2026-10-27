//! The host API of a remote headset (`MAKEPAD_XR_REMOTE=1`, Linux today): what a remote-render
//! server (makepad_desk_link's XR host, makepad_xr_stream) drives. The app renders its XR view as
//! on a headset, into its own textures (`info_flags::XR_OFFSCREEN`); these calls feed it the
//! headset's poses and pace it (`backend::linux::xr_remote`). JIT-compiled code reaches it only
//! through these functions.

/// This process offers a remote headset slot (its apps see `info_flags::XR`).
#[makepad_relax_api::relax_api]
pub fn hal_xr_remote_enabled() -> bool {
    #[cfg(target_os = "linux")]
    return crate::backend::linux::xr_remote::enabled();
    #[cfg(not(target_os = "linux"))]
    return false;
}

/// The headset's eye size (pixels) and display refresh (mHz): the XR window takes them.
#[makepad_relax_api::relax_api]
pub fn hal_xr_remote_attach(eye_w: u32, eye_h: u32, refresh_mhz: u32) {
    #[cfg(target_os = "linux")]
    crate::backend::linux::xr_remote::attach([eye_w, eye_h], refresh_mhz as f32 / 1000.0);
    #[cfg(not(target_os = "linux"))]
    let _ = (eye_w, eye_h, refresh_mhz);
}

/// The pose of the headset's next frame: `frame` is one `XrFrame` (`buffers::Wire`), predicted
/// for `display_ns` (the headset's clock: passed through). The finished frame comes back from
/// [`hal_xr_remote_take`] with `seq`. False while no app presents XR.
#[makepad_relax_api::relax_api]
pub fn hal_xr_remote_pose(seq: u64, display_ns: u64, frame: &[u8]) -> bool {
    #[cfg(target_os = "linux")]
    return crate::backend::linux::xr_remote::pose(seq, display_ns, frame);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (seq, display_ns, frame);
        false
    }
}

/// The app built the XR frame of the pose it was asked for and has room for the next one (a
/// picture slot): the newest pose that came meanwhile is asked for now. Until then the slot asks
/// for no frame (the frames in flight stay bounded by the app's slots).
#[makepad_relax_api::relax_api]
pub fn hal_xr_remote_drawn() {
    #[cfg(target_os = "linux")]
    crate::backend::linux::xr_remote::drawn();
}
