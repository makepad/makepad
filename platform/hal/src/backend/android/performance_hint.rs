use super::xr::sys::ModuleLoader;
use std::ffi::c_void;
use std::time::Duration;

type GetManagerFn = unsafe extern "C" fn() -> *mut c_void;
type CreateSessionFn = unsafe extern "C" fn(*mut c_void, *const i32, usize, i64) -> *mut c_void;
type ReportActualFn = unsafe extern "C" fn(*mut c_void, i64) -> i32;
type CloseSessionFn = unsafe extern "C" fn(*mut c_void);

/// A performance hint session (ADPF, API 33+) for the thread that draws. Each cycle that drew a frame reports
/// its CPU time against the beat period, so Android can raise clocks or move the thread to a bigger core in time.
pub struct PerformanceHint {
    session: *mut c_void,
    report_actual: ReportActualFn,
    close_session: CloseSessionFn,
    _lib: ModuleLoader,
    cycle_start: Option<Duration>,
    beat: Option<(u64, Duration)>,
}

impl PerformanceHint {
    /// Returns `None` below API 33, and on devices that don't support performance hints.
    pub fn for_this_thread(beat_period: Duration) -> Option<Self> {
        unsafe extern "C" {
            fn gettid() -> i32;
        }
        let render_thread_id = unsafe { gettid() };
        let lib = ModuleLoader::load("libandroid.so").ok()?;
        let get_manager: GetManagerFn = lib.get_symbol("APerformanceHint_getManager").ok()?;
        let create_session: CreateSessionFn = lib.get_symbol("APerformanceHint_createSession").ok()?;
        let report_actual = lib.get_symbol("APerformanceHint_reportActualWorkDuration").ok()?;
        let close_session = lib.get_symbol("APerformanceHint_closeSession").ok()?;
        let session = unsafe {
            let manager = get_manager();
            if manager.is_null() {
                return None;
            }
            create_session(manager, &render_thread_id, 1, beat_period.as_nanos() as i64)
        };
        if session.is_null() {
            return None;
        }
        Some(Self {
            session,
            report_actual,
            close_session,
            _lib: lib,
            cycle_start: None,
            beat: None,
        })
    }

    /// Called on each beat, before its drawing, with the app's `repaint_id`. A beat after a pause starts a new
    /// cycle, since the pause isn't part of a frame's work.
    pub fn start_frame(&mut self, repaint_id: u64, after_pause: bool) {
        let cycle_start = match self.cycle_start {
            Some(start) if !after_pause => start,
            _ => thread_cpu_time(),
        };
        self.beat = Some((repaint_id, cycle_start));
    }

    /// Called instead of `start_frame` for a beat whose frame isn't reported, so the next frame's cycle starts
    /// at its own beat.
    pub fn skip_frame(&mut self) {
        self.cycle_start = None;
    }

    /// Called at the end of each loop turn. Once a frame went out to the GPU, reports its cycle's CPU time.
    pub fn finish_frame(&mut self, repaint_id: u64) {
        let Some((beat_repaint_id, cycle_start)) = self.beat.take() else { return };
        let now = thread_cpu_time();
        // a beat that painted nothing (video polling, shader compiles) drew no frame
        if repaint_id != beat_repaint_id {
            let work = now.saturating_sub(cycle_start);
            unsafe { (self.report_actual)(self.session, work.as_nanos().max(1) as i64) };
        }
        // The events handled until the next beat compete for its frame, so the whole cycle counts.
        self.cycle_start = Some(now);
    }
}

impl Drop for PerformanceHint {
    fn drop(&mut self) {
        unsafe { (self.close_session)(self.session) };
    }
}

/// This thread's CPU time, which leaves out any time it spent blocked, like waiting on a free buffer.
fn thread_cpu_time() -> Duration {
    #[repr(C)]
    struct Timespec {
        tv_sec: std::ffi::c_long,
        tv_nsec: std::ffi::c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut time = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut time) };
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}
