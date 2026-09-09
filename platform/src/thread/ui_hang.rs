//! Always-on UI stall sampling. The UI publishes atomics; only the watchdog
//! unwinds/symbolizes/logs. No timer, repaint, lock, or worker wait on the UI.
use std::{marker::PhantomData, rc::Rc};
#[path = "ui_hash.rs"]
pub mod hashing;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::cell::Cell;

#[derive(Clone, Copy, Debug)]
#[repr(u32)]
pub enum UiPhase {
    NativeEvent = 256,
    DrawList,
    WidgetDraw,
    CodePageInstall,
    CodePrepare,
    FontAtlas,
    StagePlacement,
    WorkerPump,
    Present,
    GpuWait,
    EditorAttach,
    CodeCache,
    CodeHover,
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn phase_name(id: u32) -> &'static str {
    match id {
        256 => "native-event",
        257 => "draw-list",
        258 => "widget-draw",
        259 => "code-page-install",
        260 => "code-prepare",
        261 => "font-atlas",
        262 => "stage-placement",
        263 => "worker-pump",
        264 => "present",
        265 => "gpu-wait",
        266 => "editor-attach",
        267 => "code-cache",
        268 => "code-hover",
        id => crate::event::Event::name_from_u32(id.saturating_sub(1)),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod native;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use native::State;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
thread_local! {
    static CURRENT: Cell<*const State> = const { Cell::new(std::ptr::null()) };
}

/// Register the calling UI thread once, at Cx construction. Workers never
/// register and their phase scopes are no-ops. No thread is spawned per frame.
pub fn initialize() {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    native::initialize();
}

/// Nesting restores the previous phase. The outermost scope defines one busy
/// event/frame; the event loop's idle wait must remain outside that scope.
#[must_use]
pub struct UiPhaseGuard {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    state: *const State,
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    previous: u64,
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    root: bool,
    _thread: PhantomData<Rc<()>>,
}

#[inline]
pub fn ui_phase(phase: UiPhase) -> UiPhaseGuard {
    ui_phase_detail(phase, 0)
}

#[inline]
pub fn ui_phase_detail(phase: UiPhase, detail: u32) -> UiPhaseGuard {
    enter(phase as u64 | ((detail as u64) << 32))
}

#[inline]
pub fn ui_event_phase(event: &crate::event::Event) -> UiPhaseGuard {
    enter(event.to_u32() as u64 + 1)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[inline]
fn enter(phase: u64) -> UiPhaseGuard {
    let state = CURRENT.with(Cell::get);
    let mut guard = UiPhaseGuard {
        state,
        previous: 0,
        root: false,
        _thread: PhantomData,
    };
    if !state.is_null() {
        // Registration and guards are confined to this thread. The sampler
        // owns another Arc, so State also outlives TLS teardown.
        let state = unsafe { &*state };
        guard.previous = state.phase.load(std::sync::atomic::Ordering::Relaxed);
        state.phase.store(phase, std::sync::atomic::Ordering::Relaxed);
        guard.root = guard.previous == 0;
        if guard.root {
            state.begin();
        }
    }
    guard
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
#[inline]
fn enter(_: u64) -> UiPhaseGuard {
    UiPhaseGuard { _thread: PhantomData }
}

impl Drop for UiPhaseGuard {
    #[inline]
    fn drop(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        if !self.state.is_null() {
            let state = unsafe { &*self.state };
            if self.root {
                state.end();
            }
            state.phase.store(self.previous, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
