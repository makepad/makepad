//! The macOS desktop of Desk Host: ScreenCaptureKit for the screen (NV12 IOSurface frames, only
//! when the screen changed; no cursor in them), CoreGraphics events for input (posted at the HID
//! level, as a local keyboard and mouse), the cursor's position and shape apart from the video.
//!
//! Both need a one-time grant in System Settings > Privacy & Security for this app: Screen
//! Recording (the capture) and Accessibility (posting events). `permissions(true)` asks for
//! them; macOS opens the panes where the user enables "Desk Host".

use crate::log;
use makepad_desk_link::host::{clock_ns, Desktop, Frame, Picture};
use makepad_apple_sys::*;
use makepad_desk_link::server::{Input, InputFactory};
use makepad_desk_link::{button, GamepadState, ScreenInfo};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

type CGDirectDisplayID = u32;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut CGDirectDisplayID, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: CGDirectDisplayID) -> NSRect;
    fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> *mut c_void;
    fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *mut c_void) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: *mut c_void) -> f64;
    fn CGDisplayModeRelease(mode: *mut c_void);
    fn CGEventCreate(source: ObjcId) -> ObjcId;
    fn CGEventGetLocation(event: ObjcId) -> NSPoint;
    fn CGEventCreateKeyboardEvent(source: ObjcId, key: u16, down: bool) -> ObjcId;
    fn CGEventSetFlags(event: ObjcId, flags: u64);
    /// Not variadic (the macOS 10.13+ form of CGEventCreateScrollWheelEvent).
    fn CGEventCreateScrollWheelEvent2(source: ObjcId, units: u32, wheel_count: u32, wheel1: i32, wheel2: i32, wheel3: i32) -> ObjcId;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    static kCGDisplayStreamYCbCrMatrix_ITU_R_709_2: CFStringRef;
    static kCGColorSpaceSRGB: CFStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXIsProcessTrusted() -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferGetIOSurface(pixel_buffer: CVPixelBufferRef) -> IOSurfaceRef;
}

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {
    static SCStreamFrameInfoStatus: ObjcId;
}

/// Why there is no picture / no input: the grant and where the user gives it (said in the log, the
/// status file and to the viewer).
pub const SCREEN_MISSING: &str = "Screen Recording is not granted to Desk Host (on that Mac: System Settings > Privacy & Security > Screen & System Audio Recording: turn on \"Desk Host\", or add it with +)";
pub const INPUT_MISSING: &str = "Accessibility is not granted to Desk Host, so viewers can only watch (on that Mac: System Settings > Privacy & Security > Accessibility: turn on \"Desk Host\", or add it with +)";

/// kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange ('420v'): NV12, BT.709 limited range.
const PIXEL_420V: u32 = four_char_as_u32("420v");

fn displays() -> Vec<CGDirectDisplayID> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut count) };
    ids[..count as usize].to_vec()
}

fn pixel_size(id: CGDirectDisplayID) -> (u32, u32) {
    unsafe {
        let mode = CGDisplayCopyDisplayMode(id);
        if mode.is_null() {
            let b = CGDisplayBounds(id);
            return (b.size.width as u32, b.size.height as u32);
        }
        let size = (CGDisplayModeGetPixelWidth(mode) as u32, CGDisplayModeGetPixelHeight(mode) as u32);
        CGDisplayModeRelease(mode);
        size
    }
}

/// An autorelease pool for one pass of the host's loop: it runs no AppKit event loop, so
/// nothing else drains what AppKit and VideoToolbox autorelease (the cursor queries alone, 125
/// times a second, grew the host to 3 GB in an afternoon).
pub struct Pool(*mut c_void);

impl Pool {
    pub fn new() -> Pool {
        Pool(unsafe { makepad_objc_sys::runtime::objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { makepad_objc_sys::runtime::objc_autoreleasePoolPop(self.0) }
    }
}

/// A CoreFoundation object kept alive while a frame waits for the encoder.
struct Retained(*const c_void);
unsafe impl Send for Retained {}
impl Drop for Retained {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

struct Sink {
    frames: Sender<Frame>,
}

fn output_class() -> *const Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        extern "C" fn did_output(this: &Object, _sel: Sel, _stream: ObjcId, sample: ObjcId, ty: i64) {
            // 0 = SCStreamOutputTypeScreen
            if ty != 0 {
                return;
            }
            unsafe {
                let sink: *mut c_void = *this.get_ivar("sink");
                if sink.is_null() {
                    return;
                }
                let sink = &*(sink as *const Mutex<Sink>);
                // only complete frames carry a picture (idle ones say nothing changed)
                let attachments = CMSampleBufferGetSampleAttachmentsArray(sample as CMSampleBufferRef, 0);
                if attachments.is_null() || CFArrayGetCount(attachments) == 0 {
                    return;
                }
                let info = CFArrayGetValueAtIndex(attachments, 0) as CFDictionaryRef;
                let status = CFDictionaryGetValue(info, SCStreamFrameInfoStatus as *const c_void) as ObjcId;
                if status == nil {
                    return;
                }
                let status: i64 = msg_send![status, integerValue];
                if status != 0 {
                    return;
                }
                let pixel_buffer = CMSampleBufferGetImageBuffer(sample as CMSampleBufferRef);
                if pixel_buffer.is_null() {
                    return;
                }
                let surface = CVPixelBufferGetIOSurface(pixel_buffer);
                if surface.is_null() {
                    return;
                }
                let (width, height) = (CVPixelBufferGetWidth(pixel_buffer) as u32, CVPixelBufferGetHeight(pixel_buffer) as u32);
                let frame = Frame {
                    picture: Picture::Shared(makepad_video::SurfaceHandle::new(IOSurfaceGetID(surface) as u64, makepad_video::SurfaceKind::IoSurface, [width, height], makepad_video::SurfaceFormat::Nv12)),
                    width,
                    height,
                    captured_ns: clock_ns(),
                    _hold: Box::new(Retained(CFRetain(pixel_buffer as *const c_void))),
                };
                let _ = sink.lock().unwrap().frames.send(frame);
            }
        }
        let mut decl = ClassDecl::new("MakepadDeskHostStreamOutput", class!(NSObject)).unwrap();
        unsafe {
            decl.add_method(sel!(stream:didOutputSampleBuffer:ofType:), did_output as extern "C" fn(&Object, Sel, ObjcId, ObjcId, i64));
        }
        decl.add_ivar::<*mut c_void>("sink");
        decl.register() as *const Class as usize
    }) as *const Class
}

struct Capture {
    stream: Arc<Mutex<ObjcId>>,
    output: ObjcId,
    sink: *mut Mutex<Sink>,
}

pub struct MacDesktop {
    capture: Option<Capture>,

    /// The bounds (points, global) of the screen viewers watch: where their input lands.
    target: Arc<Mutex<NSRect>>,
    standard_cursors: Vec<(&'static str, NSPoint, NSSize)>,
}

/// The command-line modes (`--check`, `--capture-test`, ...): AppKit for the cursor queries, no
/// window, no Dock icon, no menu-bar item. The tray app has AppKit from its window system.
pub fn cli_app() {
    unsafe {
        let app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![app, setActivationPolicy: 2i64];
    }
}

impl MacDesktop {
    pub fn new() -> MacDesktop {
        let mut standard_cursors = Vec::new();
        for (sel_name, shape) in [
            // the arrow first: cursors with its hot spot and size (not-allowed) cannot be told
            // apart from it this way, and the arrow is by far the likelier one
            ("arrowCursor", "Default"),
            ("IBeamCursor", "Text"),
            ("pointingHandCursor", "Hand"),
            ("crosshairCursor", "Crosshair"),
            ("openHandCursor", "Grab"),
            ("closedHandCursor", "Grabbing"),
            ("resizeLeftRightCursor", "EwResize"),
            ("resizeUpDownCursor", "NsResize"),
            ("operationNotAllowedCursor", "NotAllowed"),
        ] {
            unsafe {
                let cursor: ObjcId = match sel_name {
                    "IBeamCursor" => msg_send![class!(NSCursor), IBeamCursor],
                    "pointingHandCursor" => msg_send![class!(NSCursor), pointingHandCursor],
                    "crosshairCursor" => msg_send![class!(NSCursor), crosshairCursor],
                    "openHandCursor" => msg_send![class!(NSCursor), openHandCursor],
                    "closedHandCursor" => msg_send![class!(NSCursor), closedHandCursor],
                    "resizeLeftRightCursor" => msg_send![class!(NSCursor), resizeLeftRightCursor],
                    "resizeUpDownCursor" => msg_send![class!(NSCursor), resizeUpDownCursor],
                    "operationNotAllowedCursor" => msg_send![class!(NSCursor), operationNotAllowedCursor],
                    _ => msg_send![class!(NSCursor), arrowCursor],
                };
                if cursor == nil {
                    continue;
                }
                let hot: NSPoint = msg_send![cursor, hotSpot];
                let image: ObjcId = msg_send![cursor, image];
                let size: NSSize = msg_send![image, size];
                standard_cursors.push((shape, hot, size));
            }
        }
        MacDesktop { capture: None, target: Arc::new(Mutex::new(NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 1.0, height: 1.0 } })), standard_cursors }
    }

    /// The grants this app needs, asking for the missing ones when `ask` (macOS shows its
    /// prompt and the System Settings pane once: only on `--ask`, the user's own call; the
    /// agent never asks). A line saying where they stand.
    pub fn permissions(&self, ask: bool) -> String {
        unsafe {
            let mut screen = CGPreflightScreenCaptureAccess();
            if !screen && ask {
                screen = CGRequestScreenCaptureAccess();
            }
            let keys = [kAXTrustedCheckOptionPrompt as *const c_void];
            let values = [if ask { kCFBooleanTrue } else { kCFBooleanFalse } as *const c_void];
            let options = CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 1, std::ptr::null(), std::ptr::null());
            let input = AXIsProcessTrustedWithOptions(options);
            CFRelease(options as *const c_void);
            format!(
                "permissions: Screen Recording {}, Accessibility {}{}",
                if screen { "granted" } else { "MISSING" },
                if input { "granted" } else { "MISSING" },
                if screen && input { "" } else { " (System Settings > Privacy & Security: turn on \"Desk Host\" under Screen & System Audio Recording and under Accessibility)" }
            )
        }
    }
}

impl Desktop for MacDesktop {
    fn screens(&mut self) -> Vec<ScreenInfo> {
        displays()
            .into_iter()
            .enumerate()
            .map(|(i, id)| {
                let b = unsafe { CGDisplayBounds(id) };
                let (w, h) = pixel_size(id);
                let scale = w as f64 / b.size.width.max(1.0);
                ScreenInfo { name: format!("display {}", i + 1), x: (b.origin.x * scale).max(0.0) as u32, y: (b.origin.y * scale).max(0.0) as u32, width: w, height: h }
            })
            .collect()
    }

    fn refresh(&mut self, index: usize) -> u32 {
        let Some(&id) = displays().get(index) else { return 0 };
        unsafe {
            let mode = CGDisplayCopyDisplayMode(id);
            if mode.is_null() {
                return 0;
            }
            let hz = CGDisplayModeGetRefreshRate(mode);
            CGDisplayModeRelease(mode);
            hz.round().max(0.0) as u32
        }
    }

    fn grants(&mut self) -> String {
        self.permissions(false)
    }

    /// Each turn of the host loop drains its own autorelease pool (the capture's and the
    /// encoder's Objective-C objects).
    fn turn(&mut self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(Pool::new()))
    }

    fn capture(&mut self, index: usize, fps: u32, frames: Sender<Frame>) -> Result<(), String> {
        self.stop_capture();
        // checked first: without the grant ScreenCaptureKit would show macOS's prompt, and the
        // grants are the user's to give (System Settings), never asked for by the agent
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            return Err(SCREEN_MISSING.into());
        }
        let ids = displays();
        let id = *ids.get(index).ok_or("no such screen")?;
        *self.target.lock().unwrap() = unsafe { CGDisplayBounds(id) };
        let (width, height) = pixel_size(id);
        let sink = Box::into_raw(Box::new(Mutex::new(Sink { frames })));
        let stream = Arc::new(Mutex::new(nil));
        unsafe {
            let output: ObjcId = msg_send![output_class(), alloc];
            let output: ObjcId = msg_send![output, init];
            (*output).set_ivar("sink", sink as *mut c_void);
            let (stream2, output2) = (stream.clone(), output as usize);
            let interval = CMTime { value: 1, timescale: fps.clamp(1, 240) as CMTimeScale, flags: kCMTimeFlags_Valid, epoch: 0 };
            let completion = objc_block!(move |content: ObjcId, error: ObjcId| {
                if error != nil || content == nil {
                    let text = if error != nil { nsstring_to_string(msg_send![error, localizedDescription]) } else { "no shareable content".into() };
                    log(&format!("capture: {text} (Screen Recording permission?)"));
                    return;
                }
                let list: ObjcId = msg_send![content, displays];
                let count: usize = msg_send![list, count];
                let mut display = nil;
                for i in 0..count {
                    let d: ObjcId = msg_send![list, objectAtIndex: i];
                    let did: u32 = msg_send![d, displayID];
                    if did == id {
                        display = d;
                    }
                }
                if display == nil {
                    log("capture: the screen is gone");
                    return;
                }
                let empty: ObjcId = msg_send![class!(NSArray), array];
                let filter: ObjcId = msg_send![class!(SCContentFilter), alloc];
                let filter: ObjcId = msg_send![filter, initWithDisplay: display excludingWindows: empty];
                let config: ObjcId = msg_send![class!(SCStreamConfiguration), alloc];
                let config: ObjcId = msg_send![config, init];
                let () = msg_send![config, setWidth: width as usize];
                let () = msg_send![config, setHeight: height as usize];
                let () = msg_send![config, setPixelFormat: PIXEL_420V];
                let () = msg_send![config, setColorMatrix: kCGDisplayStreamYCbCrMatrix_ITU_R_709_2];
                // sRGB samples (BT.709 primaries), what the stream says and every viewer shows. Unset, the
                // capture keeps the display's own colour space (Display P3 or a wide monitor's profile), whose
                // numbers shown as sRGB come out pale.
                let () = msg_send![config, setColorSpaceName: kCGColorSpaceSRGB];
                let () = msg_send![config, setShowsCursor: NO];
                let () = msg_send![config, setQueueDepth: 4i64];
                // at most `fps` frames a second (ScreenCaptureKit's own default is 60)
                let () = msg_send![config, setMinimumFrameInterval: interval];
                let s: ObjcId = msg_send![class!(SCStream), alloc];
                let s: ObjcId = msg_send![s, initWithFilter: filter configuration: config delegate: nil];
                if s == nil {
                    log("capture: no stream");
                    return;
                }
                let queue = dispatch_queue_create(b"nl.makepad.deskhost.capture\0".as_ptr(), nil);
                let mut add_error: ObjcId = nil;
                let added: bool = msg_send![s, addStreamOutput: output2 as ObjcId type: 0i64 sampleHandlerQueue: queue error: &mut add_error];
                if !added {
                    log("capture: the stream takes no output");
                    return;
                }
                let started = objc_block!(move |e: ObjcId| {
                    if e != nil {
                        log(&format!("capture: {}", nsstring_to_string(msg_send![e, localizedDescription])));
                    } else {
                        log(&format!("capture: screen {index} at {width}x{height}, up to {fps} frames a second"));
                    }
                });
                let () = msg_send![s, startCaptureWithCompletionHandler: &started];
                *stream2.lock().unwrap() = s;
            });
            let () = msg_send![class!(SCShareableContent), getShareableContentExcludingDesktopWindows: NO onScreenWindowsOnly: YES completionHandler: &completion];
            self.capture = Some(Capture { stream, output, sink });
        }
        Ok(())
    }

    fn stop_capture(&mut self) {
        if let Some(c) = self.capture.take() {
            unsafe {
                let s = *c.stream.lock().unwrap();
                if s != nil {
                    let stopped = objc_block!(move |_e: ObjcId| {});
                    let () = msg_send![s, stopCaptureWithCompletionHandler: &stopped];
                }
                // the output object stays (its queue may still deliver a frame); its sink is
                // emptied so nothing is sent anymore
                (*c.output).set_ivar("sink", std::ptr::null_mut::<c_void>());
                let _ = Box::from_raw(c.sink);
            }
        }
    }

    fn cursor(&mut self, _index: usize) -> (u16, u16, bool, u8) {
        let bounds = *self.target.lock().unwrap();
        unsafe {
            let event = CGEventCreate(nil);
            let at = CGEventGetLocation(event);
            CFRelease(event as *const c_void);
            let nx = (at.x - bounds.origin.x) / bounds.size.width.max(1.0);
            let ny = (at.y - bounds.origin.y) / bounds.size.height.max(1.0);
            let inside = (0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny);
            let mut shape = "Default";
            let current: ObjcId = msg_send![class!(NSCursor), currentSystemCursor];
            if current != nil {
                let hot: NSPoint = msg_send![current, hotSpot];
                let image: ObjcId = msg_send![current, image];
                let size: NSSize = msg_send![image, size];
                if let Some((name, _, _)) = self.standard_cursors.iter().find(|(_, h, s)| (h.x - hot.x).abs() < 0.5 && (h.y - hot.y).abs() < 0.5 && (s.width - size.width).abs() < 0.5 && (s.height - size.height).abs() < 0.5) {
                    shape = name;
                }
            }
            ((nx.clamp(0.0, 1.0) * 65535.0) as u16, (ny.clamp(0.0, 1.0) * 65535.0) as u16, inside, makepad_desk_link::cursor_index(shape))
        }
    }

    fn input(&self) -> InputFactory {
        let target = self.target.clone();
        // checked per session: a grant given while the host runs counts from the next viewer
        Arc::new(move || {
            if !unsafe { AXIsProcessTrusted() } {
                return Err(INPUT_MISSING.into());
            }
            Ok(Box::new(MacInput { target: target.clone(), at: NSPoint { x: 0.0, y: 0.0 }, held: 0, clicks: (Instant::now(), NSPoint { x: -1e9, y: -1e9 }, 0), wheel: [0, 0], flags: 0 }) as Box<dyn Input>)
        })
    }
}

/// makepad `KeyCode` -> macOS virtual key code (`kVK_*`): the inverse of the AppKit backend's
/// map (hal/backend/apple/appkit/keys.rs), plus the modifiers.
fn vk_of(key: makepad_key_code::KeyCode) -> Option<u16> {
    use makepad_key_code::KeyCode::*;
    Some(match key {
        KeyA => 0x00, KeyS => 0x01, KeyD => 0x02, KeyF => 0x03, KeyH => 0x04, KeyG => 0x05, KeyZ => 0x06, KeyX => 0x07, KeyC => 0x08, KeyV => 0x09,
        KeyB => 0x0b, KeyQ => 0x0c, KeyW => 0x0d, KeyE => 0x0e, KeyR => 0x0f, KeyY => 0x10, KeyT => 0x11, Key1 => 0x12, Key2 => 0x13, Key3 => 0x14,
        Key4 => 0x15, Key6 => 0x16, Key5 => 0x17, Equals => 0x18, Key9 => 0x19, Key7 => 0x1a, Minus => 0x1b, Key8 => 0x1c, Key0 => 0x1d,
        RBracket => 0x1e, KeyO => 0x1f, KeyU => 0x20, LBracket => 0x21, KeyI => 0x22, KeyP => 0x23, ReturnKey => 0x24, KeyL => 0x25, KeyJ => 0x26,
        Quote => 0x27, KeyK => 0x28, Semicolon => 0x29, Backslash => 0x2a, Comma => 0x2b, Slash => 0x2c, KeyN => 0x2d, KeyM => 0x2e, Period => 0x2f,
        Tab => 0x30, Space => 0x31, Backtick => 0x32, Backspace => 0x33, Escape => 0x35, Logo => 0x37, Shift => 0x38, Capslock => 0x39, Alt => 0x3a,
        Control => 0x3b, NumpadDecimal => 0x41, NumpadMultiply => 0x43, NumpadAdd => 0x45, Numlock => 0x47, NumpadDivide => 0x4b,
        NumpadEnter => 0x4c, NumpadSubtract => 0x4e, NumpadEquals => 0x51, Numpad0 => 0x52, Numpad1 => 0x53, Numpad2 => 0x54, Numpad3 => 0x55,
        Numpad4 => 0x56, Numpad5 => 0x57, Numpad6 => 0x58, Numpad7 => 0x59, Numpad8 => 0x5b, Numpad9 => 0x5c, F5 => 0x60, F6 => 0x61, F7 => 0x62,
        F3 => 0x63, F8 => 0x64, F9 => 0x65, F11 => 0x67, PrintScreen => 0x69, F10 => 0x6d, F12 => 0x6f, Insert => 0x72, Home => 0x73,
        PageUp => 0x74, Delete => 0x75, F4 => 0x76, End => 0x77, F2 => 0x78, PageDown => 0x79, F1 => 0x7a, ArrowLeft => 0x7b, ArrowRight => 0x7c,
        ArrowDown => 0x7d, ArrowUp => 0x7e,
        _ => return None,
    })
}

/// CGEventFlags of the modifiers held.
const FLAG_SHIFT: u64 = 0x20000;
const FLAG_CONTROL: u64 = 0x40000;
const FLAG_ALT: u64 = 0x80000;
const FLAG_COMMAND: u64 = 0x100000;

struct MacInput {
    target: Arc<Mutex<NSRect>>,
    at: NSPoint,
    /// Buttons held (bit = the protocol's button number).
    held: u32,
    /// The last press: when, where, its click count (double clicks are counted here).
    clicks: (Instant, NSPoint, i64),
    /// Wheel motion below a point, carried over.
    wheel: [i32; 2],
    flags: u64,
}

unsafe impl Send for MacInput {}

impl MacInput {
    fn post(&self, event: ObjcId) {
        if event == nil {
            return;
        }
        unsafe {
            CGEventPost(0, event);
            CFRelease(event as *const c_void);
        }
    }

    fn mouse(&self, ty: u32, cg_button: u32, clicks: i64) {
        unsafe {
            let e = CGEventCreateMouseEvent(nil, ty, self.at, cg_button);
            if e != nil && clicks > 0 {
                CGEventSetIntegerValueField(e, kCGMouseEventClickState, clicks as u64);
            }
            if e != nil {
                CGEventSetFlags(e, self.flags);
            }
            self.post(e);
        }
    }
}

impl Input for MacInput {
    fn pointer(&mut self, x: u16, y: u16) {
        let b = *self.target.lock().unwrap();
        self.at = NSPoint { x: b.origin.x + x as f64 / 65535.0 * b.size.width, y: b.origin.y + y as f64 / 65535.0 * b.size.height };
        // a drag while a button is held (the receiving app tracks it as one)
        let (ty, cg) = if self.held & 1 != 0 {
            (6, 0)
        } else if self.held & 2 != 0 {
            (7, 1)
        } else if self.held != 0 {
            (27, 2)
        } else {
            (5, 0)
        };
        self.mouse(ty, cg, 0);
    }

    fn button(&mut self, down: bool, b: u8) {
        let (down_ty, up_ty, cg) = match b {
            button::LEFT => (1, 2, 0),
            button::RIGHT => (3, 4, 1),
            button::MIDDLE => (25, 26, 2),
            button::BACK => (25, 26, 3),
            button::FORWARD => (25, 26, 4),
            _ => return,
        };
        if down {
            let (when, at, count) = self.clicks;
            let near = (at.x - self.at.x).abs() < 4.0 && (at.y - self.at.y).abs() < 4.0;
            let count = if near && when.elapsed().as_secs_f64() < 0.4 { count + 1 } else { 1 };
            self.clicks = (Instant::now(), self.at, count);
            self.held |= 1 << b;
            self.mouse(down_ty, cg, count);
        } else {
            self.held &= !(1 << b);
            self.mouse(up_ty, cg, self.clicks.2.max(1));
        }
    }

    fn scroll(&mut self, dx: i32, dy: i32) {
        // 1/120 notches -> points (a notch is 40 points); wheel 1 is vertical, + = away (up)
        self.wheel[0] += dy * 40;
        self.wheel[1] += -dx * 40;
        let points = [self.wheel[0] / 120, self.wheel[1] / 120];
        self.wheel[0] -= points[0] * 120;
        self.wheel[1] -= points[1] * 120;
        if points != [0, 0] {
            unsafe {
                let e = CGEventCreateScrollWheelEvent2(nil, 0, 2, points[0], points[1], 0);
                self.post(e);
            }
        }
    }

    fn key(&mut self, down: bool, keycode: u32) {
        let key = makepad_key_code::KeyCode::from_index(keycode);
        let Some(vk) = vk_of(key) else { return };
        let flag = match key {
            makepad_key_code::KeyCode::Shift => FLAG_SHIFT,
            makepad_key_code::KeyCode::Control => FLAG_CONTROL,
            makepad_key_code::KeyCode::Alt => FLAG_ALT,
            makepad_key_code::KeyCode::Logo => FLAG_COMMAND,
            _ => 0,
        };
        if down {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
        unsafe {
            let e = CGEventCreateKeyboardEvent(nil, vk, down);
            if e != nil {
                CGEventSetFlags(e, self.flags);
            }
            self.post(e);
        }
    }

    /// macOS offers no virtual gamepad without a driver entitlement: left out.
    fn gamepad(&mut self, _slot: u8, _state: &GamepadState) {}
    fn gamepad_gone(&mut self, _slot: u8) {}
}
