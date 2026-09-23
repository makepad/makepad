use crate::{image::DrawImage, makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.BrowserBackend = #(BrowserBackend::script_api(vm))
    mod.widgets.splat(mod.widgets.BrowserBackend)

    mod.widgets.BrowserBase = #(Browser::register_widget(vm))

    mod.widgets.Browser = set_type_default() do mod.widgets.BrowserBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserBackend {
    #[pick]
    Native,
    CEF,
}

/// The IOSurface handed to CEF is rounded up to this pixel grid, so a drag
/// resize walks dozens of page sizes inside ONE surface instead of allocating
/// (and showing) a blank one at every step.
pub const SURFACE_GRID: usize = 256;

/// Round a page size up to [`SURFACE_GRID`].
pub fn surface_alloc(width: usize, height: usize) -> (usize, usize) {
    fn up(v: usize) -> usize {
        v.max(1).div_ceil(SURFACE_GRID) * SURFACE_GRID
    }
    (up(width), up(height))
}

/// Does the page need a surface other than the one it has (or the one already
/// on its way)? Growing is immediate — a surface smaller than the page clips
/// the blit. Shrinking waits for the size to settle, so a drag does not
/// allocate at every step.
pub fn needs_new_surface(
    current: Option<(usize, usize)>,
    pending: Option<(usize, usize)>,
    want: (usize, usize),
    settled: bool,
) -> bool {
    match (current, pending) {
        // Something bigger is already on its way: only a page that outgrows
        // even that needs another surface.
        (_, Some((pw, ph))) => want.0 > pw || want.1 > ph,
        (Some((aw, ah)), None) => want.0 > aw || want.1 > ah || (settled && want != (aw, ah)),
        (None, None) => true,
    }
}

/// How long a page size has to hold still before the surface may shrink back
/// down to it. Growing never waits.
pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);
/// Floor on the spacing between `was_resized` calls: CEF's paint always lags
/// the size, and driving resizes faster than the display refresh starves the
/// paint callback outright (measured: zero accelerated frames at ~2 kHz).
pub const RESIZE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(8);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
enum ActiveBrowserBackend {
    #[default]
    None,
    Native,
    CEF,
    Unsupported,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Browser {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawImage,
    #[live(BrowserBackend::Native)]
    backend: BrowserBackend,
    #[live]
    url: ArcStringMut,
    #[visible]
    #[live(true)]
    visible: bool,
    /// The last good frame. Never dropped for a resize — only replaced once
    /// its successor holds a page frame.
    #[rust]
    texture: Option<Texture>,
    /// Pixel size `texture` is allocated at, and the sub-rect of it that holds
    /// page pixels (the last blit's copy region). The quad samples exactly
    /// that sub-rect and stretches it over the current rect.
    #[rust]
    texture_alloc: Option<(usize, usize)>,
    #[rust]
    texture_valid: Option<(usize, usize)>,
    /// A surface already handed to CEF that has not been painted yet: it stays
    /// invisible until it holds a frame, so a resize never shows a blank.
    #[rust]
    pending_texture: Option<Texture>,
    #[rust]
    pending_alloc: Option<(usize, usize)>,
    #[rust]
    painted: bool,
    #[cfg(feature = "cef")]
    #[rust]
    resized_to: Option<(usize, usize)>,
    #[cfg(feature = "cef")]
    #[rust]
    resized_at: Option<f64>,
    #[cfg(feature = "cef")]
    #[rust]
    deferred_resize: Option<(usize, usize, f32)>,
    #[cfg(feature = "cef")]
    #[rust]
    wanted_size: Option<(usize, usize)>,
    #[cfg(feature = "cef")]
    #[rust]
    wanted_at: Option<f64>,
    #[cfg(feature = "cef")]
    #[rust]
    cef_browser: Option<makepad_cef::Browser>,
    #[rust]
    pump_timer: Timer,
    #[rust]
    init_error: Option<String>,
    #[rust]
    last_url: String,
    #[rust]
    active_backend: ActiveBrowserBackend,
    #[rust]
    system_browser_spawned: bool,
    #[cfg(feature = "cef")]
    #[rust]
    pressed_buttons: MouseButton,
    #[cfg(feature = "cef")]
    #[rust]
    scroll_remainder: Vec2d,
    #[cfg(feature = "cef")]
    #[rust]
    suppress_next_paste_shortcut: bool,
    /// Size of the IOSurface-backed texture currently registered as the
    /// accelerated paint target (None on the software path).
    #[cfg(feature = "cef")]
    #[rust]
    accel_target_size: Option<(usize, usize)>,
    #[cfg(feature = "cef")]
    #[rust]
    accel_frame_counter: u64,
}

impl Browser {
    const PUMP_INTERVAL: f64 = 1.0 / 60.0;

    fn system_browser_id(&self) -> SystemBrowserId {
        SystemBrowserId(LiveId(self.uid.0))
    }

    fn resolved_backend(&self) -> ActiveBrowserBackend {
        match self.backend {
            BrowserBackend::Native => {
                #[cfg(any(target_os = "macos", target_os = "ios"))]
                {
                    return ActiveBrowserBackend::Native;
                }
                #[cfg(not(any(target_os = "macos", target_os = "ios")))]
                {
                    return ActiveBrowserBackend::Unsupported;
                }
            }
            BrowserBackend::CEF => {
                #[cfg(feature = "cef")]
                {
                    return ActiveBrowserBackend::CEF;
                }
                #[cfg(not(feature = "cef"))]
                {
                    return ActiveBrowserBackend::Unsupported;
                }
            }
        }
    }

    fn unsupported_backend_message(&self) -> &'static str {
        match self.backend {
            BrowserBackend::Native => {
                "Native browser backend is currently only implemented on macOS and iOS"
            }
            BrowserBackend::CEF => "CEF browser backend requires the `cef` feature",
        }
    }

    fn sync_backend_transition(&mut self, cx: &mut Cx, desired_backend: ActiveBrowserBackend) {
        if self.active_backend == desired_backend {
            return;
        }

        match self.active_backend {
            ActiveBrowserBackend::Native => {
                if self.system_browser_spawned {
                    cx.system_browser(self.system_browser_id()).close();
                    self.system_browser_spawned = false;
                }
            }
            ActiveBrowserBackend::CEF => {
                self.texture = None;
                self.texture_alloc = None;
                self.texture_valid = None;
                self.pending_texture = None;
                self.pending_alloc = None;
                self.painted = false;
                #[cfg(feature = "cef")]
                {
                    self.cef_browser = None;
                    self.accel_target_size = None;
                    self.resized_to = None;
                    self.resized_at = None;
                    self.deferred_resize = None;
                }
            }
            ActiveBrowserBackend::None | ActiveBrowserBackend::Unsupported => {}
        }

        self.active_backend = desired_backend;
        if desired_backend != ActiveBrowserBackend::Unsupported {
            self.init_error = None;
        }
    }

    fn sync_system_browser(&mut self, cx: &mut Cx) {
        let browser_id = self.system_browser_id();
        if !self.system_browser_spawned {
            cx.system_browser(browser_id).spawn(self.url.as_ref());
            self.system_browser_spawned = true;
            self.last_url.clear();
            self.last_url.push_str(self.url.as_ref());
        }

        let url = self.url.as_ref();
        if self.last_url != url {
            cx.system_browser(browser_id).set_url(url, false);
            self.last_url.clear();
            self.last_url.push_str(url);
        }

        let area = self.browser_area();
        if self.visible && area.is_valid(cx) {
            cx.system_browser(browser_id).update(area, true);
        } else if self.system_browser_spawned {
            cx.system_browser(browser_id).detach();
        }
    }

    fn browser_area(&self) -> Area {
        self.draw_bg.area()
    }

    #[cfg(feature = "cef")]
    fn browser_rect(&self, cx: &mut Cx) -> Option<Rect> {
        let area = self.browser_area();
        if area.is_valid(cx) {
            Some(area.rect(cx))
        } else {
            None
        }
    }

    /// CEF takes mouse coordinates in view points (it applies the device
    /// scale factor itself), so no dpi scaling here.
    #[cfg(feature = "cef")]
    fn cef_position(&self, cx: &mut Cx, abs: Vec2d) -> Option<(i32, i32)> {
        let rect = self.browser_rect(cx)?;
        let local = abs - rect.pos;
        Some((local.x.round() as i32, local.y.round() as i32))
    }

    #[cfg(feature = "cef")]
    pub fn cef_modifiers(modifiers: KeyModifiers, pressed_buttons: MouseButton) -> u32 {
        let mut out = makepad_cef::EVENTFLAG_NONE;
        if modifiers.shift {
            out |= makepad_cef::EVENTFLAG_SHIFT_DOWN;
        }
        if modifiers.control {
            out |= makepad_cef::EVENTFLAG_CONTROL_DOWN;
        }
        if modifiers.alt {
            out |= makepad_cef::EVENTFLAG_ALT_DOWN;
        }
        if modifiers.logo {
            out |= makepad_cef::EVENTFLAG_COMMAND_DOWN;
        }
        if pressed_buttons.is_primary() {
            out |= makepad_cef::EVENTFLAG_LEFT_MOUSE_BUTTON;
        }
        if pressed_buttons.is_middle() {
            out |= makepad_cef::EVENTFLAG_MIDDLE_MOUSE_BUTTON;
        }
        if pressed_buttons.is_secondary() {
            out |= makepad_cef::EVENTFLAG_RIGHT_MOUSE_BUTTON;
        }
        out
    }

    /// Makepad ScrollEvent uses +y for down and +x for right; CEF uses the
    /// opposite. Fractional pixels stay in `remainder` across events.
    #[cfg(feature = "cef")]
    pub fn cef_scroll_delta(delta: Vec2d, remainder: &mut Vec2d) -> (i32, i32) {
        *remainder += -delta;
        let dx = remainder.x as i32;
        let dy = remainder.y as i32;
        remainder.x -= dx as f64;
        remainder.y -= dy as f64;
        (dx, dy)
    }

    #[cfg(feature = "cef")]
    pub fn cef_mouse_button(button: Option<MouseButton>) -> i32 {
        match button {
            Some(button) if button.is_secondary() => makepad_cef::MOUSE_BUTTON_RIGHT,
            Some(button) if button.is_middle() => makepad_cef::MOUSE_BUTTON_MIDDLE,
            _ => makepad_cef::MOUSE_BUTTON_LEFT,
        }
    }

    #[cfg(feature = "cef")]
    pub fn windows_key_code(key_code: KeyCode) -> i32 {
        match key_code {
            KeyCode::Escape => 0x1B,
            KeyCode::Back => 0x08,
            KeyCode::Backtick => 0xC0,
            KeyCode::Key0 => 0x30,
            KeyCode::Key1 => 0x31,
            KeyCode::Key2 => 0x32,
            KeyCode::Key3 => 0x33,
            KeyCode::Key4 => 0x34,
            KeyCode::Key5 => 0x35,
            KeyCode::Key6 => 0x36,
            KeyCode::Key7 => 0x37,
            KeyCode::Key8 => 0x38,
            KeyCode::Key9 => 0x39,
            KeyCode::Minus => 0xBD,
            KeyCode::Equals => 0xBB,
            KeyCode::Backspace => 0x08,
            KeyCode::Tab => 0x09,
            KeyCode::KeyQ => 0x51,
            KeyCode::KeyW => 0x57,
            KeyCode::KeyE => 0x45,
            KeyCode::KeyR => 0x52,
            KeyCode::KeyT => 0x54,
            KeyCode::KeyY => 0x59,
            KeyCode::KeyU => 0x55,
            KeyCode::KeyI => 0x49,
            KeyCode::KeyO => 0x4F,
            KeyCode::KeyP => 0x50,
            KeyCode::LBracket => 0xDB,
            KeyCode::RBracket => 0xDD,
            KeyCode::ReturnKey => 0x0D,
            KeyCode::KeyA => 0x41,
            KeyCode::KeyS => 0x53,
            KeyCode::KeyD => 0x44,
            KeyCode::KeyF => 0x46,
            KeyCode::KeyG => 0x47,
            KeyCode::KeyH => 0x48,
            KeyCode::KeyJ => 0x4A,
            KeyCode::KeyK => 0x4B,
            KeyCode::KeyL => 0x4C,
            KeyCode::Semicolon => 0xBA,
            KeyCode::Quote => 0xDE,
            KeyCode::Backslash => 0xDC,
            KeyCode::KeyZ => 0x5A,
            KeyCode::KeyX => 0x58,
            KeyCode::KeyC => 0x43,
            KeyCode::KeyV => 0x56,
            KeyCode::KeyB => 0x42,
            KeyCode::KeyN => 0x4E,
            KeyCode::KeyM => 0x4D,
            KeyCode::Comma => 0xBC,
            KeyCode::Period => 0xBE,
            KeyCode::Slash => 0xBF,
            KeyCode::Control => 0x11,
            KeyCode::Alt => 0x12,
            KeyCode::Shift => 0x10,
            KeyCode::Logo => 0x5B,
            KeyCode::Space => 0x20,
            KeyCode::Capslock => 0x14,
            KeyCode::F1 => 0x70,
            KeyCode::F2 => 0x71,
            KeyCode::F3 => 0x72,
            KeyCode::F4 => 0x73,
            KeyCode::F5 => 0x74,
            KeyCode::F6 => 0x75,
            KeyCode::F7 => 0x76,
            KeyCode::F8 => 0x77,
            KeyCode::F9 => 0x78,
            KeyCode::F10 => 0x79,
            KeyCode::F11 => 0x7A,
            KeyCode::F12 => 0x7B,
            KeyCode::PrintScreen => 0x2C,
            KeyCode::ScrollLock => 0x91,
            KeyCode::Pause => 0x13,
            KeyCode::Insert => 0x2D,
            KeyCode::Delete => 0x2E,
            KeyCode::Home => 0x24,
            KeyCode::End => 0x23,
            KeyCode::PageUp => 0x21,
            KeyCode::PageDown => 0x22,
            KeyCode::Numpad0 => 0x60,
            KeyCode::Numpad1 => 0x61,
            KeyCode::Numpad2 => 0x62,
            KeyCode::Numpad3 => 0x63,
            KeyCode::Numpad4 => 0x64,
            KeyCode::Numpad5 => 0x65,
            KeyCode::Numpad6 => 0x66,
            KeyCode::Numpad7 => 0x67,
            KeyCode::Numpad8 => 0x68,
            KeyCode::Numpad9 => 0x69,
            KeyCode::NumpadEquals => 0x92,
            KeyCode::NumpadSubtract => 0x6D,
            KeyCode::NumpadAdd => 0x6B,
            KeyCode::NumpadDecimal => 0x6E,
            KeyCode::NumpadMultiply => 0x6A,
            KeyCode::NumpadDivide => 0x6F,
            KeyCode::Numlock => 0x90,
            KeyCode::NumpadEnter => 0x0D,
            KeyCode::ArrowUp => 0x26,
            KeyCode::ArrowDown => 0x28,
            KeyCode::ArrowLeft => 0x25,
            KeyCode::ArrowRight => 0x27,
            KeyCode::Unknown => 0,
        }
    }

    /// What Chromium's Mac keycode table answers with `DomCode::NONE`
    /// (`kInvalidNativeKeycode` in `keycode_converter.cc`). The only value a
    /// key without a Carbon code may carry: the Windows VK range overlaps the
    /// Carbon range everywhere, and a VK passed off as a Carbon code is a
    /// different key -- VK_L (0x4C) is kVK_ANSI_KeypadEnter, VK_NUMPAD0
    /// (0x60) is F5, VK_F13 (0x7C) is the right arrow.
    #[cfg(all(feature = "cef", target_os = "macos"))]
    pub const NO_MAC_KEY: i32 = 0xFFFF;

    /// Platform scan / virtual key Chromium wants in `native_key_code`.
    /// On macOS that is the Carbon/NSEvent keyCode, never the Windows VK.
    #[cfg(feature = "cef")]
    pub fn native_key_code(key_code: KeyCode) -> i32 {
        #[cfg(target_os = "macos")]
        {
            return match key_code {
                KeyCode::KeyA => 0x00,
                KeyCode::KeyS => 0x01,
                KeyCode::KeyD => 0x02,
                KeyCode::KeyF => 0x03,
                KeyCode::KeyH => 0x04,
                KeyCode::KeyG => 0x05,
                KeyCode::KeyZ => 0x06,
                KeyCode::KeyX => 0x07,
                KeyCode::KeyC => 0x08,
                KeyCode::KeyV => 0x09,
                KeyCode::KeyB => 0x0b,
                KeyCode::KeyQ => 0x0c,
                KeyCode::KeyW => 0x0d,
                KeyCode::KeyE => 0x0e,
                KeyCode::KeyR => 0x0f,
                KeyCode::KeyY => 0x10,
                KeyCode::KeyT => 0x11,
                KeyCode::Key1 => 0x12,
                KeyCode::Key2 => 0x13,
                KeyCode::Key3 => 0x14,
                KeyCode::Key4 => 0x15,
                KeyCode::Key6 => 0x16,
                KeyCode::Key5 => 0x17,
                KeyCode::Equals => 0x18,
                KeyCode::Key9 => 0x19,
                KeyCode::Key7 => 0x1a,
                KeyCode::Minus => 0x1b,
                KeyCode::Key8 => 0x1c,
                KeyCode::Key0 => 0x1d,
                KeyCode::RBracket => 0x1e,
                KeyCode::KeyO => 0x1f,
                KeyCode::KeyU => 0x20,
                KeyCode::LBracket => 0x21,
                KeyCode::KeyI => 0x22,
                KeyCode::KeyP => 0x23,
                KeyCode::ReturnKey => 0x24,
                KeyCode::KeyL => 0x25,
                KeyCode::KeyJ => 0x26,
                KeyCode::Quote => 0x27,
                KeyCode::KeyK => 0x28,
                KeyCode::Semicolon => 0x29,
                KeyCode::Backslash => 0x2a,
                KeyCode::Comma => 0x2b,
                KeyCode::Slash => 0x2c,
                KeyCode::KeyN => 0x2d,
                KeyCode::KeyM => 0x2e,
                KeyCode::Period => 0x2f,
                KeyCode::Tab => 0x30,
                KeyCode::Space => 0x31,
                KeyCode::Backtick => 0x32,
                KeyCode::Backspace | KeyCode::Back => 0x33,
                KeyCode::Escape => 0x35,
                KeyCode::Capslock => 0x39,
                KeyCode::Shift => 0x38,
                KeyCode::Alt => 0x3a,
                KeyCode::Control => 0x3b,
                KeyCode::Logo => 0x37,
                KeyCode::F5 => 0x60,
                KeyCode::F6 => 0x61,
                KeyCode::F7 => 0x62,
                KeyCode::F3 => 0x63,
                KeyCode::F8 => 0x64,
                KeyCode::F9 => 0x65,
                KeyCode::F11 => 0x67,
                KeyCode::F10 => 0x6d,
                KeyCode::F12 => 0x6f,
                KeyCode::Insert => 0x72,
                KeyCode::Home => 0x73,
                KeyCode::PageUp => 0x74,
                KeyCode::Delete => 0x75,
                KeyCode::F4 => 0x76,
                KeyCode::End => 0x77,
                KeyCode::F2 => 0x78,
                KeyCode::PageDown => 0x79,
                KeyCode::F1 => 0x7a,
                KeyCode::ArrowLeft => 0x7b,
                KeyCode::ArrowRight => 0x7c,
                KeyCode::ArrowDown => 0x7d,
                KeyCode::ArrowUp => 0x7e,
                KeyCode::NumpadDecimal => 0x41,
                KeyCode::NumpadMultiply => 0x43,
                KeyCode::NumpadAdd => 0x45,
                KeyCode::Numlock => 0x47,
                KeyCode::NumpadDivide => 0x4b,
                KeyCode::NumpadEnter => 0x4c,
                KeyCode::NumpadSubtract => 0x4e,
                KeyCode::NumpadEquals => 0x51,
                KeyCode::Numpad0 => 0x52,
                KeyCode::Numpad1 => 0x53,
                KeyCode::Numpad2 => 0x54,
                KeyCode::Numpad3 => 0x55,
                KeyCode::Numpad4 => 0x56,
                KeyCode::Numpad5 => 0x57,
                KeyCode::Numpad6 => 0x58,
                KeyCode::Numpad7 => 0x59,
                KeyCode::Numpad8 => 0x5b,
                KeyCode::Numpad9 => 0x5c,
                // A Mac keyboard has F13-F15 where a PC one has these.
                KeyCode::PrintScreen => 0x69,
                KeyCode::ScrollLock => 0x6b,
                KeyCode::Pause => 0x71,
                KeyCode::Unknown => Self::NO_MAC_KEY,
            };
        }
        #[cfg(not(target_os = "macos"))]
        Self::windows_key_code(key_code)
    }

    /// Carbon keyCode for a Unicode character on Mac. Must not use the
    /// Windows VK: 0x4C is VK_L and also kVK_ANSI_KeypadEnter.
    #[cfg(feature = "cef")]
    pub fn native_key_code_for_char(ch: char) -> i32 {
        #[cfg(target_os = "macos")]
        {
            let code = match ch.to_ascii_lowercase() {
                'a' => KeyCode::KeyA,
                'b' => KeyCode::KeyB,
                'c' => KeyCode::KeyC,
                'd' => KeyCode::KeyD,
                'e' => KeyCode::KeyE,
                'f' => KeyCode::KeyF,
                'g' => KeyCode::KeyG,
                'h' => KeyCode::KeyH,
                'i' => KeyCode::KeyI,
                'j' => KeyCode::KeyJ,
                'k' => KeyCode::KeyK,
                'l' => KeyCode::KeyL,
                'm' => KeyCode::KeyM,
                'n' => KeyCode::KeyN,
                'o' => KeyCode::KeyO,
                'p' => KeyCode::KeyP,
                'q' => KeyCode::KeyQ,
                'r' => KeyCode::KeyR,
                's' => KeyCode::KeyS,
                't' => KeyCode::KeyT,
                'u' => KeyCode::KeyU,
                'v' => KeyCode::KeyV,
                'w' => KeyCode::KeyW,
                'x' => KeyCode::KeyX,
                'y' => KeyCode::KeyY,
                'z' => KeyCode::KeyZ,
                '0' => KeyCode::Key0,
                '1' => KeyCode::Key1,
                '2' => KeyCode::Key2,
                '3' => KeyCode::Key3,
                '4' => KeyCode::Key4,
                '5' => KeyCode::Key5,
                '6' => KeyCode::Key6,
                '7' => KeyCode::Key7,
                '8' => KeyCode::Key8,
                '9' => KeyCode::Key9,
                ' ' => KeyCode::Space,
                '\r' | '\n' => KeyCode::ReturnKey,
                '\t' => KeyCode::Tab,
                '\u{8}' => KeyCode::Backspace,
                // The US layout's punctuation, shifted or not: the code
                // point of every one of these is some other Carbon key
                // ('.' is 0x2E = M, '-' is 0x2D = N, ';' is 0x3B = Control).
                '`' | '~' => KeyCode::Backtick,
                '-' | '_' => KeyCode::Minus,
                '=' | '+' => KeyCode::Equals,
                '[' | '{' => KeyCode::LBracket,
                ']' | '}' => KeyCode::RBracket,
                '\\' | '|' => KeyCode::Backslash,
                ';' | ':' => KeyCode::Semicolon,
                '\'' | '"' => KeyCode::Quote,
                ',' | '<' => KeyCode::Comma,
                '.' | '>' => KeyCode::Period,
                '/' | '?' => KeyCode::Slash,
                ')' => KeyCode::Key0,
                '!' => KeyCode::Key1,
                '@' => KeyCode::Key2,
                '#' => KeyCode::Key3,
                '$' => KeyCode::Key4,
                '%' => KeyCode::Key5,
                '^' => KeyCode::Key6,
                '&' => KeyCode::Key7,
                '*' => KeyCode::Key8,
                '(' => KeyCode::Key9,
                // A character with no key on the US layout (an accented
                // letter from a dead key, anything the IME composed): no
                // Carbon code at all, never its code point.
                _ => return Self::NO_MAC_KEY,
            };
            return Self::native_key_code(code);
        }
        #[cfg(not(target_os = "macos"))]
        {
            if ch.is_ascii_alphabetic() {
                ch.to_ascii_uppercase() as i32
            } else {
                ch as i32
            }
        }
    }

    #[cfg(feature = "cef")]
    pub fn key_char(key_code: KeyCode, shift: bool) -> Option<char> {
        match key_code {
            KeyCode::Backspace | KeyCode::Back => Some('\u{8}'),
            KeyCode::Backtick => Some(if shift { '~' } else { '`' }),
            KeyCode::Key0 => Some(if shift { ')' } else { '0' }),
            KeyCode::Key1 => Some(if shift { '!' } else { '1' }),
            KeyCode::Key2 => Some(if shift { '@' } else { '2' }),
            KeyCode::Key3 => Some(if shift { '#' } else { '3' }),
            KeyCode::Key4 => Some(if shift { '$' } else { '4' }),
            KeyCode::Key5 => Some(if shift { '%' } else { '5' }),
            KeyCode::Key6 => Some(if shift { '^' } else { '6' }),
            KeyCode::Key7 => Some(if shift { '&' } else { '7' }),
            KeyCode::Key8 => Some(if shift { '*' } else { '8' }),
            KeyCode::Key9 => Some(if shift { '(' } else { '9' }),
            KeyCode::Minus => Some(if shift { '_' } else { '-' }),
            KeyCode::Equals => Some(if shift { '+' } else { '=' }),
            KeyCode::KeyQ => Some(if shift { 'Q' } else { 'q' }),
            KeyCode::KeyW => Some(if shift { 'W' } else { 'w' }),
            KeyCode::KeyE => Some(if shift { 'E' } else { 'e' }),
            KeyCode::KeyR => Some(if shift { 'R' } else { 'r' }),
            KeyCode::KeyT => Some(if shift { 'T' } else { 't' }),
            KeyCode::KeyY => Some(if shift { 'Y' } else { 'y' }),
            KeyCode::KeyU => Some(if shift { 'U' } else { 'u' }),
            KeyCode::KeyI => Some(if shift { 'I' } else { 'i' }),
            KeyCode::KeyO => Some(if shift { 'O' } else { 'o' }),
            KeyCode::KeyP => Some(if shift { 'P' } else { 'p' }),
            KeyCode::LBracket => Some(if shift { '{' } else { '[' }),
            KeyCode::RBracket => Some(if shift { '}' } else { ']' }),
            KeyCode::KeyA => Some(if shift { 'A' } else { 'a' }),
            KeyCode::KeyS => Some(if shift { 'S' } else { 's' }),
            KeyCode::KeyD => Some(if shift { 'D' } else { 'd' }),
            KeyCode::KeyF => Some(if shift { 'F' } else { 'f' }),
            KeyCode::KeyG => Some(if shift { 'G' } else { 'g' }),
            KeyCode::KeyH => Some(if shift { 'H' } else { 'h' }),
            KeyCode::KeyJ => Some(if shift { 'J' } else { 'j' }),
            KeyCode::KeyK => Some(if shift { 'K' } else { 'k' }),
            KeyCode::KeyL => Some(if shift { 'L' } else { 'l' }),
            KeyCode::Semicolon => Some(if shift { ':' } else { ';' }),
            KeyCode::Quote => Some(if shift { '"' } else { '\'' }),
            KeyCode::Backslash => Some(if shift { '|' } else { '\\' }),
            KeyCode::KeyZ => Some(if shift { 'Z' } else { 'z' }),
            KeyCode::KeyX => Some(if shift { 'X' } else { 'x' }),
            KeyCode::KeyC => Some(if shift { 'C' } else { 'c' }),
            KeyCode::KeyV => Some(if shift { 'V' } else { 'v' }),
            KeyCode::KeyB => Some(if shift { 'B' } else { 'b' }),
            KeyCode::KeyN => Some(if shift { 'N' } else { 'n' }),
            KeyCode::KeyM => Some(if shift { 'M' } else { 'm' }),
            KeyCode::Comma => Some(if shift { '<' } else { ',' }),
            KeyCode::Period => Some(if shift { '>' } else { '.' }),
            KeyCode::Slash => Some(if shift { '?' } else { '/' }),
            KeyCode::Tab => Some('\t'),
            KeyCode::ReturnKey => Some('\r'),
            KeyCode::Space => Some(' '),
            KeyCode::Numpad0 => Some('0'),
            KeyCode::Numpad1 => Some('1'),
            KeyCode::Numpad2 => Some('2'),
            KeyCode::Numpad3 => Some('3'),
            KeyCode::Numpad4 => Some('4'),
            KeyCode::Numpad5 => Some('5'),
            KeyCode::Numpad6 => Some('6'),
            KeyCode::Numpad7 => Some('7'),
            KeyCode::Numpad8 => Some('8'),
            KeyCode::Numpad9 => Some('9'),
            KeyCode::NumpadEquals => Some('='),
            KeyCode::NumpadSubtract => Some('-'),
            KeyCode::NumpadAdd => Some('+'),
            KeyCode::NumpadDecimal => Some('.'),
            KeyCode::NumpadMultiply => Some('*'),
            KeyCode::NumpadDivide => Some('/'),
            KeyCode::NumpadEnter => Some('\r'),
            _ => None,
        }
    }

    #[cfg(feature = "cef")]
    pub fn sends_char_on_keydown(key_code: KeyCode) -> bool {
        matches!(
            key_code,
            KeyCode::Backspace
                | KeyCode::Back
                | KeyCode::Tab
                | KeyCode::ReturnKey
                | KeyCode::NumpadEnter
        )
    }

    #[cfg(feature = "cef")]
    pub fn char_event_data(text: &str) -> Option<(i32, u16)> {
        let mut chars = text.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let mut utf16 = text.encode_utf16();
        let unit = utf16.next()?;
        if utf16.next().is_some() {
            return None;
        }
        let windows_key_code = if ch.is_ascii_alphabetic() {
            ch.to_ascii_uppercase() as i32
        } else {
            unit as i32
        };
        Some((windows_key_code, unit))
    }

    #[cfg(feature = "cef")]
    pub fn key_event_modifiers(key_event: &KeyEvent) -> u32 {
        let mut modifiers = Self::cef_modifiers(key_event.modifiers, MouseButton::empty());
        if key_event.is_repeat {
            modifiers |= makepad_cef::EVENTFLAG_IS_REPEAT;
        }
        if matches!(
            key_event.key_code,
            KeyCode::Numpad0
                | KeyCode::Numpad1
                | KeyCode::Numpad2
                | KeyCode::Numpad3
                | KeyCode::Numpad4
                | KeyCode::Numpad5
                | KeyCode::Numpad6
                | KeyCode::Numpad7
                | KeyCode::Numpad8
                | KeyCode::Numpad9
                | KeyCode::NumpadEquals
                | KeyCode::NumpadSubtract
                | KeyCode::NumpadAdd
                | KeyCode::NumpadDecimal
                | KeyCode::NumpadMultiply
                | KeyCode::NumpadDivide
                | KeyCode::NumpadEnter
        ) {
            modifiers |= makepad_cef::EVENTFLAG_IS_KEY_PAD;
        }
        modifiers
    }

    #[cfg(feature = "cef")]
    fn send_mouse_move_internal(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        modifiers: KeyModifiers,
        mouse_leave: bool,
    ) {
        let Some((x, y)) = self.cef_position(cx, abs) else {
            return;
        };
        let cef_modifiers = Self::cef_modifiers(modifiers, self.pressed_buttons);
        if let Some(browser) = &mut self.cef_browser {
            if let Err(err) = browser.send_mouse_move(x, y, cef_modifiers, mouse_leave) {
                log!("Browser mouse move failed: {err}");
            }
        }
    }

    #[cfg(feature = "cef")]
    fn send_mouse_click_internal(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        modifiers: KeyModifiers,
        button: Option<MouseButton>,
        mouse_up: bool,
        click_count: i32,
    ) {
        let Some((x, y)) = self.cef_position(cx, abs) else {
            return;
        };
        let cef_modifiers = Self::cef_modifiers(modifiers, self.pressed_buttons);
        let cef_button = Self::cef_mouse_button(button);
        if let Some(browser) = &mut self.cef_browser {
            if let Err(err) = browser.send_mouse_click(
                x,
                y,
                cef_modifiers,
                cef_button,
                mouse_up,
                click_count.max(1),
            ) {
                log!("Browser mouse click failed: {err}");
            }
        }
    }

    /// End a press the page holds as well as this build of the CEF binding
    /// allows without a release: the pointer leaves the page with no button
    /// held, and NO mouse-up is sent. A real downstream cancellation
    /// (`CefBrowserHost::SendCaptureLostEvent`) has no wrapper in the
    /// committed `makepad_cef` yet; an up — even off the page — can commit a
    /// drag the page captured (Pointer Events send captured events to the
    /// capturing element) or click. So the page is left holding its press
    /// until the person's next click ends it: nothing is committed. When the
    /// binding gains `send_capture_lost_event`, call it here instead.
    #[cfg(feature = "cef")]
    fn send_mouse_cancel_internal(&mut self, modifiers: KeyModifiers, button: MouseButton) {
        self.pressed_buttons.remove(button);
        let cef_modifiers = Self::cef_modifiers(modifiers, self.pressed_buttons);
        if let Some(browser) = &mut self.cef_browser {
            let off = -10_000;
            if let Err(err) = browser.send_mouse_move(off, off, cef_modifiers, true) {
                log!("Browser mouse cancel failed: {err}");
            }
        }
    }

    #[cfg(feature = "cef")]
    fn send_mouse_wheel_internal(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        modifiers: KeyModifiers,
        delta: Vec2d,
    ) {
        let Some((x, y)) = self.cef_position(cx, abs) else {
            return;
        };
        if self.cef_browser.is_none() {
            return;
        }
        let (dx, dy) = Self::cef_scroll_delta(delta, &mut self.scroll_remainder);
        if dx == 0 && dy == 0 {
            return;
        }
        let cef_modifiers = Self::cef_modifiers(modifiers, self.pressed_buttons)
            | makepad_cef::EVENTFLAG_PRECISION_SCROLLING_DELTA;
        if let Some(browser) = &mut self.cef_browser {
            if let Err(err) = browser.send_mouse_wheel(x, y, cef_modifiers, dx, dy) {
                log!("Browser mouse wheel failed: {err}");
            }
        }
    }

    #[cfg(feature = "cef")]
    fn update_ime_spot(&self, cx: &mut Cx, pos: Vec2d) {
        let area = self.browser_area();
        if area.is_valid(cx) {
            cx.show_text_ime(area, pos);
        }
    }

    #[cfg(feature = "cef")]
    fn ensure_browser(&mut self, _cx: &mut Cx2d, width: usize, height: usize, scale_factor: f32) {
        if self.cef_browser.is_none() && self.init_error.is_none() {
            match makepad_cef::Browser::new(self.url.as_ref(), width, height, scale_factor) {
                Ok(browser) => {
                    self.last_url = self.url.as_ref().to_string();
                    self.cef_browser = Some(browser);
                }
                Err(err) => {
                    let message = err.to_string();
                    log!("Browser widget initialization failed: {message}");
                    self.init_error = Some(message);
                }
            }
        }

        let now = Cx::monotonic_now();
        if self.wanted_size != Some((width, height)) {
            self.wanted_size = Some((width, height));
            self.wanted_at = Some(now);
        }
        self.sync_browser_size(width, height, scale_factor, now);
        self.sync_accelerated_target(_cx, width, height, now);

        if let Some(browser) = &mut self.cef_browser {
            let url = self.url.as_ref();
            if self.last_url != url {
                if let Err(err) = browser.set_url(url) {
                    let message = err.to_string();
                    log!("Browser widget navigation failed: {message}");
                    self.init_error = Some(message);
                    self.cef_browser = None;
                } else {
                    self.last_url.clear();
                    self.last_url.push_str(url);
                }
            }
        }
    }

    /// Tell CEF the page size, at most once per [`RESIZE_INTERVAL`]; anything
    /// faster is remembered and applied by the next pump, so the final size of
    /// a drag always lands.
    #[cfg(feature = "cef")]
    fn sync_browser_size(
        &mut self,
        width: usize,
        height: usize,
        scale_factor: f32,
        now: f64,
    ) {
        let same = self.resized_to == Some((width, height));
        if !same
            && self
                .resized_at
                .is_some_and(|last| now - last < RESIZE_INTERVAL.as_secs_f64())
        {
            self.deferred_resize = Some((width, height, scale_factor));
            return;
        }
        // `Browser::resize` no-ops on an unchanged size, so this still lets a
        // dpi change through without another `was_resized`.
        if let Some(browser) = &mut self.cef_browser {
            if let Err(err) = browser.resize(width, height, scale_factor) {
                let message = err.to_string();
                log!("Browser widget resize failed: {message}");
                self.init_error = Some(message);
                self.cef_browser = None;
            }
        }
        if !same {
            self.resized_to = Some((width, height));
            self.resized_at = Some(now);
        }
        self.deferred_resize = None;
    }

    /// GPU path: the browser paints into pooled IOSurfaces and blits them into
    /// a Makepad-owned IOSurface texture (`create_iosurface_render_texture`)
    /// that the quad samples directly — no CPU readback, no upload.
    ///
    /// The surface is over-allocated on [`SURFACE_GRID`] and grows only when
    /// the page outgrows it (it shrinks only once the size settles). A fresh
    /// surface is BLANK, so it waits in `pending_texture` while the last good
    /// one keeps covering the page area — that is what keeps a drag-resize
    /// from flashing empty.
    #[cfg(feature = "cef")]
    fn sync_accelerated_target(
        &mut self,
        cx: &mut Cx,
        width: usize,
        height: usize,
        now: f64,
    ) {
        #[cfg(target_os = "macos")]
        {
            if !self
                .cef_browser
                .as_ref()
                .is_some_and(|browser| browser.is_accelerated())
            {
                return;
            }
            let want = surface_alloc(width, height);
            let settled = self
                .wanted_at
                .is_some_and(|since| now - since >= SETTLE.as_secs_f64());
            if !needs_new_surface(self.accel_target_size, self.pending_alloc, want, settled) {
                return;
            }
            let (texture, iosurface, _id) = cx.create_iosurface_render_texture(want.0, want.1);
            let Some(browser) = &mut self.cef_browser else {
                return;
            };
            match browser.set_accelerated_target(iosurface, want.0, want.1) {
                Ok(()) => {
                    self.accel_target_size = Some(want);
                    if self.painted && self.texture.is_some() {
                        self.pending_texture = Some(texture);
                        self.pending_alloc = Some(want);
                    } else {
                        self.texture = Some(texture);
                        self.texture_alloc = Some(want);
                        self.texture_valid = None;
                        self.pending_texture = None;
                        self.pending_alloc = None;
                    }
                }
                Err(err) => {
                    log!("Browser accelerated target failed, staying on software frames: {err}");
                    self.accel_target_size = None;
                    self.pending_texture = None;
                    self.pending_alloc = None;
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (cx, width, height, now);
        }
    }

    #[cfg(feature = "cef")]
    fn pump_browser(&mut self, cx: &mut Cx) {
        makepad_cef::do_message_loop_work();
        let mut latest_frame = None;
        let mut accelerated_dirty = false;
        if let Some(browser) = &mut self.cef_browser {
            while let Some(frame) = browser.take_frame() {
                latest_frame = Some(frame);
            }
            let counter = browser.accelerated_frame_counter();
            if counter != self.accel_frame_counter {
                self.accel_frame_counter = counter;
                accelerated_dirty = true;
                let stats = browser.accelerated_stats();
                let valid = (stats.last_copy_width, stats.last_copy_height);
                if stats.target_frames > 0 && valid.0 > 0 && valid.1 > 0 {
                    // The surface CEF paints into now holds a real frame: if it
                    // was the pending one, this is the moment to swap — never
                    // before, so the page area never goes blank.
                    if let (Some(texture), Some(alloc)) =
                        (self.pending_texture.take(), self.pending_alloc.take())
                    {
                        self.texture = Some(texture);
                        self.texture_alloc = Some(alloc);
                    }
                    self.texture_valid = Some(valid);
                    self.painted = true;
                }
            }
        }
        if let Some(frame) = latest_frame {
            // A software frame on an accelerated browser means CEF fell back;
            // the texture becomes a plain upload target again.
            self.accel_target_size = None;
            self.apply_frame(cx, frame);
            self.redraw(cx);
        } else if accelerated_dirty {
            self.redraw(cx);
        }
        // A resize that arrived faster than CEF can take them: apply the last
        // one now, so the end of a drag always reaches the page.
        if let Some((w, h, dpi)) = self.deferred_resize {
            self.sync_browser_size(w, h, dpi, Cx::monotonic_now());
            self.redraw(cx);
        }
        // A shrink waits for the size to settle; the next *draw* used to be
        // the only thing that allocated the new surface, so a resize that
        // ended without another draw left Chromium on the old view.
        if let Some((w, h)) = self.wanted_size {
            self.sync_accelerated_target(cx, w, h, Cx::monotonic_now());
        }
    }

    #[cfg(feature = "cef")]
    fn apply_frame(&mut self, cx: &mut Cx, frame: makepad_cef::Frame) {
        let size = (frame.width, frame.height);
        match &self.texture {
            Some(texture) if texture.get_format(cx).vec_width_height() == Some(size) => {
                texture.set_data_u32(cx, frame.width, frame.height, frame.pixels);
            }
            _ => {
                self.pending_texture = None;
                self.pending_alloc = None;
                self.texture = Some(Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        data: Some(frame.pixels),
                        width: frame.width,
                        height: frame.height,
                        updated: TextureUpdated::Full,
                    },
                ));
            }
        }
        // An upload texture is exactly the page: all of it is valid.
        self.texture_alloc = Some(size);
        self.texture_valid = Some(size);
        self.painted = true;
    }

    fn set_url_internal(&mut self, cx: &mut Cx, url: &str) {
        self.url.set(url);
        self.last_url.clear();
        match self.active_backend {
            ActiveBrowserBackend::Native if self.system_browser_spawned => {
                cx.system_browser(self.system_browser_id())
                    .set_url(url, false);
            }
            ActiveBrowserBackend::Native => {}
            ActiveBrowserBackend::CEF =>
            {
                #[cfg(feature = "cef")]
                if let Some(browser) = &mut self.cef_browser {
                    if let Err(err) = browser.set_url(url) {
                        let message = err.to_string();
                        log!("Browser widget navigation failed: {message}");
                        self.init_error = Some(message);
                        self.cef_browser = None;
                    } else {
                        // The page has been told: the next draw must not
                        // tell it again and start the same load twice.
                        self.last_url.push_str(url);
                    }
                }
            }
            ActiveBrowserBackend::None | ActiveBrowserBackend::Unsupported => {}
        }
        self.redraw(cx);
    }

    fn set_visible_internal(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible && self.system_browser_spawned {
            cx.system_browser(self.system_browser_id()).detach();
        }
        self.redraw(cx);
    }
}

impl Widget for Browser {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Startup = event {
            self.pump_timer = cx.start_interval(Self::PUMP_INTERVAL);
        }

        let desired_backend = self.resolved_backend();
        self.sync_backend_transition(cx, desired_backend);

        if matches!(event, Event::Shutdown) {
            if self.system_browser_spawned {
                cx.system_browser(self.system_browser_id()).close();
                self.system_browser_spawned = false;
            }
            #[cfg(feature = "cef")]
            {
                self.cef_browser = None;
                // The profile's cookies are committed in batches; a process
                // that ends now would lose the last half minute of them, and
                // on macOS this event is the last thing the app hears.
                makepad_cef::flush_profile();
            }
            return;
        }

        if desired_backend == ActiveBrowserBackend::Unsupported {
            if self.init_error.is_none() {
                let message = self.unsupported_backend_message().to_string();
                log!("{message}");
                self.init_error = Some(message);
            }
            return;
        }

        if let Event::WindowGeomChange(_) = event {
            self.redraw(cx);
        }

        if self.pump_timer.is_event(event).is_some() {
            #[cfg(feature = "cef")]
            if desired_backend == ActiveBrowserBackend::CEF {
                self.pump_browser(cx);
            }
        }

        if !self.visible {
            if desired_backend == ActiveBrowserBackend::Native && self.system_browser_spawned {
                cx.system_browser(self.system_browser_id()).detach();
            }
            return;
        }

        #[cfg(feature = "cef")]
        if desired_backend == ActiveBrowserBackend::CEF {
            // `capture_overload` below hands the page the FingerDown for a
            // press another widget already took: a splitter between the panes,
            // a control floating over the page. A control that is dragged
            // continuously owns the pointer from the press to the release, and
            // nothing else may take a press, a hover or the keyboard from that
            // pointer on the way — so a press it holds never reaches the page.
            // Mouse only, deliberately: `is_mouse_held_outside` ignores touch
            // captures, so a finger still reaches the page.
            let pointer_held_elsewhere = cx.fingers.is_mouse_held_outside(&[self.browser_area()]);
            match event.hits_with_capture_overload(cx, self.browser_area(), true) {
                Hit::KeyFocus(_) => {
                    if let Some(browser) = &mut self.cef_browser {
                        if let Err(err) = browser.set_focus(true) {
                            log!("Browser focus failed: {err}");
                        }
                    }
                    if let Some(rect) = self.browser_rect(cx) {
                        self.update_ime_spot(cx, rect.pos);
                    }
                }
                Hit::KeyFocusLost(_) => {
                    if let Some(browser) = &mut self.cef_browser {
                        if let Err(err) = browser.set_focus(false) {
                            log!("Browser blur failed: {err}");
                        }
                    }
                    cx.hide_text_ime();
                    self.suppress_next_paste_shortcut = false;
                }
                Hit::FingerDown(fe) if !pointer_held_elsewhere => {
                    let button = fe.mouse_button().unwrap_or(MouseButton::PRIMARY);
                    self.pressed_buttons.insert(button);
                    cx.set_key_focus(self.browser_area());
                    if let Some(browser) = &mut self.cef_browser {
                        if let Err(err) = browser.set_focus(true) {
                            log!("Browser focus on pointer down failed: {err}");
                        }
                    }
                    self.update_ime_spot(cx, fe.abs);
                    self.send_mouse_move_internal(cx, fe.abs, fe.modifiers, false);
                    self.send_mouse_click_internal(
                        cx,
                        fe.abs,
                        fe.modifiers,
                        Some(button),
                        false,
                        fe.tap_count as i32,
                    );
                }
                Hit::FingerMove(fe) if !pointer_held_elsewhere => {
                    self.send_mouse_move_internal(cx, fe.abs, fe.modifiers, false);
                }
                // The press was taken away (a list or the host took the finger):
                // the page must let go of it too, but nothing may click.
                Hit::FingerUp(fe) if fe.cancelled => {
                    let button = fe.mouse_button().unwrap_or(MouseButton::PRIMARY);
                    if self.pressed_buttons.contains(button) {
                        self.send_mouse_cancel_internal(fe.modifiers, button);
                    }
                    self.pressed_buttons.remove(button);
                }
                Hit::FingerUp(fe) => {
                    let button = fe.mouse_button().unwrap_or(MouseButton::PRIMARY);
                    // Only release a button the page was actually told about: a
                    // press that stood down above never reached it, and a lone
                    // mouse-up leaves the page believing a drag is still live.
                    if self.pressed_buttons.contains(button) {
                        self.send_mouse_move_internal(cx, fe.abs, fe.modifiers, false);
                        self.send_mouse_click_internal(
                            cx,
                            fe.abs,
                            fe.modifiers,
                            Some(button),
                            true,
                            fe.tap_count as i32,
                        );
                    }
                    self.pressed_buttons.remove(button);
                }
                Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                    self.send_mouse_move_internal(cx, fe.abs, fe.modifiers, false);
                }
                Hit::FingerHoverOut(fe) => {
                    self.send_mouse_move_internal(cx, fe.abs, fe.modifiers, true);
                }
                Hit::FingerScroll(fe) => {
                    self.send_mouse_wheel_internal(cx, fe.abs, fe.modifiers, fe.scroll);
                }
                Hit::KeyDown(key_event) => {
                    if self.suppress_next_paste_shortcut
                        && key_event.key_code == KeyCode::KeyV
                        && key_event.modifiers.is_primary()
                    {
                        self.suppress_next_paste_shortcut = false;
                    } else {
                        let modifiers = Self::key_event_modifiers(&key_event);
                        let windows_key_code = Self::windows_key_code(key_event.key_code);
                        let native_key_code = Self::native_key_code(key_event.key_code);
                        let character = if key_event.modifiers.control
                            || key_event.modifiers.alt
                            || key_event.modifiers.logo
                        {
                            0
                        } else {
                            Self::key_char(key_event.key_code, key_event.modifiers.shift)
                                .map(|ch| ch as u16)
                                .unwrap_or(0)
                        };

                        if let Some(browser) = &mut self.cef_browser {
                            // OSR expects RAWKEYDOWN (the translated KEYDOWN is
                            // Chromium's internal follow-up). Sending KEYDOWN
                            // from here dropped Backspace and let some keys
                            // look like browser chrome (history / reload).
                            if let Err(err) = browser.send_key_event(
                                makepad_cef::KEY_EVENT_RAWKEYDOWN,
                                modifiers,
                                windows_key_code,
                                native_key_code,
                                character,
                                character,
                                false,
                            ) {
                                log!("Browser key down failed: {err}");
                            }
                            if character != 0
                                && !key_event.modifiers.control
                                && !key_event.modifiers.alt
                                && !key_event.modifiers.logo
                                && Self::sends_char_on_keydown(key_event.key_code)
                            {
                                if let Err(err) = browser.send_key_event(
                                    makepad_cef::KEY_EVENT_CHAR,
                                    modifiers,
                                    windows_key_code,
                                    native_key_code,
                                    character,
                                    character,
                                    false,
                                ) {
                                    log!("Browser key char failed: {err}");
                                }
                            }
                        }
                    }
                }
                Hit::KeyUp(key_event) => {
                    let modifiers = Self::key_event_modifiers(&key_event);
                    let windows_key_code = Self::windows_key_code(key_event.key_code);
                    let native_key_code = Self::native_key_code(key_event.key_code);
                    let character = if key_event.modifiers.control
                        || key_event.modifiers.alt
                        || key_event.modifiers.logo
                    {
                        0
                    } else {
                        Self::key_char(key_event.key_code, key_event.modifiers.shift)
                            .map(|ch| ch as u16)
                            .unwrap_or(0)
                    };

                    if let Some(browser) = &mut self.cef_browser {
                        if let Err(err) = browser.send_key_event(
                            makepad_cef::KEY_EVENT_KEYUP,
                            modifiers,
                            windows_key_code,
                            native_key_code,
                            character,
                            character,
                            false,
                        ) {
                            log!("Browser key up failed: {err}");
                        }
                    }
                }
                Hit::TextInput(text_event) => {
                    let ime_pos = self
                        .browser_rect(cx)
                        .map(|rect| rect.pos)
                        .unwrap_or_default();
                    self.update_ime_spot(cx, ime_pos);
                    if text_event.was_paste {
                        self.suppress_next_paste_shortcut = true;
                    }

                    if let Some(browser) = &mut self.cef_browser {
                        let modifiers =
                            Self::cef_modifiers(cx.keyboard.modifiers(), MouseButton::empty());
                        if text_event.was_paste
                            || text_event.replace_last
                            || Self::char_event_data(&text_event.input).is_none()
                        {
                            if let Err(err) = browser.ime_commit_text(&text_event.input) {
                                log!("Browser text commit failed: {err}");
                            }
                        } else if let Some((windows_key_code, character)) =
                            Self::char_event_data(&text_event.input)
                        {
                            // native_key_code is Carbon/NSEvent on Mac, NOT the
                            // Windows VK. VK_L is 0x4C, which on Mac is
                            // kVK_ANSI_KeypadEnter — L was submitting like Enter.
                            let native_key_code = Self::native_key_code_for_char(
                                char::from_u32(character as u32).unwrap_or('\0'),
                            );
                            if let Err(err) = browser.send_key_event(
                                makepad_cef::KEY_EVENT_CHAR,
                                modifiers,
                                windows_key_code,
                                native_key_code,
                                character,
                                character,
                                false,
                            ) {
                                log!("Browser char input failed: {err}");
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let desired_backend = self.resolved_backend();
        self.sync_backend_transition(cx, desired_backend);

        if !self.visible {
            if desired_backend == ActiveBrowserBackend::Native && self.system_browser_spawned {
                cx.system_browser(self.system_browser_id()).detach();
            }
            return DrawStep::done();
        }

        if desired_backend == ActiveBrowserBackend::Unsupported {
            if self.init_error.is_none() {
                let message = self.unsupported_backend_message().to_string();
                log!("{message}");
                self.init_error = Some(message);
            }
            self.draw_bg.draw_vars.empty_texture(0);
            self.draw_bg.draw_walk(cx, walk);
            return DrawStep::done();
        }

        match desired_backend {
            ActiveBrowserBackend::Native => {
                self.draw_bg.draw_vars.empty_texture(0);
                self.draw_bg.draw_walk(cx, walk);
                self.sync_system_browser(cx);
                DrawStep::done()
            }
            ActiveBrowserBackend::CEF => {
                #[cfg(feature = "cef")]
                {
                    let rect = cx.peek_walk_turtle(walk);
                    let dpi = cx.current_dpi_factor() as f32;
                    let width = (rect.size.x.max(1.0) * dpi as f64).round().max(1.0) as usize;
                    let height = (rect.size.y.max(1.0) * dpi as f64).round().max(1.0) as usize;

                    self.ensure_browser(cx, width, height, dpi);

                    match (self.painted, &self.texture) {
                        (true, Some(texture)) => {
                            // Sample only the part of the surface that holds
                            // page pixels and stretch it over the current
                            // rect: mid-drag that is the last good frame at a
                            // slightly older size, never a blank margin.
                            let scale = match (self.texture_alloc, self.texture_valid) {
                                (Some((aw, ah)), Some((vw, vh)))
                                    if aw > 0 && ah > 0 && vw > 0 && vh > 0 =>
                                {
                                    vec2(
                                        (vw as f32 / aw as f32).min(1.0),
                                        (vh as f32 / ah as f32).min(1.0),
                                    )
                                }
                                _ => vec2(1.0, 1.0),
                            };
                            self.draw_bg.image_scale = scale;
                            self.draw_bg.draw_vars.set_texture(0, texture);
                        }
                        _ => {
                            self.draw_bg.image_scale = vec2(1.0, 1.0);
                            self.draw_bg.draw_vars.empty_texture(0);
                        }
                    }

                    self.draw_bg.draw_walk(cx, walk);
                    cx.add_nav_stop(self.draw_bg.area(), NavRole::TextInput, Inset::default());
                    DrawStep::done()
                }
                #[cfg(not(feature = "cef"))]
                {
                    DrawStep::done()
                }
            }
            ActiveBrowserBackend::None | ActiveBrowserBackend::Unsupported => DrawStep::done(),
        }
    }
}

impl BrowserRef {
    pub fn set_url(&self, cx: &mut Cx, url: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_url_internal(cx, url);
        }
    }

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_visible_internal(cx, visible);
        }
    }

    /// The CEF page behind this widget, for what the widget does not wrap:
    /// where the page has navigated to, its history, its audio capture.
    /// `None` until the page exists (it is created by the first draw) and
    /// under any other backend. Call on the UI thread, which is the thread
    /// that pumps CEF.
    #[cfg(feature = "cef")]
    pub fn with_cef_browser<R>(
        &self,
        f: impl FnOnce(&mut makepad_cef::Browser) -> R,
    ) -> Option<R> {
        let mut inner = self.borrow_mut()?;
        inner.cef_browser.as_mut().map(f)
    }
}
