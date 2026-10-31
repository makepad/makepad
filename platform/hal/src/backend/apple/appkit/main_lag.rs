//! The main-lag meter's clock on macOS (`backend::main_lag`): a run-loop timer in the common
//! modes (so also inside live resize, window drags and menu tracking) ticks it every 4 ms.
use crate::backend::main_lag;
use makepad_apple_sys::{CFRunLoopGetMain, CFRunLoopRef, CFStringRef};
use std::cell::Cell;
use std::os::raw::c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopTimerCreate(
        allocator: *const c_void,
        fire_date: f64,
        interval: f64,
        flags: u64,
        order: isize,
        callout: extern "C" fn(*mut c_void, *mut c_void),
        context: *mut c_void,
    ) -> *mut c_void;
    fn CFRunLoopAddTimer(rl: CFRunLoopRef, timer: *mut c_void, mode: CFStringRef);
    fn CFRunLoopTimerInvalidate(timer: *mut c_void);
    fn CFRelease(cf: *const c_void);
}

thread_local! {
    static TIMER: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
}

extern "C" fn tick(_: *mut c_void, _: *mut c_void) {
    main_lag::tick();
}

/// Starts the meter when the environment asks for it (main thread, once).
pub fn start() {
    if !main_lag::wanted() {
        return;
    }
    main_lag::start();
    unsafe {
        let interval = main_lag::TICK.as_secs_f64();
        let timer = CFRunLoopTimerCreate(std::ptr::null(), CFAbsoluteTimeGetCurrent() + interval, interval, 0, 0, tick, std::ptr::null_mut());
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        TIMER.set(timer);
    }
}

/// Ends the meter and prints its summary (main thread, when the main loop ends).
pub fn finish() {
    let timer = TIMER.replace(std::ptr::null_mut());
    if !timer.is_null() {
        unsafe {
            CFRunLoopTimerInvalidate(timer);
            CFRelease(timer);
        }
    }
    main_lag::finish();
}
