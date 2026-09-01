//! Native file and folder dialogs on Android.
//!
//! Android has no file dialog. It has the Storage Access Framework: a
//! document-provider Activity started with `startActivityForResult` and
//! answered later on the activity's own result callback. That shape is
//! already the one [`FileDialogAction`] describes — the platform-op drain
//! fires an Intent and returns, and the answer arrives from Java long
//! after, as an action.
//!
//! ## A picked document is a `content://` URI, not a path
//!
//! SAF hands back a `content://` URI. There is no filesystem path to give:
//! the document may live in a cloud provider, on a removable volume, or
//! behind a provider that never materialises a file at all. So the URI
//! travels *as text* inside the [`PathBuf`] the action carries. That is
//! what the third-party picker this replaces did (`PickedFile::uri`), and
//! what the consumers in this tree already expect —
//! [`VideoSource::needs_native_player`](crate::event::video_playback::VideoSource)
//! and `examples/video_player`'s `parse_media_ref` both test for a
//! `content://` prefix and route to the native player. Android's own APIs
//! (`ContentResolver`, `MediaPlayer.setDataSource`, `MediaMetadataRetriever`)
//! take the URI directly. `std::fs` will not open it, and that is inherent
//! to the platform rather than a shortcut taken here.

use {
    crate::{
        cx::Cx,
        file_dialogs::{FileDialog, FileDialogAction},
        makepad_live_id::LiveId,
        os::linux::android::android_jni,
    },
    std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicI32, Ordering},
            Mutex,
        },
    },
};

/// Which SAF Intent to fire. The discriminants cross into Java as the
/// `kind` argument of `MakepadActivity.openFileDialog`.
#[derive(Clone, Copy, PartialEq)]
enum DialogKind {
    /// `ACTION_OPEN_DOCUMENT`
    OpenFile = 0,
    /// `ACTION_CREATE_DOCUMENT`
    CreateFile = 1,
    /// `ACTION_OPEN_DOCUMENT_TREE`
    OpenTree = 2,
}

struct Pending {
    request_code: i32,
    id: LiveId,
    kind: DialogKind,
}

/// Dialogs we have fired and not yet been answered about. A `Vec` rather
/// than a map because it holds one entry, occasionally two.
static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// `startActivityForResult` request codes are a namespace the whole
/// activity shares, so we take a small dedicated window of it and hand the
/// codes out round-robin. Wrapping would need 256 dialogs open at once.
/// The range stays inside 16 bits, which is all Android promises to keep
/// when the result is routed through a fragment.
const REQUEST_CODE_BASE: i32 = 0x7A00;
const REQUEST_CODE_COUNT: i32 = 0x100;
static NEXT_REQUEST_CODE: AtomicI32 = AtomicI32::new(0);

pub fn open_select_file_dialog(settings: FileDialog) {
    start(settings, DialogKind::OpenFile);
}

pub fn open_save_file_dialog(settings: FileDialog) {
    start(settings, DialogKind::CreateFile);
}

/// SAF's folder picker is `ACTION_OPEN_DOCUMENT_TREE`; it answers with a
/// *tree* URI, which is the only kind of directory handle Android grants.
pub fn open_select_folder_dialog(settings: FileDialog) {
    start(settings, DialogKind::OpenTree);
}

/// Same picker as [`open_select_folder_dialog`]: "save into this folder"
/// and "choose this folder" are one Intent on Android, and the tree URI it
/// returns is writable when the grant asked for write.
pub fn open_save_folder_dialog(settings: FileDialog) {
    start(settings, DialogKind::OpenTree);
}

fn start(settings: FileDialog, kind: DialogKind) {
    let request_code = REQUEST_CODE_BASE
        + NEXT_REQUEST_CODE.fetch_add(1, Ordering::Relaxed).rem_euclid(REQUEST_CODE_COUNT);

    // A tree picker browses providers, not documents: it takes no MIME type
    // and honours none, so asking for one only confuses the picker.
    let (mime_type, mime_types) = if kind == DialogKind::OpenTree {
        ("*/*".to_string(), Vec::new())
    } else {
        intent_mime_types(&settings)
    };

    // `EXTRA_ALLOW_MULTIPLE` is meaningless on a create/tree Intent, and
    // some providers behave badly when it is set anyway.
    let allow_multiple = settings.multiple && kind == DialogKind::OpenFile;
    let file_name = settings.filename.clone().unwrap_or_default();

    if settings.location.is_some() {
        // `EXTRA_INITIAL_URI` wants a document-tree URI from a previous
        // pick, not a filesystem path, so a `location` set by portable app
        // code has nothing to translate into here.
        crate::log!("Android file dialog: start location is not expressible as a SAF URI, ignoring it");
    }

    if let Ok(mut pending) = PENDING.lock() {
        pending.push(Pending {
            request_code,
            id: settings.id,
            kind,
        });
    }

    unsafe {
        android_jni::to_java_open_file_dialog(
            request_code,
            kind as i32,
            &mime_type,
            &mime_types,
            allow_multiple,
            &file_name,
        );
    }
}

/// Answer a dialog. Called from the JNI result callback, which Android runs
/// on the UI thread — not the render thread — so this goes out through
/// [`Cx::post_action`], which is the platform-wide contract for a dialog
/// answer and is already safe to call from any thread.
pub(crate) fn on_result(request_code: i32, uris: Vec<String>) {
    let Some(pending) = take_pending(request_code) else {
        // A result for a request code we no longer own. The activity was
        // recreated while the picker was up (rotation, process death) and
        // the pending table went with the old process; there is no dialog
        // left to answer.
        return;
    };
    let id = pending.id;
    let mut paths = uris.into_iter().map(PathBuf::from);
    match pending.kind {
        DialogKind::OpenFile => {
            let paths: Vec<PathBuf> = paths.collect();
            Cx::post_action(if paths.is_empty() {
                FileDialogAction::FileCancelled { id }
            } else {
                FileDialogAction::FileSelected { id, paths }
            });
        }
        DialogKind::CreateFile => {
            Cx::post_action(match paths.next() {
                Some(path) => FileDialogAction::SaveFileSelected { id, path },
                None => FileDialogAction::SaveFileCancelled { id },
            });
        }
        DialogKind::OpenTree => {
            Cx::post_action(match paths.next() {
                Some(path) => FileDialogAction::FolderSelected(path),
                None => FileDialogAction::FolderCancelled,
            });
        }
    }
}

fn take_pending(request_code: i32) -> Option<Pending> {
    let mut pending = PENDING.lock().ok()?;
    let index = pending.iter().position(|p| p.request_code == request_code)?;
    Some(pending.remove(index))
}

/// The Intent's `type` and its `EXTRA_MIME_TYPES` list, derived from the
/// dialog's filters.
///
/// Android's picker has no filter dropdown: `EXTRA_MIME_TYPES` is one flat
/// allow-list. So the desktop convention of adding an "All Files" `*` row
/// *beside* a real filter cannot be honoured the way macOS honours it
/// (there, selecting `*` drops every restriction). Here a lone `*` means
/// `*/*`, but a `*` sitting next to concrete extensions is ignored:
/// widening to `*/*` would throw away the filter the app asked for, and
/// SAF already lets the user reach anything through its Browse pane.
///
/// When every mapped type shares a top level the Intent asks for `video/*`
/// (or `image/*`, …) instead of listing a dozen siblings — that is the
/// query document providers are tuned for, and it is exactly what the
/// picker this replaces sent.
fn intent_mime_types(settings: &FileDialog) -> (String, Vec<String>) {
    let mut mimes: Vec<&'static str> = Vec::new();
    for filter in &settings.filters {
        for extension in &filter.extensions {
            let cleaned = extension
                .trim()
                .trim_start_matches('*')
                .trim_start_matches('.');
            if cleaned.is_empty() {
                continue;
            }
            if let Some(mime) = mime_for_extension(cleaned) {
                if !mimes.contains(&mime) {
                    mimes.push(mime);
                }
            }
        }
    }

    if mimes.is_empty() {
        return ("*/*".to_string(), Vec::new());
    }

    let top_level = mimes[0].split('/').next().unwrap_or("*");
    if mimes
        .iter()
        .all(|m| m.split('/').next() == Some(top_level))
    {
        return (format!("{top_level}/*"), Vec::new());
    }

    (
        "*/*".to_string(),
        mimes.into_iter().map(str::to_string).collect(),
    )
}

/// The MIME type Android knows a filename extension by.
///
/// An extension this does not know maps to nothing rather than to `*/*`:
/// one unrecognised entry in a filter must not quietly widen the whole
/// dialog. If a filter names *only* unknown extensions the caller ends up
/// at `*/*`, which is the honest answer — we cannot describe what it wants.
fn mime_for_extension(extension: &str) -> Option<&'static str> {
    let extension = extension.to_ascii_lowercase();
    Some(match extension.as_str() {
        // video
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" | "qt" => "video/quicktime",
        "flv" => "video/x-flv",
        "ts" | "m2ts" | "mts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        "mpg" | "mpeg" | "mpe" => "video/mpeg",
        "3gp" => "video/3gpp",
        "m3u8" => "video/mp2t",
        "ogv" => "video/ogg",
        // audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",
        "mid" | "midi" => "audio/midi",
        "aiff" | "aif" => "audio/aiff",
        // image
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "heic" | "heif" => "image/heic",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        // documents and text
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "json" => "application/json",
        "xml" => "text/xml",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "md" | "markdown" => "text/markdown",
        "txt" | "log" | "rs" | "toml" | "yaml" | "yml" | "ini" | "cfg" => "text/plain",
        _ => return None,
    })
}
