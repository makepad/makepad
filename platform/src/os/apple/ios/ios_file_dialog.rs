//! Native file and folder dialogs on iOS.
//!
//! `UIDocumentPickerViewController`, presented on the root view controller,
//! answering through a delegate — which is the same shape
//! [`FileDialogAction`] already describes: the platform-op drain presents
//! and returns, and the answer arrives later, from UIKit.
//!
//! ## What "save" means here
//!
//! iOS has no save panel. Its exporter (`initForExportingURLs:`) wants the
//! file to already exist, which is the opposite of what
//! `Cx::open_save_file_dialog` promises: a path to write to — the bytes do
//! not exist yet. So a save dialog is the document picker
//! restricted to folders, and the action carries `<chosen folder>/<filename>`.
//! That is a real, writable destination — and the only one the platform
//! offers before the bytes exist.

use {
    crate::{
        cx::Cx,
        file_dialogs::{FileDialog, FileDialogAction},
        makepad_live_id::LiveId,
        os::{
            apple::apple_sys::*,
            apple::apple_util::{nsstring_to_string, str_to_nsstring},
            apple::ios::ios_app::get_ios_class_global,
            apple::ios::ios_delegates::try_with_ios_app,
        },
    },
    makepad_objc_sys::objc_block,
    std::path::PathBuf,
};

/// Holds the boxed [`Pending`] for the dialog a delegate instance is
/// serving, as a raw pointer. Zero once the dialog has been answered.
const PENDING_IVAR: &str = "makepad_pending_dialog";

#[derive(Clone, Copy, PartialEq)]
enum DialogKind {
    OpenFile,
    SaveFile,
    SelectFolder,
}

struct Pending {
    id: LiveId,
    kind: DialogKind,
    /// For [`DialogKind::SaveFile`]: the name to give the file inside the
    /// folder the user picks.
    filename: String,
}

pub fn open_select_file_dialog(settings: FileDialog) {
    present(settings, DialogKind::OpenFile);
}

pub fn open_save_file_dialog(settings: FileDialog) {
    present(settings, DialogKind::SaveFile);
}

pub fn open_select_folder_dialog(settings: FileDialog) {
    present(settings, DialogKind::SelectFolder);
}

/// The folder picker again: on iOS "choose a folder" and "save into this
/// folder" are the same controller, and the app decides what to put there.
pub fn open_save_folder_dialog(settings: FileDialog) {
    present(settings, DialogKind::SelectFolder);
}

/// Deferred onto the main queue for the same reason macOS defers its
/// panels: this runs inside the platform-op drain, which holds the `Cx`
/// borrow, and presenting a controller from there runs UIKit layout (and
/// on a re-entrant path, our own event callbacks) under that borrow.
fn present(settings: FileDialog, kind: DialogKind) {
    let id = settings.id;
    let title = settings.title.clone().unwrap_or_default();
    let filename = settings.filename.clone().unwrap_or_default();
    let multiple = settings.multiple && kind == DialogKind::OpenFile;
    let extensions = filter_extensions(&settings);
    unsafe {
        let main_thread_block = objc_block!(move || {
            show_picker(id, kind, &title, &filename, &extensions, multiple);
        });
        let main_queue: ObjcId = msg_send![class!(NSOperationQueue), mainQueue];
        let block_operation: ObjcId =
            msg_send![class!(NSBlockOperation), blockOperationWithBlock: &main_thread_block];
        let () = msg_send![main_queue, addOperation: block_operation];
    }
}

unsafe fn show_picker(
    id: LiveId,
    kind: DialogKind,
    title: &str,
    filename: &str,
    extensions: &[String],
    multiple: bool,
) {
    let Some(presenter) = presenting_view_controller() else {
        crate::error!("iOS file dialog: no view controller to present on");
        post_cancelled(id, kind);
        return;
    };
    let Some(content_types) = content_types(kind, extensions) else {
        post_cancelled(id, kind);
        return;
    };

    let picker: ObjcId = msg_send![class!(UIDocumentPickerViewController), alloc];
    // `asCopy` for files: the picked document is copied into the app's own
    // temporary directory, which turns it into a plain sandbox path that
    // `std::fs` opens with no ceremony. A folder cannot be copied, so the
    // folder picker takes the security-scoped original instead (see
    // `urls_to_paths`).
    let as_copy = if kind == DialogKind::OpenFile { YES } else { NO };
    let picker: ObjcId =
        msg_send![picker, initForOpeningContentTypes: content_types asCopy: as_copy];
    if picker == nil {
        crate::error!("iOS file dialog: UIDocumentPickerViewController would not initialise");
        post_cancelled(id, kind);
        return;
    }

    let () = msg_send![picker, setAllowsMultipleSelection: if multiple { YES } else { NO }];
    let () = msg_send![picker, setShouldShowFileExtensions: YES];
    if !title.is_empty() {
        let () = msg_send![picker, setTitle: str_to_nsstring(title)];
    }

    let delegate: ObjcId = msg_send![get_ios_class_global().document_picker_delegate, alloc];
    let delegate: ObjcId = msg_send![delegate, init];
    let pending = Box::into_raw(Box::new(Pending {
        id,
        kind,
        filename: filename.to_string(),
    }));
    (*delegate).set_ivar::<i64>(PENDING_IVAR, pending as i64);
    // `UIDocumentPickerViewController.delegate` is a *weak* reference: the
    // +1 from `alloc`/`init` above is the only thing keeping this delegate
    // alive while the picker is up. `finish` gives it back.
    let () = msg_send![picker, setDelegate: delegate];

    // The presentation controller reports the swipe-down dismissal, which
    // is a cancel that neither delegate method above ever hears about. It
    // only exists once the controller has a presentation, and UIKit
    // replaces it during `presentViewController:`, so set it on both sides
    // of the call.
    set_presentation_delegate(picker, delegate);
    let () = msg_send![presenter, presentViewController: picker animated: YES completion: nil];
    set_presentation_delegate(picker, delegate);

    // The presenter took its own reference the moment it became the
    // `presentedViewController`, so the +1 from `alloc` above is ours to give
    // back — otherwise every dialog leaks a whole view controller.
    let () = msg_send![picker, release];
}

unsafe fn set_presentation_delegate(picker: ObjcId, delegate: ObjcId) {
    let presentation_controller: ObjcId = msg_send![picker, presentationController];
    if presentation_controller != nil {
        let () = msg_send![presentation_controller, setDelegate: delegate];
    }
}

/// The top-most controller that can present: whatever is already modal over
/// the root, or the root itself.
unsafe fn presenting_view_controller() -> Option<ObjcId> {
    // `try_with_ios_app` rather than `with_ios_app`: this runs from a main
    // queue block that can land while a UIKit callback still holds the
    // app's borrow, and a failed borrow must not panic a picker open.
    let mut controller = try_with_ios_app(|app| app.view_controller)
        .flatten()
        .or_else(|| {
            let ui_application: ObjcId = msg_send![class!(UIApplication), sharedApplication];
            if ui_application == nil {
                return None;
            }
            let window: ObjcId = msg_send![ui_application, keyWindow];
            if window == nil {
                return None;
            }
            let root: ObjcId = msg_send![window, rootViewController];
            (root != nil).then_some(root)
        })?;
    loop {
        let presented: ObjcId = msg_send![controller, presentedViewController];
        if presented == nil {
            return Some(controller);
        }
        controller = presented;
    }
}

/// The `UTType`s the picker should offer, or `None` when the
/// UniformTypeIdentifiers runtime is not there at all (pre-iOS-14, which
/// this tree does not ship to — but a null class deref is not the way to
/// find that out).
unsafe fn content_types(kind: DialogKind, extensions: &[String]) -> Option<ObjcId> {
    let ut_type =
        makepad_objc_sys::runtime::objc_getClass(b"UTType\0".as_ptr() as *const _) as ObjcId;
    if ut_type.is_null() {
        crate::error!("iOS file dialog: UTType is unavailable, cannot build a document picker");
        return None;
    }
    let types: ObjcId = msg_send![class!(NSMutableArray), array];

    if kind == DialogKind::OpenFile {
        for extension in extensions {
            let content_type: ObjcId =
                msg_send![ut_type, typeWithFilenameExtension: str_to_nsstring(extension)];
            if content_type != nil {
                let () = msg_send![types, addObject: content_type];
            }
        }
    }

    let count: usize = msg_send![types, count];
    if count == 0 {
        // No filters, an "All Files" filter, or nothing the system
        // recognises: `public.folder` for the folder-shaped dialogs,
        // `public.item` — every file — for an unrestricted open.
        let identifier = if kind == DialogKind::OpenFile {
            "public.item"
        } else {
            "public.folder"
        };
        let fallback: ObjcId = msg_send![ut_type, typeWithIdentifier: str_to_nsstring(identifier)];
        if fallback == nil {
            crate::error!("iOS file dialog: no usable content type for {identifier}");
            return None;
        }
        let () = msg_send![types, addObject: fallback];
    }
    Some(types)
}

/// Every extension the dialog's filters name, flattened. A filter of `*`
/// (the conventional "All Files" row) means "no restriction at all" and
/// wins over every other filter, exactly as it does on the desktops.
fn filter_extensions(settings: &FileDialog) -> Vec<String> {
    let mut out = Vec::new();
    for filter in &settings.filters {
        for extension in &filter.extensions {
            let cleaned = extension
                .trim()
                .trim_start_matches('*')
                .trim_start_matches('.');
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

pub fn define_document_picker_delegate() -> *const Class {
    let superclass = class!(NSObject);
    let mut decl = ClassDecl::new("MakepadDocumentPickerDelegate", superclass).unwrap();
    decl.add_ivar::<i64>(PENDING_IVAR);

    extern "C" fn document_picker_did_pick_documents(
        this: &mut Object,
        _: Sel,
        _: ObjcId,
        urls: ObjcId,
    ) {
        unsafe { finish(this, urls) }
    }

    extern "C" fn document_picker_was_cancelled(this: &mut Object, _: Sel, _: ObjcId) {
        unsafe { finish(this, nil) }
    }

    extern "C" fn presentation_controller_did_dismiss(this: &mut Object, _: Sel, _: ObjcId) {
        unsafe { finish(this, nil) }
    }

    unsafe {
        decl.add_method(
            sel!(documentPicker: didPickDocumentsAtURLs:),
            document_picker_did_pick_documents
                as extern "C" fn(&mut Object, Sel, ObjcId, ObjcId),
        );
        decl.add_method(
            sel!(documentPickerWasCancelled:),
            document_picker_was_cancelled as extern "C" fn(&mut Object, Sel, ObjcId),
        );
        decl.add_method(
            sel!(presentationControllerDidDismiss:),
            presentation_controller_did_dismiss as extern "C" fn(&mut Object, Sel, ObjcId),
        );
    }

    decl.register()
}

/// Answer the dialog this delegate is serving.
///
/// Several UIKit callbacks can fire for one dialog — a pick is followed by
/// the presentation-dismissed notification — so the pending box is taken
/// exactly once and every later call falls straight back out.
unsafe fn finish(this: &mut Object, urls: ObjcId) {
    let Some(pending) = take_pending(this) else {
        return;
    };
    let security_scoped = pending.kind != DialogKind::OpenFile;
    let mut paths = urls_to_paths(urls, security_scoped).into_iter();
    let id = pending.id;
    match pending.kind {
        DialogKind::OpenFile => {
            let paths: Vec<PathBuf> = paths.collect();
            Cx::post_action(if paths.is_empty() {
                FileDialogAction::FileCancelled { id }
            } else {
                FileDialogAction::FileSelected { id, paths }
            });
        }
        DialogKind::SaveFile => {
            let filename = if pending.filename.is_empty() {
                "untitled"
            } else {
                pending.filename.as_str()
            };
            Cx::post_action(match paths.next() {
                Some(folder) => FileDialogAction::SaveFileSelected {
                    id,
                    path: folder.join(filename),
                },
                None => FileDialogAction::SaveFileCancelled { id },
            });
        }
        DialogKind::SelectFolder => {
            Cx::post_action(match paths.next() {
                Some(folder) => FileDialogAction::FolderSelected(folder),
                None => FileDialogAction::FolderCancelled,
            });
        }
    }
    release_later(this as *mut Object as ObjcId);
}

unsafe fn take_pending(this: &mut Object) -> Option<Box<Pending>> {
    let raw = *this.get_ivar::<i64>(PENDING_IVAR);
    if raw == 0 {
        return None;
    }
    this.set_ivar::<i64>(PENDING_IVAR, 0);
    Some(Box::from_raw(raw as *mut Pending))
}

/// Give back the delegate's retain — but not from inside one of its own
/// methods. UIKit sends this object more than one message per dismissal,
/// and a delegate freed mid-cascade is a use-after-free. One hop through
/// the main queue puts the release after the whole cascade; the later
/// callbacks find a null `pending` and do nothing.
unsafe fn release_later(delegate: ObjcId) {
    let delegate = delegate as usize;
    let block = objc_block!(move || {
        let delegate = delegate as ObjcId;
        let () = msg_send![delegate, release];
    });
    let main_queue: ObjcId = msg_send![class!(NSOperationQueue), mainQueue];
    let block_operation: ObjcId =
        msg_send![class!(NSBlockOperation), blockOperationWithBlock: &block];
    let () = msg_send![main_queue, addOperation: block_operation];
}

/// The picked URLs as paths.
///
/// A folder URL points outside the app sandbox and only opens while its
/// security scope is held, so the scope is started here and never stopped:
/// the action hands the app a plain path with no "finished with it"
/// callback to hang the matching stop off, and the grant costs one handle
/// for the life of the process. File URLs come back as copies inside the
/// sandbox and need none of this.
unsafe fn urls_to_paths(urls: ObjcId, security_scoped: bool) -> Vec<PathBuf> {
    if urls == nil {
        return Vec::new();
    }
    let count: usize = msg_send![urls, count];
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let url: ObjcId = msg_send![urls, objectAtIndex: index];
        if url == nil {
            continue;
        }
        if security_scoped {
            let _: BOOL = msg_send![url, startAccessingSecurityScopedResource];
        }
        let path: ObjcId = msg_send![url, path];
        if path == nil {
            continue;
        }
        let path = nsstring_to_string(path);
        if !path.is_empty() {
            out.push(PathBuf::from(path));
        }
    }
    out
}

fn post_cancelled(id: LiveId, kind: DialogKind) {
    Cx::post_action(match kind {
        DialogKind::OpenFile => FileDialogAction::FileCancelled { id },
        DialogKind::SaveFile => FileDialogAction::SaveFileCancelled { id },
        DialogKind::SelectFolder => FileDialogAction::FolderCancelled,
    });
}
