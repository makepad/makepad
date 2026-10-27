//! The window controls a makepad window manager gives the windows it hosts. Under the WM a makepad
//! window draws its own caption, controls included, in the WM's desktop family: traffic lights at
//! the left in a macOS-like family, minimise / maximise / close at the right in a Windows-like one,
//! none in a tiling or mobile one. This is the one table of that: widgets' `resolve_chrome` reads
//! it for an app's caption, and `makepad run`'s load screen (relax-boot, which draws with fonts and
//! shaders only) reads [`host_window_controls`] for its caption, so the two look the same.
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

/// The window controls of a desktop family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowControls {
    /// No window controls (a tiling desktop, a phone).
    None = 1,
    /// Close / minimise / maximise as coloured lights at the caption's left.
    Lights = 2,
    /// Minimise / maximise / close at the caption's right.
    Buttons = 3,
}

impl WindowControls {
    /// The controls of the family `family`: its id (`macos`, `windows-2000`; widgets'
    /// `DesktopStyle::id`) or its name as a style sheet carries it (`Macos`, `Windows2000`).
    /// An unknown family has none.
    pub fn of_family(family: &str) -> Self {
        let mut key = [0u8; 16];
        let mut n = 0;
        for b in family.bytes().filter(|b| *b != b'-') {
            if n == key.len() {
                return Self::None;
            }
            key[n] = b.to_ascii_lowercase();
            n += 1;
        }
        match &key[..n] {
            b"macos" | b"nextstep" => Self::Lights,
            b"windows" | b"windows2000" => Self::Buttons,
            _ => Self::None,
        }
    }

    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::None),
            2 => Some(Self::Lights),
            3 => Some(Self::Buttons),
            _ => None,
        }
    }
}

/// The family a style sheet names (the `family` of a `{"makepad_style": {...}}` message or of a
/// `MAKEPAD_WIDGET_STYLE_FILE`): `{"Macos":[]}` as the sheet serialises it, or a plain string.
pub fn sheet_family(json: &str) -> Option<&str> {
    let bytes = json.as_bytes();
    let mut from = 0;
    while let Some(at) = json[from..].find("\"family\"").map(|i| i + from) {
        from = at + 1;
        // a key, not text inside a string (a quote in a string is always escaped)
        if at > 0 && bytes[at - 1] == b'\\' {
            continue;
        }
        let rest = json[at + 8..].trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('{').map(str::trim_start).unwrap_or(rest);
        let name = rest.strip_prefix('"')?;
        return Some(&name[..name.find('"')?]);
    }
    None
}

/// Set from the WM's style sheet: 0 not read yet, 0xff read and none, else a `WindowControls`.
static HOST_CONTROLS: AtomicU8 = AtomicU8::new(0);

/// The window controls of the window manager this process runs under, from its style sheet: the
/// one it hands a child at spawn (the family of `MAKEPAD_WIDGET_STYLE_FILE`, else
/// `MAKEPAD_WIDGET_STYLE_FAMILY` next to `MAKEPAD_WIDGET_STYLE`, as widgets' `system_style` reads
/// the sheet), then its `makepad_style` messages (the WM-hosted shell, [`set_host_sheet`]).
/// `None` without a WM's sheet. Meaningful only where a WM hosts the windows
/// (`info_flags::WM_HOSTED`).
pub fn host_window_controls() -> Option<WindowControls> {
    match HOST_CONTROLS.load(Ordering::Acquire) {
        0 => {
            let controls = env_family().map(|family| WindowControls::of_family(&family));
            // a message that came meanwhile wins
            let _ = HOST_CONTROLS.compare_exchange(0, controls.map_or(0xff, |c| c as u8), Ordering::AcqRel, Ordering::Acquire);
            WindowControls::from_u8(HOST_CONTROLS.load(Ordering::Acquire))
        }
        v => WindowControls::from_u8(v),
    }
}

/// The WM sent its style sheet (`makepad_style`): its family's controls hold from now on.
/// Returns whether they changed.
pub fn set_host_sheet(json: &str) -> bool {
    let Some(family) = sheet_family(json) else { return false };
    let v = WindowControls::of_family(family) as u8;
    HOST_CONTROLS.swap(v, Ordering::AcqRel) != v
}

/// The stock caption a window draws under the WM's style sheet, as widgets resolves it
/// (`desktop_style::caption_look`, sent by the WM as `HostedCaption`): bar and title colours
/// (0xRRGGBBAA) with focus and without, the title's font size, the buttons' glyph colour.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostCaption {
    pub bar: u32,
    pub bar_unfocused: u32,
    pub ink: u32,
    pub ink_unfocused: u32,
    /// points (widgets' font size)
    pub size: f32,
    /// the window buttons' glyph colour
    pub button: u32,
}

static HOST_CAPTION: Mutex<Option<HostCaption>> = Mutex::new(None);

/// The caption of the WM's style sheet (the WM-hosted shell sets it from the WM's message):
/// `None` before one came, or without a WM.
pub fn host_caption() -> Option<HostCaption> {
    *HOST_CAPTION.lock().unwrap_or_else(|e| e.into_inner())
}

/// The WM sent its sheet's caption. Returns whether it changed.
pub fn set_host_caption(caption: HostCaption) -> bool {
    let mut at = HOST_CAPTION.lock().unwrap_or_else(|e| e.into_inner());
    let changed = *at != Some(caption);
    *at = Some(caption);
    changed
}

fn env_family() -> Option<String> {
    if let Some(path) = std::env::var_os("MAKEPAD_WIDGET_STYLE_FILE") {
        if let Some(family) = std::fs::read_to_string(path).ok().as_deref().and_then(sheet_family) {
            return Some(family.to_string());
        }
    }
    std::env::var("MAKEPAD_WIDGET_STYLE").ok()?;
    std::env::var("MAKEPAD_WIDGET_STYLE_FAMILY").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_by_id_and_by_sheet_name() {
        assert_eq!(WindowControls::of_family("macos"), WindowControls::Lights);
        assert_eq!(WindowControls::of_family("NextStep"), WindowControls::Lights);
        assert_eq!(WindowControls::of_family("windows-2000"), WindowControls::Buttons);
        assert_eq!(WindowControls::of_family("Windows2000"), WindowControls::Buttons);
        assert_eq!(WindowControls::of_family("Windows"), WindowControls::Buttons);
        assert_eq!(WindowControls::of_family("omarchy"), WindowControls::None);
        assert_eq!(WindowControls::of_family("BlackOrange"), WindowControls::None);
        assert_eq!(WindowControls::of_family("ios"), WindowControls::None);
        assert_eq!(WindowControls::of_family("a-family-name-longer-than-any"), WindowControls::None);
    }

    #[test]
    fn the_family_of_a_style_message() {
        let json = r#"{"makepad_style":{"name":"macos-dark","family":{"Macos":[]},"resources":"","theme":"x \"family\": {\"Windows\":[]}","widgets":"","icons":[]}}"#;
        assert_eq!(sheet_family(json), Some("Macos"));
        let escaped_first = r#"{"makepad_style":{"name":"\"family\":\"Windows\"","family" : { "NextStep" : [] }}}"#;
        assert_eq!(sheet_family(escaped_first), Some("NextStep"));
        assert_eq!(sheet_family(r#"{"family":"windows"}"#), Some("windows"));
        assert_eq!(sheet_family(r#"{"makepad_style":{"name":"windows"}}"#), None);
    }
}
