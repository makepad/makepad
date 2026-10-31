//! Clipboard OS work runs on the HAL's main thread. Replies own their bytes.
use makepad_apple_sys::*;
use crate::clipboard::ClipboardData;

pub(super) fn read() -> ClipboardData {
    let mut content = ClipboardData::default();
    unsafe {
        let pasteboard: ObjcId = msg_send![class!(NSPasteboard), generalPasteboard];
        if pasteboard == nil {
            return content;
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
                let png: ObjcId =
                    msg_send![rep, representationUsingType: 4u64 properties: properties];
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
            content.png = std::slice::from_raw_parts(bytes, len as usize).to_vec();
            break;
        }
        let text: ObjcId =
            msg_send![pasteboard, stringForType: str_to_nsstring("public.utf8-plain-text")];
        if text != nil {
            content.text = nsstring_to_string(text);
        }
        let items: ObjcId = msg_send![pasteboard, pasteboardItems];
        if items != nil {
            let count: usize = msg_send![items, count];
            for index in 0..count {
                let item: ObjcId = msg_send![items, objectAtIndex: index];
                let text: ObjcId = msg_send![item, stringForType: str_to_nsstring("public.file-url")];
                if text == nil { continue; }
                let url: ObjcId = msg_send![class!(NSURL), URLWithString: text];
                if url == nil { continue; }
                let is_file: BOOL = msg_send![url, isFileURL];
                if is_file != YES { continue; }
                let path: ObjcId = msg_send![url, path];
                if path != nil {
                    content.files.push((nsstring_to_string(text), nsstring_to_string(path)));
                }
            }
        }
        content
    }
}
