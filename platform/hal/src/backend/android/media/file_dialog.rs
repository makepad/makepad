//! Storage Access Framework: return content:// URIs unchanged, including tree URIs.
use super::{exception, java, lock, string, void, Message, Output};
use crate::{backend::exec::OsCmd, types::dialog_kind};
use makepad_jni_sys::{registration as reg, *};
use std::{collections::{HashMap, VecDeque}, ffi::c_void, sync::{Mutex, OnceLock}};

struct Pending { output: Output, request: u32 }
fn pending() -> &'static Mutex<HashMap<i32, Pending>> {
    static PENDING: OnceLock<Mutex<HashMap<i32, Pending>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}
// This is the Activity's existing SAF request-code namespace. Never reuse a
// live code: exhaustion queues the dialog until a result frees one.
const CODES: std::ops::Range<i32> = 0x7a00..0x7b00;

pub(super) struct Dialogs {
    output: Output,
    waiting: VecDeque<OsCmd>,
    pressured: bool,
}
impl Dialogs {
    pub fn new(output: Output) -> Self { Self { output, waiting: VecDeque::new(), pressured: false } }
    pub fn open(&mut self, command: OsCmd) { self.waiting.push_back(command); self.step(); }
    pub fn active(&self) -> bool { !self.waiting.is_empty() }
    pub fn step(&mut self) {
        while let Some(command) = self.waiting.pop_front() {
            let OsCmd::FileDialog { req, kind, title: _, location, filename, filters } = &command else { continue; };
            let code = {
                let mut p = lock(pending());
                CODES.clone().find(|code| !p.contains_key(code)).inspect(|code| {
                    p.insert(*code, Pending { output: self.output.clone(), request: *req });
                })
            };
            let Some(code) = code else {
                self.waiting.push_front(command);
                if !self.pressured { eprintln!("[hal-android-dialog] SAF request codes busy; retaining picker request"); self.pressured = true; }
                break;
            };
            if !location.is_empty() { eprintln!("[hal-android-dialog] start location is not expressible as a SAF URI; ignoring it"); }
            let kind = *kind;
            let native_kind = match kind & 0xff {
                dialog_kind::SELECT_FILE => 0,
                dialog_kind::SAVE_FILE => 1,
                dialog_kind::SELECT_FOLDER | dialog_kind::SAVE_FOLDER => 2,
                _ => { finish(code, Vec::new()); continue; }
            };
            let (mime, mimes) = if native_kind == 2 { ("*/*".to_string(), Vec::new()) } else { intent_mimes(filters) };
            let result = java(|env, activity| unsafe {
                let mime = string(env, &mime)?;
                let filename = string(env, filename)?;
                let cls = ((**env).FindClass.unwrap())(env, c"java/lang/String".as_ptr());
                exception(env)?;
                let array = ((**env).NewObjectArray.unwrap())(env, mimes.len() as i32, cls, std::ptr::null_mut());
                exception(env)?;
                for (i, mime) in mimes.iter().enumerate() {
                    let value = string(env, mime)?;
                    ((**env).SetObjectArrayElement.unwrap())(env, array, i as i32, value);
                    ((**env).DeleteLocalRef.unwrap())(env, value);
                    exception(env)?;
                }
                void(env, activity, c"openFileDialog", c"(IILjava/lang/String;[Ljava/lang/String;ZLjava/lang/String;)V", &[
                    jvalue { i: code }, jvalue { i: native_kind }, jvalue { l: mime }, jvalue { l: array },
                    jvalue { z: (native_kind == 0 && kind & dialog_kind::MULTIPLE != 0) as u8 }, jvalue { l: filename },
                ])
            });
            if let Err(e) = result { eprintln!("[hal-android-dialog] {e}"); finish(code, Vec::new()); }
        }
        if self.waiting.is_empty() { self.pressured = false; }
    }
}
impl Drop for Dialogs {
    fn drop(&mut self) {
        // Keep outstanding Java request codes reserved until their replies arrive.
        // Output's alive flag discards replies to the closed HAL session.
        self.waiting.clear();
    }
}
fn finish(code: i32, paths: Vec<String>) {
    let p = lock(pending()).remove(&code);
    if let Some(p) = p { p.output.push(Message::Dialog(p.request, paths)); }
}
unsafe extern "system" fn result(env: *mut JNIEnv, _: jclass, code: jint, uris: jobjectArray) {
    let mut paths = Vec::new();
    if !uris.is_null() {
        let count = ((**env).GetArrayLength.unwrap())(env, uris);
        for i in 0..count {
            let value = ((**env).GetObjectArrayElement.unwrap())(env, uris, i);
            if !value.is_null() { paths.push(reg::string(env, value)); ((**env).DeleteLocalRef.unwrap())(env, value); }
        }
    }
    if let Err(e) = exception(env) { eprintln!("[hal-android-dialog] {e}"); paths.clear(); }
    finish(code, paths);
}
pub(super) fn native_methods() -> Vec<(&'static str, &'static str, *mut c_void)> {
    vec![("onFileDialogResult", "(I[Ljava/lang/String;)V", result as *mut c_void)]
}

fn intent_mimes(filters: &[String]) -> (String, Vec<String>) {
    let mut mimes = Vec::new();
    // HAL filters are (description, comma-separated extensions) pairs.
    for filter in filters.chunks_exact(2) {
        for ext in filter[1].split(',') {
            let ext = ext.trim().trim_start_matches('*').trim_start_matches('.');
            if let Some(mime) = mime_for_extension(ext) {
                if !mimes.contains(&mime) { mimes.push(mime); }
            }
        }
    }
    if mimes.is_empty() { return ("*/*".into(), Vec::new()); }
    let top = mimes[0].split('/').next().unwrap_or("*");
    if mimes.iter().all(|m| m.split('/').next() == Some(top)) { (format!("{top}/*"), Vec::new()) }
    else { ("*/*".into(), mimes.into_iter().map(str::to_string).collect()) }
}
// Same extension table as the old Android SAF picker.
fn mime_for_extension(extension: &str) -> Option<&'static str> {
    Some(match extension.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4", "mkv" => "video/x-matroska", "webm" => "video/webm",
        "avi" => "video/x-msvideo", "mov" | "qt" => "video/quicktime", "flv" => "video/x-flv",
        "ts" | "m2ts" | "mts" | "m3u8" => "video/mp2t", "wmv" => "video/x-ms-wmv",
        "mpg" | "mpeg" | "mpe" => "video/mpeg", "3gp" => "video/3gpp", "ogv" => "video/ogg",
        "mp3" => "audio/mpeg", "wav" => "audio/wav", "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg", "opus" => "audio/opus", "aac" => "audio/aac",
        "m4a" => "audio/mp4", "mid" | "midi" => "audio/midi", "aiff" | "aif" => "audio/aiff",
        "png" => "image/png", "jpg" | "jpeg" | "jpe" => "image/jpeg", "gif" => "image/gif",
        "webp" => "image/webp", "bmp" => "image/bmp", "heic" | "heif" => "image/heic",
        "svg" => "image/svg+xml", "tif" | "tiff" => "image/tiff", "ico" => "image/x-icon",
        "pdf" => "application/pdf", "zip" => "application/zip", "gz" => "application/gzip",
        "json" => "application/json", "xml" => "text/xml", "html" | "htm" => "text/html",
        "csv" => "text/csv", "md" | "markdown" => "text/markdown",
        "txt" | "log" | "rs" | "toml" | "yaml" | "yml" | "ini" | "cfg" => "text/plain",
        _ => return None,
    })
}
