//! Pictures and files on the system clipboard, read when the app asks
//! (on its cmd-V): the text paste stays `Event::TextInput`, this is for an
//! app that also takes a pasted screenshot, a copied photo or a copied file.
//!
//! macOS reads `NSPasteboard`: a picture as PNG bytes (PNG as it is; TIFF,
//! what most apps and screenshots put there, converted to PNG), and copied
//! files as their paths. Other platforms have nothing yet.

/// The clipboard's picture as PNG bytes, if it holds one.
pub fn clipboard_image_png() -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        macos::image_png()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Paths of the files on the clipboard (copied in the Finder), if any.
pub fn clipboard_file_paths() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        macos::file_paths()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::os::apple::apple_sys::*;

    /// `NSBitmapImageFileTypePNG`.
    const PNG_FILE_TYPE: u64 = 4;
    /// A picture larger than this is not read (a paste is a reference, not
    /// a master).
    const MAX_BYTES: u64 = 64 << 20;

    unsafe fn data_bytes(data: ObjcId) -> Option<Vec<u8>> {
        if data.is_null() {
            return None;
        }
        let len: u64 = msg_send![data, length];
        let bytes: *const u8 = msg_send![data, bytes];
        if bytes.is_null() || len == 0 || len > MAX_BYTES {
            return None;
        }
        Some(std::slice::from_raw_parts(bytes, len as usize).to_vec())
    }

    unsafe fn pasteboard() -> ObjcId {
        msg_send![class!(NSPasteboard), generalPasteboard]
    }

    pub fn image_png() -> Option<Vec<u8>> {
        unsafe {
            let board = pasteboard();
            if board.is_null() {
                return None;
            }
            let png: ObjcId = msg_send![board, dataForType: str_to_nsstring("public.png")];
            if let Some(bytes) = data_bytes(png) {
                return Some(bytes);
            }
            let tiff: ObjcId = msg_send![board, dataForType: str_to_nsstring("public.tiff")];
            if tiff.is_null() {
                return None;
            }
            let rep: ObjcId = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
            if rep.is_null() {
                return None;
            }
            let properties: ObjcId = msg_send![class!(NSDictionary), dictionary];
            let png: ObjcId = msg_send![rep, representationUsingType: PNG_FILE_TYPE properties: properties];
            data_bytes(png)
        }
    }

    pub fn file_paths() -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let board = pasteboard();
            if board.is_null() {
                return out;
            }
            let items: ObjcId = msg_send![board, pasteboardItems];
            if items.is_null() {
                return out;
            }
            let count: u64 = msg_send![items, count];
            for index in 0..count {
                let item: ObjcId = msg_send![items, objectAtIndex: index];
                let text: ObjcId = msg_send![item, stringForType: str_to_nsstring("public.file-url")];
                if text.is_null() {
                    continue;
                }
                let url: ObjcId = msg_send![class!(NSURL), URLWithString: text];
                if url.is_null() {
                    continue;
                }
                let path: ObjcId = msg_send![url, path];
                if !path.is_null() {
                    out.push(nsstring_to_string(path));
                }
            }
        }
        out
    }
}
