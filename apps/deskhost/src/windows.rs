//! The Windows desktop of Desk Host: DXGI desktop duplication for the screen (GPU frames, only
//! when the desktop presented something new; the pointer is not in them), SendInput for input
//! (as a local keyboard and mouse), the cursor's position and shape apart from the video.
//!
//! The duplicated frame is copied, on the GPU, into a keyed-mutex shared texture named the way
//! the HAL names its shared textures (`Local\makepad-hal-<pid>-<serial>`), which the stream
//! encoder (`makepad_video`, Windows backend: `makepad_hal::shared_texture`) reads by its key on
//! its own device. The capture has a device of its own on the output's GPU: a duplication waits
//! in AcquireNextFrame (up to its timeout) holding its device's lock, and the encoder's calls on
//! a shared device waited behind it (pictures took 100-300 ms instead of a few).
//!
//! Duplication works only in the logged-in user's interactive session (winsta0\default), never
//! in session 0: the host runs from an at-logon task of the user. It also needs the session on
//! the console: a session shown over Remote Desktop has only the remote display adapter (no GPU
//! output, no hardware encoder), a disconnected one no display at all. When a viewer wants to
//! watch such a session, the host runs the task `Makepad Desk Host console` (installed by
//! setup.ps1, runs as SYSTEM: `tscon <session> /dest:console`), which moves it to the console:
//! the Remote Desktop client is disconnected, the desktop and its apps stay as they are.
//!
//! The process is per-monitor DPI aware: the duplication's desktop coordinates, the cursor's
//! position and SendInput's virtual desktop are all physical pixels.

use crate::log;
use makepad_desk_link::host::{clock_ns, Desktop, Frame, Picture};
use makepad_desk_link::server::{Input, InputFactory};
use makepad_desk_link::{button, GamepadState, ScreenInfo};
use makepad_windows_sys as windows;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The shared picture texture the duplicated screen is copied into, by the HAL's key
/// (pid << 32 | serial): a new serial for each size, the old one closed.
struct Slot {
    texture: ID3D11Texture2D,
    mutex: IDXGIKeyedMutex,
    handle: windows::Win32::Foundation::HANDLE,
    key: u64,
    width: u32,
    height: u32,
}

impl Drop for Slot {
    fn drop(&mut self) {
        makepad_hal::shared_texture::release(self.key);
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

fn outputs() -> Vec<(IDXGIOutput, DXGI_OUTPUT_DESC)> {
    adapter_outputs().into_iter().map(|(_, o, d)| (o, d)).collect()
}

/// The desktop's outputs with the GPU each is on.
fn adapter_outputs() -> Vec<(IDXGIAdapter1, IDXGIOutput, DXGI_OUTPUT_DESC)> {
    let mut out = Vec::new();
    unsafe {
        let Ok(raw) = CreateDXGIFactory1(&IDXGIFactory1::IID) else { return out };
        let Ok(factory) = IDXGIFactory1::from_raw(raw) else { return out };
        let mut a = 0;
        loop {
            let mut adapter: Option<IDXGIAdapter1> = None;
            if factory.EnumAdapters1(a, &mut adapter).is_err() {
                break;
            }
            let Some(adapter) = adapter else { break };
            let mut o = 0;
            loop {
                let mut output: Option<IDXGIOutput> = None;
                if adapter.EnumOutputs(o, &mut output).is_err() {
                    break;
                }
                let Some(output) = output else { break };
                if let Ok(desc) = output.GetDesc() {
                    if desc.AttachedToDesktop.as_bool() {
                        out.push((adapter.clone(), output, desc));
                    }
                }
                o += 1;
            }
            a += 1;
        }
    }
    out
}

fn rect_size(r: &RECT) -> (u32, u32) {
    ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
}

struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub struct WinDesktop {
    capture: Option<Capture>,
    /// The streamed output's rectangle on the virtual desktop (pixels).
    target: Arc<Mutex<RECT>>,
    cursors: Vec<(isize, &'static str)>,
    /// The captured screen's refresh (Hz, from its duplication; 0 until known) and its index.
    hz: Arc<AtomicU32>,
    captured: Option<usize>,
}

mod session;

impl WinDesktop {
    pub fn new() -> WinDesktop {
        // physical pixels everywhere (capture, cursor, input); fails only when already set
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        log(&session::status(false));
        let mut cursors = Vec::new();
        for (id, shape) in [
            (IDC_ARROW, "Default"),
            (IDC_IBEAM, "Text"),
            (IDC_HAND, "Hand"),
            (IDC_CROSS, "Crosshair"),
            (IDC_SIZEWE, "EwResize"),
            (IDC_SIZENS, "NsResize"),
            (IDC_SIZENWSE, "NwseResize"),
            (IDC_SIZENESW, "NeswResize"),
            (IDC_SIZEALL, "Move"),
            (IDC_NO, "NotAllowed"),
            (IDC_WAIT, "Wait"),
        ] {
            if let Ok(c) = unsafe { LoadCursorW(None, id) } {
                cursors.push((c.0 as isize, shape));
            }
        }
        WinDesktop { capture: None, target: Arc::new(Mutex::new(RECT::default())), cursors, hz: Arc::new(AtomicU32::new(0)), captured: None }
    }
}

/// The capture thread: duplication of `output` into the shared slot, a frame per new picture.
/// The refresh rate of a duplication's mode, rounded (Hz).
unsafe fn mode_hz(dup: &IDXGIOutputDuplication) -> u32 {
    let mut d = DXGI_OUTDUPL_DESC::default();
    dup.GetDesc(&mut d);
    let r = d.ModeDesc.RefreshRate;
    if r.Denominator == 0 { 0 } else { (r.Numerator + r.Denominator / 2) / r.Denominator }
}

fn capture_loop(adapter: IDXGIAdapter1, output: IDXGIOutput, frames: Sender<Frame>, stop: Arc<AtomicBool>, hz: Arc<AtomicU32>) -> Result<(), String> {
    unsafe {
        use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
        let mut device: Option<ID3D11Device> = None;
        let mut ctx: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            Some(adapter.as_IDXGIAdapter()),
            D3D_DRIVER_TYPE_UNKNOWN,
            None,
            D3D11_CREATE_DEVICE_FLAG(D3D11_CREATE_DEVICE_BGRA_SUPPORT.0),
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )
        .map_err(|e| format!("D3D11CreateDevice (capture): {e:?}"))?;
        let device = device.ok_or("D3D11CreateDevice (capture) returned no device")?;
        let ctx = ctx.ok_or("D3D11CreateDevice (capture) returned no context")?;
        let output1 = IDXGIOutput1::query(output.as_raw()).map_err(|e| format!("IDXGIOutput1: {e:?}"))?;
        let mut dup = Some(output1.DuplicateOutput(device.as_IUnknown()).map_err(|e| format!("DuplicateOutput: {e:?} (the host must run in the user's session, on the desktop's GPU)"))?);
        hz.store(mode_hz(dup.as_ref().unwrap()), Ordering::Release);
        let mut serial = 0u64;
        let mut slot: Option<Slot> = None;
        let mut said = String::new();
        while !stop.load(Ordering::Acquire) {
            // lost (a mode change, the secure desktop of a UAC prompt or the lock screen, the
            // session gone to Remote Desktop): duplicate again until it works
            let Some(d) = dup.as_ref() else {
                std::thread::sleep(std::time::Duration::from_millis(250));
                match output1.DuplicateOutput(device.as_IUnknown()) {
                    Ok(d) => {
                        hz.store(mode_hz(&d), Ordering::Release);
                        if !said.is_empty() {
                            log("capture: the screen is back");
                            said.clear();
                        }
                        dup = Some(d);
                    }
                    Err(e) => {
                        let why = format!("{e:?}; capture will retry (see session status)");
                        if why != said {
                            log(&format!("capture: waits for the screen ({why})"));
                            said = why;
                        }
                    }
                }
                continue;
            };
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match d.AcquireNextFrame(100, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e == DXGI_ERROR_WAIT_TIMEOUT => continue,
                Err(e) if e == DXGI_ERROR_ACCESS_LOST || e == DXGI_ERROR_INVALID_CALL => {
                    dup = None;
                    continue;
                }
                Err(e) => return Err(format!("AcquireNextFrame: {e:?}")),
            }
            let d = dup.as_ref().unwrap();
            // a pointer-only update has no new picture
            if info.LastPresentTime != 0 {
                if let Some(resource) = resource {
                    let texture = ID3D11Texture2D::query(resource.as_raw()).map_err(|e| format!("ID3D11Texture2D: {e:?}"))?;
                    let mut td = D3D11_TEXTURE2D_DESC::default();
                    texture.GetDesc(&mut td);
                    if slot.as_ref().is_none_or(|s| s.width != td.Width || s.height != td.Height) {
                        serial += 1;
                        let key = ((std::process::id() as u64) << 32) | serial;
                        slot = Some(make_slot(&device, key, td.Width, td.Height)?);
                    }
                    let s = slot.as_ref().unwrap();
                    if s.mutex.AcquireSync(0, 100).is_ok() {
                        ctx.CopyResource(s.texture.as_ID3D11Resource(), texture.as_ID3D11Resource());
                        let _ = s.mutex.ReleaseSync(0);
                        ctx.Flush();
                        let _ = frames.send(Frame { picture: Picture::Shared(makepad_video::SurfaceHandle::new(s.key, makepad_video::SurfaceKind::D3d11Keyed, [s.width, s.height], makepad_video::SurfaceFormat::Bgra8)), width: s.width, height: s.height, captured_ns: clock_ns(), _hold: Box::new(()) });
                    }
                }
            }
            let _ = d.ReleaseFrame();
        }
    }
    Ok(())
}

/// The shared BGRA texture the encoder reads by `key` (its keyed mutex, key 0 on both sides).
unsafe fn make_slot(device: &ID3D11Device, key: u64, w: u32, h: u32) -> Result<Slot, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
    };
    let mut out = None;
    device.CreateTexture2D(&desc, None, Some(&mut out)).map_err(|e| format!("CreateTexture2D(slot): {e:?}"))?;
    let texture: ID3D11Texture2D = out.ok_or("CreateTexture2D(slot) returned nothing")?;
    let resource = IDXGIResource1::query(texture.as_raw()).map_err(|e| format!("IDXGIResource1: {e:?}"))?;
    // the HAL's shared-texture name of `key` (makepad_hal::shared_texture opens it by name)
    let name: Vec<u16> = format!("Local\\makepad-hal-{}-{}", key >> 32, key & 0xffff_ffff).encode_utf16().chain(std::iter::once(0)).collect();
    let access = (DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0) as u32;
    // the open handle keeps the name: it lives as long as the slot
    let handle = resource.CreateSharedHandle(None, access, Some(windows::core::PCWSTR(name.as_ptr()))).map_err(|e| format!("CreateSharedHandle: {e:?}"))?;
    let mutex = IDXGIKeyedMutex::query(texture.as_raw()).map_err(|e| format!("IDXGIKeyedMutex: {e:?}"))?;
    Ok(Slot { texture, mutex, handle, key, width: w, height: h })
}

impl Desktop for WinDesktop {
    fn screens(&mut self) -> Vec<ScreenInfo> {
        outputs()
            .into_iter()
            .enumerate()
            .map(|(i, (_, d))| {
                let (w, h) = rect_size(&d.DesktopCoordinates);
                ScreenInfo { name: format!("display {}", i + 1), x: d.DesktopCoordinates.left.max(0) as u32, y: d.DesktopCoordinates.top.max(0) as u32, width: w, height: h }
            })
            .collect()
    }

    fn grants(&mut self) -> String {
        session::status(false)
    }

    fn watch_note(&mut self, preparing: bool) -> String {
        let note = session::status(preparing);
        if note == "the session is on the console" { String::new() } else { note }
    }

    fn prepare_watch(&mut self) -> Result<bool, String> {
        session::prepare()
    }

    fn capture(&mut self, index: usize, _fps: u32, frames: Sender<Frame>) -> Result<(), String> {
        self.stop_capture();
        let mut list = adapter_outputs();
        if index >= list.len() {
            return Err("no such screen".into());
        }
        let (adapter, output, desc) = list.swap_remove(index);
        *self.target.lock().unwrap() = desc.DesktopCoordinates;
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let hz = self.hz.clone();
        if self.captured != Some(index) {
            hz.store(0, Ordering::Release);
        }
        self.captured = Some(index);
        // COM objects move to the thread that alone uses them
        struct SendOutput(IDXGIAdapter1, IDXGIOutput);
        unsafe impl Send for SendOutput {}
        let output = SendOutput(adapter, output);
        let thread = std::thread::Builder::new()
            .name("deskhost-capture".into())
            .spawn(move || {
                let output = output;
                if let Err(e) = capture_loop(output.0, output.1, frames, stop2, hz) {
                    log(&format!("capture: {e}"));
                }
            })
            .map_err(|e| e.to_string())?;
        log(&format!("capture: screen {index} at {}x{}", rect_size(&desc.DesktopCoordinates).0, rect_size(&desc.DesktopCoordinates).1));
        self.capture = Some(Capture { stop, thread: Some(thread) });
        Ok(())
    }

    fn stop_capture(&mut self) {
        if let Some(mut c) = self.capture.take() {
            c.stop.store(true, Ordering::Release);
            if let Some(t) = c.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn refresh(&mut self, index: usize) -> u32 {
        if self.captured == Some(index) { self.hz.load(Ordering::Acquire) } else { 0 }
    }

    fn cursor(&mut self, _index: usize) -> (u16, u16, bool, u8) {
        let r = *self.target.lock().unwrap();
        let (w, h) = rect_size(&r);
        let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if unsafe { GetCursorInfo(&mut ci) }.is_err() {
            return (0, 0, false, 1);
        }
        let nx = (ci.ptScreenPos.x - r.left) as f64 / w.max(1) as f64;
        let ny = (ci.ptScreenPos.y - r.top) as f64 / h.max(1) as f64;
        let showing = ci.flags.0 & 1 != 0;
        let inside = showing && (0.0..=1.0).contains(&nx) && (0.0..=1.0).contains(&ny);
        let shape = self.cursors.iter().find(|(c, _)| *c == ci.hCursor.0 as isize).map_or("Default", |(_, s)| s);
        ((nx.clamp(0.0, 1.0) * 65535.0) as u16, (ny.clamp(0.0, 1.0) * 65535.0) as u16, inside, makepad_desk_link::cursor_index(shape))
    }

    fn input(&self) -> InputFactory {
        let target = self.target.clone();
        Arc::new(move || Ok(Box::new(WinInput { target: target.clone() }) as Box<dyn Input>))
    }
}

/// makepad `KeyCode` -> Windows virtual key, and whether it is an extended key.
fn vk_of(key: makepad_key_code::KeyCode) -> Option<(u16, bool)> {
    use makepad_key_code::KeyCode::*;
    let letter = |c: u8| Some((c as u16, false));
    match key {
        KeyA => letter(b'A'), KeyB => letter(b'B'), KeyC => letter(b'C'), KeyD => letter(b'D'), KeyE => letter(b'E'), KeyF => letter(b'F'),
        KeyG => letter(b'G'), KeyH => letter(b'H'), KeyI => letter(b'I'), KeyJ => letter(b'J'), KeyK => letter(b'K'), KeyL => letter(b'L'),
        KeyM => letter(b'M'), KeyN => letter(b'N'), KeyO => letter(b'O'), KeyP => letter(b'P'), KeyQ => letter(b'Q'), KeyR => letter(b'R'),
        KeyS => letter(b'S'), KeyT => letter(b'T'), KeyU => letter(b'U'), KeyV => letter(b'V'), KeyW => letter(b'W'), KeyX => letter(b'X'),
        KeyY => letter(b'Y'), KeyZ => letter(b'Z'),
        Key0 => letter(b'0'), Key1 => letter(b'1'), Key2 => letter(b'2'), Key3 => letter(b'3'), Key4 => letter(b'4'),
        Key5 => letter(b'5'), Key6 => letter(b'6'), Key7 => letter(b'7'), Key8 => letter(b'8'), Key9 => letter(b'9'),
        Escape => Some((0x1b, false)), Backspace => Some((0x08, false)), Tab => Some((0x09, false)), ReturnKey => Some((0x0d, false)),
        Space => Some((0x20, false)), Shift => Some((0x10, false)), Control => Some((0x11, false)), Alt => Some((0x12, false)),
        Logo => Some((0x5b, true)), Capslock => Some((0x14, false)), Minus => Some((0xbd, false)), Equals => Some((0xbb, false)),
        LBracket => Some((0xdb, false)), RBracket => Some((0xdd, false)), Backslash => Some((0xdc, false)), Semicolon => Some((0xba, false)),
        Quote => Some((0xde, false)), Backtick => Some((0xc0, false)), Comma => Some((0xbc, false)), Period => Some((0xbe, false)),
        Slash => Some((0xbf, false)),
        F1 => Some((0x70, false)), F2 => Some((0x71, false)), F3 => Some((0x72, false)), F4 => Some((0x73, false)), F5 => Some((0x74, false)),
        F6 => Some((0x75, false)), F7 => Some((0x76, false)), F8 => Some((0x77, false)), F9 => Some((0x78, false)), F10 => Some((0x79, false)),
        F11 => Some((0x7a, false)), F12 => Some((0x7b, false)), PrintScreen => Some((0x2c, true)), ScrollLock => Some((0x91, false)),
        Pause => Some((0x13, false)), Insert => Some((0x2d, true)), Delete => Some((0x2e, true)), Home => Some((0x24, true)),
        End => Some((0x23, true)), PageUp => Some((0x21, true)), PageDown => Some((0x22, true)), ArrowLeft => Some((0x25, true)),
        ArrowUp => Some((0x26, true)), ArrowRight => Some((0x27, true)), ArrowDown => Some((0x28, true)),
        Numpad0 => Some((0x60, false)), Numpad1 => Some((0x61, false)), Numpad2 => Some((0x62, false)), Numpad3 => Some((0x63, false)),
        Numpad4 => Some((0x64, false)), Numpad5 => Some((0x65, false)), Numpad6 => Some((0x66, false)), Numpad7 => Some((0x67, false)),
        Numpad8 => Some((0x68, false)), Numpad9 => Some((0x69, false)), NumpadMultiply => Some((0x6a, false)), NumpadAdd => Some((0x6b, false)),
        NumpadSubtract => Some((0x6d, false)), NumpadDecimal => Some((0x6e, false)), NumpadDivide => Some((0x6f, true)),
        NumpadEnter => Some((0x0d, true)), Numlock => Some((0x90, true)),
        _ => None,
    }
}

struct WinInput {
    target: Arc<Mutex<RECT>>,
}

impl WinInput {
    fn mouse(&self, dx: i32, dy: i32, data: u32, flags: u32) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: MOUSE_EVENT_FLAGS(flags), time: 0, dwExtraInfo: 0 } },
        };
        sent(unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) });
    }

    fn keyboard(&self, vk: u16, extended: bool, down: bool) {
        let mut flags = if down { 0 } else { KEYEVENTF_KEYUP.0 };
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY.0;
        }
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: KEYBD_EVENT_FLAGS(flags), time: 0, dwExtraInfo: 0 } },
        };
        sent(unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) });
    }
}

/// SendInput's answer: 0 when Windows blocked the input (the secure desktop of a UAC prompt or
/// the lock screen, a window of a higher integrity level in front), said once per blocked run.
fn sent(n: u32) {
    static BLOCKED: AtomicBool = AtomicBool::new(false);
    if n == 0 {
        if !BLOCKED.swap(true, Ordering::AcqRel) {
            log("input: Windows blocks it now (a UAC prompt or the lock screen, or an elevated window in front)");
        }
    } else if BLOCKED.swap(false, Ordering::AcqRel) {
        log("input: taken again");
    }
}

impl Input for WinInput {
    fn pointer(&mut self, x: u16, y: u16) {
        let r = *self.target.lock().unwrap();
        let (w, h) = rect_size(&r);
        let (vx, vy, vw, vh) = unsafe {
            (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
        };
        // the point on the virtual desktop, normalised over it as SendInput wants
        let px = r.left as f64 + x as f64 / 65535.0 * w.saturating_sub(1) as f64;
        let py = r.top as f64 + y as f64 / 65535.0 * h.saturating_sub(1) as f64;
        let nx = ((px - vx as f64) * 65535.0 / (vw - 1).max(1) as f64).round() as i32;
        let ny = ((py - vy as f64) * 65535.0 / (vh - 1).max(1) as f64).round() as i32;
        self.mouse(nx, ny, 0, MOUSEEVENTF_MOVE.0 | MOUSEEVENTF_ABSOLUTE.0 | MOUSEEVENTF_VIRTUALDESK.0);
    }

    fn button(&mut self, down: bool, b: u8) {
        let (flags, data) = match (b, down) {
            (button::LEFT, true) => (MOUSEEVENTF_LEFTDOWN.0, 0),
            (button::LEFT, false) => (MOUSEEVENTF_LEFTUP.0, 0),
            (button::RIGHT, true) => (MOUSEEVENTF_RIGHTDOWN.0, 0),
            (button::RIGHT, false) => (MOUSEEVENTF_RIGHTUP.0, 0),
            (button::MIDDLE, true) => (MOUSEEVENTF_MIDDLEDOWN.0, 0),
            (button::MIDDLE, false) => (MOUSEEVENTF_MIDDLEUP.0, 0),
            (button::BACK, d) => (if d { MOUSEEVENTF_XDOWN.0 } else { MOUSEEVENTF_XUP.0 }, XBUTTON1 as u32),
            (button::FORWARD, d) => (if d { MOUSEEVENTF_XDOWN.0 } else { MOUSEEVENTF_XUP.0 }, XBUTTON2 as u32),
            _ => return,
        };
        self.mouse(0, 0, data, flags);
    }

    /// Windows wheels count 120 per notch, + away from the user: the protocol's units.
    fn scroll(&mut self, dx: i32, dy: i32) {
        if dy != 0 {
            self.mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL.0);
        }
        if dx != 0 {
            self.mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL.0);
        }
    }

    fn key(&mut self, down: bool, keycode: u32) {
        let Some((vk, extended)) = vk_of(makepad_key_code::KeyCode::from_index(keycode)) else { return };
        self.keyboard(vk, extended, down);
    }

    /// Windows has no virtual gamepad without a driver: left out.
    fn gamepad(&mut self, _slot: u8, _state: &GamepadState) {}
    fn gamepad_gone(&mut self, _slot: u8) {}
}
