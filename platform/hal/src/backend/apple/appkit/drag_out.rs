//! Drags out of the app (`DragStart`): existing files as plain file URLs, and files the app is
//! still making (`input::drag_item::PROMISE`, see `crate::drag_promise`) as file promises.
//!
//! A promise is an `NSFilePromiseProvider` subclass, `HalFilePromise`, that also offers a
//! lazily resolved `public.file-url`:
//! - a receiver that takes promises (Finder, Mail, chat apps) gets `writePromiseToURL`, answered
//!   on a background operation queue: wait for the file, copy it next to the destination, sync,
//!   rename it into place, and only then call the completion handler;
//! - a receiver that only reads file URLs (most chat and web apps) gets the URL of the finished
//!   file when it asks for it at the drop. The type is declared `Promised`, so nothing is
//!   resolved while the pointer only hovers.
//! The app's producer runs from the moment the drag starts, so either wait is usually short.

use crate::drag_promise;
use makepad_apple_sys::*;
use std::sync::OnceLock;

const FILE_URL: &str = "public.file-url";
/// `NSPasteboardWritingPromised`.
const WRITING_PROMISED: u64 = 1 << 9;

struct Classes {
    promise: usize,
    delegate: usize,
    queue: usize,
}

fn classes() -> &'static Classes {
    static C: OnceLock<Classes> = OnceLock::new();
    C.get_or_init(|| unsafe {
        let promise = define_promise_class();
        let delegate_class = define_delegate_class();
        // The provider's delegate property is weak: the one delegate lives for the process.
        let delegate: ObjcId = msg_send![delegate_class as *const Class, new];
        let queue: ObjcId = msg_send![class!(NSOperationQueue), new];
        let () = msg_send![queue, setQualityOfService: 0x19i64]; // user-initiated
        Classes { promise: promise as usize, delegate: delegate as usize, queue: queue as usize }
    })
}

unsafe fn promise_path(provider: &Object) -> String {
    let path: ObjcId = *provider.get_ivar("halPath");
    nsstring_to_string(path)
}

unsafe fn is_file_url(ty: ObjcId) -> bool {
    let equal: BOOL = msg_send![ty, isEqualToString: str_to_nsstring(FILE_URL)];
    equal != NO
}

fn define_promise_class() -> *const Class {
    extern "C" fn writable_types(this: &Object, _: Sel, pasteboard: ObjcId) -> ObjcId {
        unsafe {
            let types: ObjcId = msg_send![super(this, class!(NSFilePromiseProvider)), writableTypesForPasteboard: pasteboard];
            msg_send![types, arrayByAddingObject: str_to_nsstring(FILE_URL)]
        }
    }
    extern "C" fn writing_options(this: &Object, _: Sel, ty: ObjcId, pasteboard: ObjcId) -> u64 {
        unsafe {
            if is_file_url(ty) {
                return WRITING_PROMISED;
            }
            msg_send![super(this, class!(NSFilePromiseProvider)), writingOptionsForType: ty pasteboard: pasteboard]
        }
    }
    extern "C" fn property_list(this: &Object, _: Sel, ty: ObjcId) -> ObjcId {
        unsafe {
            if !is_file_url(ty) {
                return msg_send![super(this, class!(NSFilePromiseProvider)), pasteboardPropertyListForType: ty];
            }
            let path = promise_path(this);
            match drag_promise::wait(std::path::Path::new(&path), drag_promise::WAIT) {
                // The URL's own pasteboard form, exactly what a plain file URL item writes (and
                // whatever AppKit attaches to it for a sandboxed reader), not a bare string.
                Ok(()) => {
                    let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(&path) isDirectory: NO];
                    msg_send![url, pasteboardPropertyListForType: str_to_nsstring(FILE_URL)]
                }
                Err(e) => {
                    eprintln!("[hal] drag out: {e}");
                    nil
                }
            }
        }
    }
    extern "C" fn dealloc(this: &Object, _: Sel) {
        unsafe {
            let path: ObjcId = *this.get_ivar("halPath");
            if path != nil {
                let () = msg_send![path, release];
            }
            let () = msg_send![super(this, class!(NSFilePromiseProvider)), dealloc];
        }
    }
    let mut decl = ClassDecl::new("HalFilePromise", class!(NSFilePromiseProvider)).unwrap();
    unsafe {
        decl.add_method(sel!(writableTypesForPasteboard:), writable_types as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId);
        decl.add_method(sel!(writingOptionsForType: pasteboard:), writing_options as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> u64);
        decl.add_method(sel!(pasteboardPropertyListForType:), property_list as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId);
        decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&Object, Sel));
    }
    decl.add_ivar::<ObjcId>("halPath");
    decl.register()
}

fn define_delegate_class() -> *const Class {
    extern "C" fn file_name(_this: &Object, _: Sel, provider: ObjcId, _ty: ObjcId) -> ObjcId {
        unsafe {
            let path = promise_path(&*provider);
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            str_to_nsstring(&name)
        }
    }
    extern "C" fn write_promise(_this: &Object, _: Sel, provider: ObjcId, url: ObjcId, completion: ObjcId) {
        unsafe {
            let path = promise_path(&*provider);
            let dest: ObjcId = msg_send![url, path];
            let dest = nsstring_to_string(dest);
            // The completion handler runs only after the whole file sits under its final name.
            let result = drag_promise::wait(std::path::Path::new(&path), drag_promise::WAIT)
                .and_then(|()| drag_promise::deliver(std::path::Path::new(&path), std::path::Path::new(&dest)));
            let error: ObjcId = match result {
                Ok(()) => nil,
                Err(e) => {
                    eprintln!("[hal] drag out: {e}");
                    let info: ObjcId = msg_send![class!(NSDictionary), dictionaryWithObject: str_to_nsstring(&e) forKey: str_to_nsstring("NSLocalizedDescription")];
                    msg_send![class!(NSError), errorWithDomain: str_to_nsstring("makepad.drag") code: 1i64 userInfo: info]
                }
            };
            objc_block_invoke!(completion, invoke((error): ObjcId));
        }
    }
    extern "C" fn queue(_this: &Object, _: Sel, _provider: ObjcId) -> ObjcId {
        classes().queue as ObjcId
    }
    let mut decl = ClassDecl::new("HalFilePromiseDelegate", class!(NSObject)).unwrap();
    unsafe {
        decl.add_method(sel!(filePromiseProvider: fileNameForType:), file_name as extern "C" fn(&Object, Sel, ObjcId, ObjcId) -> ObjcId);
        decl.add_method(
            sel!(filePromiseProvider: writePromiseToURL: completionHandler:),
            write_promise as extern "C" fn(&Object, Sel, ObjcId, ObjcId, ObjcId),
        );
        decl.add_method(sel!(operationQueueForFilePromiseProvider:), queue as extern "C" fn(&Object, Sel, ObjcId) -> ObjcId);
    }
    if let Some(protocol) = Protocol::get("NSFilePromiseProviderDelegate") {
        decl.add_protocol(protocol);
    }
    decl.register()
}

/// The uniform type of a promised file, from its MIME type, else its extension.
unsafe fn uniform_type(path: &str, mime: &str) -> ObjcId {
    if let Some(ut) = Class::get("UTType") {
        let mut ty: ObjcId = nil;
        if !mime.is_empty() {
            ty = msg_send![ut, typeWithMIMEType: str_to_nsstring(mime)];
        }
        if ty == nil {
            if let Some(ext) = std::path::Path::new(path).extension() {
                ty = msg_send![ut, typeWithFilenameExtension: str_to_nsstring(&ext.to_string_lossy())];
            }
        }
        if ty != nil {
            return msg_send![ty, identifier];
        }
    }
    str_to_nsstring("public.data")
}

/// A pasteboard writer (+1 retained) for a promise item `<path>\n<mime>`.
pub(super) unsafe fn promise_writer(item: &str) -> ObjcId {
    let (path, mime) = drag_promise::decode(item);
    let c = classes();
    let provider: ObjcId = msg_send![c.promise as *const Class, alloc];
    let provider: ObjcId = msg_send![provider, initWithFileType: uniform_type(path, mime) delegate: c.delegate as ObjcId];
    if provider == nil {
        return nil;
    }
    let ns_path: ObjcId = str_to_nsstring(path);
    let () = msg_send![ns_path, retain];
    (*provider).set_ivar("halPath", ns_path);
    provider
}

/// The promise as AppKit's receivers see it, without a drag session (a drag needs a person at the
/// pointer): the provider's types, the lazily resolved file URL, the promise name, and a
/// `writePromiseToURL` that waits for the producer, copies, and reports failures as an NSError.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Calls the delegate the way a receiver's drop does. Like an impatient receiver, the
    /// completion handler reads the destination the instant it is called: the bytes it saw there
    /// are returned with the error.
    unsafe fn write_promise_read(provider: ObjcId, dest: &std::path::Path) -> (Option<String>, Option<Vec<u8>>) {
        let result: Arc<Mutex<Option<(Option<String>, Option<Vec<u8>>)>>> = Arc::new(Mutex::new(None));
        let got = result.clone();
        let read_at = dest.to_path_buf();
        let block = objc_block!(move |error: ObjcId| {
            let seen = std::fs::read(&read_at).ok();
            let message = if error == nil {
                None
            } else {
                let text: ObjcId = msg_send![error, localizedDescription];
                Some(nsstring_to_string(text))
            };
            *got.lock().unwrap() = Some((message, seen));
        });
        let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(&dest.to_string_lossy()) isDirectory: NO];
        let delegate: ObjcId = msg_send![provider, delegate];
        let () = msg_send![delegate, filePromiseProvider: provider writePromiseToURL: url completionHandler: &block];
        let out = result.lock().unwrap().take().expect("the completion handler was called");
        out
    }

    unsafe fn write_promise(provider: ObjcId, dest: &std::path::Path) -> Option<String> {
        write_promise_read(provider, dest).0
    }

    #[test]
    fn a_promise_resolves_for_promise_and_file_url_receivers() {
        let dir = std::env::temp_dir().join(format!("hal-drag-out-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("out")).unwrap();
        let path = dir.join("Night Drive.mp3");
        let item = drag_promise::encode(&path.to_string_lossy(), "audio/mpeg");
        unsafe {
            let provider = promise_writer(&item);
            assert!(provider != nil);
            let ty: ObjcId = msg_send![provider, fileType];
            assert_eq!(nsstring_to_string(ty), "public.mp3");
            let types: ObjcId = msg_send![provider, writableTypesForPasteboard: nil];
            let has_url: BOOL = msg_send![types, containsObject: str_to_nsstring(FILE_URL)];
            assert!(has_url != NO, "offers a file URL too");
            let delegate: ObjcId = msg_send![provider, delegate];
            let name: ObjcId = msg_send![delegate, filePromiseProvider: provider fileNameForType: ty];
            assert_eq!(nsstring_to_string(name), "Night Drive.mp3");
            // the producer finishes a little after the receiver asks
            let producer_path = path.clone();
            let producer = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                let part = drag_promise::part_path(&producer_path);
                std::fs::write(&part, b"ID3 made for the drop").unwrap();
                std::fs::rename(&part, &producer_path).unwrap();
            });
            let url: ObjcId = msg_send![provider, pasteboardPropertyListForType: str_to_nsstring(FILE_URL)];
            assert!(url != nil, "the file URL resolves once the file is written");
            // exactly what a plain NSURL item of the finished file puts on the pasteboard
            let plain: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(&path.to_string_lossy()) isDirectory: NO];
            let plain: ObjcId = msg_send![plain, pasteboardPropertyListForType: str_to_nsstring(FILE_URL)];
            let same: BOOL = msg_send![url, isEqual: plain];
            assert!(same != NO, "the lazy file URL is the URL's own pasteboard form");
            let url = nsstring_to_string(url);
            assert!(url.starts_with("file://") && url.ends_with("Night%20Drive.mp3"), "{url}");
            producer.join().unwrap();
            let dest = dir.join("out").join("Night Drive.mp3");
            assert_eq!(write_promise(provider, &dest), None);
            assert_eq!(std::fs::read(&dest).unwrap(), b"ID3 made for the drop");
            let () = msg_send![provider, release];

            // a video for chat apps: typed public.mpeg-4, and the receiver reading the file the
            // moment the completion handler runs gets all of it
            let video = dir.join("Night Drive.mp4");
            let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
            let producer_path = video.clone();
            let producer_bytes = bytes.clone();
            let producer = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let part = drag_promise::part_path(&producer_path);
                std::fs::write(&part, &producer_bytes).unwrap();
                std::fs::rename(&part, &producer_path).unwrap();
            });
            let provider = promise_writer(&drag_promise::encode(&video.to_string_lossy(), "video/mp4"));
            let ty: ObjcId = msg_send![provider, fileType];
            assert_eq!(nsstring_to_string(ty), "public.mpeg-4");
            let dest = dir.join("out").join("Night Drive.mp4");
            let (error, seen) = write_promise_read(provider, &dest);
            producer.join().unwrap();
            assert_eq!(error, None);
            let seen = seen.expect("the file is there when the completion handler runs");
            assert_eq!(seen.len(), bytes.len(), "whole file at completion");
            assert!(seen == bytes);
            assert!(!drag_promise::part_path(&dest).exists(), "no part file left beside it");
            // what a receiver sees of the delivered file: its type from the name, no quarantine
            let dest_url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(&dest.to_string_lossy()) isDirectory: NO];
            let mut content_type: ObjcId = nil;
            let mut error: ObjcId = nil;
            let ok: BOOL = msg_send![dest_url, getResourceValue: &mut content_type forKey: str_to_nsstring("NSURLContentTypeKey") error: &mut error];
            assert!(ok != NO && content_type != nil);
            let content_type: ObjcId = msg_send![content_type, identifier];
            let content_type = nsstring_to_string(content_type);
            let quarantine: ObjcId = nil;
            let mut quarantine = quarantine;
            let _: BOOL = msg_send![dest_url, getResourceValue: &mut quarantine forKey: str_to_nsstring("NSURLQuarantinePropertiesKey") error: &mut error];
            eprintln!("DROP-UTTYPE: {} -> {content_type}, {} bytes, quarantine {}", dest.display(), seen.len(), if quarantine == nil { "none" } else { "SET" });
            assert_eq!(content_type, "public.mpeg-4");
            assert!(quarantine == nil, "the delivered file carries no quarantine");
            let () = msg_send![provider, release];

            // a producer that failed: the receiver gets an error, not a file
            let bad = dir.join("Broken.mp3");
            std::fs::write(drag_promise::failed_path(&bad), "the song could not be decoded").unwrap();
            let provider = promise_writer(&drag_promise::encode(&bad.to_string_lossy(), "audio/mpeg"));
            let message = write_promise(provider, &dir.join("out").join("Broken.mp3"));
            assert_eq!(message.as_deref(), Some("the song could not be decoded"));
            assert!(!dir.join("out").join("Broken.mp3").exists());
            let url: ObjcId = msg_send![provider, pasteboardPropertyListForType: str_to_nsstring(FILE_URL)];
            assert!(url == nil);
            let () = msg_send![provider, release];
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
