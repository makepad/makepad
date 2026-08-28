use crate::cx::Cx;
use crate::file_dialogs::{FileDialog, FileDialogAction};

use {
    crate::{
        cursor::MouseCursor,
        //macos_menu::{
        //    CxCommandSetting
        //},
        //turtle::{
        //    Rect
        //},
        event::{
            KeyCode, KeyEvent, KeyModifiers, ScrollPhase, TextClipboardEvent, TextInputEvent,
            TimerEvent,
        },
        macos_menu::MacosMenu,
        makepad_live_id::*,
        makepad_math::Vec2d,
        os::{
            apple::apple_sys::*,
            apple_util::{
                get_event_key_modifier, get_event_keycode, keycode_to_menu_key, nsstring_to_string,
                str_to_nsstring,
            },
            cx_native::EventFlow,
            macos::{macos_delegates::*, macos_event::*, macos_window::MacosWindow},
        },
        window::WindowId,
    },
    makepad_objc_sys::{objc_block, Encode, Encoding},
    std::{cell::RefCell, collections::HashMap, os::raw::c_void, rc::Rc, time::Instant},
};

// this is unsafe, however we don't have much choice since the system calls into
// the objective C entrypoints we need to enter our eventloop
// So wherever we put this boundary, it will be unsafe

// this value will be fetched from multiple threads (post signal uses it)
pub static mut MACOS_CLASSES: *const MacosClasses = 0 as *const _;
// this value should not. Todo: guard this somehow proper

thread_local! {
    pub static MACOS_APP: RefCell<Option<MacosApp>> = RefCell::new(None);
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

static METAL_LINK_TRACE_UPDATES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static METAL_LINK_TRACE_DRAWABLES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static METAL_LINK_TRACE_PRESENTED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static METAL_LINK_TRACE_LAST_US: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn metal_link_frame_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_FRAME_TRACE").is_some())
}

pub(super) fn metal_link_trace_drawable_consumed() {
    if metal_link_frame_trace_enabled() {
        METAL_LINK_TRACE_DRAWABLES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(super) fn metal_link_trace_presented() {
    if metal_link_frame_trace_enabled() {
        METAL_LINK_TRACE_PRESENTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

fn metal_link_trace_update_fired() {
    if metal_link_frame_trace_enabled() {
        METAL_LINK_TRACE_UPDATES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

fn metal_link_trace_report() {
    if !metal_link_frame_trace_enabled() {
        return;
    }
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let now_us = START
        .get_or_init(Instant::now)
        .elapsed()
        .as_micros()
        .min(u64::MAX as u128) as u64;
    let last_us = METAL_LINK_TRACE_LAST_US.load(std::sync::atomic::Ordering::Relaxed);
    if now_us.saturating_sub(last_us) < 1_000_000
        || METAL_LINK_TRACE_LAST_US
            .compare_exchange(
                last_us,
                now_us,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Relaxed,
            )
            .is_err()
    {
        return;
    }
    let updates = METAL_LINK_TRACE_UPDATES.swap(0, std::sync::atomic::Ordering::AcqRel);
    let drawables = METAL_LINK_TRACE_DRAWABLES.swap(0, std::sync::atomic::Ordering::AcqRel);
    let presented = METAL_LINK_TRACE_PRESENTED.swap(0, std::sync::atomic::Ordering::AcqRel);
    eprintln!(
        "[frame-trace] metal-link interval_ms={:.1} updates_fired={} drawables_consumed={} presented={}",
        now_us.saturating_sub(last_us) as f64 / 1000.0,
        updates,
        drawables,
        presented,
    );
}

pub fn with_macos_app<R>(f: impl FnOnce(&mut MacosApp) -> R) -> R {
    MACOS_APP.with_borrow_mut(|app| f(app.as_mut().unwrap()))
}

/// Like [`with_macos_app`], but returns `None` instead of panicking when the
/// app is already borrowed — for callbacks AppKit fires re-entrantly from
/// inside our own event handling (`resetCursorRects` after an
/// `invalidateCursorRects`, tracking-area updates), where a RefCell panic
/// cannot unwind through the ObjC frame and aborts the whole process.
pub fn try_with_macos_app<R>(f: impl FnOnce(&mut MacosApp) -> R) -> Option<R> {
    MACOS_APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => app.as_mut().map(f),
        Err(_) => None,
    })
}

/// Whether this process may activate itself or make a window key.
///
/// USER LAW (2026-08-26): an agent-driven or test instance must never steal
/// the user's focus — they could not type in their own terminal while lanes
/// launched and clicked windows. `--remote` therefore means VISIBLE BUT
/// UNFOCUSED: the window is ordered on screen, never made key, and the app
/// never activates — the bridge injects input through the event loop, not
/// the OS, so key-window status is not needed for anything it does.
/// `MAKEPAD_NO_FOCUS=1` asks for the same without `--remote`;
/// `MAKEPAD_FOCUS=1` restores activation for a remote run the user wants
/// in front. Decided once per process.
pub fn focus_allowed() -> bool {
    static ALLOWED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALLOWED.get_or_init(|| {
        if std::env::var_os("MAKEPAD_FOCUS").is_some() {
            return true;
        }
        if std::env::var_os("MAKEPAD_NO_FOCUS").is_some() {
            return false;
        }
        !crate::remote::requested()
    })
}

/// Activate one Cocoa window without holding the global `MacosApp` RefCell
/// borrow across AppKit calls (which can synchronously re-enter delegates).
/// A no-focus process (see [`focus_allowed`]) never activates: bridge
/// clicks run through here too, and each one used to raise the window over
/// whatever the user was typing in.
pub fn activate_cocoa_window_on_pointer_down(window: ObjcId) -> bool {
    if window == nil || std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() || !focus_allowed() {
        return false;
    }
    unsafe {
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let active: bool = msg_send![ns_app, isActive];
        if !active {
            let () = msg_send![ns_app, activateIgnoringOtherApps: YES];
        }
        let key: bool = msg_send![window, isKeyWindow];
        if !key {
            let () = msg_send![window, makeKeyAndOrderFront: nil];
        }
        !active || !key
    }
}

extern "C" {
    /// libobjc: the hook called with an exception object before it is
    /// thrown. The only place that still sees the reason when the unwind
    /// later meets a Rust frame and aborts ("Rust cannot catch foreign
    /// exceptions") before AppKit's top-level handler can print anything.
    fn objc_setExceptionPreprocessor(
        f: extern "C" fn(ObjcId) -> ObjcId,
    ) -> Option<extern "C" fn(ObjcId) -> ObjcId>;
}

unsafe fn objc_exception_text(obj: ObjcId) -> String {
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
        eprintln!(
            "makepad: ObjC exception {}: {}",
            objc_exception_text(name),
            objc_exception_text(reason)
        );
        let symbols: ObjcId = msg_send![exception, callStackSymbols];
        if !symbols.is_null() {
            let count: u64 = msg_send![symbols, count];
            for i in 0..count.min(16) {
                let line: ObjcId = msg_send![symbols, objectAtIndex: i];
                eprintln!("    {}", objc_exception_text(line));
            }
        }
    }
    exception
}

pub fn init_macos_app_global(event_callback: Box<dyn FnMut(MacosEvent) -> EventFlow>) {
    unsafe {
        MACOS_CLASSES = Box::into_raw(Box::new(MacosClasses::new()));
        objc_setExceptionPreprocessor(log_objc_exception);
    }
    MACOS_APP.with(|app| {
        *app.borrow_mut() = Some(MacosApp::new(event_callback));
    })
}

pub fn get_macos_class_global() -> &'static MacosClasses {
    unsafe { &*(MACOS_CLASSES) }
}

/// Reads CFBundleName from `[NSBundle mainBundle]`'s Info.plist. Returns
/// `None` when the running binary has no bundle metadata (e.g. truly bare
/// `cargo run` without the platform crate's generated stub Info.plist).
pub unsafe fn current_bundle_name() -> Option<String> {
    let bundle: ObjcId = msg_send![class!(NSBundle), mainBundle];
    if bundle == nil {
        return None;
    }
    let key = str_to_nsstring("CFBundleName");
    let name: ObjcId = msg_send![bundle, objectForInfoDictionaryKey: key];
    if name == nil {
        return None;
    }
    Some(nsstring_to_string(name))
}

#[derive(Clone)]
pub struct CocoaTimer {
    timer_id: u64,
    nstimer: ObjcId,
    repeats: bool,
}

pub struct MacosClasses {
    pub window: *const Class,
    pub panel: *const Class,
    pub window_delegate: *const Class,
    pub menu_delegate: *const Class,
    pub app_delegate: *const Class,
    pub menu_target: *const Class,
    pub view: *const Class,
    pub timer_delegate: *const Class,
    /// Subclass swapped onto AppKit's NSTitlebarContainerView so the
    /// transparent titlebar stops eating drags (see macos_delegates).
    /// Null when the private class is absent — callers skip the swap.
    pub titlebar_container: *const Class,
}

impl MacosClasses {
    pub fn new() -> Self {
        /*let const_attributes = vec![
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSMarkedClauseSegment")).unwrap()).forget(),
            RcObjcId::from_unowned(NonNull::new(str_to_nsstring("NSGlyphInfo")).unwrap()).forget(),
        ];*/
        Self {
            timer_delegate: define_macos_timer_delegate(),
            window: define_macos_window_class(),
            panel: define_macos_panel_class(),
            window_delegate: define_macos_window_delegate(),
            //post_delegate: define_cocoa_post_delegate(),
            menu_delegate: define_menu_delegate(),
            app_delegate: define_app_delegate(),
            menu_target: define_menu_target_class(),
            view: define_cocoa_view_class(),
            titlebar_container: define_titlebar_container_class(),
        }
    }
}

pub struct MacosApp {
    menu_delegate_instance: ObjcId,
    //app_delegate_instance: ObjcId,
    pub time_start: Instant,
    pub timer_delegate_instance: ObjcId,
    timers: Vec<CocoaTimer>,
    /// Per-layer CAMetalDisplayLink paint pacing on macOS 14+, with the
    /// existing per-view CADisplayLink path as its runtime fallback: each
    /// window's paint beat fires FROM its own panel's refresh callback
    /// instead of an NSTimer racing it — the real frame-flip clock, per
    /// window, per display. Empty until a window exists (or unsupported:
    /// NSTimer pacing stays). Entries are (cocoa window, link).
    display_links: Vec<(ObjcId, ObjcId)>,
    display_links_paused: bool,
    //pub signals: Mutex<RefCell<HashSet<Signal>>>,
    pub cocoa_windows: Vec<(ObjcId, ObjcId)>,
    /// Exact framework-to-Cocoa lookup for bridge-injected pointer activation.
    pub cocoa_window_ids: Vec<(WindowId, ObjcId)>,
    /// Cocoa owns/retains window delegates and views beyond `windowWillClose:`.
    /// Keep their Rust callback targets alive for the same lifetime so a queued
    /// native callback can never dereference a freed `MacosWindow`.
    retired_cocoa_windows: Vec<Box<MacosWindow>>,
    last_key_mod: KeyModifiers,
    #[allow(unused)]
    pasteboard: ObjcId,
    startup_focus_hack_ran: bool,
    event_callback: Option<Box<dyn FnMut(MacosEvent) -> EventFlow>>,
    pub(crate) event_flow: EventFlow,
    pub(crate) terminating_from_app_delegate: bool,

    pub cursors: HashMap<MouseCursor, ObjcId>,
    pub current_cursor: MouseCursor,
    /// Pointer lock (FPS mouse capture): while true the hardware cursor is
    /// frozen+hidden and MouseMove positions are synthesized from NSEvent
    /// deltas into `virtual_mouse` — downstream consumers never know.
    pub mouse_pointer_lock: bool,
    pub virtual_mouse: Option<Vec2d>,
    /// Whether the lock's physical effects (hidden cursor, disassociation)
    /// are currently applied. Diverges from `mouse_pointer_lock` while the
    /// window is not key: macOS re-associates the cursor when the app
    /// deactivates, so focus loss suspends the effects and focus gain
    /// re-applies them — the same dance GLFW does for disabled-cursor mode.
    pub pointer_lock_applied: bool,
    /// The pin, cocoa global coords (bottom-left origin) — where the locked
    /// cursor must stay.
    pub lock_pin: Option<Vec2d>,
    /// Where a widget-scoped pointer pin must restore the cursor on
    /// release: the press point, cocoa-global coords (`set_pointer_pin`).
    pub pin_restore: Option<Vec2d>,
    /// True while the lock is a widget-scoped SCRUB pin (not a game FPS
    /// lock): `abs` integrates the deltas into an unbounded virtual
    /// position (the drag needs continuous positions; the pressed widget
    /// holds the finger capture, so routing cannot wander), instead of the
    /// game model where `abs` stays pinned and motion rides `lock_delta`.
    pub pointer_pin_mode: bool,
    //current_ns_event: Option<ObjcId>,

    /// Set by `send_command_event()` to avoid sending keyboard events
    /// for keyboard shortcuts that trigger a macOS menu command.
    pub(crate) menu_command_fired: bool,
}

impl MacosApp {
    pub fn new(event_callback: Box<dyn FnMut(MacosEvent) -> EventFlow>) -> MacosApp {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let app_delegate_instance: ObjcId =
                msg_send![get_macos_class_global().app_delegate, new];

            let () = msg_send![ns_app, setDelegate: app_delegate_instance];
            let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];

            // Construct the bits that are shared between windows
            MacosApp {
                startup_focus_hack_ran: false,
                pasteboard: msg_send![class!(NSPasteboard), generalPasteboard],
                time_start: Instant::now(),
                timer_delegate_instance: msg_send![get_macos_class_global().timer_delegate, new],
                display_links: Vec::new(),
                display_links_paused: false,
                menu_delegate_instance: msg_send![get_macos_class_global().menu_delegate, new],
                //app_delegate_instance,
                //signals: Mutex::new(RefCell::new(HashSet::new())),
                timers: Vec::new(),
                cocoa_windows: Vec::new(),
                cocoa_window_ids: Vec::new(),
                retired_cocoa_windows: Vec::new(),
                event_flow: EventFlow::Poll,
                last_key_mod: KeyModifiers {
                    ..Default::default()
                },
                event_callback: Some(event_callback),
                cursors: HashMap::new(),
                current_cursor: MouseCursor::Default,
                mouse_pointer_lock: false,
                virtual_mouse: None,
                pointer_lock_applied: false,
                lock_pin: None,
                pin_restore: None,
                pointer_pin_mode: false,
                terminating_from_app_delegate: false,
                //current_ns_event: None,
                menu_command_fired: false,
            }
        }
    }
    pub fn init_quit_menu(&mut self) {
        // Use the running app's CFBundleName (which is what macOS already
        // shows as the application menu title in the menu bar) so the
        // submenu and the "Quit X" item label match. NSBundle returns nil
        // when the binary isn't bundled at all; fall back to a generic
        // label in that case.
        let app_name = unsafe { current_bundle_name() }
            .unwrap_or_else(|| "Application".to_string());
        self.update_macos_menu(&MacosMenu::Main {
            items: vec![MacosMenu::Sub {
                name: app_name.clone(),
                items: vec![MacosMenu::Item {
                    command: live_id!(quit),
                    key: KeyCode::KeyQ,
                    shift: false,
                    enabled: true,
                    name: format!("Quit {}", app_name),
                }],
            }],
        });
    }

    // Determines whether to show your application in the dock when it runs. The default value is true.
    pub fn show_in_dock(&mut self, show: bool) {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            if show {
                let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular as i64];
            } else {
                let () = msg_send![ns_app, setActivationPolicy: NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory as i64];
            }
        }
    }

    pub fn cocoa_window_for_id(&self, window_id: WindowId) -> Option<ObjcId> {
        self.cocoa_window_ids
            .iter()
            .find(|(candidate, _)| *candidate == window_id)
            .map(|(_, window)| *window)
    }
    pub fn update_macos_menu(&mut self, menu: &MacosMenu) {
        unsafe fn make_menu(
            parent_menu: ObjcId,
            delegate: ObjcId,
            menu_target_class: *const Class,
            menu: &MacosMenu,
        ) {
            match menu {
                MacosMenu::Main { items } => {
                    let main_menu: ObjcId = msg_send![class!(NSMenu), new];
                    let () = msg_send![main_menu, setTitle: str_to_nsstring("MainMenu")];
                    let () = msg_send![main_menu, setAutoenablesItems: NO];
                    let () = msg_send![main_menu, setDelegate: delegate];

                    for item in items {
                        make_menu(main_menu, delegate, menu_target_class, item);
                    }
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![
                        ns_app,
                        setMainMenu: main_menu
                    ];
                }
                MacosMenu::Sub { name, items } => {
                    let sub_menu: ObjcId = msg_send![class!(NSMenu), new];
                    let () = msg_send![sub_menu, setTitle: str_to_nsstring(name)];
                    let () = msg_send![sub_menu, setAutoenablesItems: NO];
                    let () = msg_send![sub_menu, setDelegate: delegate];
                    // append item to parebt
                    let sub_item: ObjcId = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: nil
                        keyEquivalent: str_to_nsstring("")
                    ];
                    // connect submenu
                    let () = msg_send![parent_menu, setSubmenu: sub_menu forItem: sub_item];
                    for item in items {
                        make_menu(sub_menu, delegate, menu_target_class, item);
                    }
                }
                MacosMenu::Item {
                    name,
                    command,
                    shift,
                    key,
                    enabled,
                } => {
                    // Wire the well-known `quit` command to NSApp's standard
                    // `terminate:` selector instead of our custom
                    // `menuAction:` callback. `terminate:` routes through
                    // `applicationShouldTerminate:`, which dispatches
                    // `MacosEvent::AppQuitRequested` from the main loop —
                    // i.e. *outside* any in-flight event handler — so apps
                    // can call `cx.request_quit` from their `QuitRequested`
                    // arm without re-entering `call_event_handler` and
                    // panicking on `event_handler.take().unwrap()`. Other
                    // commands keep going through `menuAction:` →
                    // `Event::MacosMenuCommand`.
                    let is_quit = *command == live_id!(quit);
                    let action = if is_quit {
                        sel!(terminate:)
                    } else {
                        sel!(menuAction:)
                    };
                    let sub_item: ObjcId = msg_send![
                        parent_menu,
                        addItemWithTitle: str_to_nsstring(name)
                        action: action
                        keyEquivalent: str_to_nsstring(keycode_to_menu_key(*key, *shift))
                    ];
                    let () = msg_send![sub_item, setEnabled: if *enabled {YES}else {NO}];
                    if !is_quit {
                        // Leave target nil for `terminate:` so it bubbles up
                        // to NSApp; for everything else, install the
                        // MenuTarget instance that re-emits the LiveId.
                        let target: ObjcId = msg_send![menu_target_class, new];
                        let () = msg_send![sub_item, setTarget: target];
                        (*target).set_ivar("command_u64", command.0);
                    }
                }
                MacosMenu::Line => {
                    let sep_item: ObjcId = msg_send![class!(NSMenuItem), separatorItem];
                    let () = msg_send![
                        parent_menu,
                        addItem: sep_item
                    ];
                }
            }
        }
        unsafe {
            make_menu(
                nil,
                self.menu_delegate_instance,
                get_macos_class_global().menu_target,
                menu,
            );
        }
    }
    /*
    pub fn startup_focus_hack(&mut self) {

        unsafe {
            self.startup_focus_hack_ran = true;
            if !self.startup_focus_hack_ran {
                self.startup_focus_hack_ran = true;

                //let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                //let active: bool = msg_send![ns_app, isActive];
                //if !active {
                let dock_bundle_id = str_to_nsstring("com.apple.dock");
                let dock_array: ObjcId = msg_send![
                    class!(NSRunningApplication),
                    runningApplicationsWithBundleIdentifier: dock_bundle_id
                ];
                let my_app: ObjcId = msg_send![
                    class!(NSRunningApplication),
                    runningApplicationWithProcessIdentifier: std::process::id()
                ];
                let dock_array_len: u64 = msg_send![dock_array, count];
                if dock_array_len == 0 {
                    panic!("Dock not running");
                } else {
                    let dock: ObjcId = msg_send![dock_array, objectAtIndex: 0];
                        let _status: BOOL = msg_send![
                            dock,
                            activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                        ];
                        //let ns_running_app: ObjcId = msg_send![class!(NSRunningApplication), currentApplication];

                    let () = msg_send![
                        my_app,
                        activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                    ];
                    if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_none() {
                        let () = msg_send![self.cocoa_windows[0].0, makeKeyAndOrderFront: nil];
                    }
                }
                //}
            }
        }
    }*/
    pub fn startup_focus_hack(&mut self) {
        if !focus_allowed() {
            // Visible-but-unfocused launch: no Dock dance, no activation.
            self.startup_focus_hack_ran = true;
            return;
        }
        return unsafe {
            if !self.startup_focus_hack_ran {
                self.startup_focus_hack_ran = true;
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                let active: bool = msg_send![ns_app, isActive];
                if !active {
                    let dock_bundle_id: ObjcId = str_to_nsstring("com.apple.dock");
                    let dock_array: ObjcId = msg_send![
                        class!(NSRunningApplication),
                        runningApplicationsWithBundleIdentifier: dock_bundle_id
                    ];
                    let dock_array_len: u64 = msg_send![dock_array, count];
                    if dock_array_len == 0 {
                        panic!("Dock not running");
                    } else {
                        let dock: ObjcId = msg_send![dock_array, objectAtIndex: 0];
                        let _status: BOOL = msg_send![
                            dock,
                            activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                        ];
                        let ns_running_app: ObjcId =
                            msg_send![class!(NSRunningApplication), currentApplication];
                        let () = msg_send![
                            ns_running_app,
                            activateWithOptions: NSApplicationActivationOptions::NSApplicationActivateIgnoringOtherApps
                        ];
                    }
                }
            }
        };
    }

    pub fn time_now(&self) -> f64 {
        let time_now = Instant::now(); //unsafe {mach_absolute_time()};
        (time_now.duration_since(self.time_start)).as_secs_f64()
    }

    unsafe fn process_ns_event(ns_event: ObjcId) {
        let ev_type: NSEventType = msg_send![ns_event, type];

        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        // Clear the menu-consumed marker so we can tell after `sendEvent:`
        // whether the main menu took this NSEvent as a key equivalent.
        with_macos_app(|app| app.menu_command_fired = false);
        let () = msg_send![ns_app, sendEvent: ns_event];

        if ev_type as u64 == 21 {
            // some missing event from cocoa-rs crate
            return;
        }

        match ev_type {
            NSEventType::NSApplicationDefined => { // event loop unblocker
            }
            NSEventType::NSKeyUp => {
                if let Some(key_code) = get_event_keycode(ns_event) {
                    let modifiers = get_event_key_modifier(ns_event);
                    //let key_char = get_event_char(ns_event);
                    let is_repeat: bool = msg_send![ns_event, isARepeat];
                    let time = with_macos_app(|app| app.time_now());
                    MacosApp::do_callback(MacosEvent::KeyUp(KeyEvent {
                        key_code: key_code,
                        //key_char: key_char,
                        is_repeat: is_repeat,
                        modifiers: modifiers,
                        time,
                    }));
                }
            }
            NSEventType::NSKeyDown => {
                if with_macos_app(|app| app.menu_command_fired) {
                    return;
                }
                if let Some(key_code) = get_event_keycode(ns_event) {
                    let modifiers = get_event_key_modifier(ns_event);
                    //let key_char = get_event_char(ns_event);
                    let is_repeat: bool = msg_send![ns_event, isARepeat];
                    //let is_return = if let KeyCode::Return = key_code{true} else{false};

                    #[cfg(target_os = "macos")]
                    match key_code {
                        KeyCode::KeyV => {
                            if modifiers.logo || modifiers.control {
                                // was a paste
                                let pasteboard: ObjcId = with_macos_app(|app| app.pasteboard);
                                let nsstring: ObjcId =
                                    msg_send![pasteboard, stringForType: NSStringPboardType];
                                if nsstring != std::ptr::null_mut() {
                                    let string = nsstring_to_string(nsstring);
                                    MacosApp::do_callback(MacosEvent::TextInput(TextInputEvent {
                                        input: string,
                                        was_paste: true,
                                        replace_last: false,
                                        ..Default::default()
                                    }));
                                }
                            }
                        }
                        KeyCode::KeyC => {
                            if modifiers.logo || modifiers.control {
                                let pasteboard: ObjcId = with_macos_app(|app| app.pasteboard);
                                let response = Rc::new(RefCell::new(None));
                                MacosApp::do_callback(MacosEvent::TextCopy(TextClipboardEvent {
                                    response: response.clone(),
                                }));
                                let response = response.borrow();
                                if let Some(response) = response.as_ref() {
                                    let nsstring = str_to_nsstring(&response);
                                    let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
                                    let () = msg_send![pasteboard, declareTypes: array owner: nil];
                                    let () = msg_send![pasteboard, setString: nsstring forType: NSStringPboardType];
                                }
                            }
                        }
                        KeyCode::KeyX => {
                            if modifiers.logo || modifiers.control {
                                let pasteboard: ObjcId = with_macos_app(|app| app.pasteboard);
                                let response = Rc::new(RefCell::new(None));
                                MacosApp::do_callback(MacosEvent::TextCut(TextClipboardEvent {
                                    response: response.clone(),
                                }));
                                let response = response.borrow();
                                if let Some(response) = response.as_ref() {
                                    let nsstring = str_to_nsstring(&response);
                                    let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
                                    let () = msg_send![pasteboard, declareTypes: array owner: nil];
                                    let () = msg_send![pasteboard, setString: nsstring forType: NSStringPboardType];
                                }
                            }
                        }
                        _ => {}
                    }
                    let time = with_macos_app(|app: &mut MacosApp| app.time_now());
                    // lets check if we have marked text
                    if KeyCode::Backspace == key_code {
                        // we have to check if we dont have any marked text in our windows
                        if with_macos_app(|app| {
                            for (_, view) in &app.cocoa_windows {
                                let marked = unsafe { msg_send![*view, hasMarkedText] };
                                if marked {
                                    return true;
                                }
                            }
                            false
                        }) {
                            return;
                        }
                    }
                    MacosApp::do_callback(MacosEvent::KeyDown(KeyEvent {
                        key_code: key_code,
                        is_repeat: is_repeat,
                        modifiers: modifiers,
                        time,
                    }));
                    /*
                    if is_return{
                        self.do_callback(&mut vec![
                            Event::TextInput(TextInputEvent{
                                input:"\n".to_string(),
                                was_paste:false,
                                replace_last:false
                            })
                        ]);
                    }*/
                }
            }
            NSEventType::NSFlagsChanged => {
                let modifiers = get_event_key_modifier(ns_event);
                let last_key_mod = with_macos_app(|app| app.last_key_mod.clone());
                with_macos_app(|app| app.last_key_mod = modifiers.clone());
                let mut events = Vec::new();
                fn add_event(
                    time: f64,
                    old: bool,
                    new: bool,
                    modifiers: KeyModifiers,
                    events: &mut Vec<MacosEvent>,
                    key_code: KeyCode,
                ) {
                    if old != new {
                        let event = KeyEvent {
                            key_code: key_code,
                            //key_char: '\0',
                            is_repeat: false,
                            modifiers: modifiers,
                            time: time,
                        };
                        if new {
                            events.push(MacosEvent::KeyDown(event));
                        } else {
                            events.push(MacosEvent::KeyUp(event));
                        }
                    }
                }
                let time = with_macos_app(|app| app.time_now());
                add_event(
                    time,
                    last_key_mod.shift,
                    modifiers.shift,
                    modifiers.clone(),
                    &mut events,
                    KeyCode::Shift,
                );
                add_event(
                    time,
                    last_key_mod.alt,
                    modifiers.alt,
                    modifiers.clone(),
                    &mut events,
                    KeyCode::Alt,
                );
                add_event(
                    time,
                    last_key_mod.logo,
                    modifiers.logo,
                    modifiers.clone(),
                    &mut events,
                    KeyCode::Logo,
                );
                add_event(
                    time,
                    last_key_mod.control,
                    modifiers.control,
                    modifiers.clone(),
                    &mut events,
                    KeyCode::Control,
                );
                if events.len() > 0 {
                    for event in events {
                        MacosApp::do_callback(event);
                    }
                }
            }
            NSEventType::NSMouseEntered => {}
            NSEventType::NSMouseExited => {}
            NSEventType::NSScrollWheel => {
                let window: ObjcId = msg_send![ns_event, window];
                if window == nil {
                    return;
                }
                let window_delegate: ObjcId = msg_send![window, delegate];
                if window_delegate == nil {
                    return;
                }
                // Foreign windows land in this event loop too (the macOS
                // screen-capture overlay's TUINSWindow among them) and their
                // delegates don't carry our ivar — skip them, don't panic.
                if (*window_delegate)
                    .class()
                    .instance_variable("macos_window_ptr")
                    .is_none()
                {
                    return;
                }
                let ptr: *mut c_void = *(*window_delegate).get_ivar("macos_window_ptr");
                let cocoa_window = &mut *(ptr as *mut MacosWindow);
                let dx: f64 = msg_send![ns_event, scrollingDeltaX];
                let dy: f64 = msg_send![ns_event, scrollingDeltaY];
                let has_prec: BOOL = msg_send![ns_event, hasPreciseScrollingDeltas];
                // NSEventPhase bitmask values (NSEvent.h): Began = 1<<0, Stationary = 1<<1,
                // Changed = 1<<2, Ended = 1<<3, Cancelled = 1<<4, MayBegin = 1<<5.
                // `phase` covers the finger-driven part of a trackpad gesture; `momentumPhase`
                // covers the momentum stream after lift-off. Classic wheel mice report 0 for
                // both. See `ScrollPhase` for how widgets use these.
                let phase_bits: u64 = msg_send![ns_event, phase];
                let momentum_bits: u64 = msg_send![ns_event, momentumPhase];
                let phase = if momentum_bits & ((1 << 3) | (1 << 4)) != 0 {
                    ScrollPhase::MomentumEnded
                } else if momentum_bits != 0 {
                    ScrollPhase::Momentum
                } else if phase_bits & ((1 << 0) | (1 << 5)) != 0 {
                    ScrollPhase::Began
                } else if phase_bits & ((1 << 1) | (1 << 2)) != 0 {
                    ScrollPhase::Changed
                } else if phase_bits & (1 << 3) != 0 {
                    ScrollPhase::Ended
                } else if phase_bits & (1 << 4) != 0 {
                    // Cancelled: the system took over the gesture, e.g. a system-wide swipe.
                    // Native behavior is to abort without momentum, so map it to `Began`, which
                    // widgets treat as a gesture reset (samples cleared, fling stopped), rather
                    // than `Ended`, which would start a fling.
                    ScrollPhase::Began
                } else {
                    ScrollPhase::None
                };
                return if has_prec == YES {
                    cocoa_window.send_scroll(
                        Vec2d { x: -dx, y: -dy },
                        get_event_key_modifier(ns_event),
                        false,
                        phase,
                    );
                } else {
                    cocoa_window.send_scroll(
                        Vec2d {
                            x: -dx * 32.,
                            y: -dy * 32.,
                        },
                        get_event_key_modifier(ns_event),
                        true,
                        phase,
                    );
                };
            }
            NSEventType::NSEventTypePressure => {}
            _ => (),
        }
    }

    pub fn event_loop() {
        unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            //let () = msg_send![ns_app, activateIgnoringOtherApps:YES];
            let () = msg_send![ns_app, finishLaunching];
            // Install a minimal default app menu (just "Quit X" bound to
            // Cmd+Q) so unbundled `cargo run` apps get the standard macOS
            // Quit affordance out of the box. Apps that build their own menu
            // (via `cx.update_macos_menu` or the `WindowMenu` widget) will
            // overwrite this; the call is harmless either way.
            with_macos_app(|app| app.init_quit_menu());
            // get_macos_app_global().startup_focus_hack();

            loop {
                let event_flow = with_macos_app(|app| app.event_flow);
                match event_flow {
                    EventFlow::Exit => {
                        break;
                    }
                    EventFlow::Poll | EventFlow::Wait => {
                        let event_wait =
                            if let EventFlow::Wait = with_macos_app(|app| app.event_flow) {
                                true
                            } else {
                                false
                            };
                        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                        let ns_until: ObjcId = if event_wait {
                            msg_send![class!(NSDate), distantFuture]
                        } else {
                            msg_send![class!(NSDate), distantPast]
                        };

                        let ns_event: ObjcId = msg_send![
                            ns_app,
                            nextEventMatchingMask: NSEventMask::NSAnyEventMask as u64 | NSEventMask::NSEventMaskPressure as u64
                            untilDate: ns_until
                            inMode: NSDefaultRunLoopMode
                            dequeue: YES
                        ];
                        //self.current_ns_event = Some(ns_event);
                        if ns_event != nil {
                            MacosApp::process_ns_event(ns_event);
                        }

                        //let event_wait = if let EventFlow::Wait = with_macos_app(|app| app.event_flow) {true}else {false};

                        //if ns_event == nil || event_wait {
                        //if event_wait{
                        //    with_macos_app(|app| app.event_flow = EventFlow::Wait);
                        //}
                        //self.current_ns_event = None;

                        let () = msg_send![pool, release];
                    }
                }
            }
        }
    }

    pub fn do_callback(event: MacosEvent) {
        let cb = with_macos_app(|app| app.event_callback.take());
        if let Some(mut callback) = cb {
            let event_flow = callback(event);
            let should_terminate = with_macos_app(|app| {
                app.event_flow = event_flow;
                event_flow == EventFlow::Exit && !app.terminating_from_app_delegate
            });
            if should_terminate {
                unsafe {
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![ns_app, terminate: nil];
                }
            }
            with_macos_app(|app| app.event_callback = Some(callback));
        }
    }

    /// Deliver `WindowClosed` after the current Cocoa delegate callback has
    /// returned. Removing a `MetalWindow` synchronously from
    /// `windowWillClose:` would otherwise drop the boxed `MacosWindow` while
    /// that same object is still borrowed by the delegate callback.
    pub fn defer_window_closed(window_id: WindowId) {
        unsafe {
            let main_thread_block = objc_block!(move || {
                MacosApp::do_callback(MacosEvent::WindowClosed(
                    crate::event::WindowClosedEvent { window_id },
                ));
            });
            let main_queue: ObjcId = msg_send![class!(NSOperationQueue), mainQueue];
            let block_operation: ObjcId =
                msg_send![class!(NSBlockOperation), blockOperationWithBlock: &main_thread_block];
            let () = msg_send![main_queue, addOperation: block_operation];
        }
    }

    /// Retire a closed window without invalidating the raw pointers stored in
    /// its still-retained Cocoa view and delegate. Those native objects retain
    /// their original `alloc` ownership until process teardown; keeping this
    /// small Rust peer for the same lifetime closes the corresponding UAF.
    pub fn retire_cocoa_window(&mut self, mut window: Box<MacosWindow>) {
        window.retire();
        let native_window = window.window;
        self.cocoa_window_ids
            .retain(|(window_id, _)| *window_id != window.window_id);
        self.cocoa_windows
            .retain(|(window, _view)| *window != native_window);
        self.retired_cocoa_windows.push(window);
        // Drop the closing window's link; if the PRIMARY died, beats died
        // with it — keep a heartbeat alive so the paint loop wakes and
        // re-anchors (ensure_timer0_started spots the missing link).
        let had = !self.display_links.is_empty();
        self.display_links.retain(|(window, link)| {
            if *window == native_window {
                unsafe {
                    let () = msg_send![*link, invalidate];
                }
                false
            } else {
                true
            }
        });
        if had && self.display_links.is_empty() {
            self.stop_timer(0);
            self.start_timer(0, 0.2, true);
        }
    }

    /// True when link pacing SHOULD be re-armed: a window exists without
    /// its own link (fresh window, or the self-heal after a close).
    pub fn display_link_needs_rearm(&self) -> bool {
        self.cocoa_windows
            .iter()
            .any(|(window, _)| !self.display_links.iter().any(|(w, _)| w == window))
    }
    /*
    pub fn post_signal(signal: Signal) {
        unsafe {
            let cocoa_app = get_macos_app_global();
            if let Ok(signals) = cocoa_app.signals.lock(){
                let mut signals = signals.borrow_mut();
                // if empty, we do shit. otherwise we add
                if signals.is_empty(){
                    signals.insert(signal);
                    let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                    //let cocoa_app = get_macos_app_global();
                    let post_delegate_instance: ObjcId = msg_send![get_macos_class_global().post_delegate, new];
                    //(*post_delegate_instance).set_ivar("macos_app_ptr", GLOBAL_COCOA_APP as *mut _ as *mut c_void);
                    let nstimer: ObjcId = msg_send![
                        class!(NSTimer),
                        timerWithTimeInterval: 0.
                        target: post_delegate_instance
                        selector: sel!(receivedPost:)
                        userInfo: nil
                        repeats: false
                    ];
                    let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
                    let () = msg_send![nsrunloop, addTimer: nstimer forMode: NSRunLoopCommonModes];

                    let () = msg_send![pool, release];
                }
                else{
                    signals.insert(signal);
                }
            }

        }
    }*/

    /// Apply or suspend the pointer lock's PHYSICAL effects, in GLFW's
    /// proven order. Enable: hide → warp to the key window's centre (while
    /// still associated — warping a disassociated cursor stalls event
    /// delivery) → disassociate. Disable: re-associate → unhide. The logical
    /// `mouse_pointer_lock` flag is managed by the caller; this only touches
    /// the OS state, tracked in `pointer_lock_applied` so focus churn never
    /// double-hides or double-shows the cursor.
    pub fn apply_pointer_lock_effects(&mut self, on: bool) {
        // Only an ACTIVE app may take the user's pointer: a bridge-driven,
        // unfocused instance clicking its own fps view must never hide and
        // pin the cursor the user is using elsewhere. A real click activates
        // the app before it arrives here, so players are unaffected.
        if on {
            let active: bool = unsafe {
                let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                msg_send![ns_app, isActive]
            };
            if !active {
                self.pointer_lock_applied = false;
                return;
            }
        }
        if self.pointer_lock_applied == on {
            return;
        }
        self.pointer_lock_applied = on;
        // THE deadzone bug: after every CGWarpMouseCursorPosition, macOS
        // suppresses local hardware mouse events for 0.25s BY DEFAULT — so
        // a moving locked mouse (drift repins = warps) had rolling windows
        // where clicks reached no app at all, not even our own NSView's
        // mouseDown. SDL zeroes this interval in Cocoa_InitMouse for
        // exactly this reason; zero it before the first warp ever fires.
        unsafe {
            let _ = CGSetLocalEventsSuppressionInterval(0.0);
        }
        self.virtual_mouse = None;
        unsafe {
            if on {
                let () = msg_send![class!(NSCursor), hide];
                if let Some((win, _)) = self.cocoa_windows.first() {
                    let frame: NSRect = msg_send![*win, frame];
                    // Pin the cursor mid-way into the window's portion on
                    // the PRIMARY screen — deterministic, never the seam or
                    // an edge pixel (a cursor frozen on the screen edge has
                    // its clicks eaten by macOS edge behaviour before they
                    // ever reach the app). [win screen] was ambiguous on a
                    // straddling/mirrored window and degenerated to the
                    // frame centre = the exact edge.
                    let screens0: ObjcId = msg_send![class!(NSScreen), screens];
                    let wscreen: ObjcId = msg_send![screens0, firstObject];
                    let svis: NSRect = if wscreen != nil {
                        msg_send![wscreen, visibleFrame]
                    } else {
                        frame
                    };
                    let lo_x = frame.origin.x.max(svis.origin.x);
                    let hi_x = (frame.origin.x + frame.size.width)
                        .min(svis.origin.x + svis.size.width);
                    let lo_y = frame.origin.y.max(svis.origin.y);
                    let hi_y = (frame.origin.y + frame.size.height)
                        .min(svis.origin.y + svis.size.height);
                    let gx = if lo_x < hi_x { (lo_x + hi_x) * 0.5 } else { frame.origin.x + frame.size.width * 0.5 };
                    let gy_cocoa = if lo_y < hi_y { (lo_y + hi_y) * 0.5 } else { frame.origin.y + frame.size.height * 0.5 };
                    let screens: ObjcId = msg_send![class!(NSScreen), screens];
                    let primary: ObjcId = msg_send![screens, firstObject];
                    let sframe: NSRect = msg_send![primary, frame];
                    let _ = CGWarpMouseCursorPosition(NSPoint {
                        x: gx,
                        y: sframe.size.height - gy_cocoa,
                    });
                    self.lock_pin = Some(Vec2d { x: gx, y: gy_cocoa });
                }
                CGAssociateMouseAndMouseCursorPosition(0);
            } else {
                CGAssociateMouseAndMouseCursorPosition(1);
                let () = msg_send![class!(NSCursor), unhide];
            }
        }
    }

    /// Focus left the window (cmd-tab, a click elsewhere): whatever the
    /// game thinks, the OS pointer goes back to the user NOW. A lock held
    /// past focus loss — and re-pinned every frame — left the user with no
    /// mouse at all (2026-08-26). The game re-locks on its next click.
    pub fn release_pointer_lock_on_focus_loss(&mut self) {
        if self.mouse_pointer_lock || self.pointer_lock_applied {
            self.mouse_pointer_lock = false;
            self.pointer_lock_applied = false;
            self.lock_pin = None;
            // A pin lost to focus loss does not warp anywhere: the pointer
            // belongs to whoever has focus now. Just forget the restore.
            self.pin_restore = None;
            self.pointer_pin_mode = false;
            self.apply_pointer_lock_effects(false);
        }
    }

    /// Widget-scoped pointer pin (value scrubbing): the FPS lock machinery,
    /// but the cursor pins AT ITS CURRENT POSITION and is restored there on
    /// release. Engage only when a drag actually starts (the threshold
    /// crossing), never on the initial press; release on mouse-up. Deltas
    /// keep flowing (virtual mouse), `repin_pointer` holds the pin each
    /// frame, and focus loss releases it like any lock.
    pub fn set_pointer_pin(&mut self, on: bool) {
        if on {
            if self.mouse_pointer_lock || self.pointer_lock_applied {
                return; // an FPS-style lock is already holding the pointer
            }
            unsafe {
                let _ = CGSetLocalEventsSuppressionInterval(0.0);
            }
            self.virtual_mouse = None;
            self.mouse_pointer_lock = true;
            self.pointer_lock_applied = true;
            self.pointer_pin_mode = true;
            unsafe {
                let loc: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                self.lock_pin = Some(Vec2d { x: loc.x, y: loc.y });
                self.pin_restore = Some(Vec2d { x: loc.x, y: loc.y });
                let () = msg_send![class!(NSCursor), hide];
                CGAssociateMouseAndMouseCursorPosition(0);
            }
        } else {
            if !self.mouse_pointer_lock && !self.pointer_lock_applied {
                self.pin_restore = None;
                return;
            }
            self.mouse_pointer_lock = false;
            self.pointer_lock_applied = false;
            self.pointer_pin_mode = false;
            self.lock_pin = None;
            unsafe {
                CGAssociateMouseAndMouseCursorPosition(1);
                if let Some(restore) = self.pin_restore.take() {
                    let screens: ObjcId = msg_send![class!(NSScreen), screens];
                    let primary: ObjcId = msg_send![screens, firstObject];
                    let sframe: NSRect = msg_send![primary, frame];
                    let _ = CGWarpMouseCursorPosition(NSPoint {
                        x: restore.x,
                        y: sframe.size.height - restore.y,
                    });
                }
                let () = msg_send![class!(NSCursor), unhide];
            }
        }
    }

    /// Per-frame while locked: force the hardware cursor back onto the pin
    /// and re-assert the disassociation. On systems where the association
    /// silently drops (observed live: deadzones the size of the desktop
    /// minus the window), this is the enforcement that actually holds.
    /// Never while the app is inactive: the pointer belongs to whoever has
    /// focus, and repinning it from the background is a trap.
    pub fn repin_pointer(&mut self) {
        if !self.mouse_pointer_lock || !self.pointer_lock_applied {
            return;
        }
        let active: bool = unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            msg_send![ns_app, isActive]
        };
        if !active {
            return;
        }
        let Some(pin) = self.lock_pin else { return };
        unsafe {
            // mouseLocation is cocoa global (bottom-left origin, points).
            let loc: NSPoint = msg_send![class!(NSEvent), mouseLocation];
            let drift = (loc.x - pin.x).abs() + (loc.y - pin.y).abs();
            if drift > 2.0 {
                let screens: ObjcId = msg_send![class!(NSScreen), screens];
                let primary: ObjcId = msg_send![screens, firstObject];
                let sframe: NSRect = msg_send![primary, frame];
                let _ = CGWarpMouseCursorPosition(NSPoint {
                    x: pin.x,
                    y: sframe.size.height - pin.y,
                });
                CGAssociateMouseAndMouseCursorPosition(0);
            }
        }
    }

    pub fn set_mouse_cursor(&mut self, cursor: MouseCursor) {
        if self.current_cursor != cursor {
            self.current_cursor = cursor;
            // todo set it on all windows
            unsafe {
                for (window, view) in &self.cocoa_windows {
                    let _: () = msg_send![
                        *window,
                        invalidateCursorRectsForView: *view
                    ];
                }
            }
        }
    }

    /// Arm (or resume) display-link pacing: one link per window. On macOS 14+
    /// CAMetalDisplayLink is built from that view's CAMetalLayer so its update
    /// owns both the beat and drawable. Older systems take the existing
    /// NSView.displayLink path unchanged. Returns false when neither can run,
    /// so the caller falls back to NSTimer pacing.
    pub fn ensure_display_link(&mut self) -> bool {
        unsafe {
            // Prune links whose window is gone.
            let windows: Vec<ObjcId> = self.cocoa_windows.iter().map(|(w, _)| *w).collect();
            self.display_links.retain(|(window, link)| {
                if windows.contains(window) {
                    true
                } else {
                    let () = msg_send![*link, invalidate];
                    false
                }
            });
            // Create missing ones.
            for (window, view) in self.cocoa_windows.clone() {
                if self.display_links.iter().any(|(w, _)| *w == window) {
                    continue;
                }
                // Runtime availability is the contract here: referring to the
                // class by name keeps the binary loadable before macOS 14.
                let mut is_metal_link = false;
                let mut link = nil;
                // Opt-in until it paces at the display's rate: measured 11 fps
                // visible on 2026-08-25 against 62 fps on the CVDisplayLink path.
                let metal_link_wanted = std::env::var("MAKEPAD_METAL_DISPLAY_LINK")
                    .map(|v| v != "0")
                    .unwrap_or(false);
                if let Some(link_class) = Class::get("CAMetalDisplayLink").filter(|_| metal_link_wanted) {
                    let layer: ObjcId = msg_send![view, layer];
                    if layer != nil {
                        let allocated: ObjcId = msg_send![link_class, alloc];
                        link = msg_send![allocated, initWithMetalLayer: layer];
                        if link != nil {
                            let () = msg_send![link, setDelegate: self.timer_delegate_instance];
                            let default_range: CAFrameRateRange =
                                msg_send![link, preferredFrameRateRange];
                            let default_latency: isize =
                                msg_send![link, preferredFrameLatency];
                            let screen: ObjcId = msg_send![window, screen];
                            let maximum_fps: isize = if screen != nil {
                                msg_send![screen, maximumFramesPerSecond]
                            } else {
                                60
                            };
                            let requested_fps = maximum_fps.max(1) as f32;
                            let requested_range = CAFrameRateRange {
                                minimum: requested_fps,
                                maximum: requested_fps,
                                preferred: requested_fps,
                            };
                            // The defaults do not promise the panel maximum. Request it
                            // explicitly, and keep two frames of render latency against
                            // the CAMetalLayer's three-drawable pool.
                            let () = msg_send![link, setPreferredFrameRateRange: requested_range];
                            let () = msg_send![link, setPreferredFrameLatency: 2isize];
                            if metal_link_frame_trace_enabled() {
                                eprintln!(
                                    "[frame-trace] metal-link defaults rate={:.1}..{:.1}@{:.1} latency={} requested={:.1} latency=2",
                                    default_range.minimum,
                                    default_range.maximum,
                                    default_range.preferred,
                                    default_latency,
                                    requested_fps,
                                );
                            }
                            is_metal_link = true;
                        }
                    }
                }
                if link == nil {
                    let responds: bool = msg_send![
                        view,
                        respondsToSelector: sel!(displayLinkWithTarget: selector:)
                    ];
                    if !responds {
                        return false;
                    }
                    link = msg_send![
                        view,
                        displayLinkWithTarget: self.timer_delegate_instance
                        selector: sel!(receivedDisplayLink:)
                    ];
                }
                if link == nil {
                    continue;
                }
                let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
                let () =
                    msg_send![link, addToRunLoop: nsrunloop forMode: NSRunLoopCommonModes];
                if self.display_links_paused {
                    let () = msg_send![link, setPaused: YES];
                }
                self.display_links.push((window, link));
                crate::log!(
                    "macos: paint pacing on {} (frame-flip clock), window {}",
                    if is_metal_link { "CAMetalDisplayLink" } else { "CADisplayLink" },
                    self.display_links.len()
                );
            }
            if self.display_links_paused {
                for (_w, link) in &self.display_links {
                    let () = msg_send![*link, setPaused: NO];
                }
                self.display_links_paused = false;
            }
            !self.display_links.is_empty()
        }
    }

    pub fn pause_display_link(&mut self) {
        unsafe {
            if !self.display_links_paused {
                for (_w, link) in &self.display_links {
                    let () = msg_send![*link, setPaused: YES];
                }
                self.display_links_paused = true;
            }
        }
    }

    /// One window's display link fired: dispatch a LinkFire carrying WHICH
    /// window and the flip's TARGET timestamp mapped into app time — the
    /// rock-solid per-window clock the paint below samples.
    pub fn send_display_link_fired(link: ObjcId) {
        let Some((window, primary, app_now)) = try_with_macos_app(|app| {
            app.display_links.iter().position(|(_w, l)| *l == link).map(|i| {
                (app.display_links[i].0, i == 0, app.time_now())
            })
        }).flatten() else {
            return;
        };
        let (target, media_now): (f64, f64) = unsafe {
            (msg_send![link, targetTimestamp], CACurrentMediaTime())
        };
        let time = app_now + (target - media_now).clamp(0.0, 0.1);
        MacosApp::do_callback(MacosEvent::LinkFire {
            window,
            time,
            primary,
            drawable: None,
            target_presentation_time: target,
        });
    }

    /// CAMetalDisplayLink's delegate update is the authoritative frame:
    /// transport time comes from its presentation target, and rendering uses
    /// the drawable delivered for that same target instead of polling the
    /// layer. A re-entrant callback simply skips this update rather than
    /// panicking through the Objective-C delegate frame.
    pub fn send_metal_display_link_update(link: ObjcId, update: ObjcId) {
        metal_link_trace_update_fired();
        if update == nil {
            metal_link_trace_report();
            return;
        }
        let (target, target_presentation, drawable, media_now): (f64, f64, ObjcId, f64) =
            unsafe {
                (
                    msg_send![update, targetTimestamp],
                    msg_send![update, targetPresentationTimestamp],
                    msg_send![update, drawable],
                    CACurrentMediaTime(),
                )
            };
        let Some((window, primary, app_now)) = try_with_macos_app(|app| {
            app.display_links.iter().position(|(_w, l)| *l == link).map(|i| {
                (app.display_links[i].0, i == 0, app.time_now())
            })
        }).flatten() else {
            metal_link_trace_report();
            return;
        };
        let flip_target = if target_presentation > 0.0 {
            target_presentation
        } else {
            target
        };
        let time = app_now + (flip_target - media_now).clamp(-0.1, 0.1);
        MacosApp::do_callback(MacosEvent::LinkFire {
            window,
            time,
            primary,
            drawable: Some(drawable),
            target_presentation_time: flip_target,
        });
        metal_link_trace_report();
    }

    pub fn start_timer(&mut self, timer_id: u64, interval: f64, repeats: bool) {
        unsafe {
            let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];

            let nstimer: ObjcId = msg_send![
                class!(NSTimer),
                timerWithTimeInterval: interval
                target: self.timer_delegate_instance
                selector: sel!(receivedTimer:)
                userInfo: nil
                repeats: repeats
            ];
            let nsrunloop: ObjcId = msg_send![class!(NSRunLoop), mainRunLoop];
            let () = msg_send![nsrunloop, addTimer: nstimer forMode: NSRunLoopCommonModes];

            self.timers.push(CocoaTimer {
                timer_id: timer_id,
                nstimer: nstimer,
                repeats: repeats,
            });
            let () = msg_send![pool, release];
        }
    }

    pub fn stop_timer(&mut self, timer_id: u64) {
        for i in 0..self.timers.len() {
            if self.timers[i].timer_id == timer_id {
                unsafe {
                    let () = msg_send![self.timers[i].nstimer, invalidate];
                }
                self.timers.remove(i);
                return;
            }
        }
    }

    pub fn send_timer_received(nstimer: ObjcId) {
        let len = with_macos_app(|app| app.timers.len());
        for i in 0..len {
            let time = with_macos_app(|app| app.time_now());
            if with_macos_app(|app| app.timers[i].nstimer == nstimer) {
                let timer_id = with_macos_app(|app| app.timers[i].timer_id);
                if !with_macos_app(|app| app.timers[i].repeats) {
                    with_macos_app(|app| app.timers.remove(i));
                }

                MacosApp::do_callback(MacosEvent::Timer(TimerEvent {
                    time: Some(time),
                    timer_id: timer_id,
                }));
                // break the eventloop if its in blocked mode
                unsafe {
                    let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
                    let nsevent: ObjcId = msg_send![
                        class!(NSEvent),
                        otherEventWithType: NSEventType::NSApplicationDefined
                        location: NSPoint {x: 0., y: 0.}
                        modifierFlags: 0u64
                        timestamp: 0f64
                        windowNumber: 1u64
                        context: nil
                        subtype: 0i16
                        data1: 0u64
                        data2: 0u64
                    ];
                    let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    let () = msg_send![ns_app, postEvent: nsevent atStart: 0];
                    let () = msg_send![pool, release];
                }
                return;
            }
        }
    }
    /*
    pub fn send_signal_event(&mut self) {
        let signals = if let Ok(signals) = self.signals.lock(){
            let mut new_signals = HashSet::new();
            std::mem::swap(&mut *signals.borrow_mut(), &mut new_signals);
            new_signals
        }else{panic!()};

        self.do_callback(vec![
            MacosEvent::Signal(SignalEvent {
                signals,
            })
        ]);
        self.do_callback(vec![MacosEvent::Paint]);
    }*/

    pub fn send_command_event(command: LiveId) {
        with_macos_app(|app| app.menu_command_fired = true);
        MacosApp::do_callback(MacosEvent::MacosMenuCommand(command));
        MacosApp::do_callback(MacosEvent::Paint);
    }

    pub fn send_paint_event() {
        MacosApp::do_callback(MacosEvent::Paint);
    }
    /*
    #[cfg(target_os = "macos")]
    pub fn start_dragging(&mut self, items: Vec<DragItem>) {
       unsafe {
            let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
            let ns_event: ObjcId = msg_send![ns_app, currentEvent];

            let window: ObjcId = msg_send![ns_event, window];
            let window_delegate: ObjcId = msg_send![window, delegate];
            if window == nil {
                crate::error!("start_dragging: Cocoa window nil on event");
                return
            }
            let cocoa_window: *mut c_void = *(*window_delegate).get_ivar("macos_window_ptr");
            let cocoa_window = &mut *(cocoa_window as *mut MacosWindow);
            cocoa_window.start_dragging(ns_event, items);
        };
    }*/

    pub fn copy_to_clipboard(&mut self, content: &str) {
        unsafe {
            let pasteboard: ObjcId = self.pasteboard;
            let nsstring = str_to_nsstring(content);
            let array: ObjcId = msg_send![class!(NSArray), arrayWithObject: NSStringPboardType];
            let () = msg_send![pasteboard, declareTypes: array owner: nil];
            let () = msg_send![pasteboard, setString: nsstring forType: NSStringPboardType];
        }
    }

    pub fn open_save_file_dialog(&mut self, _settings: FileDialog) {
        println!("open save file dialog!");
    }

    pub fn open_select_file_dialog(&mut self, _settings: FileDialog) {
        println!("open select file dialog!");
    }

    pub fn open_save_folder_dialog(&mut self, _settings: FileDialog) {
        println!("open save folder dialog!");
    }

    /// Native folder picker (`NSOpenPanel`, directories only).
    ///
    /// The panel is NOT opened inline: `runModal` spins its own run loop, and
    /// this call runs from inside the platform-op drain, where the `Cx` is
    /// already borrowed. Deferring onto the main queue (same trick as
    /// [`MacosApp::defer_window_closed`]) puts the modal loop *between*
    /// callbacks, where nothing holds the borrow. The answer comes back as a
    /// [`FileDialogAction`], because by then the call that asked is long gone.
    pub fn open_select_folder_dialog(&mut self, settings: FileDialog) {
        let title = settings.title.clone().unwrap_or_default();
        let location = settings
            .location
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        unsafe {
            let main_thread_block = objc_block!(move || {
                let picked = run_select_folder_panel(&title, &location);
                Cx::post_action(match picked {
                    Some(path) => FileDialogAction::FolderSelected(path),
                    None => FileDialogAction::FolderCancelled,
                });
            });
            let main_queue: ObjcId = msg_send![class!(NSOperationQueue), mainQueue];
            let block_operation: ObjcId =
                msg_send![class!(NSBlockOperation), blockOperationWithBlock: &main_thread_block];
            let () = msg_send![main_queue, addOperation: block_operation];
        }
    }
}

/// `NSModalResponseOK`. Cancel is 0; anything else is "not a choice".
const NS_MODAL_RESPONSE_OK: i64 = 1;

/// Run the directory-only open panel to completion on the main thread.
/// `None` = the user cancelled (or the panel returned nothing usable).
fn run_select_folder_panel(title: &str, location: &str) -> Option<std::path::PathBuf> {
    unsafe {
        let panel: ObjcId = msg_send![class!(NSOpenPanel), openPanel];
        if panel == nil {
            return None;
        }
        // Files AND folders: one panel serves both "import this clip" and
        // "import this library" — the consumer's scan handles either.
        let () = msg_send![panel, setCanChooseFiles: YES];
        let () = msg_send![panel, setCanChooseDirectories: YES];
        let () = msg_send![panel, setAllowsMultipleSelection: NO];
        let () = msg_send![panel, setCanCreateDirectories: NO];
        if !title.is_empty() {
            let () = msg_send![panel, setMessage: str_to_nsstring(title)];
        }
        if !location.is_empty() {
            let url: ObjcId = msg_send![
                class!(NSURL),
                fileURLWithPath: str_to_nsstring(location)
                isDirectory: YES
            ];
            if url != nil {
                let () = msg_send![panel, setDirectoryURL: url];
            }
        }
        let response: i64 = msg_send![panel, runModal];
        if response != NS_MODAL_RESPONSE_OK {
            return None;
        }
        let url: ObjcId = msg_send![panel, URL];
        if url == nil {
            return None;
        }
        let path: ObjcId = msg_send![url, path];
        if path == nil {
            return None;
        }
        let path = nsstring_to_string(path);
        if path.is_empty() {
            return None;
        }
        Some(std::path::PathBuf::from(path))
    }
}
