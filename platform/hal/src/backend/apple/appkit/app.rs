//! The application side of the shell: `NSApp` on the main thread, its loop, the ops sessions
//! send, the menu, cursors, pointer lock, dialogs and the per-window display links. Moved from
//! os/apple/macos/macos_app.rs (`MacosApp`) and the platform-op handlers of macos.rs.

use super::classes::Classes;
use super::ime::ImeKeyboard;
use super::keys;
use super::window::{self, focus_allowed, Win};
use super::{lock, Op, Shared};
use crate::backend::exec::OsCmd;
use crate::buffers::pack_drag_items;
use crate::types::*;
use makepad_apple_sys::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::os::raw::c_void;
use std::sync::atomic::Ordering;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopPerformBlock(rl: *mut c_void, mode: ObjcId, block: *const c_void);
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CAFrameRateRange {
    minimum: f32,
    maximum: f32,
    preferred: f32,
}

unsafe impl Encode for CAFrameRateRange {
    fn encode() -> Encoding {
        unsafe { Encoding::from_str("{CAFrameRateRange=fff}") }
    }
}

/// How long a requested frame may wait for the display link before it is delivered anyway (a
/// window that is occluded or in the Dock gets no vsync).
const FALLBACK_SECS: f64 = 0.1;

/// The system appearance as the main thread last read it: 0 unknown, 1 light, 2 dark.
static APPEARANCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Whether the system is in dark mode, as of the last look the main thread took (at start and
/// whenever the system announced a change); `None` before the window system is up. Readable
/// from any thread, so an app that is not on the main thread needs no AppKit call for it.
pub fn host_dark() -> Option<bool> {
    match APPEARANCE.load(Ordering::Acquire) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// Publishes the attached displays for `makepad_hal::hal_screens`.
pub(super) fn publish_screens() {
    let list: Vec<crate::ScreenInfo> = window::screens()
        .into_iter()
        .map(|s| crate::ScreenInfo {
            bounds: [s.bounds.x, s.bounds.y, s.bounds.w, s.bounds.h],
            work_area: [s.work_area.x, s.work_area.y, s.work_area.w, s.work_area.h],
            is_primary: s.is_primary,
        })
        .collect();
    *lock(&Shared::global().screens) = list;
}

/// Reads `NSApp.effectiveAppearance`, matching Aqua / DarkAqua (which also resolves the
/// accessibility contrast variants).
pub(super) fn read_appearance() {
    extern "C" {
        static NSAppearanceNameAqua: ObjcId;
        static NSAppearanceNameDarkAqua: ObjcId;
    }
    unsafe {
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let appearance: ObjcId = msg_send![ns_app, effectiveAppearance];
        let names = [NSAppearanceNameAqua, NSAppearanceNameDarkAqua];
        let choices: ObjcId = msg_send![class!(NSArray), arrayWithObjects: names.as_ptr() count: names.len()];
        let best: ObjcId = msg_send![appearance, bestMatchFromAppearancesWithNames: choices];
        if !best.is_null() {
            let dark: BOOL = msg_send![best, isEqualToString: NSAppearanceNameDarkAqua];
            APPEARANCE.store(if dark != NO { 2 } else { 1 }, Ordering::Release);
        }
    }
}

static EXIT_REQUESTED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// What only the main thread touches.
pub struct App {
    classes: OnceCell<Classes>,
    /// every window ever made, in order; closed ones stay allocated (the view and delegate keep
    /// pointers to them past `windowWillClose:`) and are flagged retired
    windows: RefCell<Vec<&'static Win>>,
    /// the windows' badges (`window_kind::BADGE`)
    badges: RefCell<Vec<super::badge::Badge>>,
    ime: RefCell<ImeKeyboard>,
    last_key_mod: Cell<u32>,
    /// the main menu consumed the key event being dispatched (`menuAction:` ran)
    menu_command_fired: Cell<bool>,
    menu_sid: Cell<u32>,
    pulse: Cell<ObjcId>,
    menu_delegate: Cell<ObjcId>,
    pasteboard: Cell<ObjcId>,
    nap: Cell<ObjcId>,
    marked_attributes: Cell<ObjcId>,
    cursor: Cell<u32>,
    cursors: RefCell<HashMap<u32, ObjcId>>,
    /// Whether the shown cursor was set directly, bypassing cursor rects, because a mouse button
    /// was held (cursor rects do not apply then).
    cursor_set_directly: Cell<bool>,
    // pointer lock (FPS mouse capture): the hardware cursor is frozen and hidden, positions are
    // synthesized from NSEvent deltas
    locked: Cell<bool>,
    lock_applied: Cell<bool>,
    pin_mode: Cell<bool>,
    virtual_mouse: Cell<Option<[f64; 2]>>,
    lock_pin: Cell<Option<[f64; 2]>>,
    pin_restore: Cell<Option<[f64; 2]>>,
    start: Cell<f64>,
    focus_hack_ran: Cell<bool>,
}

/// SAFETY: the app is only ever touched on the main thread.
struct MainOnly(App);
unsafe impl Sync for MainOnly {}
unsafe impl Send for MainOnly {}

static APP: std::sync::OnceLock<MainOnly> = std::sync::OnceLock::new();

pub fn app() -> &'static App {
    &APP.get_or_init(|| {
        MainOnly(App {
            classes: OnceCell::new(),
            windows: RefCell::new(Vec::new()),
            badges: RefCell::new(Vec::new()),
            ime: RefCell::new(ImeKeyboard::default()),
            last_key_mod: Cell::new(0),
            menu_command_fired: Cell::new(false),
            menu_sid: Cell::new(0),
            pulse: Cell::new(nil),
            menu_delegate: Cell::new(nil),
            pasteboard: Cell::new(nil),
            nap: Cell::new(nil),
            marked_attributes: Cell::new(nil),
            cursor: Cell::new(cursor::DEFAULT),
            cursors: RefCell::new(HashMap::new()),
            cursor_set_directly: Cell::new(false),
            locked: Cell::new(false),
            lock_applied: Cell::new(false),
            pin_mode: Cell::new(false),
            virtual_mouse: Cell::new(None),
            lock_pin: Cell::new(None),
            pin_restore: Cell::new(None),
            start: Cell::new(0.0),
            focus_hack_ran: Cell::new(false),
        })
    })
    .0
}

/// Seconds on the media clock (monotonic, the clock display links use).
pub fn time_now() -> f64 {
    unsafe { CACurrentMediaTime() }
}

pub fn note_exit_requested() {
    if let Ok(mut t) = EXIT_REQUESTED.lock() {
        *t = Some(std::time::Instant::now());
    }
}

// ------------------------------------------------------------------ the loop

/// Runs the process's main thread as the window system until [`stop_main_loop`]: waits until a
/// backend asks for windows, then runs `NSApp`'s event loop. Returns when told to stop (or at
/// once when the host stopped it before any window was wanted).
pub fn run_main_loop() {
    let shared = Shared::global();
    // Until a backend wants windows the main thread still runs its run loop: system frameworks
    // finish their work there (AVFoundation's player items only become ready through the main
    // queue), so offscreen sessions (hidden runs) get video, location etc. as windowed ones do.
    // `wake_idle_main` ends each wait when the run state changes.
    unsafe {
        let mut context = CFRunLoopSourceContext {
            version: 0, info: std::ptr::null_mut(), retain: None, release: None, copy_description: None,
            equal: None, hash: None, schedule: None, cancel: None, perform: Some(idle_wake_perform),
        };
        let source = CFRunLoopSourceCreate(std::ptr::null(), 0, &mut context);
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopDefaultMode);
        IDLE_WAKE.store(source as usize, Ordering::Release);
    }
    super::main_lag::start();
    loop {
        {
            let run = lock(&shared.run);
            if run.wanted {
                // the window system's loop serves the run loop from here on
                IDLE_WAKE.store(0, Ordering::Release);
                break;
            }
            if run.stop {
                drop(run);
                super::main_lag::finish();
                return;
            }
        }
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0e10, 1); }
    }
    unsafe {
        init();
        // the loop ends when `stop` is set: a posted event ends the wait for the next event
        while !stopping() {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
            drain_ops();
            if stopping() {
                let () = msg_send![pool, release];
                break;
            }
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let until: ObjcId = msg_send![class!(NSDate), distantFuture];
            let event: ObjcId = msg_send![
                ns_app,
                nextEventMatchingMask: NSEventMask::NSAnyEventMask as u64 | NSEventMask::NSEventMaskPressure as u64
                untilDate: until
                inMode: NSDefaultRunLoopMode
                dequeue: YES
            ];
            if event != nil {
                process_ns_event(event);
            }
            let () = msg_send![pool, release];
        }
        drain_ops();
        windows_gone_before_exit();
    }
    super::main_lag::finish();
}

/// The main thread's wake source while it waits for a backend to want windows.
static IDLE_WAKE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

extern "C" fn idle_wake_perform(_: *mut c_void) {}

/// Ends the main thread's wait in [`run_main_loop`] for a backend to want windows, so it looks
/// at the run state again (any thread; nothing to do once the window system runs).
pub(crate) fn wake_idle_main() {
    let source = IDLE_WAKE.load(Ordering::Acquire);
    if source != 0 {
        unsafe {
            CFRunLoopSourceSignal(source as CFRunLoopSourceRef);
            CFRunLoopWakeUp(CFRunLoopGetMain());
        }
    }
}

/// Ends [`run_main_loop`] (any thread).
pub fn stop_main_loop() {
    let shared = Shared::global();
    {
        let mut run = lock(&shared.run);
        run.stop = true;
        shared.run_cv.notify_all();
    }
    wake_idle_main();
    // an event ends the main loop's wait for the next event, which then sees the stop
    let ready = lock(&shared.run).ready;
    if ready {
        wake_event_loop();
    }
}

fn stopping() -> bool {
    let run = lock(&Shared::global().run);
    run.stop && run.keep_alive == 0
}

/// Posts an application-defined event, which ends the wait for the next event.
pub(super) fn wake_event_loop() {
    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let event: ObjcId = msg_send![
            class!(NSEvent),
            otherEventWithType: NSEventType::NSApplicationDefined
            location: NSPoint { x: 0., y: 0. }
            modifierFlags: 0u64
            timestamp: 0f64
            windowNumber: 0isize
            context: nil
            subtype: 0i16
            data1: 0isize
            data2: 0isize
        ];
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, postEvent: event atStart: false];
        let () = msg_send![pool, release];
    }
}

/// Runs `f` on the main thread in every run-loop mode, also inside OS nested loops (live
/// resize, menu tracking, modal panels), and also while another such block is itself in a
/// nested loop: a main dispatch queue would hold a serial lock there.
pub(super) fn run_on_main(f: impl Fn() + Send + 'static) {
    unsafe {
        let block = objc_block!(move || {
            f();
        });
        let main = CFRunLoopGetMain();
        CFRunLoopPerformBlock(main, NSRunLoopCommonModes, &block as *const _ as *const c_void);
        CFRunLoopWakeUp(main);
    }
}

/// Ops are waiting: a run-loop block applies them (in every mode, so also inside OS nested
/// loops), and an event ends the main loop's wait for the next event, which then applies them
/// itself (whichever comes first finds them; the other finds none).
pub(super) fn schedule_drain() {
    run_on_main(drain_ops);
    wake_event_loop();
}

/// Applies the ops sessions sent, in order.
fn drain_ops() {
    let ops = Shared::global().take_ops();
    for op in ops {
        let pool: ObjcId = unsafe { msg_send![class!(NSAutoreleasePool), new] };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(op)));
        if result.is_err() {
            eprintln!("makepad-hal: PANIC contained applying a window-system op; the shell goes on");
        }
        let () = unsafe { msg_send![pool, release] };
    }
}

unsafe fn init() {
    let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
    let a = app();
    a.start.set(time_now());
    let classes = Classes::new();
    objc_set_exception_preprocessor(log_objc_exception);
    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
    let delegate: ObjcId = msg_send![classes.app_delegate, new];
    // AppKit would also hand the command line's arguments to `application:openURLs:`; they
    // arrive through the app itself, so only the OS's own open requests come through
    let defaults: ObjcId = msg_send![class!(NSUserDefaults), standardUserDefaults];
    let no_open: ObjcId = msg_send![class!(NSDictionary), dictionaryWithObject: str_to_nsstring("NO") forKey: str_to_nsstring("NSTreatUnknownArgumentsAsOpen")];
    let () = msg_send![defaults, registerDefaults: no_open];
    let () = msg_send![ns_app, setDelegate: delegate];
    let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];
    a.pulse.set(msg_send![classes.pulse, new]);
    a.menu_delegate.set(msg_send![classes.menu_delegate, new]);
    a.pasteboard.set(msg_send![class!(NSPasteboard), generalPasteboard]);
    let _ = a.classes.set(classes);
    let () = msg_send![ns_app, finishLaunching];
    // a minimal app menu (Quit, Cmd+Q) for unbundled `cargo run` apps; an app's own menu
    // (`MenuSet`) replaces it
    install_quit_menu();
    begin_test_instance_activity();
    // the system appearance, now and whenever it changes
    read_appearance();
    let center: ObjcId = msg_send![class!(NSDistributedNotificationCenter), defaultCenter];
    let () = msg_send![center, addObserver: a.pulse.get() selector: sel!(appearanceChanged:) name: str_to_nsstring("AppleInterfaceThemeChangedNotification") object: nil];
    // the displays, now and whenever the arrangement changes
    publish_screens();
    let notes: ObjcId = msg_send![class!(NSNotificationCenter), defaultCenter];
    let () = msg_send![notes, addObserver: a.pulse.get() selector: sel!(screensChanged:) name: str_to_nsstring("NSApplicationDidChangeScreenParametersNotification") object: nil];
    let screen: ObjcId = msg_send![class!(NSScreen), mainScreen];
    let scale: f64 = if screen != nil { msg_send![screen, backingScaleFactor] } else { 1.0 };
    let () = msg_send![pool, release];
    Shared::global().publish_ready(scale as f32);
}

/// Holds an `ACTIVITY_USER_INITIATED` token so a no-focus (agent, test) instance is not
/// App-Napped during a long rest.
unsafe fn begin_test_instance_activity() {
    if focus_allowed() || app().nap.get() != nil {
        return;
    }
    let info: ObjcId = msg_send![class!(NSProcessInfo), processInfo];
    let reason = str_to_nsstring("makepad no-focus instance");
    let options = ACTIVITY_USER_INITIATED | ACTIVITY_IDLE_SYSTEM_SLEEP_DISABLED;
    let token: ObjcId = msg_send![info, beginActivityWithOptions: options reason: reason];
    let () = msg_send![token, retain];
    app().nap.set(token);
}

/// Foundation `NSActivityIdleSystemSleepDisabled` (`1ULL << 20`) and `NSActivityUserInitiated`.
const ACTIVITY_IDLE_SYSTEM_SLEEP_DISABLED: u64 = 1 << 20;
const ACTIVITY_USER_INITIATED: u64 = 0x00FFFFFF | ACTIVITY_IDLE_SYSTEM_SLEEP_DISABLED;

extern "C" {
    /// libobjc: the hook called with an exception object before it is thrown. The only place that
    /// still sees the reason when the unwind later meets a Rust frame and aborts.
    #[link_name = "objc_setExceptionPreprocessor"]
    fn objc_set_exception_preprocessor(f: extern "C" fn(ObjcId) -> ObjcId) -> Option<extern "C" fn(ObjcId) -> ObjcId>;
}

unsafe fn objc_text(obj: ObjcId) -> String {
    if obj.is_null() {
        return String::new();
    }
    let utf8: *const std::os::raw::c_char = msg_send![obj, UTF8String];
    if utf8.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
}

extern "C" fn log_objc_exception(exception: ObjcId) -> ObjcId {
    unsafe {
        let name: ObjcId = msg_send![exception, name];
        let reason: ObjcId = msg_send![exception, reason];
        eprintln!("makepad-hal: ObjC exception {}: {}", objc_text(name), objc_text(reason));
        let symbols: ObjcId = msg_send![exception, callStackSymbols];
        if !symbols.is_null() {
            let count: u64 = msg_send![symbols, count];
            for i in 0..count.min(16) {
                let line: ObjcId = msg_send![symbols, objectAtIndex: i];
                eprintln!("    {}", objc_text(line));
            }
        }
    }
    exception
}

/// The process is about to end: every visible window is ordered out and the window server told
/// NOW (`CATransaction flush`), so a closed window disappears while exit work runs.
/// MAKEPAD_EXIT_TRACE=1 prints the ms from the close/quit request to this point.
pub fn windows_gone_before_exit() {
    unsafe {
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let windows: ObjcId = msg_send![ns_app, windows];
        let count: usize = msg_send![windows, count];
        let mut ordered_out = 0;
        for i in 0..count {
            let window: ObjcId = msg_send![windows, objectAtIndex: i];
            let visible: BOOL = msg_send![window, isVisible];
            if visible == YES {
                let () = msg_send![window, orderOut: nil];
                ordered_out += 1;
            }
        }
        let () = msg_send![class!(CATransaction), flush];
        if std::env::var_os("MAKEPAD_EXIT_TRACE").is_some() {
            let since = EXIT_REQUESTED.lock().ok().and_then(|t| *t).map(|t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3);
            eprintln!("makepad-exit: windows gone ({} ordered out, {} already closed) {} ms after the close/quit request", ordered_out, count - ordered_out, since.map_or("?".to_string(), |ms| format!("{:.1}", ms)));
        }
    }
}

// ------------------------------------------------------------------ NSEvents

/// The window an NSEvent belongs to. Foreign windows land in this event loop too (the screen
/// capture overlay among them) and their delegates carry no window pointer: `None`.
unsafe fn window_of_ns_event(event: ObjcId) -> Option<&'static Win> {
    let window: ObjcId = msg_send![event, window];
    if window == nil {
        return None;
    }
    let delegate: ObjcId = msg_send![window, delegate];
    if delegate == nil {
        return None;
    }
    (*delegate).class().instance_variable("macos_window_ptr")?;
    let ptr: *mut c_void = *(*delegate).get_ivar("macos_window_ptr");
    Some(&*(ptr as *const Win))
}

unsafe fn process_ns_event(event: ObjcId) {
    let a = app();
    let kind: NSEventType = msg_send![event, type];
    // the receiving view records how its input context handled this event: marked text in
    // another view (or an inactive IME) must not consume it
    if matches!(kind, NSEventType::NSKeyDown) {
        a.ime.borrow_mut().begin_key_down();
    }
    // cleared so we can tell after `sendEvent:` whether the main menu took this event as a key
    // equivalent
    a.menu_command_fired.set(false);
    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
    let () = msg_send![ns_app, sendEvent: event];
    if kind as u64 == 21 {
        return;
    }
    match kind {
        NSEventType::NSKeyUp => {
            let native_key: u16 = msg_send![event, keyCode];
            if !a.ime.borrow_mut().key_up(native_key) {
                return;
            }
            if let (Some(key), Some(win)) = (keys::get_event_keycode(event), key_window(event)) {
                win.key(key.to_index(), false, keys::event_mods(event), false);
            }
        }
        NSEventType::NSKeyDown => {
            let native_key: u16 = msg_send![event, keyCode];
            let repeat: bool = msg_send![event, isARepeat];
            let forward = a.ime.borrow_mut().end_key_down(native_key, repeat);
            if a.menu_command_fired.get() || !forward {
                // an IME-owned press (Escape release included) must not dismiss a containing modal
                return;
            }
            let (Some(key), Some(win)) = (keys::get_event_keycode(event), key_window(event)) else { return };
            let mods = keys::event_mods(event);
            if (mods & (input::MOD_LOGO | input::MOD_CONTROL)) != 0 && key == makepad_key_code::KeyCode::KeyV {
                // the paste chord: the pasteboard's text arrives as an event, ahead of the key
                let string: ObjcId = msg_send![a.pasteboard.get(), stringForType: NSStringPboardType];
                if string != nil {
                    win.paste(&nsstring_to_string(string));
                }
            }
            win.key(key.to_index(), repeat, mods, true);
        }
        NSEventType::NSFlagsChanged => {
            let mods = keys::event_mods(event);
            let last = a.last_key_mod.replace(mods);
            let Some(win) = key_window(event) else { return };
            for (bit, key) in [
                (input::MOD_SHIFT, makepad_key_code::KeyCode::Shift),
                (input::MOD_ALT, makepad_key_code::KeyCode::Alt),
                (input::MOD_LOGO, makepad_key_code::KeyCode::Logo),
                (input::MOD_CONTROL, makepad_key_code::KeyCode::Control),
            ] {
                if (last & bit) != (mods & bit) {
                    win.key(key.to_index(), false, mods, mods & bit != 0);
                }
            }
        }
        NSEventType::NSScrollWheel => {
            let Some(win) = window_of_ns_event(event) else { return };
            let point: NSPoint = msg_send![event, locationInWindow];
            win.set_pointer(win.view_point(point));
            let dx: f64 = msg_send![event, scrollingDeltaX];
            let dy: f64 = msg_send![event, scrollingDeltaY];
            let precise: BOOL = msg_send![event, hasPreciseScrollingDeltas];
            // NSEventPhase bits: Began 1<<0, Stationary 1<<1, Changed 1<<2, Ended 1<<3,
            // Cancelled 1<<4, MayBegin 1<<5. `phase` covers the finger-driven part of a trackpad
            // gesture, `momentumPhase` the stream after lift-off; wheel mice report 0 for both.
            let phase_bits: u64 = msg_send![event, phase];
            let momentum_bits: u64 = msg_send![event, momentumPhase];
            let phase = if momentum_bits & ((1 << 3) | (1 << 4)) != 0 {
                input::scroll::MOMENTUM_ENDED
            } else if momentum_bits != 0 {
                input::scroll::MOMENTUM
            } else if phase_bits & ((1 << 0) | (1 << 5)) != 0 {
                input::scroll::BEGAN
            } else if phase_bits & ((1 << 1) | (1 << 2)) != 0 {
                input::scroll::CHANGED
            } else if phase_bits & (1 << 3) != 0 {
                input::scroll::ENDED
            } else if phase_bits & (1 << 4) != 0 {
                // cancelled: the system took over the gesture; abort without momentum, which
                // widgets treat as a reset when it arrives as `Began`
                input::scroll::BEGAN
            } else {
                input::scroll::NONE
            };
            let mods = keys::event_mods(event);
            if precise == YES {
                win.scroll([-dx, -dy], mods, true, phase);
            } else {
                win.scroll([-dx * 32., -dy * 32.], mods, false, phase);
            }
        }
        NSEventType::NSEventTypeMagnify => {
            let Some(win) = window_of_ns_event(event) else { return };
            let point: NSPoint = msg_send![event, locationInWindow];
            win.set_pointer(win.view_point(point));
            // `magnification` is the change since the previous magnify event; a cancelled
            // gesture ends like a lifted one: the zoom stays where it got
            let magnification: f64 = msg_send![event, magnification];
            let phase_bits: u64 = msg_send![event, phase];
            let phase = if phase_bits & ((1 << 0) | (1 << 5)) != 0 {
                input::pinch::BEGIN
            } else if phase_bits & ((1 << 3) | (1 << 4)) != 0 {
                input::pinch::END
            } else {
                input::pinch::UPDATE
            };
            win.pinch(1.0 + magnification, phase, keys::event_mods(event));
        }
        _ => (),
    }
}

/// The window key events go to: the event's own, else the key window, else the first live one.
unsafe fn key_window(event: ObjcId) -> Option<&'static Win> {
    if let Some(win) = window_of_ns_event(event) {
        return Some(win);
    }
    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
    let key: ObjcId = msg_send![ns_app, keyWindow];
    let windows = app().windows.borrow();
    windows.iter().copied().find(|w| !w.retired.get() && w.window == key).or_else(|| windows.iter().copied().find(|w| !w.retired.get()))
}

// ------------------------------------------------------------------ App methods

impl App {
    pub fn classes(&self) -> &Classes {
        self.classes.get().expect("shell classes")
    }

    pub fn add_window(&self, win: &'static Win) {
        self.windows.borrow_mut().push(win);
    }

    /// A window is gone (closing or closed): its display link and fallback timer go with it.
    pub fn window_gone(&self, win: &Win) {
        self.remove_badges(|b| std::ptr::eq(b.parent, win));
        unsafe {
            let link = win.link.replace(nil);
            if link != nil {
                let () = msg_send![link, invalidate];
                let () = msg_send![link, release];
            }
            let timer = win.fallback.replace(nil);
            if timer != nil {
                let () = msg_send![timer, invalidate];
            }
        }
        win.want_frame.set(false);
    }

    fn remove_badges(&self, which: impl Fn(&super::badge::Badge) -> bool) {
        let mut badges = self.badges.borrow_mut();
        let mut i = 0;
        while i < badges.len() {
            if which(&badges[i]) {
                badges.swap_remove(i).remove();
            } else {
                i += 1;
            }
        }
    }

    pub(super) fn window_view(&self, sid: u32, id: Id) -> Option<ObjcId> {
        self.find(sid, id).map(|w| w.view)
    }

    fn find(&self, sid: u32, id: Id) -> Option<&'static Win> {
        self.windows.borrow().iter().copied().find(|w| !w.retired.get()
            && ((w.sid.get() == sid && w.id.get() == id) || (w.owner_sid == sid && w.owner_id == id)))
    }

    pub fn first_window(&self) -> Option<&'static Win> {
        self.first_live()
    }

    fn first_live(&self) -> Option<&'static Win> {
        self.windows.borrow().iter().copied().find(|w| !w.retired.get())
    }

    /// `applicationShouldTerminate:`: each session with a window is asked to quit; true when
    /// there is anyone to ask (the termination is cancelled, the app's `Quit` ends it).
    pub fn ask_sessions_to_quit(&self) -> bool {
        let mut sids: Vec<u32> = self.windows.borrow().iter().filter(|w| !w.retired.get()).map(|w| w.sid.get()).collect();
        sids.sort();
        sids.dedup();
        for sid in &sids {
            Shared::global().push(*sid, |q| q.lifecycle(lifecycle::QUIT_REQUESTED));
        }
        !sids.is_empty()
    }

    /// Files and URLs the OS asks the app to open go to the newest session.
    pub fn send_app_open(&self, items: &[(u32, String)]) {
        let Some(sid) = self.windows.borrow().iter().rev().find(|w| !w.retired.get()).map(|w| w.sid.get()).or(Some(self.menu_sid.get()).filter(|s| *s != 0)) else { return };
        let list: Vec<(u32, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
        let payload = pack_drag_items(&list);
        Shared::global().push(sid, |q| q.app_open(items.len() as u32, &payload));
    }

    pub fn menu_command(&self, cmd: u64) {
        self.menu_command_fired.set(true);
        let sid = self.menu_sid.get();
        if sid != 0 {
            Shared::global().push(sid, |q| q.menu_cmd(cmd));
        }
    }

    /// The first pointer motion of a window brings the app to the front, as a launch from a
    /// terminal or by an agent otherwise stays behind: activation goes through the Dock, which
    /// macOS lets take the focus from another app. A no-focus process never does.
    pub fn startup_focus_hack(&self) {
        if self.focus_hack_ran.replace(true) || !focus_allowed() {
            return;
        }
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let active: bool = msg_send![ns_app, isActive];
            if active {
                return;
            }
            let dock_array: ObjcId = msg_send![class!(NSRunningApplication), runningApplicationsWithBundleIdentifier: str_to_nsstring("com.apple.dock")];
            let dock_count: u64 = msg_send![dock_array, count];
            if dock_count == 0 {
                return;
            }
            let dock: ObjcId = msg_send![dock_array, objectAtIndex: 0];
            let _: BOOL = msg_send![dock, activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps];
            let me: ObjcId = msg_send![class!(NSRunningApplication), currentApplication];
            let () = msg_send![me, activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps];
        }
    }

    pub fn marked_text_attributes(&self) -> ObjcId {
        if self.marked_attributes.get() == nil {
            unsafe {
                let names = [str_to_nsstring("NSMarkedClauseSegment"), str_to_nsstring("NSGlyphInfo")];
                let array: ObjcId = msg_send![class!(NSArray), arrayWithObjects: names.as_ptr() count: names.len()];
                let () = msg_send![array, retain];
                self.marked_attributes.set(array);
            }
        }
        self.marked_attributes.get()
    }

    pub fn ime_composition_changed(&self, view: usize) {
        self.ime.borrow_mut().composition_changed(view);
    }
    pub fn ime_command(&self, view: usize, cancel: bool) {
        self.ime.borrow_mut().command(view, cancel);
    }
    pub fn ime_begin_input_context(&self, view: usize, marked: bool) {
        self.ime.borrow_mut().begin_input_context(view, marked);
    }
    pub fn ime_end_input_context(&self, view: usize, handled: bool, marked: bool) {
        self.ime.borrow_mut().end_input_context(view, handled, marked);
    }

    // ---- cursor

    /// The NSCursor of the current cursor (cached and retained: AppKit must never be handed one
    /// we do not own), and which cursor it is.
    pub fn current_cursor_object(&self) -> (u32, ObjcId) {
        let which = self.cursor.get();
        let mut cursors = self.cursors.borrow_mut();
        let id = *cursors.entry(which).or_insert_with(|| {
            let id = load_cursor(which);
            if !id.is_null() {
                unsafe {
                    let _: ObjcId = msg_send![id, retain];
                }
            }
            id
        });
        (which, id)
    }

    fn set_cursor(&self, which: u32) {
        if self.cursor.replace(which) != which {
            for win in self.windows.borrow().iter().filter(|w| !w.retired.get()) {
                unsafe {
                    let _: () = msg_send![win.window, invalidateCursorRectsForView: win.view];
                }
            }
            // Cursor rects don't apply while a mouse button is held, so a visible cursor requested
            // then, and the first one requested after the release, is set directly.
            unsafe {
                let buttons: u64 = msg_send![class!(NSEvent), pressedMouseButtons];
                if buttons != 0 || self.cursor_set_directly.get() {
                    let (current, native) = self.current_cursor_object();
                    if current != cursor::HIDDEN && !native.is_null() {
                        let _: () = msg_send![native, set];
                        self.cursor_set_directly.set(buttons != 0);
                    }
                }
            }
        }
    }

    // ---- pointer lock

    /// Applies or suspends the lock's PHYSICAL effects, in the proven order. Enable: hide, warp to
    /// the window's centre (while still associated: warping a disassociated cursor stalls event
    /// delivery), disassociate. Disable: re-associate, unhide.
    fn apply_pointer_lock_effects(&self, on: bool, win: Option<&Win>) {
        // only an ACTIVE app may take the user's pointer: a bridge-driven, unfocused instance
        // must never hide and pin the cursor the user is using elsewhere
        if on {
            let active: bool = unsafe {
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                msg_send![ns_app, isActive]
            };
            if !active {
                self.lock_applied.set(false);
                return;
            }
        }
        if self.lock_applied.get() == on {
            return;
        }
        self.lock_applied.set(on);
        // after every CGWarpMouseCursorPosition macOS suppresses local hardware mouse events for
        // 0.25 s by default: moving locked mouse had rolling windows where clicks reached no app
        unsafe {
            let _ = CGSetLocalEventsSuppressionInterval(0.0);
        }
        self.virtual_mouse.set(None);
        unsafe {
            if on {
                let () = msg_send![class!(NSCursor), hide];
                let target = win.or_else(|| self.first_live());
                if let Some(win) = target {
                    let frame: NSRect = msg_send![win.window, frame];
                    // pin mid-way into the window's portion on the PRIMARY screen: never the seam
                    // or an edge pixel (a cursor frozen on the screen edge has its clicks eaten)
                    let screens: ObjcId = msg_send![class!(NSScreen), screens];
                    let primary: ObjcId = msg_send![screens, firstObject];
                    let visible: NSRect = if primary != nil { msg_send![primary, visibleFrame] } else { frame };
                    let lo_x = frame.origin.x.max(visible.origin.x);
                    let hi_x = (frame.origin.x + frame.size.width).min(visible.origin.x + visible.size.width);
                    let lo_y = frame.origin.y.max(visible.origin.y);
                    let hi_y = (frame.origin.y + frame.size.height).min(visible.origin.y + visible.size.height);
                    let gx = if lo_x < hi_x { (lo_x + hi_x) * 0.5 } else { frame.origin.x + frame.size.width * 0.5 };
                    let gy = if lo_y < hi_y { (lo_y + hi_y) * 0.5 } else { frame.origin.y + frame.size.height * 0.5 };
                    let sframe: NSRect = msg_send![primary, frame];
                    let _ = CGWarpMouseCursorPosition(NSPoint { x: gx, y: sframe.size.height - gy });
                    self.lock_pin.set(Some([gx, gy]));
                }
                CGAssociateMouseAndMouseCursorPosition(0);
            } else {
                CGAssociateMouseAndMouseCursorPosition(1);
                let () = msg_send![class!(NSCursor), unhide];
            }
        }
    }

    pub fn pointer_locked(&self) -> bool {
        self.locked.get()
    }

    /// Focus gained: macOS re-associates the cursor on deactivation, so a locked pointer
    /// re-applies its physical lock.
    pub fn focus_gained(&self) {
        if self.locked.get() {
            self.apply_pointer_lock_effects(true, None);
        }
    }

    /// Focus left the window: the lock is suspended, then released altogether. A lock held past
    /// focus loss left the user with no mouse; the game re-locks on its next click.
    pub fn focus_lost(&self, _win: &Win) {
        if self.locked.get() {
            self.apply_pointer_lock_effects(false, None);
        }
        if self.locked.get() || self.lock_applied.get() {
            self.locked.set(false);
            self.lock_applied.set(false);
            self.lock_pin.set(None);
            self.pin_restore.set(None);
            self.pin_mode.set(false);
            self.apply_pointer_lock_effects(false, None);
        }
    }

    /// The pointer-lock transform every mouse move takes. Under a scrub pin the delta integrates
    /// into an unbounded virtual position (the drag owner needs continuous positions); under a
    /// game lock the position stays pinned and the motion rides the delta.
    pub fn locked_mouse_transform(&self, raw: [f64; 2], delta: [f64; 2], seed: [f64; 2]) -> ([f64; 2], [f64; 2]) {
        if !self.locked.get() {
            return (raw, [0.0, 0.0]);
        }
        if self.pin_mode.get() {
            let mut v = self.virtual_mouse.get().unwrap_or(seed);
            v[0] += delta[0];
            v[1] += delta[1];
            self.virtual_mouse.set(Some(v));
            (v, delta)
        } else {
            (self.virtual_mouse.get().unwrap_or_else(|| {
                self.virtual_mouse.set(Some(seed));
                seed
            }), delta)
        }
    }

    /// Widget-scoped pointer pin (value scrubbing): the lock machinery, but the cursor pins AT ITS
    /// CURRENT POSITION and is restored there on release.
    fn set_pointer_pin(&self, on: bool) {
        unsafe {
            if on {
                if self.locked.get() || self.lock_applied.get() {
                    return; // an FPS-style lock already holds the pointer
                }
                let _ = CGSetLocalEventsSuppressionInterval(0.0);
                self.virtual_mouse.set(None);
                self.locked.set(true);
                self.lock_applied.set(true);
                self.pin_mode.set(true);
                let loc: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                self.lock_pin.set(Some([loc.x, loc.y]));
                self.pin_restore.set(Some([loc.x, loc.y]));
                let () = msg_send![class!(NSCursor), hide];
                CGAssociateMouseAndMouseCursorPosition(0);
            } else {
                if !self.locked.get() && !self.lock_applied.get() {
                    self.pin_restore.set(None);
                    return;
                }
                self.locked.set(false);
                self.lock_applied.set(false);
                self.pin_mode.set(false);
                self.lock_pin.set(None);
                CGAssociateMouseAndMouseCursorPosition(1);
                if let Some(restore) = self.pin_restore.take() {
                    let screens: ObjcId = msg_send![class!(NSScreen), screens];
                    let primary: ObjcId = msg_send![screens, firstObject];
                    let sframe: NSRect = msg_send![primary, frame];
                    let _ = CGWarpMouseCursorPosition(NSPoint { x: restore[0], y: sframe.size.height - restore[1] });
                }
                let () = msg_send![class!(NSCursor), unhide];
            }
        }
    }

    /// The physical button-up ends a scrub pin at the platform layer.
    pub fn end_pin_on_release(&self) {
        if self.pin_mode.get() {
            self.set_pointer_pin(false);
        }
    }

    /// Per frame while locked: force the hardware cursor back onto the pin and re-assert the
    /// disassociation. Never while the app is inactive.
    fn repin_pointer(&self) {
        if !self.locked.get() || !self.lock_applied.get() {
            return;
        }
        let active: bool = unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            msg_send![ns_app, isActive]
        };
        let Some(pin) = self.lock_pin.get() else { return };
        if !active {
            return;
        }
        unsafe {
            let loc: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let drift = (loc.x - pin[0]).abs() + (loc.y - pin[1]).abs();
            if drift > 2.0 {
                let screens: ObjcId = msg_send![class!(NSScreen), screens];
                let primary: ObjcId = msg_send![screens, firstObject];
                let sframe: NSRect = msg_send![primary, frame];
                let _ = CGWarpMouseCursorPosition(NSPoint { x: pin[0], y: sframe.size.height - pin[1] });
                CGAssociateMouseAndMouseCursorPosition(0);
            }
        }
    }
}

fn load_cursor(which: u32) -> ObjcId {
    match which {
        cursor::ARROW | cursor::DEFAULT | cursor::HIDDEN => load_native_cursor("arrowCursor"),
        cursor::HAND => load_native_cursor("pointingHandCursor"),
        cursor::TEXT => load_native_cursor("IBeamCursor"),
        cursor::NOT_ALLOWED => load_native_cursor("operationNotAllowedCursor"),
        cursor::CROSSHAIR => load_native_cursor("crosshairCursor"),
        cursor::GRAB => load_native_cursor("openHandCursor"),
        cursor::GRABBING => load_native_cursor("closedHandCursor"),
        cursor::E_RESIZE => load_native_cursor("resizeRightCursor"),
        cursor::N_RESIZE => load_native_cursor("resizeUpCursor"),
        cursor::W_RESIZE => load_native_cursor("resizeLeftCursor"),
        cursor::S_RESIZE => load_native_cursor("resizeDownCursor"),
        cursor::NE_RESIZE => load_frame_resize_cursor(1 | 8, "_windowResizeNorthEastCursor"),
        cursor::NW_RESIZE => load_frame_resize_cursor(1 | 2, "_windowResizeNorthWestCursor"),
        cursor::SE_RESIZE => load_frame_resize_cursor(4 | 8, "_windowResizeSouthEastCursor"),
        cursor::SW_RESIZE => load_frame_resize_cursor(4 | 2, "_windowResizeSouthWestCursor"),
        cursor::EW_RESIZE | cursor::COL_RESIZE => load_native_cursor("resizeLeftRightCursor"),
        cursor::NS_RESIZE | cursor::ROW_RESIZE => load_native_cursor("resizeUpDownCursor"),
        cursor::HELP => load_undocumented_cursor("_helpCursor"),
        cursor::NESW_RESIZE => load_frame_resize_cursor(1 | 8, "_windowResizeNorthEastSouthWestCursor"),
        cursor::NWSE_RESIZE => load_frame_resize_cursor(4 | 8, "_windowResizeNorthWestSouthEastCursor"),
        // the wrong semantics for `Wait`, but what Safari and Chrome use
        cursor::WAIT => load_undocumented_cursor("busyButClickableCursor"),
        cursor::MOVE => load_webkit_cursor("move"),
        // the cursors macOS has no counterpart for
        _ => load_native_cursor("arrowCursor"),
    }
}

fn load_native_cursor(cursor_name: &str) -> ObjcId {
    let sel = Sel::register(cursor_name);
    unsafe { msg_send![class!(NSCursor), performSelector: sel] }
}

fn load_undocumented_cursor(cursor_name: &str) -> ObjcId {
    unsafe {
        let class = class!(NSCursor);
        let sel = Sel::register(cursor_name);
        let available: BOOL = msg_send![class, respondsToSelector: sel];
        if available == YES {
            let id: ObjcId = msg_send![class, performSelector: sel];
            if !id.is_null() {
                return id;
            }
        }
        load_native_cursor("arrowCursor")
    }
}

fn load_frame_resize_cursor(position: usize, legacy_name: &str) -> ObjcId {
    unsafe {
        let class = class!(NSCursor);
        let available: BOOL = msg_send![class, respondsToSelector: sel!(frameResizeCursorFromPosition:inDirections:)];
        if available == YES {
            // public since macOS 15: NSCursorFrameResizeDirectionsAll = 3
            let id: ObjcId = msg_send![class, frameResizeCursorFromPosition: position inDirections: 3usize];
            if !id.is_null() {
                return id;
            }
        }
    }
    load_undocumented_cursor(legacy_name)
}

fn load_webkit_cursor(cursor_name_str: &str) -> ObjcId {
    unsafe {
        static CURSOR_ROOT: &str = "/System/Library/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/Resources/cursors";
        let cursor_root = str_to_nsstring(CURSOR_ROOT);
        let cursor_name = str_to_nsstring(cursor_name_str);
        let cursor_path: ObjcId = msg_send![cursor_root, stringByAppendingPathComponent: cursor_name];
        let pdf_path: ObjcId = msg_send![cursor_path, stringByAppendingPathComponent: str_to_nsstring("cursor.pdf")];
        let info_path: ObjcId = msg_send![cursor_path, stringByAppendingPathComponent: str_to_nsstring("info.plist")];
        let ns_image: ObjcId = msg_send![class!(NSImage), alloc];
        let ns_image: ObjcId = msg_send![ns_image, initByReferencingFile: pdf_path];
        let info: ObjcId = msg_send![class!(NSDictionary), dictionaryWithContentsOfFile: info_path];
        if ns_image == nil || info == nil {
            return load_native_cursor("arrowCursor");
        }
        let x: ObjcId = msg_send![info, valueForKey: str_to_nsstring("hotx")];
        let y: ObjcId = msg_send![info, valueForKey: str_to_nsstring("hoty")];
        let point = NSPoint { x: msg_send![x, doubleValue], y: msg_send![y, doubleValue] };
        let cursor: ObjcId = msg_send![class!(NSCursor), alloc];
        msg_send![cursor, initWithImage: ns_image hotSpot: point]
    }
}

// ------------------------------------------------------------------ ops

fn apply(op: Op) {
    match op {
        Op::Host(f) => f(),
        Op::Lend { host, session, alias } => {
            if let Some(win) = app().windows.borrow().iter().find(|w| w.owner_sid == crate::session::HOST && w.owner_id == host && !w.retired.get()) {
                win.lend(session, alias);
            }
        }
        Op::Reveal => {
            for win in app().windows.borrow().iter().filter(|w| !w.retired.get()) {
                unsafe {
                    let () = msg_send![win.window, setAlphaValue: 1.0f64];
                    let () = msg_send![win.window, setIgnoresMouseEvents: NO];
                }
            }
        }
        Op::CloseSession(sid) => {
            app().remove_badges(|b| b.sid == sid);
            let windows: Vec<&'static Win> = app().windows.borrow().iter().copied().filter(|w| w.sid.get() == sid).collect();
            for win in windows {
                win.close_silently();
            }
            if app().menu_sid.get() == sid {
                app().menu_sid.set(0);
            }
        }
        Op::Cmd(sid, cmd, _) => command(sid, cmd),
    }
}

fn command(sid: u32, cmd: OsCmd) {
    let a = app();
    let window_of = |id: Id| a.find(sid, id);
    match cmd {
        OsCmd::WindowCreate { id, kind, parent, geom } if kind & 0xff == window_kind::BADGE => {
            if let Some(parent) = window_of(parent) {
                a.badges.borrow_mut().push(super::badge::Badge::create(sid, id, parent, geom));
            }
        }
        OsCmd::WindowClose { id } if a.badges.borrow().iter().any(|b| b.sid == sid && b.id == id) => {
            a.remove_badges(|b| b.sid == sid && b.id == id);
        }
        OsCmd::WindowGeom { id, geom } if a.badges.borrow().iter().any(|b| b.sid == sid && b.id == id) => {
            for b in a.badges.borrow().iter().filter(|b| b.sid == sid && b.id == id) {
                b.set_geom(geom);
            }
        }
        OsCmd::WindowCreate { id, kind, parent, geom } => {
            let parent = if parent.is_none() { None } else { window_of(parent) };
            let win = Win::create(sid, id, kind, parent, geom);
            // the window exists: the app hears of its geometry, and may ask for frames
            win.send_geom();
        }
        OsCmd::WindowClose { id } => {
            if let Some(win) = window_of(id) {
                unsafe {
                    let () = msg_send![win.window, close];
                }
            }
        }
        OsCmd::WindowGeom { id, geom } => {
            if let Some(win) = window_of(id) {
                win.set_geom(geom);
                win.send_geom();
            }
        }
        OsCmd::WindowState { id, state } => {
            if let Some(win) = window_of(id) {
                win.set_state(state);
            }
        }
        OsCmd::WindowTitle { id, title } => {
            if let Some(win) = window_of(id) {
                win.set_title(&title);
            }
        }
        OsCmd::WindowVisuals { id, visuals, params } => {
            if let Some(win) = window_of(id) {
                win.set_visuals(visuals, params[0]);
            }
        }
        OsCmd::WindowTopmost { id, on } => {
            if let Some(win) = window_of(id) {
                win.set_topmost(on);
            }
        }
        OsCmd::WindowButtons { id, show } => {
            if let Some(win) = window_of(id) {
                win.set_buttons_visible(show);
            }
        }
        OsCmd::WindowButtonsCenter { id, y_center } => {
            if let Some(win) = window_of(id) {
                win.center_window_buttons(y_center as f64);
            }
        }
        OsCmd::WindowDpiOverride { id, dpi_override } => {
            if let Some(win) = window_of(id) {
                win.set_dpi_override(dpi_override);
            }
        }
        OsCmd::Cursor { cursor, .. } => a.set_cursor(cursor),
        OsCmd::PointerLock { id, mode } => {
            let win = window_of(id);
            match mode {
                pointer_lock::LOCK => {
                    a.locked.set(true);
                    a.apply_pointer_lock_effects(true, win);
                }
                pointer_lock::UNLOCK => {
                    a.locked.set(false);
                    a.apply_pointer_lock_effects(false, win);
                }
                pointer_lock::PIN => a.set_pointer_pin(true),
                pointer_lock::UNPIN => a.set_pointer_pin(false),
                pointer_lock::REPIN => a.repin_pointer(),
                _ => {}
            }
        }
        OsCmd::ShowInDock { on } => unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let policy = if on { NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular } else { NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory };
            let () = msg_send![ns_app, setActivationPolicy: policy as i64];
        },
        OsCmd::ActivateApp => unsafe {
            // a process launched to stay out of the user's way stays out
            if !focus_allowed() {
                return;
            }
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![ns_app, activateIgnoringOtherApps: YES];
            let windows: Vec<&'static Win> = a.windows.borrow().iter().copied().filter(|w| !w.retired.get()).collect();
            for win in windows {
                let miniaturized: BOOL = msg_send![win.window, isMiniaturized];
                if miniaturized != NO {
                    let () = msg_send![win.window, deminiaturize: nil];
                }
                let () = msg_send![win.window, orderFrontRegardless];
            }
        },
        OsCmd::ImeShow { id, area, .. } => {
            if let Some(win) = window_of(id) {
                win.set_ime(true, area.map(|v| v as f64));
            }
        }
        OsCmd::ImeHide { id } => {
            if let Some(win) = window_of(id) {
                win.set_ime(false, [0.0; 4]);
            }
        }
        OsCmd::AppIcon { w, h, rgba } => set_application_icon(w, h, &rgba),
        OsCmd::ImeSync { .. } | OsCmd::SelectionSet { .. } | OsCmd::ShareText { .. } | OsCmd::ClipboardActions { .. } | OsCmd::ChromelessWhenMax { .. } | OsCmd::MobileUi { .. } | OsCmd::BackUnhandled | OsCmd::SelectionHandles { .. } => {}
        OsCmd::ClipRead { req, .. } => {
            let content = super::clipboard::read().pack();
            Shared::global().push(sid, |q| {
                q.payload_size(content.len() as u32);
                q.clipboard(req, true, &content);
            });
        }
        OsCmd::ClipSet { text } => unsafe {
            let pasteboard = a.pasteboard.get();
            let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
            let () = msg_send![pasteboard, declareTypes: array owner: nil];
            let () = msg_send![pasteboard, setString: str_to_nsstring(&text) forType: NSStringPboardType];
        },
        OsCmd::DragStart { id, items } => {
            if let Some(win) = window_of(id) {
                if !win.start_external_drag(&items) {
                    eprintln!("[hal] could not start external file drag");
                    // a native session ends with its source callback; a rejected start needs the
                    // same completion signal so application gesture guards recover at once
                    win.drag_drop([-100000.0, -100000.0], false, &[]);
                }
            }
        }
        OsCmd::MenuSet { items } => {
            a.menu_sid.set(sid);
            update_menu(&items);
        }
        OsCmd::FileDialog { req, kind, title, location, filename, filters } => file_dialog(sid, req, kind, &title, &location, &filename, &filters),
        OsCmd::DragReply { id, to, answer } => {
            if let Some(win) = window_of(id) {
                match to {
                    input::reply::QUERY => win.caption_answer(answer),
                    _ => win.drag_answer.set(answer),
                }
            }
        }
        OsCmd::FrameRequest { id } => {
            if let Some(win) = window_of(id) {
                request_frame(win);
            }
        }
    }
}

/// The Dock icon of an unbundled app (RGBA8 rows).
fn set_application_icon(w: u32, h: u32, rgba: &[u8]) {
    let (width, height) = (w as usize, h as usize);
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return;
    }
    unsafe {
        let bitmap_rep: ObjcId = msg_send![class!(NSBitmapImageRep), alloc];
        let bitmap_rep: ObjcId = msg_send![bitmap_rep,
            initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
            pixelsWide: width as i64
            pixelsHigh: height as i64
            bitsPerSample: 8i64
            samplesPerPixel: 4i64
            hasAlpha: YES
            isPlanar: NO
            colorSpaceName: str_to_nsstring("NSDeviceRGBColorSpace")
            bytesPerRow: (width * 4) as i64
            bitsPerPixel: 32i64
        ];
        if bitmap_rep == nil {
            return;
        }
        let bitmap_data: *mut u8 = msg_send![bitmap_rep, bitmapData];
        if !bitmap_data.is_null() {
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), bitmap_data, width * height * 4);
        }
        let ns_image: ObjcId = msg_send![class!(NSImage), alloc];
        let ns_image: ObjcId = msg_send![ns_image, initWithSize: NSSize { width: width as f64, height: height as f64 }];
        let () = msg_send![ns_image, addRepresentation: bitmap_rep];
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, setApplicationIconImage: ns_image];
    }
}

// ------------------------------------------------------------------ frame pacing

/// The app has work for the window: a frame is announced at the next vsync. The display link is
/// unpaused for it and paused again once the frame was announced, so an idle app costs nothing.
fn request_frame(win: &'static Win) {
    unsafe {
        win.want_frame.set(true);
        let mut link = win.link.get();
        if link == nil {
            // NSView.displayLink(target:selector:), macOS 14
            let responds: bool = msg_send![win.view, respondsToSelector: sel!(displayLinkWithTarget: selector:)];
            if responds {
                link = msg_send![win.view, displayLinkWithTarget: app().pulse.get() selector: sel!(receivedDisplayLink:)];
                if link != nil {
                    // an unconstrained link lets the system pick the rate and it throttles a
                    // "static" window to 30 Hz: pin the range to the panel's maximum
                    let responds: bool = msg_send![link, respondsToSelector: sel!(setPreferredFrameRateRange:)];
                    if responds {
                        let screen: ObjcId = msg_send![win.window, screen];
                        let maximum: isize = if screen != nil { msg_send![screen, maximumFramesPerSecond] } else { 60 };
                        let fps = maximum.max(1) as f32;
                        let () = msg_send![link, setPreferredFrameRateRange: CAFrameRateRange { minimum: fps, maximum: fps, preferred: fps }];
                    }
                    // our own reference, released after `invalidate`: the view's is gone once
                    // AppKit closes the window
                    let () = msg_send![link, retain];
                    let runloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
                    let () = msg_send![link, addToRunLoop: runloop forMode: NSRunLoopCommonModes];
                    win.link.set(link);
                }
            }
        }
        if link != nil {
            let () = msg_send![link, setPaused: NO];
        }
        // a window that gets no vsync (occluded, in the Dock) is announced anyway
        if win.fallback.get() == nil {
            let timer: ObjcId = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: if link != nil { FALLBACK_SECS } else { 1.0 / 60.0 }
                target: app().pulse.get()
                selector: sel!(receivedFallback:)
                userInfo: nil
                repeats: NO
            ];
            let runloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![runloop, addTimer: timer forMode: NSRunLoopCommonModes];
            win.fallback.set(timer);
        }
    }
}

/// Announces the frame the window asked for.
fn deliver_frame(win: &Win, next_ns: u64) {
    if !win.want_frame.replace(false) {
        return;
    }
    unsafe {
        let timer = win.fallback.replace(nil);
        if timer != nil {
            let () = msg_send![timer, invalidate];
        }
        let link = win.link.get();
        if link != nil {
            let () = msg_send![link, setPaused: YES];
        }
    }
    if !win.retired.get() {
        Shared::global().push(win.sid.get(), |q| q.frame_ready(win.id.get(), next_ns));
    }
}

pub fn display_link_fired(link: ObjcId) {
    let win = app().windows.borrow().iter().copied().find(|w| w.link.get() == link);
    let Some(win) = win else { return };
    let (target, media): (f64, f64) = unsafe { (msg_send![link, targetTimestamp], CACurrentMediaTime()) };
    let next_ns = crate::clock_ns() + ((target - media).clamp(0.0, 0.1) * 1e9) as u64;
    deliver_frame(win, next_ns);
}

pub fn fallback_fired(timer: ObjcId) {
    let win = app().windows.borrow().iter().copied().find(|w| w.fallback.get() == timer);
    let Some(win) = win else { return };
    win.fallback.set(nil);
    deliver_frame(win, crate::clock_ns());
}

/// A window went into or out of the Dock: its link is stale until the next request.
pub fn display_links_changed() {
    let windows: Vec<&'static Win> = app().windows.borrow().iter().copied().filter(|w| !w.retired.get() && w.want_frame.get()).collect();
    for win in windows {
        request_frame(win);
    }
}

// ------------------------------------------------------------------ live resize

/// AppKit's live-resize tracking loop of a window began / ended. The sessions run on while the
/// main thread tracks; they only hear of it.
pub fn resize_loop(win: &Win, start: bool) {
    super::test_resize::note_loop(win.window, start);
    if !win.retired.get() {
        Shared::global().push(win.sid.get(), |q| q.resize_loop(win.id.get(), start));
    }
}

// ------------------------------------------------------------------ menu

fn install_quit_menu() {
    // the running app's CFBundleName is what macOS shows as the application menu title; none
    // for a binary that is not bundled at all
    let name = unsafe {
        let bundle: ObjcId = msg_send![class!(NSBundle), mainBundle];
        let key = str_to_nsstring("CFBundleName");
        let value: ObjcId = if bundle == nil { nil } else { msg_send![bundle, objectForInfoDictionaryKey: key] };
        if value == nil {
            "Application".to_string()
        } else {
            nsstring_to_string(value)
        }
    };
    let sub = MenuItem { parent: 0, kind: menu_kind::SUB, ..Default::default() };
    let quit = MenuItem { parent: 1, kind: menu_kind::ITEM | menu_kind::QUIT, cmd: 0, key: makepad_key_code::KeyCode::KeyQ.to_index(), str: 0 };
    update_menu(&[(sub, name.clone()), (quit, format!("Quit {name}"))]);
}

/// Builds the main menu from the flat list: node `i` (1-based) is `items[i - 1]`, `parent` 0 is
/// the root. A `QUIT` item goes to `terminate:` (which routes through `applicationShouldTerminate:`
/// outside any in-flight handler); the others to the menu target that re-emits their command.
fn update_menu(items: &[(MenuItem, String)]) {
    unsafe {
        let a = app();
        let main_menu: ObjcId = msg_send![class!(NSMenu), new];
        let () = msg_send![main_menu, setTitle: str_to_nsstring("MainMenu")];
        let () = msg_send![main_menu, setAutoenablesItems: NO];
        let () = msg_send![main_menu, setDelegate: a.menu_delegate.get()];
        // menus[0] is the root, menus[i] the submenu of node i
        let mut menus: Vec<ObjcId> = vec![main_menu];
        for (item, name) in items {
            let parent = menus.get(item.parent as usize).copied().unwrap_or(main_menu);
            match item.kind & 0xff {
                menu_kind::SUB => {
                    let sub_menu: ObjcId = msg_send![class!(NSMenu), new];
                    let () = msg_send![sub_menu, setTitle: str_to_nsstring(name)];
                    let () = msg_send![sub_menu, setAutoenablesItems: NO];
                    let () = msg_send![sub_menu, setDelegate: a.menu_delegate.get()];
                    let sub_item: ObjcId = msg_send![parent, addItemWithTitle: str_to_nsstring(name) action: nil keyEquivalent: str_to_nsstring("")];
                    let () = msg_send![parent, setSubmenu: sub_menu forItem: sub_item];
                    menus.push(sub_menu);
                }
                menu_kind::ITEM => {
                    menus.push(nil);
                    let quit = item.kind & menu_kind::QUIT != 0;
                    let action = if quit { sel!(terminate:) } else { sel!(menuAction:) };
                    let shift = item.kind & menu_kind::SHIFT != 0;
                    let key = makepad_key_code::KeyCode::from_index(item.key);
                    let sub_item: ObjcId = msg_send![
                        parent,
                        addItemWithTitle: str_to_nsstring(name)
                        action: action
                        keyEquivalent: str_to_nsstring(keys::keycode_to_menu_key(key, shift))
                    ];
                    let enabled = item.kind & menu_kind::DISABLED == 0;
                    let () = msg_send![sub_item, setEnabled: if enabled { YES } else { NO }];
                    if !quit {
                        let target: ObjcId = msg_send![a.classes().menu_target, new];
                        let () = msg_send![sub_item, setTarget: target];
                        (*target).set_ivar("command_u64", item.cmd as usize);
                    }
                }
                _ => {
                    menus.push(nil);
                    let separator: ObjcId = msg_send![class!(NSMenuItem), separatorItem];
                    let () = msg_send![parent, addItem: separator];
                }
            }
        }
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, setMainMenu: main_menu];
    }
}

// ------------------------------------------------------------------ dialogs

const NS_MODAL_RESPONSE_OK: i64 = 1;

/// Every extension the dialog's filters name, flattened. A filter of `*` means "no restriction at
/// all" and wins over every other filter. `filters` are (description, "ext,ext") pairs.
fn filter_extensions(filters: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for pair in filters.chunks(2) {
        let Some(list) = pair.get(1) else { continue };
        for extension in list.split(',') {
            let cleaned = extension.trim().trim_start_matches('*').trim_start_matches('.');
            if cleaned.is_empty() || extension.trim() == "*" {
                return Vec::new();
            }
            if !out.iter().any(|e: &String| e.eq_ignore_ascii_case(cleaned)) {
                out.push(cleaned.to_string());
            }
        }
    }
    out
}

unsafe fn set_allowed_types(panel: ObjcId, extensions: &[String]) {
    if extensions.is_empty() {
        return;
    }
    let array: ObjcId = msg_send![class!(NSMutableArray), array];
    for extension in extensions {
        let () = msg_send![array, addObject: str_to_nsstring(extension)];
    }
    let () = msg_send![panel, setAllowedFileTypes: array];
}

unsafe fn set_start_location(panel: ObjcId, location: &str) {
    if location.is_empty() {
        return;
    }
    let is_directory = std::path::Path::new(location).is_dir();
    let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(location) isDirectory: if is_directory { YES } else { NO }];
    if url != nil {
        let () = msg_send![panel, setDirectoryURL: url];
    }
}

unsafe fn url_to_path(url: ObjcId) -> Option<String> {
    if url == nil {
        return None;
    }
    let path: ObjcId = msg_send![url, path];
    if path == nil {
        return None;
    }
    let path = nsstring_to_string(path);
    (!path.is_empty()).then_some(path)
}

/// Runs the panel to completion (its modal loop runs inside this op; further ops and the
/// sessions keep going) and sends the paths back as `DialogResult`.
fn file_dialog(sid: u32, req: u32, kind: u32, title: &str, location: &str, filename: &str, filters: &[String]) {
    let multiple = kind & dialog_kind::MULTIPLE != 0;
    let extensions = filter_extensions(filters);
    let mut paths: Vec<String> = Vec::new();
    unsafe {
        match kind & 0xff {
            dialog_kind::SELECT_FILE => {
                let panel: ObjcId = msg_send![class!(NSOpenPanel), openPanel];
                if panel != nil {
                    let () = msg_send![panel, setCanChooseFiles: YES];
                    let () = msg_send![panel, setCanChooseDirectories: NO];
                    let () = msg_send![panel, setAllowsMultipleSelection: if multiple { YES } else { NO }];
                    let () = msg_send![panel, setCanCreateDirectories: NO];
                    if !title.is_empty() {
                        let () = msg_send![panel, setMessage: str_to_nsstring(title)];
                    }
                    set_start_location(panel, location);
                    set_allowed_types(panel, &extensions);
                    let response: i64 = msg_send![panel, runModal];
                    if response == NS_MODAL_RESPONSE_OK {
                        let urls: ObjcId = msg_send![panel, URLs];
                        let count: usize = if urls == nil { 0 } else { msg_send![urls, count] };
                        for index in 0..count {
                            let url: ObjcId = msg_send![urls, objectAtIndex: index];
                            paths.extend(url_to_path(url));
                        }
                    }
                }
            }
            dialog_kind::SAVE_FILE => {
                let panel: ObjcId = msg_send![class!(NSSavePanel), savePanel];
                if panel != nil {
                    let () = msg_send![panel, setCanCreateDirectories: YES];
                    if !title.is_empty() {
                        let () = msg_send![panel, setMessage: str_to_nsstring(title)];
                    }
                    if !filename.is_empty() {
                        let () = msg_send![panel, setNameFieldStringValue: str_to_nsstring(filename)];
                    }
                    set_start_location(panel, location);
                    set_allowed_types(panel, &extensions);
                    let response: i64 = msg_send![panel, runModal];
                    if response == NS_MODAL_RESPONSE_OK {
                        let url: ObjcId = msg_send![panel, URL];
                        paths.extend(url_to_path(url));
                    }
                }
            }
            _ => {
                // files AND folders: one panel serves both "import this clip" and "import this
                // library"; "save into this folder" may also create one
                let can_create = kind & 0xff == dialog_kind::SAVE_FOLDER;
                let panel: ObjcId = msg_send![class!(NSOpenPanel), openPanel];
                if panel != nil {
                    let () = msg_send![panel, setCanChooseFiles: YES];
                    let () = msg_send![panel, setCanChooseDirectories: YES];
                    let () = msg_send![panel, setAllowsMultipleSelection: NO];
                    let () = msg_send![panel, setCanCreateDirectories: if can_create { YES } else { NO }];
                    if !title.is_empty() {
                        let () = msg_send![panel, setMessage: str_to_nsstring(title)];
                    }
                    if !location.is_empty() {
                        let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(location) isDirectory: YES];
                        if url != nil {
                            let () = msg_send![panel, setDirectoryURL: url];
                        }
                    }
                    let response: i64 = msg_send![panel, runModal];
                    if response == NS_MODAL_RESPONSE_OK {
                        let url: ObjcId = msg_send![panel, URL];
                        paths.extend(url_to_path(url));
                    }
                }
            }
        }
    }
    let mut bytes = Vec::new();
    for p in &paths {
        bytes.extend_from_slice(&(p.len() as u32).to_le_bytes());
        bytes.extend_from_slice(p.as_bytes());
    }
    Shared::global().push(sid, |q| q.dialog_result(req, paths.len() as u32, &bytes));
}
