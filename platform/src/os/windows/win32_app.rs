use {
    crate::{
        cursor::MouseCursor,
        error,
        event::*,
        log,
        os::{
            cx_native::EventFlow,
            windows::{
                dataobject::DragItemWindows, dropsource::*, win32_event::Win32Event,
                win32_window::Win32Window,
            },
        },
        window::WindowId,
        windows::{
            core::BOOL,
            core::HRESULT,
            core::PCSTR,
            core::PCWSTR,
            //core::IntoParam,
            Win32::{
                Foundation::{
                    COLORREF, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, FARPROC, HANDLE, HWND, LPARAM,
                    S_OK, WPARAM,
                },
                Graphics::Gdi::{
                    CreateSolidBrush, GetDC, GetDeviceCaps, MonitorFromWindow, HMONITOR,
                    LOGPIXELSX, MONITOR_DEFAULTTONEAREST,
                },
                System::{
                    Com::IDataObject,
                    LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA},
                    Ole::{
                        DoDragDrop, IDropSource, OleInitialize, DROPEFFECT, DROPEFFECT_COPY,
                        DROPEFFECT_MOVE,
                    },
                    Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
                    Threading::ExitProcess,
                },
                UI::{
                    HiDpi::{
                        DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
                        MONITOR_DPI_TYPE, PROCESS_DPI_AWARENESS, PROCESS_PER_MONITOR_DPI_AWARE,
                    },
                    WindowsAndMessaging::{
                        DispatchMessageW, GetMessageW, GetSystemMetrics, IsGUIThread,
                        IsProcessDPIAware, KillTimer, LoadCursorW, LoadIconW, LoadImageW,
                        PeekMessageW, RegisterClassExW, SetCursor, SetTimer, ShowCursor,
                        TranslateMessage, CS_OWNDC, HICON, IDC_ARROW, IDC_CROSS, IDC_HAND,
                        IDC_HELP, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENESW, IDC_SIZENS,
                        IDC_SIZENWSE, IDC_SIZEWE, IDI_WINLOGO, IMAGE_ICON, LR_DEFAULTCOLOR, MSG,
                        PM_NOREMOVE, PM_REMOVE, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON,
                        SYSTEM_METRICS_INDEX, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT,
                        WNDCLASSEXW,
                    },
                },
            },
        },
    },
    std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        ffi::OsStr,
        mem,
        os::windows::ffi::OsStrExt,
        sync::atomic::{AtomicU32, Ordering},
    },
};
use crate::frame_trace::TickSource;
pub const FALSE: BOOL = BOOL(0);
pub const TRUE: BOOL = BOOL(1);

thread_local! {
    pub static WIN32_APP: RefCell<Option<Win32App>> = RefCell::new(None);
}

static UI_THREAD_ID: AtomicU32 = AtomicU32::new(0);

#[link(name = "kernel32")]
extern "system" {
    #[link_name = "GetCurrentThreadId"]
    fn get_current_thread_id() -> u32;
}

#[link(name = "user32")]
extern "system" {
    #[link_name = "PostThreadMessageW"]
    fn post_thread_message_w(thread_id: u32, message: u32, w_param: WPARAM, l_param: LPARAM) -> BOOL;
}

pub(crate) fn wake_ui_event_loop() {
    let thread_id = UI_THREAD_ID.load(Ordering::Acquire);
    if thread_id != 0 {
        unsafe {
            let _ = post_thread_message_w(thread_id, 0, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn with_win32_app<R>(f: impl FnOnce(&mut Win32App) -> R) -> R {
    WIN32_APP.with_borrow_mut(|app| f(app.as_mut().unwrap()))
}

/// Like `with_win32_app`, but returns `None` instead of panicking when there is
/// no app yet (or the thread-local is already torn down). Needed on teardown
/// paths — `Drop for D3d11Window` can run while the process is exiting.
pub fn try_with_win32_app<R>(f: impl FnOnce(&mut Win32App) -> R) -> Option<R> {
    WIN32_APP
        .try_with(|app| app.borrow_mut().as_mut().map(f))
        .ok()
        .flatten()
}

pub fn init_win32_app_global(event_callback: Box<dyn FnMut(Win32Event) -> EventFlow>) {
    UI_THREAD_ID.store(unsafe { get_current_thread_id() }, Ordering::Release);
    WIN32_APP.with(|app| {
        *app.borrow_mut() = Some(Win32App::new(event_callback));
    });
}
/*
// copied from Microsoft so it refers to the right IDataObject
#[allow(non_snake_case)]
pub unsafe fn DoDragDrop<P0, P1>(pdataobj: P0, pdropsource: P1, dwokeffects: DROPEFFECT, pdweffect: *mut DROPEFFECT) -> HRESULT
where
P0: IntoParam<IDataObject>,
P1: IntoParam<IDropSource>,
{
    ::windows_targets::link!("ole32.dll" "system" fn DoDragDrop(pdataobj: *mut::core::ffi::c_void, pdropsource: *mut::core::ffi::c_void, dwokeffects: DROPEFFECT, pdweffect: *mut DROPEFFECT) -> HRESULT);
    DoDragDrop(pdataobj.into_param().abi(), pdropsource.into_param().abi(), dwokeffects, pdweffect)
}*/

/// Coalesce a run of consecutive `WM_MOUSEMOVE` messages for the same window
/// into just the latest one.
///
/// A high-polling-rate mouse (500–1000+ Hz) floods the message queue with
/// mouse-moves. Dispatching each one separately runs redundant hover
/// hit-testing across the whole widget tree (and a paint per move), which
/// steals frame budget from an in-progress fling — producing the visible
/// scroll judder when the mouse is moved during deceleration. We only merge
/// *adjacent* moves: we peek the next queued message and stop at the first
/// non-move, so no button / key / wheel message is ever dropped or reordered.
///
/// macOS gets this for free from Cocoa's built-in mouse-move coalescing, and
/// the Android backend already coalesces consecutive touch-moves explicitly.
///
/// This discards the intermediate cursor positions within a run, which is correct
/// for hover/hit-testing but loses the full pointer path; a widget that needs every
/// sample (freehand drawing/ink, gesture recognition) would have to read raw input.
unsafe fn coalesce_mouse_move(mut msg: MSG) -> MSG {
    if msg.message != WM_MOUSEMOVE {
        return msg;
    }
    loop {
        // Peek (without removing) the next queued message for this window.
        let mut peek = std::mem::MaybeUninit::uninit();
        if PeekMessageW(peek.as_mut_ptr(), Some(msg.hwnd), 0, 0, PM_NOREMOVE) == FALSE {
            break;
        }
        if peek.assume_init().message != WM_MOUSEMOVE {
            break; // next message isn't a move — don't reorder past it
        }
        // It is a move: remove it and let it supersede the current one.
        let mut taken = std::mem::MaybeUninit::uninit();
        if PeekMessageW(taken.as_mut_ptr(), Some(msg.hwnd), WM_MOUSEMOVE, WM_MOUSEMOVE, PM_REMOVE)
            == FALSE
        {
            break;
        }
        msg = taken.assume_init();
    }
    msg
}

/// Coalesce a run of consecutive `WM_MOUSEWHEEL` messages for the same window
/// into one message carrying the summed wheel delta.
///
/// Free-spinning wheels and precision touchpads can emit wheel messages faster
/// than the vsync-paced loop consumes them, so without merging, a gesture
/// builds a queue backlog that keeps replaying deltas after it ends. As with
/// `coalesce_mouse_move`, only *adjacent* wheel messages are merged: we peek
/// the next queued message and stop at the first non-wheel, so no other
/// message is ever dropped or reordered. The wheel delta lives in the signed
/// high word of `wParam`; the sum saturates to that range instead of wrapping.
/// Position (`lParam`) and the key-state low word come from the newest message.
unsafe fn coalesce_mouse_wheel(mut msg: MSG) -> MSG {
    if msg.message != WM_MOUSEWHEEL {
        return msg;
    }
    let mut delta = (msg.wParam.0 >> 16) as u16 as i16 as i32;
    loop {
        // Peek (without removing) the next queued message for this window.
        let mut peek = std::mem::MaybeUninit::uninit();
        if PeekMessageW(peek.as_mut_ptr(), Some(msg.hwnd), 0, 0, PM_NOREMOVE) == FALSE {
            break;
        }
        if peek.assume_init().message != WM_MOUSEWHEEL {
            break; // next message isn't a wheel — don't reorder past it
        }
        // It is a wheel: remove it, accumulate its delta and take its position/state.
        let mut taken = std::mem::MaybeUninit::uninit();
        if PeekMessageW(
            taken.as_mut_ptr(),
            Some(msg.hwnd),
            WM_MOUSEWHEEL,
            WM_MOUSEWHEEL,
            PM_REMOVE,
        ) == FALSE
        {
            break;
        }
        msg = taken.assume_init();
        delta += (msg.wParam.0 >> 16) as u16 as i16 as i32;
    }
    let delta = delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    msg.wParam = WPARAM(((delta as u16 as usize) << 16) | (msg.wParam.0 & 0xffff));
    msg
}

/// `MsgWaitForMultipleObjectsEx`, `QS_ALLINPUT` and `MWMO_INPUTAVAILABLE` are not
/// in the vendored windows bindings; declare what we need (same pattern as the
/// `CreateIcon` link below).
const QS_ALLINPUT: u32 = 0x04FF;
const MWMO_INPUTAVAILABLE: u32 = 0x0004;
const WAIT_FAILED_U32: u32 = 0xFFFF_FFFF;

/// Wait until either one of `handles` is signaled (a window's DXGI frame-latency
/// waitable, i.e. "the compositor is ready for that window's next frame") or the
/// thread has queued input. Returns `WAIT_OBJECT_0 + k` for handle `k`,
/// `WAIT_OBJECT_0 + handles.len()` for input, `WAIT_TIMEOUT_U32` on timeout.
///
/// `MWMO_INPUTAVAILABLE` matters: without it the call only wakes on input that
/// arrived *after* the wait started, so a message already sitting in the queue
/// (we peek without removing while coalescing) would be ignored until the next
/// one arrived.
unsafe fn msg_wait_for_beat_or_input(handles: &[HANDLE], timeout_ms: u32) -> u32 {
    windows_core::link!("user32.dll" "system" fn MsgWaitForMultipleObjectsEx(
        n_count: u32,
        p_handles: *const HANDLE,
        dw_milliseconds: u32,
        dw_wake_mask: u32,
        dw_flags: u32
    ) -> u32);
    unsafe {
        MsgWaitForMultipleObjectsEx(
            handles.len() as u32,
            handles.as_ptr(),
            timeout_ms,
            QS_ALLINPUT,
            MWMO_INPUTAVAILABLE,
        )
    }
}

/// Drain queued win32 messages (adjacent mouse-moves and wheels coalesced) up to
/// a small count/time budget, so a flood of high-rate input can never starve the
/// paint beat. Returns false if a dispatched message asked the app to exit.
unsafe fn drain_messages() -> bool {
    let drain_start = std::time::Instant::now();
    let mut drain_budget = 32;
    loop {
        let mut msg = std::mem::MaybeUninit::uninit();
        if PeekMessageW(msg.as_mut_ptr(), None, 0, 0, PM_REMOVE) == FALSE {
            break;
        }
        let msg = coalesce_mouse_move(msg.assume_init());
        let msg = coalesce_mouse_wheel(msg);
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
        drain_budget -= 1;
        if drain_budget == 0
            || drain_start.elapsed() >= std::time::Duration::from_millis(2)
            || matches!(with_win32_app(|app| app.event_flow.clone()), EventFlow::Exit)
        {
            break;
        }
    }
    !matches!(with_win32_app(|app| app.event_flow.clone()), EventFlow::Exit)
}

pub struct Win32App {
    event_callback: Option<Box<dyn FnMut(Win32Event) -> EventFlow>>,
    /// Events queued by re-entrant `do_callback` calls; drained FIFO by the outer
    /// call so they are delivered late rather than dropped.
    pub pending_events: VecDeque<Win32Event>,
    pub window_class_name: Vec<u16>,
    pub all_windows: Vec<HWND>,
    pub time: Win32Time,
    pub timers: Vec<Win32Timer>,
    pub was_signal_poll: bool,
    pub event_flow: EventFlow,
    pub dpi_functions: DpiFunctions,
    pub current_cursor: Option<MouseCursor>,
    pub currently_clicked_window_id: Option<WindowId>,
    pub start_dragging_items: Option<Vec<DragItem>>,
    pub is_dragging_internal: Cell<bool>,
    /// The paint beat: one DXGI frame-latency waitable per vsync-paced window,
    /// in registration order — index 0 is the *primary* window, whose beat drives
    /// the whole app tick (the macOS backend picks its primary display link the
    /// same way). Registered by `D3d11Window::new`, dropped on window teardown
    /// and while a window is in a live resize (which presents unpaced).
    pub beat_handles: Vec<BeatSource>,
    /// How long the beat wait may block before falling back to an unpaced tick.
    /// The waitable is a credit semaphore refilled by *retired presents*, so a
    /// stretch of ticks that present nothing (a NextFrame listener that dirties
    /// no pass, a video player polling between decoded frames) drains it and
    /// nothing would wake us; the paint tick shortens the timeout in that case
    /// so such work keeps its old ~8 ms cadence instead of stalling to 33 ms.
    pub beat_timeout_ms: u32,
    /// The frame clock, measured (`MAKEPAD_TRACE=frames`).
    pub frame_trace: crate::frame_trace::FrameTrace,
}

/// One window's frame clock.
pub struct BeatSource {
    pub window_id: WindowId,
    /// The swap chain's frame-latency waitable — a semaphore whose count is the
    /// number of frames the compositor is ready to accept.
    pub handle: HANDLE,
    /// A credit taken from that semaphore and not yet spent on a `Present`.
    ///
    /// DXGI refills the semaphore ONLY when a present retires, and the handle it
    /// hands back is read-only — `ReleaseSemaphore` on it fails with
    /// ACCESS_DENIED (verified on a real box), so a credit taken can never be
    /// given back. Every wait must therefore be paired with a present, or the
    /// window's clock winds down to zero and it stops beating for good. A beat
    /// that finds nothing to paint keeps its credit and simply drops out of the
    /// wait until a frame is presented: the compositor is already ready for that
    /// window, so there is nothing left to wait for.
    pub credit_held: bool,
}

/// Beat timeout after a tick that actually presented: ~2 refresh intervals at
/// 60 Hz. Only reached when the compositor stops retiring presents (occluded,
/// minimized, a stalled DWM), in which case an unpaced heartbeat tick is right.
pub const BEAT_TIMEOUT_PRESENTED_MS: u32 = 33;
/// Beat timeout after a tick that presented nothing; matches the signal-poll
/// timer's 8 ms so non-presenting work is paced exactly like before.
pub const BEAT_TIMEOUT_IDLE_MS: u32 = 8;

#[derive(Clone)]
pub enum Win32Timer {
    Free,
    Timer {
        win32_id: usize,
        timer_id: u64,
        interval: f64,
        repeats: bool,
    },
    Resize {
        win32_id: usize,
    },
    DragDrop {
        win32_id: usize,
    },
    SignalPoll {
        win32_id: usize,
    },
}

pub struct Win32Time {
    pub time_start: i64,
    pub time_freq: i64,
}

impl Win32Time {
    pub fn new() -> Self {
        let mut time_start = 0i64;
        unsafe { QueryPerformanceCounter(&mut time_start).unwrap() };

        let mut time_freq = 0i64;
        unsafe { QueryPerformanceFrequency(&mut time_freq).unwrap() };
        Self {
            time_start,
            time_freq,
        }
    }

    pub fn time_now(&self) -> f64 {
        unsafe {
            let mut time_now = 0i64;
            QueryPerformanceCounter(&mut time_now).unwrap();
            (time_now - self.time_start) as f64 / self.time_freq as f64
        }
    }

    /// Map a raw `QueryPerformanceCounter` timestamp into app time. DXGI frame
    /// statistics report `SyncQPCTime` in exactly this domain, so a vblank
    /// timestamp from the driver lands on the same clock as `time_now()`.
    pub fn qpc_to_time(&self, qpc: i64) -> f64 {
        (qpc - self.time_start) as f64 / self.time_freq as f64
    }
}

impl Win32App {
    pub fn new(event_callback: Box<dyn FnMut(Win32Event) -> EventFlow>) -> Win32App {
        let window_class_name = encode_wide("MakepadWindow\0");
        let (hicon_big, hicon_small) = Self::create_default_icons();
        let class = WNDCLASSEXW {
            cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_OWNDC,
            lpfnWndProc: Some(Win32Window::window_class_proc),
            hInstance: unsafe { GetModuleHandleW(None).unwrap().into() },
            hIcon: hicon_big,
            hIconSm: hicon_small,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
            lpszClassName: PCWSTR(window_class_name.as_ptr()),
            hbrBackground: unsafe { CreateSolidBrush(COLORREF(0x3f3f3f3f)) },
            ..Default::default()
        };

        unsafe {
            RegisterClassExW(&class);
            let _ = IsGUIThread(true);

            // initialize COM using OleInitialize to allow Drag&Drop and other shell features
            OleInitialize(None).unwrap();
        }

        let win32_app = Win32App {
            start_dragging_items: None,
            window_class_name,
            was_signal_poll: false,
            time: Win32Time::new(),
            event_callback: Some(event_callback),
            pending_events: VecDeque::new(),
            event_flow: EventFlow::Poll,
            all_windows: Vec::new(),
            timers: Vec::new(),
            dpi_functions: DpiFunctions::new(),
            current_cursor: None,
            currently_clicked_window_id: None,
            is_dragging_internal: Cell::new(false),
            beat_handles: Vec::new(),
            beat_timeout_ms: BEAT_TIMEOUT_PRESENTED_MS,
            frame_trace: crate::frame_trace::FrameTrace::new(),
        };
        win32_app.dpi_functions.become_dpi_aware();

        win32_app
    }

    /// Create an HICON from RGBA8 pixel data. Returns the default system icon on failure.
    fn create_icon_from_rgba(width: u32, height: u32, rgba: &[u8]) -> HICON {
        use crate::windows::Win32::UI::WindowsAndMessaging::HICON;
        // CreateIcon expects AND mask (1bpp) + XOR mask (color).
        // We use CreateIcon with nWidth, nHeight, cPlanes=1, cBitsPixel=32.
        // The XOR mask is BGRA pixel data, the AND mask is all zeros (fully opaque).
        let pixel_count = (width * height) as usize;
        let mut bgra = Vec::with_capacity(pixel_count * 4);
        for chunk in rgba.chunks_exact(4) {
            bgra.push(chunk[2]); // B
            bgra.push(chunk[1]); // G
            bgra.push(chunk[0]); // R
            bgra.push(chunk[3]); // A
        }
        // AND mask: all zeros = fully opaque (1 bit per pixel, rows padded to 16-bit)
        let and_stride = ((width + 15) / 16) * 2;
        let and_mask = vec![0u8; (and_stride * height) as usize];

        windows_core::link!("user32.dll" "system" fn CreateIcon(
            hinstance: *mut core::ffi::c_void,
            n_width: i32,
            n_height: i32,
            c_planes: u8,
            c_bits_pixel: u8,
            lp_and_bits: *const u8,
            lp_xor_bits: *const u8
        ) -> HICON);

        unsafe {
            let hicon = CreateIcon(
                core::ptr::null_mut(),
                width as i32,
                height as i32,
                1,
                32,
                and_mask.as_ptr(),
                bgra.as_ptr(),
            );
            if hicon.0.is_null() {
                LoadIconW(None, IDI_WINLOGO).unwrap()
            } else {
                hicon
            }
        }
    }

    fn create_default_icons() -> (HICON, HICON) {
        let icon = crate::app_icon::window_icon();

        let pick = |target: u32| icon.buffers.iter().min_by_key(|b| b.width.abs_diff(target));

        // Fallback: the exe-embedded icon (resource id 1). LoadImageW, unlike LoadIconW,
        // can request the proper system size for each class icon slot.
        let load_exe_or_default = |cx: SYSTEM_METRICS_INDEX, cy: SYSTEM_METRICS_INDEX| unsafe {
            let from_exe = GetModuleHandleW(None).ok().and_then(|h| {
                LoadImageW(
                    Some(h.into()),
                    PCWSTR(1 as *const u16),
                    IMAGE_ICON,
                    GetSystemMetrics(cx),
                    GetSystemMetrics(cy),
                    LR_DEFAULTCOLOR,
                )
                .ok()
            });
            from_exe
                .map(|handle| HICON(handle.0))
                .unwrap_or_else(|| LoadIconW(None, IDI_WINLOGO).unwrap())
        };

        let big = if let Some(buf) = pick(64).or_else(|| icon.buffers.first()) {
            Self::create_icon_from_rgba(buf.width, buf.height, &buf.data)
        } else {
            load_exe_or_default(SM_CXICON, SM_CYICON)
        };

        let small = if let Some(buf) = pick(32).or_else(|| icon.buffers.first()) {
            Self::create_icon_from_rgba(buf.width, buf.height, &buf.data)
        } else {
            load_exe_or_default(SM_CXSMICON, SM_CYSMICON)
        };

        (big, small)
    }

    pub fn event_loop() {
        unsafe {
            loop {
                let event_flow = with_win32_app(|app| app.event_flow.clone());
                match event_flow {
                    EventFlow::Wait => {
                        let mut msg = std::mem::MaybeUninit::uninit();
                        let ret = GetMessageW(msg.as_mut_ptr(), None, 0, 0);
                        let msg = msg.assume_init();
                        if ret == FALSE {
                            // Only happens if the message is `WM_QUIT`.
                            debug_assert_eq!(msg.message, WM_QUIT);
                            with_win32_app(|app| app.event_flow = EventFlow::Exit);
                        } else {
                            let msg = coalesce_mouse_move(msg);
                            let msg = coalesce_mouse_wheel(msg);
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                            if !with_win32_app(|app| app.was_signal_poll()) {
                                with_win32_app(|app| {
                                    let now = app.time_now();
                                    app.frame_trace.tick(TickSource::Message, now, None);
                                });
                                Win32App::do_callback(Win32Event::Paint);
                            }
                        }
                    }
                    EventFlow::Poll => {
                        // THE BEAT. One wait covers both clocks the loop cares about: each
                        // vsync-paced window's DXGI frame-latency waitable ("the compositor
                        // retired a present of this window, send the next one") and the thread's
                        // input queue. Whichever fires first decides what this pass does.
                        //
                        // This is the same shape as the macOS backend's display link: the frame
                        // clock lives ABOVE the app tick, so a beat knows WHICH window flipped and
                        // WHEN, and the paint below can stamp every pass with that one flip time
                        // instead of sampling a fresh wall-clock per pass. Previously the wait sat
                        // inside `draw_pass_to_window`, below the tick, with no window identity and
                        // no timestamp, so multi-window apps paced each other and the app tick could
                        // not see the frame boundary at all.
                        //
                        // Input drains (adjacent mouse-moves and wheels coalesced, on a small
                        // budget) without painting: a moving mouse injects WM_MOUSEMOVE at
                        // 500–1000 Hz and must never outvote the frame clock, but neither may it
                        // starve — hence the budget, and hence the loop coming straight back here.
                        let beats: Vec<(WindowId, HANDLE, bool)> =
                            with_win32_app(|app| app.beat_wait_list());
                        if beats.is_empty() {
                            // Nothing to wait for: popup-only, a live resize (which presents
                            // unpaced and unregisters its beat), no window at all — or every
                            // paced window is already holding a credit, meaning the compositor
                            // is ready for all of them and there is nothing left to wait on.
                            // Fall back to the old drain-then-paint pass; the SetTimer
                            // heartbeats (resize / drag-drop / 8 ms signal poll) and the paint
                            // tick's own idle sleep keep it from spinning, exactly as the
                            // NSTimer fallback survives on macOS.
                            if drain_messages() {
                                with_win32_app(|app| {
                                    let now = app.time_now();
                                    app.frame_trace.tick(TickSource::Drain, now, None);
                                });
                                Win32App::do_callback(Win32Event::Paint);
                            }
                        } else {
                            let handles: Vec<HANDLE> = beats.iter().map(|(_, h, _)| *h).collect();
                            let timeout = with_win32_app(|app| app.beat_timeout_ms).max(1);
                            let count = handles.len() as u32;
                            let ret = msg_wait_for_beat_or_input(&handles, timeout);
                            if ret < count {
                                let (window_id, _, primary) = beats[ret as usize];
                                // The wait consumed one of that swap chain's credits. Record
                                // it: it can only be given back by presenting a frame.
                                let time = with_win32_app(|app| {
                                    app.take_beat_credit(window_id);
                                    app.time_now()
                                });
                                // The flip this beat aims at is only known once the window's
                                // frame statistics are read (windows.rs): the source is noted
                                // here, the lead there.
                                with_win32_app(|app| app.frame_trace.tick(TickSource::Waitable, time, None));
                                Win32App::do_callback(Win32Event::Beat {
                                    window_id,
                                    time,
                                    primary,
                                });
                            } else if ret == count {
                                let _ = drain_messages();
                            } else {
                                // WAIT_TIMEOUT: no window is being retired by the compositor
                                // (occluded, minimized, or DWM stalled). Keep the app alive with
                                // an unscoped heartbeat tick — the per-window wait inside
                                // `draw_pass_to_window` decides whether to actually present.
                                if ret == WAIT_FAILED_U32 {
                                    // A bad handle would otherwise spin this loop at full speed.
                                    static LOGGED: std::sync::atomic::AtomicBool =
                                        std::sync::atomic::AtomicBool::new(false);
                                    if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                        error!("MsgWaitForMultipleObjectsEx failed; falling back to timed paint beats");
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(4));
                                }
                                // Anything else (only WAIT_TIMEOUT, 258, is expected
                                // here — a semaphore is never abandoned) is treated
                                // as a timeout: an unpaced tick is always safe.
                                // Skip the paint only if something asked us to exit, so we don't
                                // run an extra callback (double shutdown) on the way out.
                                if !matches!(
                                    with_win32_app(|app| app.event_flow.clone()),
                                    EventFlow::Exit
                                ) {
                                    with_win32_app(|app| {
                                        let now = app.time_now();
                                        app.frame_trace.tick(TickSource::Timeout, now, None);
                                    });
                                    Win32App::do_callback(Win32Event::Paint);
                                }
                            }
                        }
                    }
                    EventFlow::Exit => panic!(),
                }
                Win32App::poll_start_drag_drop();
            }
        }
    }

    /// Dispatch `event` to the application's event callback.
    ///
    /// The callback is taken out of the global while it runs, so Win32 calls inside it
    /// that synchronously re-enter the wndproc land here re-entrantly; those events are
    /// queued and drained FIFO by the outermost call, delivered late but never dropped.
    pub fn do_callback(event: Win32Event) {
        let cb = with_win32_app(|app| app.event_callback.take());
        if let Some(mut callback) = cb {
            let event_flow = callback(event);
            with_win32_app(|app| app.event_flow = event_flow);
            let mut exit = matches!(event_flow, EventFlow::Exit);
            // Drain re-entrantly queued events (re-checked each pass) BEFORE acting on
            // an Exit, so e.g. a queued WindowClosed is not swallowed by ExitProcess.
            while let Some(event) = with_win32_app(|app| app.pending_events.pop_front()) {
                let event_flow = callback(event);
                with_win32_app(|app| app.event_flow = event_flow);
                exit |= matches!(event_flow, EventFlow::Exit);
            }
            if exit {
                unsafe {
                    ExitProcess(0);
                }
            }
            with_win32_app(|app| app.event_callback = Some(callback));
        } else {
            // Re-entered while the callback runs higher up the stack; queue for the outer call.
            with_win32_app(|app| app.pending_events.push_back(event));
        }
    }

    pub unsafe extern "system" fn timer_proc(
        _hwnd: HWND,
        _arg1: u32,
        in_win32_id: usize,
        _arg2: u32,
    ) {
        let hit_timer = {
            with_win32_app(|app| {
                let mut hit_timer = None;
                for slot in 0..app.timers.len() {
                    match app.timers[slot] {
                        Win32Timer::Timer {
                            win32_id, repeats, ..
                        } => {
                            if win32_id == in_win32_id {
                                hit_timer = Some(app.timers[slot].clone());
                                if !repeats {
                                    KillTimer(None, in_win32_id).unwrap();
                                    app.timers[slot] = Win32Timer::Free;
                                }
                                break;
                            }
                        }
                        Win32Timer::DragDrop { win32_id, .. } => {
                            if win32_id == in_win32_id {
                                hit_timer = Some(app.timers[slot].clone());
                                break;
                            }
                        }
                        Win32Timer::Resize { win32_id, .. } => {
                            if win32_id == in_win32_id {
                                hit_timer = Some(app.timers[slot].clone());
                                break;
                            }
                        }
                        Win32Timer::SignalPoll { win32_id, .. } => {
                            if win32_id == in_win32_id {
                                hit_timer = Some(app.timers[slot].clone());
                                break;
                            }
                        }
                        _ => (),
                    }
                }
                hit_timer
            })
        };
        // call the dependencies
        let time = with_win32_app(|app| app.time_now());
        if let Some(hit_timer) = hit_timer {
            match hit_timer {
                Win32Timer::Timer { timer_id, .. } => {
                    Win32App::do_callback(Win32Event::Timer(TimerEvent {
                        time: Some(time),
                        timer_id: timer_id,
                    }));
                }
                Win32Timer::Resize { .. } => {
                    with_win32_app(|app| {
                        let now = app.time_now();
                        app.frame_trace.tick(TickSource::Timer, now, None);
                    });
                    Win32App::do_callback(Win32Event::Paint);
                }
                Win32Timer::DragDrop { .. } => {
                    with_win32_app(|app| {
                        let now = app.time_now();
                        app.frame_trace.tick(TickSource::Timer, now, None);
                    });
                    Win32App::do_callback(Win32Event::Paint);
                }
                Win32Timer::SignalPoll { .. } => {
                    Win32App::do_callback(Win32Event::Signal);
                    with_win32_app(|app| app.was_signal_poll = true);
                }
                _ => (),
            }
        }
    }

    pub fn was_signal_poll(&mut self) -> bool {
        if self.was_signal_poll {
            self.was_signal_poll = false;
            true
        } else {
            false
        }
    }

    pub fn get_free_timer_slot(&mut self) -> usize {
        //let win32_app = get_win32_app_global();
        for slot in 0..self.timers.len() {
            if let Win32Timer::Free = self.timers[slot] {
                return slot;
            }
        }
        let slot = self.timers.len();
        self.timers.push(Win32Timer::Free);
        slot
    }

    pub fn start_timer(&mut self, timer_id: u64, interval: f64, repeats: bool) {
        let slot = self.get_free_timer_slot();
        let win32_id =
            unsafe { SetTimer(None, 0, (interval * 1000.0) as u32, Some(Self::timer_proc)) };
        if timer_id == 0 {
            self.timers[slot] = Win32Timer::SignalPoll { win32_id: win32_id };
        } else {
            self.timers[slot] = Win32Timer::Timer {
                timer_id: timer_id,
                win32_id: win32_id,
                interval: interval,
                repeats: repeats,
            };
        }
    }

    pub fn stop_timer(&mut self, which_timer_id: u64) {
        for slot in 0..self.timers.len() {
            let win32_id = match self.timers[slot] {
                Win32Timer::Timer {
                    win32_id, timer_id, ..
                } if timer_id == which_timer_id => win32_id,
                // `start_timer(0, ..)` installs a SignalPoll timer rather than a
                // Timer, so stopping id 0 has to be able to kill that shape too —
                // otherwise the slot leaks and its 8 ms wakeup runs forever.
                Win32Timer::SignalPoll { win32_id } if which_timer_id == 0 => win32_id,
                _ => continue,
            };
            self.timers[slot] = Win32Timer::Free;
            unsafe {
                let _ = KillTimer(None, win32_id);
            }
        }
    }

    /// Register a window's DXGI frame-latency waitable as a beat source. The
    /// first window registered becomes the primary: its beat runs the full app
    /// tick. Re-registering the same window replaces its handle in place, so a
    /// resize round-trip keeps the primary slot it had.
    pub fn register_beat_handle(&mut self, window_id: WindowId, handle: HANDLE, credit_held: bool) {
        if let Some(entry) = self
            .beat_handles
            .iter_mut()
            .find(|b| b.window_id == window_id)
        {
            entry.handle = handle;
            entry.credit_held = credit_held;
            return;
        }
        self.beat_handles.push(BeatSource {
            window_id,
            handle,
            credit_held,
        });
    }

    pub fn unregister_beat_handle(&mut self, window_id: WindowId) {
        self.beat_handles.retain(|b| b.window_id != window_id);
    }

    /// The windows to wait on this pass: everything registered that is not
    /// already holding an unspent credit, tagged with whether it is the primary.
    fn beat_wait_list(&self) -> Vec<(WindowId, HANDLE, bool)> {
        self.beat_handles
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.credit_held)
            .map(|(i, b)| (b.window_id, b.handle, i == 0))
            .collect()
    }

    /// A wait on this window's waitable succeeded: we now hold one credit.
    pub fn take_beat_credit(&mut self, window_id: WindowId) {
        if let Some(b) = self
            .beat_handles
            .iter_mut()
            .find(|b| b.window_id == window_id)
        {
            b.credit_held = true;
        }
    }

    /// A frame was handed to `Present`: the credit is spent and the compositor
    /// will refill the semaphore when that frame retires.
    pub fn spend_beat_credit(&mut self, window_id: WindowId) {
        if let Some(b) = self
            .beat_handles
            .iter_mut()
            .find(|b| b.window_id == window_id)
        {
            b.credit_held = false;
        }
    }

    /// Whether a credit is already in hand for this window — if so, the paint
    /// must NOT wait again (that would take a second credit and cost a refresh).
    pub fn has_beat_credit(&self, window_id: WindowId) -> bool {
        self.beat_handles
            .iter()
            .any(|b| b.window_id == window_id && b.credit_held)
    }

    pub fn start_resize(&mut self) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe { SetTimer(None, 0, 8 as u32, Some(Self::timer_proc)) };
        self.timers[slot] = Win32Timer::Resize { win32_id: win32_id };
    }

    pub fn poll_start_drag_drop() {
        let items = with_win32_app(|app| app.start_dragging_items.take());
        if let Some(items) = items {
            with_win32_app(|app| {
                let slot = app.get_free_timer_slot();
                let win32_id = unsafe { SetTimer(None, 0, 8 as u32, Some(Self::timer_proc)) };
                app.timers[slot] = Win32Timer::DragDrop { win32_id: win32_id };
            });

            if items.len() > 1 {
                error!("multi-item drag/drop operation not supported");
            }
            match &items[0] {
                DragItem::FilePath { path, internal_id } => {
                    //log!("win32: about to drag path \"{}\" with internal ID {:?}", path, internal_id);

                    // only drag if something is there
                    if (path.len() > 0) || internal_id.is_some() {
                        // create COM IDataObject that hosts the drag item
                        let data_object: IDataObject = DragItemWindows(DragItem::FilePath {
                            path: path.clone(),
                            internal_id: internal_id.clone(),
                        })
                        .into();

                        // create COM IDropSource to indicate when to stop dragging
                        let drop_source: IDropSource = DropSource {}.into();

                        with_win32_app(|app| app.is_dragging_internal.replace(true));
                        let mut effect = DROPEFFECT(0);
                        match unsafe {
                            DoDragDrop(
                                &data_object,
                                &drop_source,
                                DROPEFFECT_COPY | DROPEFFECT_MOVE,
                                &mut effect,
                            )
                        } {
                            DRAGDROP_S_DROP => { /*log!("DoDragDrop: succesful")*/ }
                            DRAGDROP_S_CANCEL => { /*log!("DoDragDrop: canceled")*/ }
                            _ => {
                                log!("DoDragDrop: failed for some reason")
                            }
                        }
                        with_win32_app(|app| app.is_dragging_internal.replace(false));
                    }
                }
                _ => {
                    error!("Only DragItem::FilePath supported");
                }
            }
            with_win32_app(|app| {
                for slot in 0..app.timers.len() {
                    if let Win32Timer::DragDrop { win32_id } = app.timers[slot] {
                        app.timers[slot] = Win32Timer::Free;
                        unsafe {
                            KillTimer(None, win32_id).unwrap();
                        }
                    }
                }
            })
        }
    }

    pub fn start_signal_poll(&mut self) {
        let slot = self.get_free_timer_slot();
        let win32_id = unsafe { SetTimer(None, 0, 8 as u32, Some(Self::timer_proc)) };
        self.timers[slot] = Win32Timer::SignalPoll { win32_id: win32_id };
    }

    pub fn stop_resize(&mut self) {
        for slot in 0..self.timers.len() {
            if let Win32Timer::Resize { win32_id } = self.timers[slot] {
                self.timers[slot] = Win32Timer::Free;
                unsafe {
                    KillTimer(None, win32_id).unwrap();
                }
            }
        }
    }

    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
        self.start_dragging_items = Some(items);
    }

    pub fn time_now(&self) -> f64 {
        self.time.time_now()
    }

    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor.is_none() || self.current_cursor.unwrap() != cursor {
            let win32_cursor = match cursor {
                MouseCursor::Hidden => PCWSTR::null(),
                MouseCursor::Default => IDC_ARROW,
                MouseCursor::Crosshair => IDC_CROSS,
                MouseCursor::Hand => IDC_HAND,
                // Default to Hand for non-supported cursors, until we include our own custom cursor files.
                MouseCursor::Grab | MouseCursor::Grabbing => IDC_HAND,
                MouseCursor::Arrow => IDC_ARROW,
                MouseCursor::Move => IDC_SIZEALL,
                MouseCursor::Text => IDC_IBEAM,
                MouseCursor::Wait => IDC_ARROW,
                MouseCursor::Help => IDC_HELP,
                MouseCursor::NotAllowed => IDC_NO,

                MouseCursor::EResize => IDC_SIZEWE,
                MouseCursor::NResize => IDC_SIZENS,
                MouseCursor::NeResize => IDC_SIZENESW,
                MouseCursor::NwResize => IDC_SIZENWSE,
                MouseCursor::SResize => IDC_SIZENS,
                MouseCursor::SeResize => IDC_SIZENWSE,
                MouseCursor::SwResize => IDC_SIZENESW,
                MouseCursor::WResize => IDC_SIZEWE,

                MouseCursor::NsResize => IDC_SIZENS,
                MouseCursor::NeswResize => IDC_SIZENESW,
                MouseCursor::EwResize => IDC_SIZEWE,
                MouseCursor::NwseResize => IDC_SIZENWSE,

                MouseCursor::ColResize => IDC_SIZEWE,
                MouseCursor::RowResize => IDC_SIZENS,
            };
            self.current_cursor = Some(cursor);
            unsafe {
                if win32_cursor == PCWSTR::null() {
                    ShowCursor(false);
                } else {
                    SetCursor(Some(LoadCursorW(None, win32_cursor).unwrap()));
                    ShowCursor(true);
                }
            }
            //TODO
        }
    }
}

// reworked from winit windows platform https://github.com/rust-windowing/winit/blob/eventloop-2.0/src/platform_impl/windows/dpi.rs

type SetProcessDPIAware = unsafe extern "system" fn() -> BOOL;
type SetProcessDpiAwareness = unsafe extern "system" fn(value: PROCESS_DPI_AWARENESS) -> HRESULT;
type SetProcessDpiAwarenessContext =
    unsafe extern "system" fn(value: DPI_AWARENESS_CONTEXT) -> BOOL;
type GetDpiForWindow = unsafe extern "system" fn(hwnd: HWND) -> u32;
type AdjustWindowRectExForDpi = unsafe extern "system" fn(
    lp_rect: *mut crate::windows::Win32::Foundation::RECT,
    dw_style: u32,
    b_menu: BOOL,
    dw_ex_style: u32,
    dpi: u32,
) -> BOOL;
type GetDpiForMonitor = unsafe extern "system" fn(
    hmonitor: HMONITOR,
    dpi_type: MONITOR_DPI_TYPE,
    dpi_x: *mut u32,
    dpi_y: *mut u32,
) -> HRESULT;
type EnableNonClientDpiScaling = unsafe extern "system" fn(hwnd: HWND) -> BOOL;

// Helper function to dynamically load function pointer.
// `library` and `function` must be zero-terminated.
fn get_function_impl(library: &str, function: &str) -> FARPROC {
    // Library names we will use are ASCII so we can use the A version to avoid string conversion.

    let module = unsafe { LoadLibraryA(PCSTR::from_raw(library.as_ptr())) };
    if module.is_err() {
        return None;
    }

    let function_ptr =
        unsafe { GetProcAddress(module.unwrap(), PCSTR::from_raw(function.as_ptr())) };
    if function_ptr.is_none() {
        return None;
    }

    function_ptr
}

macro_rules! get_function {
    ( $ lib: expr, $ func: ident) => {
        get_function_impl(concat!($lib, '\0'), concat!(stringify!($func), '\0'))
            .map(|f| unsafe { mem::transmute::<_, $func>(f) })
    };
}

pub fn encode_wide(string: impl AsRef<OsStr>) -> Vec<u16> {
    string
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/*
pub fn post_signal_to_hwnd(hwnd:HWND, signal:Signal){
    unsafe{PostMessageW(
        hwnd,
        WM_USER,
        WPARAM(((signal.0.0)&0xffff_ffff) as usize),
        LPARAM(((signal.0.0>>32)&0xffff_ffff) as isize),
    )};
}
*/
pub struct DpiFunctions {
    get_dpi_for_window: Option<GetDpiForWindow>,
    adjust_window_rect_ex_for_dpi: Option<AdjustWindowRectExForDpi>,
    get_dpi_for_monitor: Option<GetDpiForMonitor>,
    enable_nonclient_dpi_scaling: Option<EnableNonClientDpiScaling>,
    set_process_dpi_awareness_context: Option<SetProcessDpiAwarenessContext>,
    set_process_dpi_awareness: Option<SetProcessDpiAwareness>,
    set_process_dpi_aware: Option<SetProcessDPIAware>,
}

const BASE_DPI: u32 = 96;

impl DpiFunctions {
    fn new() -> DpiFunctions {
        DpiFunctions {
            get_dpi_for_window: get_function!("user32.dll", GetDpiForWindow),
            adjust_window_rect_ex_for_dpi: get_function!("user32.dll", AdjustWindowRectExForDpi),
            get_dpi_for_monitor: get_function!("shcore.dll", GetDpiForMonitor),
            enable_nonclient_dpi_scaling: get_function!("user32.dll", EnableNonClientDpiScaling),
            set_process_dpi_awareness_context: get_function!(
                "user32.dll",
                SetProcessDpiAwarenessContext
            ),
            set_process_dpi_awareness: get_function!("shcore.dll", SetProcessDpiAwareness),
            set_process_dpi_aware: get_function!("user32.dll", SetProcessDPIAware),
        }
    }

    fn become_dpi_aware(&self) {
        unsafe {
            if let Some(set_process_dpi_awareness_context) = self.set_process_dpi_awareness_context
            {
                // We are on Windows 10 Anniversary Update (1607) or later.
                if set_process_dpi_awareness_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                    == FALSE
                {
                    // V2 only works with Windows 10 Creators Update (1703). Try using the older
                    // V1 if we can't set V2.
                    let _ =
                        set_process_dpi_awareness_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
                }
            } else if let Some(set_process_dpi_awareness) = self.set_process_dpi_awareness {
                // We are on Windows 8.1 or later.
                set_process_dpi_awareness(PROCESS_PER_MONITOR_DPI_AWARE).unwrap();
            } else if let Some(set_process_dpi_aware) = self.set_process_dpi_aware {
                // We are on Vista or later.
                set_process_dpi_aware().unwrap();
            }
        }
    }

    pub fn enable_non_client_dpi_scaling(&self, hwnd: HWND) {
        unsafe {
            if let Some(enable_nonclient_dpi_scaling) = self.enable_nonclient_dpi_scaling {
                let _ = enable_nonclient_dpi_scaling(hwnd);
            }
        }
    }

    /// DPI-aware frame insets for a zero client rect when available (Win10 1607+).
    /// Falls back to `AdjustWindowRectEx` on older systems.
    pub fn adjust_window_rect_ex(
        &self,
        hwnd: HWND,
        style: u32,
        ex_style: u32,
        rect: &mut crate::windows::Win32::Foundation::RECT,
    ) {
        unsafe {
            if let (Some(adjust), Some(get_dpi)) = (
                self.adjust_window_rect_ex_for_dpi,
                self.get_dpi_for_window,
            ) {
                let dpi = match get_dpi(hwnd) {
                    0 => BASE_DPI,
                    d => d,
                };
                let _ = adjust(rect, style, FALSE, ex_style, dpi);
                return;
            }
            let _ = crate::windows::Win32::UI::WindowsAndMessaging::AdjustWindowRectEx(
                rect,
                crate::windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
                false,
                crate::windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(ex_style),
            );
        }
    }

    pub fn system_dpi_factor(&self) -> f32 {
        unsafe {
            let hdc = GetDC(None);
            if hdc.is_invalid() {
                return 1.0;
            }
            GetDeviceCaps(Some(hdc), LOGPIXELSX) as f32 / BASE_DPI as f32
        }
    }
    /*
    pub fn get_monitor_dpi(hmonitor: HMONITOR) -> Option<u32> {
        unsafe {
            if let Some(GetDpiForMonitor) = *GET_DPI_FOR_MONITOR {
                // We are on Windows 8.1 or later.
                let mut dpi_x = 0;
                let mut dpi_y = 0;
                if GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == S_OK {
                    // MSDN says that "the values of *dpiX and *dpiY are identical. You only need to
                    // record one of the values to determine the DPI and respond appropriately".
                    // https://msdn.microsoft.com/en-us/library/windows/desktop/dn280510(v=vs.85).aspx
                    return Some(dpi_x as u32)
                }
            }
        }
        None
    }*/

    pub fn hwnd_dpi_factor(&self, hwnd: HWND) -> f32 {
        unsafe {
            let hdc = GetDC(Some(hwnd));
            if hdc.is_invalid() {
                panic!("`GetDC` returned null!");
            }
            let dpi = if let Some(get_dpi_for_window) = self.get_dpi_for_window {
                // We are on Windows 10 Anniversary Update (1607) or later.
                match get_dpi_for_window(hwnd) {
                    0 => BASE_DPI, // 0 is returned if hwnd is invalid
                    dpi => dpi as u32,
                }
            } else if let Some(get_dpi_for_monitor) = self.get_dpi_for_monitor {
                // We are on Windows 8.1 or later.
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                if monitor.is_invalid() {
                    BASE_DPI
                } else {
                    let mut dpi_x = 0;
                    let mut dpi_y = 0;
                    if get_dpi_for_monitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)
                        == S_OK
                    {
                        dpi_x as u32
                    } else {
                        BASE_DPI
                    }
                }
            } else {
                // We are on Vista or later.
                if IsProcessDPIAware() == TRUE {
                    // If the process is DPI aware, then scaling must be handled by the application using
                    // this DPI value.
                    GetDeviceCaps(Some(hdc), LOGPIXELSX) as u32
                } else {
                    // If the process is DPI unaware, then scaling is performed by the OS; we thus return
                    // 96 (scale factor 1.0) to prevent the window from being re-scaled by both the
                    // application and the WM.
                    BASE_DPI
                }
            };
            dpi as f32 / BASE_DPI as f32
        }
    }
}
