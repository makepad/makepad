//! The clipboard: text and rich-flavour writes (`ClipSet`) and reads of every representation at once
//! (`ClipRead` -> `Clipboard`): text, HTML, Markdown, an image (the registered "PNG" format as it is, else a
//! CF_DIBV5 / CF_DIB bitmap converted to PNG on the clipboard worker, never in the WndProc) and
//! CF_HDROP file lists (file URL + native path). All representations are copied out under one
//! open of the clipboard. A clipboard another process holds open is tried again on a later timer
//! turn with the same request id.

use crate::clipboard::ClipboardData;
use crate::Id;
use makepad_windows_sys::core::PCWSTR;
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::System::DataExchange::*;
use makepad_windows_sys::Win32::System::Memory::*;
use makepad_windows_sys::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use makepad_windows_sys::Win32::UI::Shell::{DragQueryFileW, HDROP};
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use std::cell::RefCell;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

/// The dispatch window's timer that retries busy reads.
pub(super) const RETRY_TIMER: usize = 0x4d50_0001;
const RETRY_MS: u32 = 30;

thread_local! {
    /// reads waiting for the clipboard: (session, request)
    static PENDING: RefCell<Vec<(u32, u64)>> = const { RefCell::new(Vec::new()) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A copy of a global memory block's bytes.
unsafe fn global_bytes(h: HANDLE) -> Option<Vec<u8>> {
    let mem = HGLOBAL(h.0);
    let p = GlobalLock(mem) as *const u8;
    if p.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(p, GlobalSize(mem)).to_vec();
    let _ = GlobalUnlock(mem);
    Some(bytes)
}

fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|u| *u != 0).collect();
    String::from_utf16_lossy(&units)
}

fn html_format() -> u32 {
    static FORMAT: OnceLock<u32> = OnceLock::new();
    *FORMAT.get_or_init(|| unsafe { RegisterClipboardFormatW(PCWSTR(wide("HTML Format").as_ptr())) })
}

fn markdown_format() -> u32 {
    static FORMAT: OnceLock<u32> = OnceLock::new();
    *FORMAT.get_or_init(|| unsafe { RegisterClipboardFormatW(PCWSTR(wide("text/markdown").as_ptr())) })
}

fn build_cf_html(fragment: &str) -> Vec<u8> {
    let header = |start_html, end_html, start_fragment, end_fragment| {
        format!("Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n")
    };
    let prefix = "<html><body>\r\n<!--StartFragment-->";
    let suffix = "<!--EndFragment-->\r\n</body></html>";
    let start_html = header(0usize, 0usize, 0usize, 0usize).len();
    let start_fragment = start_html + prefix.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + suffix.len();
    let header = header(start_html, end_html, start_fragment, end_fragment);
    format!("{header}{prefix}{fragment}{suffix}\0").into_bytes()
}

fn parse_cf_html(bytes: &[u8]) -> Option<String> {
    let len = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    let bytes = &bytes[..len];
    let header_end = bytes.iter().position(|b| *b == b'<').unwrap_or(bytes.len());
    let header = std::str::from_utf8(&bytes[..header_end]).ok()?;
    let offset = |name: &str| {
        header.lines().find_map(|line| line.strip_prefix(name)).and_then(|value| value.trim().parse::<usize>().ok())
    };
    if let (Some(start), Some(end)) = (offset("StartFragment:"), offset("EndFragment:")) {
        if start >= header_end {
            if let Some(fragment) = bytes.get(start..end).and_then(|b| std::str::from_utf8(b).ok()) {
                return Some(fragment.to_string());
            }
        }
    }
    let start_marker = b"<!--StartFragment-->";
    let end_marker = b"<!--EndFragment-->";
    let start = bytes.windows(start_marker.len()).position(|b| b == start_marker)? + start_marker.len();
    let end = bytes[start..].windows(end_marker.len()).position(|b| b == end_marker)? + start;
    std::str::from_utf8(&bytes[start..end]).ok().map(str::to_string)
}

#[cfg(test)]
#[test]
fn cf_html_round_trip() {
    let fragment = "<p>Héllo 世界 🦀</p>";
    let bytes = build_cf_html(fragment);
    assert_eq!(parse_cf_html(&bytes).as_deref(), Some(fragment), "UTF-8 byte offsets");
    let html = String::from_utf8(bytes).unwrap();
    let offsets_only = html.replace("<!--StartFragment-->", "<!--StartContents-->");
    assert_eq!(parse_cf_html(offsets_only.as_bytes()).as_deref(), Some(fragment), "offsets without a fallback marker");
    let fallback = html.replace("StartFragment:", "MissingOffset:");
    assert_eq!(parse_cf_html(fallback.as_bytes()).as_deref(), Some(fragment), "comment marker fallback");
    assert_eq!(parse_cf_html(b"StartFragment:999\r\nEndFragment:1000\r\n<html></html>"), None, "out-of-range offsets");
}

unsafe fn set_bytes(format: u32, bytes: &[u8]) {
    if format == 0 {
        return;
    }
    // GMEM_MOVEABLE: ownership passes to the clipboard on success
    if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) {
        let p = GlobalLock(mem) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
        } else {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let _ = GlobalUnlock(mem);
            if SetClipboardData(format, Some(HANDLE(mem.0))).is_err() {
                let _ = GlobalFree(Some(mem));
            }
        }
    }
}

pub(super) fn set(data: &ClipboardData) {
    let utf16 = wide(&data.text);
    let html = (!data.html.is_empty()).then(|| build_cf_html(&data.html));
    unsafe {
        if OpenClipboard(None).is_err() {
            eprintln!("[hal-win32] clipboard busy: data not copied");
            return;
        }
        let _ = EmptyClipboard();
        set_bytes(CF_UNICODETEXT.0 as u32, std::slice::from_raw_parts(utf16.as_ptr().cast(), utf16.len() * 2));
        if let Some(html) = html {
            set_bytes(html_format(), &html);
        }
        if !data.markdown.is_empty() {
            set_bytes(markdown_format(), data.markdown.as_bytes());
        }
        let _ = CloseClipboard();
    }
}

/// The clipboard's text (the paste chord); `None` when there is none or it is busy.
pub(super) fn read_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|h| global_bytes(h)).map(|b| utf16_text(&b));
        let _ = CloseClipboard();
        text
    }
}

/// A Windows path as a file URL (`file:///C:/a%20b.txt`, UNC `file://server/share/x`).
pub(super) fn file_url(path: &str) -> String {
    let p = path.replace('\\', "/");
    let mut out = String::from(if p.starts_with("//") { "file:" } else { "file:///" });
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The paths of a CF_HDROP handle.
pub(super) unsafe fn hdrop_paths(h: HDROP) -> Vec<String> {
    let count = DragQueryFileW(h, u32::MAX, None, 0);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = DragQueryFileW(h, i, None, 0);
        let mut buf = vec![0u16; len as usize + 1];
        let n = DragQueryFileW(h, i, Some(makepad_windows_sys::core::PWSTR(buf.as_mut_ptr())), buf.len() as u32);
        if n > 0 {
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
    }
    out
}

/// What one open of the clipboard copied out; a bitmap still to convert.
struct Snapshot {
    data: ClipboardData,
    dib: Option<Vec<u8>>,
}

fn snapshot() -> Option<Snapshot> {
    static PNG: OnceLock<u32> = OnceLock::new();
    let png_format = *PNG.get_or_init(|| unsafe { RegisterClipboardFormatW(PCWSTR(wide("PNG").as_ptr())) });
    unsafe {
        OpenClipboard(None).ok()?;
        let mut s = Snapshot { data: ClipboardData::default(), dib: None };
        if let Some(b) = GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|h| global_bytes(h)) {
            s.data.text = utf16_text(&b);
        }
        let html = html_format();
        if html != 0 {
            if let Some(b) = GetClipboardData(html).ok().and_then(|h| global_bytes(h)) {
                s.data.html = parse_cf_html(&b).unwrap_or_default();
            }
        }
        let markdown = markdown_format();
        if markdown != 0 {
            if let Some(b) = GetClipboardData(markdown).ok().and_then(|h| global_bytes(h)) {
                s.data.markdown = String::from_utf8_lossy(&b).trim_end_matches('\0').to_string();
            }
        }
        if png_format != 0 && IsClipboardFormatAvailable(png_format).is_ok() {
            if let Some(b) = GetClipboardData(png_format).ok().and_then(|h| global_bytes(h)) {
                s.data.png = b;
            }
        }
        if s.data.png.is_empty() {
            for f in [CF_DIBV5, CF_DIB] {
                if IsClipboardFormatAvailable(f.0 as u32).is_ok() {
                    if let Some(b) = GetClipboardData(f.0 as u32).ok().and_then(|h| global_bytes(h)) {
                        s.dib = Some(b);
                        break;
                    }
                }
            }
        }
        if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
            for path in hdrop_paths(HDROP(h.0)) {
                s.data.files.push((file_url(&path), path));
            }
        }
        let _ = CloseClipboard();
        Some(s)
    }
}

fn answer(sid: u32, req: u64, data: &ClipboardData) {
    if super::log_enabled() {
        eprintln!("[hal-win32] clipboard read {req}: text {} bytes, png {} bytes, {} files", data.text.len(), data.png.len(), data.files.len());
    }
    let content = data.pack();
    super::Shared::global().push((sid, Id::NONE), |q, _| {
        q.payload_size(content.len() as u32);
        q.clipboard(req, true, &content);
    });
}

/// The clipboard worker: bitmap conversion and PNG encoding off the main thread.
fn convert(sid: u32, req: u64, data: ClipboardData, dib: Vec<u8>) {
    static WORKER: OnceLock<Mutex<Sender<(u32, u64, ClipboardData, Vec<u8>)>>> = OnceLock::new();
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<(u32, u64, ClipboardData, Vec<u8>)>();
        let spawned = std::thread::Builder::new().name("makepad-win32-clipboard".into()).spawn(move || {
            while let Ok((sid, req, mut data, dib)) = rx.recv() {
                if let Some((w, h, rgba)) = dib_rgba(&dib) {
                    data.png = crate::automation::png_rgba(w, h, &rgba);
                }
                answer(sid, req, &data);
            }
        });
        if let Err(e) = spawned {
            eprintln!("[hal-win32] clipboard worker: {e}");
        }
        Mutex::new(tx)
    });
    if let Err(e) = super::lock(tx).send((sid, req, data, dib)) {
        // no worker: the read answers without the image
        let (sid, req, data, _) = e.0;
        answer(sid, req, &data);
    }
}

fn try_read(sid: u32, req: u64) -> bool {
    match snapshot() {
        Some(Snapshot { data, dib: Some(dib) }) => convert(sid, req, data, dib),
        Some(Snapshot { data, dib: None }) => answer(sid, req, &data),
        None => return false,
    }
    true
}

/// `ClipRead`: answered now, or on a later turn while another process holds the clipboard.
pub(super) fn read(dispatch: HWND, sid: u32, req: u64) {
    let queued = PENDING.with_borrow(|p| !p.is_empty());
    if !queued && try_read(sid, req) {
        return;
    }
    PENDING.with_borrow_mut(|p| p.push((sid, req)));
    if !queued {
        if super::log_enabled() {
            eprintln!("[hal-win32] clipboard busy: read {req} retries");
        }
        unsafe {
            SetTimer(Some(dispatch), RETRY_TIMER, RETRY_MS, None);
        }
    }
}

/// The retry timer: the waiting reads in order, until the clipboard is busy again.
pub(super) fn retry(dispatch: HWND) {
    loop {
        let Some((sid, req)) = PENDING.with_borrow(|p| p.first().copied()) else { break };
        if !try_read(sid, req) {
            return;
        }
        PENDING.with_borrow_mut(|p| {
            p.remove(0);
        });
    }
    unsafe {
        let _ = KillTimer(Some(dispatch), RETRY_TIMER);
    }
}

pub(super) fn session_closed(sid: u32) {
    PENDING.with_borrow_mut(|p| p.retain(|(s, _)| *s != sid));
}

/// A device-independent bitmap (BITMAPINFOHEADER / V4 / V5 and its pixels) as RGBA8 rows,
/// top-down. 1/4/8-bit palettes, 16/24/32-bit RGB and bitfields.
pub(super) fn dib_rgba(b: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let u32_at = |at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let u16_at = |at: usize| b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let header = u32_at(0)? as usize;
    if header < 40 {
        return None;
    }
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bpp = u16_at(14)? as u32;
    let compression = u32_at(16)?;
    let colors_used = u32_at(32)? as usize;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    if width <= 0 || height == 0 || !matches!(compression, BI_RGB | BI_BITFIELDS) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let mut at = header;
    let masks: Option<[u32; 4]> = if compression == BI_BITFIELDS {
        if header >= 56 {
            Some([u32_at(40)?, u32_at(44)?, u32_at(48)?, u32_at(52)?])
        } else {
            at += 12;
            Some([u32_at(40)?, u32_at(44)?, u32_at(48)?, 0])
        }
    } else {
        match bpp {
            16 => Some([0x7c00, 0x03e0, 0x001f, 0]),
            32 => Some([0x00ff_0000, 0x0000_ff00, 0x0000_00ff, if header >= 56 { u32_at(52)? } else { 0 }]),
            _ => None,
        }
    };
    let mut palette: Vec<[u8; 4]> = Vec::new();
    if bpp <= 8 {
        let n = if colors_used != 0 { colors_used } else { 1 << bpp };
        for i in 0..n {
            let e = b.get(at + i * 4..at + i * 4 + 4)?;
            palette.push([e[2], e[1], e[0], 255]);
        }
        at += n * 4;
    }
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let pixels = b.get(at..at + stride * h)?;
    let channel = |v: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 255;
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        (((v & mask) >> shift) * 255 / max) as u8
    };
    let mut out = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src_row = if height > 0 { h - 1 - y } else { y };
        let row = &pixels[src_row * stride..src_row * stride + stride];
        for x in 0..w {
            let px = match bpp {
                1 | 4 | 8 => {
                    let bit = x * bpp as usize;
                    let idx = (row[bit / 8] >> (8 - bpp as usize - bit % 8)) as usize & ((1 << bpp) - 1);
                    *palette.get(idx)?
                }
                24 => [row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255],
                16 | 32 => {
                    let v = if bpp == 16 { u16::from_le_bytes([row[x * 2], row[x * 2 + 1]]) as u32 } else { u32::from_le_bytes([row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3]]) };
                    let m = masks?;
                    let a = if m[3] == 0 { 255 } else { channel(v, m[3]) };
                    if m[3] != 0 && a != 0 {
                        any_alpha = true;
                    }
                    [channel(v, m[0]), channel(v, m[1]), channel(v, m[2]), a]
                }
                _ => return None,
            };
            out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&px);
        }
    }
    // an alpha mask that is zero everywhere is an unused byte, not a transparent picture
    if masks.is_some_and(|m| m[3] != 0) && !any_alpha {
        for p in out.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    Some((w as u32, h as u32, out))
}
