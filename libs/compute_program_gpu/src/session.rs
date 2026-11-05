use makepad_platform::makepad_hal::{backend_id, hal_open, open_flags, OpenReq};
use makepad_platform::{Cx, CxOsApi};

/// A windowless Cx on an offscreen session of this machine's GPU, with compute. Dropping it
/// closes the session.
pub struct GpuSession {
    pub cx: Box<Cx>,
}

impl GpuSession {
    /// Opens the GPU; an error where there is none with compute.
    pub fn open() -> Result<GpuSession, String> {
        let mut cx = Box::new(Cx::new(Box::new(|_, _| {})));
        cx.init_cx_os();
        let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
        req.flags |= open_flags::OFFSCREEN_WINDOWS;
        let info = hal_open(&req);
        if info.session == 0 || info.backend == backend_id::NONE {
            return Err("no GPU".into());
        }
        // attached first: dropping the Cx then closes the session it opened
        cx.hal_attach_session(info);
        if info.backend == backend_id::HEADLESS {
            return Err("no GPU".into());
        }
        if !cx.compute_supported() {
            return Err("the GPU backend has no compute".into());
        }
        Ok(GpuSession { cx })
    }

    /// One turn of the session: queued compute encoded and submitted, finished work's events
    /// (readbacks, timers) delivered, or what woke it (a signal). False once the session ended.
    pub fn turn(&mut self) -> bool {
        self.cx.hal_session_turn(100_000_000, true)
    }
}
