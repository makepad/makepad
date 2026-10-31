use crate::module_loader::ModuleLoader;
use std::ffi::c_void;
use std::time::{Duration, Instant};

type GetManagerFn = unsafe extern "C" fn() -> *mut c_void;
type CreateSessionFn = unsafe extern "C" fn(*mut c_void, *const i32, usize, i64) -> *mut c_void;
type UpdateTargetFn = unsafe extern "C" fn(*mut c_void, i64) -> i32;
type ReportActualFn = unsafe extern "C" fn(*mut c_void, i64) -> i32;
type CloseSessionFn = unsafe extern "C" fn(*mut c_void);

/// How many gaps between vsync ticks we measure the vsync period over.
const PERIOD_GAPS: usize = 31;

/// A performance hint session (ADPF, API 33+) for the render thread. Each vsync cycle that drew a frame reports
/// its CPU time against the vsync period, so Android can raise clocks or move the thread to a bigger core in time.
pub struct PerformanceHint {
    session: *mut c_void,
    update_target: UpdateTargetFn,
    report_actual: ReportActualFn,
    close_session: CloseSessionFn,
    _lib: ModuleLoader,
    target: Duration,
    gaps: [Duration; PERIOD_GAPS],
    gap_count: usize,
    last_vsync: Option<Instant>,
    cycle_start: Duration,
    frame_drawn: bool,
}

impl PerformanceHint {
    /// Returns `None` below API 33, and on devices that don't support performance hints.
    pub fn new(render_thread_id: i32) -> Option<Self> {
        let lib = ModuleLoader::load("libandroid.so").ok()?;
        let get_manager: GetManagerFn = lib.get_symbol("APerformanceHint_getManager").ok()?;
        let create_session: CreateSessionFn = lib.get_symbol("APerformanceHint_createSession").ok()?;
        let update_target = lib.get_symbol("APerformanceHint_updateTargetWorkDuration").ok()?;
        let report_actual = lib.get_symbol("APerformanceHint_reportActualWorkDuration").ok()?;
        let close_session = lib.get_symbol("APerformanceHint_closeSession").ok()?;
        let target = Duration::from_micros(16_667);
        let session = unsafe {
            let manager = get_manager();
            if manager.is_null() {
                return None;
            }
            create_session(manager, &render_thread_id, 1, target.as_nanos() as i64)
        };
        if session.is_null() {
            return None;
        }
        Some(Self {
            session,
            update_target,
            report_actual,
            close_session,
            _lib: lib,
            target,
            gaps: [Duration::ZERO; PERIOD_GAPS],
            gap_count: 0,
            last_vsync: None,
            cycle_start: thread_cpu_time(),
            frame_drawn: false,
        })
    }

    /// Called for every vsync tick, even one we fell behind on, with when its callback ran.
    pub fn record_vsync(&mut self, vsync_time: Instant) {
        let Some(last_vsync) = self.last_vsync.replace(vsync_time) else { return };
        self.gaps[self.gap_count] = vsync_time.saturating_duration_since(last_vsync);
        self.gap_count += 1;
        if self.gap_count == PERIOD_GAPS {
            // The median ignores both a late tick's long gap and the short one right after it.
            self.gaps.sort();
            let period = self.gaps[PERIOD_GAPS / 2];
            if period.abs_diff(self.target) > self.target / 20 {
                self.target = period;
                unsafe { (self.update_target)(self.session, period.as_nanos() as i64) };
            }
            self.gap_count = 0;
        }
    }

    /// Called when vsync ticks start again after a pause, which isn't a vsync period or part of a frame's work.
    pub fn handle_vsync_restart(&mut self) {
        self.last_vsync = None;
        self.frame_drawn = false;
    }

    /// Called on each vsync tick the render loop handles, before its events.
    pub fn start_cycle(&mut self, vsync_time: Instant) {
        self.record_vsync(vsync_time);
        let now = thread_cpu_time();
        // The events handled between ticks compete for the same vsync, so the whole cycle counts.
        if self.frame_drawn {
            let work = now.saturating_sub(self.cycle_start);
            unsafe { (self.report_actual)(self.session, work.as_nanos().max(1) as i64) };
        }
        self.frame_drawn = false;
        self.cycle_start = now;
    }

    pub fn mark_frame_drawn(&mut self) {
        self.frame_drawn = true;
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
