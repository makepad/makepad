//! Reading what is on the system clipboard, text or an image, on request.
//!
//! Pasting into a text field already works through `TextInput { was_paste }`
//! on every desktop; this is for an app that wants to look at the clipboard
//! itself (an image included), e.g. to offer to attach it. Call it on the UI
//! thread; it answers at once from the pasteboard.
//!
//! Backend coverage: macOS (NSPasteboard: PNG or TIFF images, UTF-8 text)
//! and Windows (CF_DIB images of 24 or 32 bits, CF_UNICODETEXT). X11 and
//! Wayland answer a clipboard read asynchronously through the display
//! server's selection protocol, which this synchronous call does not speak
//! yet: there it reports [`ClipboardContent::Unsupported`].

/// An image on the clipboard.
pub enum ClipboardImage {
    /// Encoded PNG, as the pasteboard holds it (macOS).
    Png(Vec<u8>),
    /// Tightly packed top-down RGBA8 (Windows' device-independent bitmap).
    Rgba { width: u32, height: u32, rgba: Vec<u8> },
}

pub enum ClipboardContent {
    Empty,
    Text(String),
    Image(ClipboardImage),
    /// This platform cannot read the clipboard synchronously.
    Unsupported,
}

/// What is on the system clipboard now. An image wins over text when the
/// clipboard holds both (a copied picture often carries a name as text).
pub fn read_clipboard() -> ClipboardContent {
    #[cfg(all(target_os = "macos", not(gpusim)))]
    {
        macos::read()
    }
    #[cfg(all(target_os = "windows", not(gpusim)))]
    {
        windows::read()
    }
    #[cfg(not(any(all(target_os = "macos", not(gpusim)), all(target_os = "windows", not(gpusim)))))]
    {
        ClipboardContent::Unsupported
    }
}

#[cfg(all(target_os = "macos", not(gpusim)))]
mod macos {
    use super::*;
    use crate::os::apple::apple_sys::*;
    use crate::os::apple_util::{nsstring_to_string, str_to_nsstring};

    pub fn read() -> ClipboardContent {
        unsafe {
            let pasteboard: ObjcId = msg_send![class!(NSPasteboard), generalPasteboard];
            if pasteboard == nil {
                return ClipboardContent::Empty;
            }
            // Images first: PNG as it is, TIFF (what most macOS apps and
            // the screenshot tool put there) converted to PNG.
            for (uti, is_png) in [("public.png", true), ("public.tiff", false)] {
                let data: ObjcId = msg_send![pasteboard, dataForType: str_to_nsstring(uti)];
                if data == nil {
                    continue;
                }
                let png: ObjcId = if is_png {
                    data
                } else {
                    let rep: ObjcId = msg_send![class!(NSBitmapImageRep), imageRepWithData: data];
                    if rep == nil {
                        continue;
                    }
                    let properties: ObjcId = msg_send![class!(NSDictionary), dictionary];
                    // NSBitmapImageFileTypePNG
                    let png: ObjcId = msg_send![rep, representationUsingType: 4u64 properties: properties];
                    png
                };
                if png == nil {
                    continue;
                }
                let len: u64 = msg_send![png, length];
                let bytes: *const u8 = msg_send![png, bytes];
                if len == 0 || bytes.is_null() {
                    continue;
                }
                let bytes = std::slice::from_raw_parts(bytes, len as usize).to_vec();
                return ClipboardContent::Image(ClipboardImage::Png(bytes));
            }
            let text: ObjcId = msg_send![pasteboard, stringForType: str_to_nsstring("public.utf8-plain-text")];
            if text != nil {
                let text = nsstring_to_string(text);
                if !text.is_empty() {
                    return ClipboardContent::Text(text);
                }
            }
            ClipboardContent::Empty
        }
    }
}

#[cfg(all(target_os = "windows", not(gpusim)))]
mod windows {
    use super::*;
    use crate::windows::Win32::{
        Foundation::HGLOBAL,
        System::{
            DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
    };

    const CF_DIB: u32 = 8;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;

    /// The bytes behind one clipboard format, copied out, or `None`.
    unsafe fn format_bytes(format: u32) -> Option<Vec<u8>> {
        let handle = GetClipboardData(format).ok()?;
        let global = std::mem::transmute::<_, HGLOBAL>(handle);
        let ptr = GlobalLock(global) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let size = GlobalSize(global);
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }

    /// A packed DIB (BITMAPINFOHEADER + masks + pixels) of 24 or 32 bits.
    fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
        let u32_at = |o: usize| dib.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let header = u32_at(0)? as usize;
        let width = u32_at(4)? as i32;
        let height = u32_at(8)? as i32;
        let bits = u16::from_le_bytes([*dib.get(14)?, *dib.get(15)?]);
        let compression = u32_at(16)?;
        if width <= 0 || height == 0 || !(bits == 24 || bits == 32) || !(compression == BI_RGB || compression == BI_BITFIELDS) {
            return None;
        }
        let (w, h) = (width as usize, height.unsigned_abs() as usize);
        let masks = if compression == BI_BITFIELDS && header == 40 { 12 } else { 0 };
        let offset = header + masks;
        let stride = (w * bits as usize / 8 + 3) & !3;
        if dib.len() < offset + stride * h {
            return None;
        }
        let mut rgba = vec![0u8; w * h * 4];
        let mut any_alpha = false;
        for y in 0..h {
            // Positive height is bottom-up.
            let src_y = if height > 0 { h - 1 - y } else { y };
            let row = &dib[offset + src_y * stride..];
            for x in 0..w {
                let (b, g, r, a) = if bits == 32 {
                    let p = &row[x * 4..x * 4 + 4];
                    (p[0], p[1], p[2], p[3])
                } else {
                    let p = &row[x * 3..x * 3 + 3];
                    (p[0], p[1], p[2], 255)
                };
                any_alpha |= a != 0;
                let o = (y * w + x) * 4;
                rgba[o..o + 4].copy_from_slice(&[r, g, b, a]);
            }
        }
        // Most 32-bit DIBs leave alpha at zero: that means opaque.
        if !any_alpha {
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        Some((w as u32, h as u32, rgba))
    }

    pub fn read() -> ClipboardContent {
        unsafe {
            if OpenClipboard(None).is_err() {
                return ClipboardContent::Empty;
            }
            let image = format_bytes(CF_DIB).and_then(|dib| dib_to_rgba(&dib));
            let text = if image.is_none() {
                format_bytes(CF_UNICODETEXT.0 as u32).map(|bytes| {
                    let units: Vec<u16> = bytes
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .take_while(|u| *u != 0)
                        .collect();
                    String::from_utf16_lossy(&units)
                })
            } else {
                None
            };
            let _ = CloseClipboard();
            match (image, text) {
                (Some((width, height, rgba)), _) => ClipboardContent::Image(ClipboardImage::Rgba { width, height, rgba }),
                (None, Some(text)) if !text.is_empty() => ClipboardContent::Text(text),
                _ => ClipboardContent::Empty,
            }
        }
    }
}
