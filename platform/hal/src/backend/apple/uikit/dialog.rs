//! File and folder dialogs on iOS: `UIDocumentPickerViewController` presented over the root view
//! controller; its delegate answers later with a `DialogResult`. Moved from
//! os/apple/ios/ios_file_dialog.rs.
//!
//! iOS has no save panel: its exporter wants the file to exist already, the opposite of what a
//! save dialog promises (a path to write to). A save dialog is therefore the picker restricted to
//! folders, answering `<chosen folder>/<filename>`: a real, writable destination, the only one the
//! platform offers before the bytes exist. Picked files are copied into the app's sandbox
//! (`asCopy`), so they open as plain paths; a folder cannot be copied, so its security scope is
//! started and kept for the life of the process (the app gets a plain path and no "done with it"
//! callback to stop it from).

use super::app::app;
use super::classes::shielded;
use super::Shared;
use crate::types::*;
use makepad_apple_sys::*;

/// The dialog a delegate serves, boxed in its ivar (0 once answered).
struct Pending {
    sid: u32,
    req: u32,
    kind: u32,
    filename: String,
}

/// Every extension the filters name, flattened; `*` (All Files) means no restriction. `filters`
/// are (description, "ext,ext") pairs.
fn filter_extensions(filters: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for pair in filters.chunks(2) {
        let Some(list) = pair.get(1) else { continue };
        for extension in list.split(',') {
            let cleaned = extension.trim().trim_start_matches('*').trim_start_matches('.');
            if cleaned.is_empty() || extension.trim() == "*" {
                return Vec::new();
            }
            if !out.iter().any(|e: &String| e.eq_ignore_ascii_case(cleaned)) {
                out.push(cleaned.to_string());
            }
        }
    }
    out
}

fn answer(sid: u32, req: u32, paths: &[String]) {
    let mut bytes = Vec::new();
    for p in paths {
        bytes.extend_from_slice(&(p.len() as u32).to_le_bytes());
        bytes.extend_from_slice(p.as_bytes());
    }
    Shared::global().push(sid, |q| q.dialog_result(req, paths.len() as u32, &bytes));
}

/// Presents the picker (`kind` is `dialog_kind`); the answer comes when the user is done.
pub(super) fn open(sid: u32, req: u32, kind: u32, title: &str, _location: &str, filename: &str, filters: &[String]) {
    let select_file = kind & 0xff == dialog_kind::SELECT_FILE;
    let multiple = select_file && kind & dialog_kind::MULTIPLE != 0;
    unsafe {
        let Some(presenter) = presenter() else {
            eprintln!("[uikit] file dialog: no view controller to present on");
            answer(sid, req, &[]);
            return;
        };
        let Some(types) = content_types(select_file, &filter_extensions(filters)) else {
            answer(sid, req, &[]);
            return;
        };
        let picker: ObjcId = msg_send![class!(UIDocumentPickerViewController), alloc];
        let picker: ObjcId = msg_send![picker, initForOpeningContentTypes: types asCopy: if select_file { YES } else { NO }];
        if picker == nil {
            eprintln!("[uikit] file dialog: UIDocumentPickerViewController would not initialise");
            answer(sid, req, &[]);
            return;
        }
        let () = msg_send![picker, setAllowsMultipleSelection: if multiple { YES } else { NO }];
        let () = msg_send![picker, setShouldShowFileExtensions: YES];
        if !title.is_empty() {
            let () = msg_send![picker, setTitle: str_to_nsstring(title)];
        }
        let delegate: ObjcId = msg_send![delegate_class(), new];
        let pending = Box::into_raw(Box::new(Pending { sid, req, kind: kind & 0xff, filename: filename.to_string() }));
        (*delegate).set_ivar::<usize>("pending", pending as usize);
        // the picker's delegate is weak: the delegate's own +1 keeps it until `finish`
        let () = msg_send![picker, setDelegate: delegate];
        // the swipe-down dismissal is a cancel neither picker callback reports: the presentation
        // controller does, and UIKit replaces it while presenting, so it is set on both sides
        set_presentation_delegate(picker, delegate);
        let () = msg_send![presenter, presentViewController: picker animated: YES completion: nil];
        set_presentation_delegate(picker, delegate);
        let () = msg_send![picker, release];
    }
}

unsafe fn set_presentation_delegate(picker: ObjcId, delegate: ObjcId) {
    let controller: ObjcId = msg_send![picker, presentationController];
    if controller != nil {
        let () = msg_send![controller, setDelegate: delegate];
    }
}

/// The top-most controller that can present: whatever is modal over the root, or the root.
unsafe fn presenter() -> Option<ObjcId> {
    let mut controller = app().controller.get();
    if controller == nil {
        return None;
    }
    loop {
        let presented: ObjcId = msg_send![controller, presentedViewController];
        if presented == nil {
            return Some(controller);
        }
        controller = presented;
    }
}

/// The `UTType`s the picker offers: the filters' for a file, else every item (`public.item`)
/// or folders (`public.folder`).
unsafe fn content_types(select_file: bool, extensions: &[String]) -> Option<ObjcId> {
    let ut_type = makepad_objc_sys::runtime::objc_getClass(b"UTType\0".as_ptr() as *const _) as ObjcId;
    if ut_type.is_null() {
        eprintln!("[uikit] file dialog: UTType is unavailable");
        return None;
    }
    let types: ObjcId = msg_send![class!(NSMutableArray), array];
    if select_file {
        for extension in extensions {
            let content_type: ObjcId = msg_send![ut_type, typeWithFilenameExtension: str_to_nsstring(extension)];
            if content_type != nil {
                let () = msg_send![types, addObject: content_type];
            }
        }
    }
    let count: usize = msg_send![types, count];
    if count == 0 {
        let identifier = if select_file { "public.item" } else { "public.folder" };
        let fallback: ObjcId = msg_send![ut_type, typeWithIdentifier: str_to_nsstring(identifier)];
        if fallback == nil {
            return None;
        }
        let () = msg_send![types, addObject: fallback];
    }
    Some(types)
}

/// Answers the dialog the delegate serves, once (a pick is followed by the dismissal notice).
unsafe fn finish(delegate: &mut Object, urls: ObjcId) {
    let raw = *delegate.get_ivar::<usize>("pending");
    if raw == 0 {
        return;
    }
    delegate.set_ivar::<usize>("pending", 0);
    let pending = Box::from_raw(raw as *mut Pending);
    let scoped = pending.kind != dialog_kind::SELECT_FILE;
    let mut paths = Vec::new();
    if urls != nil {
        let count: usize = msg_send![urls, count];
        for index in 0..count {
            let url: ObjcId = msg_send![urls, objectAtIndex: index];
            if url == nil {
                continue;
            }
            if scoped {
                let _: BOOL = msg_send![url, startAccessingSecurityScopedResource];
            }
            let path: ObjcId = msg_send![url, path];
            if path != nil {
                let path = nsstring_to_string(path);
                if !path.is_empty() {
                    paths.push(path);
                }
            }
        }
    }
    if pending.kind == dialog_kind::SAVE_FILE {
        let name = if pending.filename.is_empty() { "untitled" } else { pending.filename.as_str() };
        paths = paths.into_iter().take(1).map(|folder| format!("{}/{}", folder.trim_end_matches('/'), name)).collect();
    } else if pending.kind != dialog_kind::SELECT_FILE {
        paths.truncate(1);
    }
    answer(pending.sid, pending.req, &paths);
    // not freed from inside its own callback cascade: one hop through the main run loop
    let delegate = delegate as *mut Object as usize;
    super::app::run_on_main(move || unsafe {
        let () = msg_send![delegate as ObjcId, release];
    });
}

fn delegate_class() -> *const Class {
    static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CLASS.get_or_init(|| {
        extern "C" fn picked(this: &mut Object, _: Sel, _: ObjcId, urls: ObjcId) {
            shielded("file-dialog", || unsafe { finish(this, urls) });
        }
        extern "C" fn cancelled(this: &mut Object, _: Sel, _: ObjcId) {
            shielded("file-dialog", || unsafe { finish(this, nil) });
        }
        let mut decl = ClassDecl::new("HalDocumentPickerDelegate", class!(NSObject)).unwrap();
        decl.add_ivar::<usize>("pending");
        unsafe {
            decl.add_method(sel!(documentPicker:didPickDocumentsAtURLs:), picked as extern "C" fn(&mut Object, Sel, ObjcId, ObjcId));
            decl.add_method(sel!(documentPickerWasCancelled:), cancelled as extern "C" fn(&mut Object, Sel, ObjcId));
            decl.add_method(sel!(presentationControllerDidDismiss:), cancelled as extern "C" fn(&mut Object, Sel, ObjcId));
        }
        decl.register() as *const Class as usize
    }) as *const Class
}
