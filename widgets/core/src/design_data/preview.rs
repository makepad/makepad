//! In-memory structural previews for registered source files.
use super::{arg, json_str, locate, need, Args};
use crate::makepad_draw::*;
use crate::makepad_micro_serde::{DeJson, JsonValue, SerJson};
use crate::makepad_platform::makepad_script::syntax::edit::Replace;

#[derive(Clone, Debug)]
pub enum PreviewPayload {
    Edits { base_hash: u64, edits: Vec<Replace> },
    FullText(String),
    Revert,
}

#[derive(Clone, Debug)]
pub struct PreviewOutcome {
    pub file: String,
    pub file_hash: u64,
    pub applied: bool,
    pub pending_reload: bool,
    pub errors: Vec<String>,
}

fn disk(file: &str) -> Result<String, String> {
    std::fs::read_to_string(file).map_err(|err| format!("{file}: {err}"))
}

fn splice(text: &str, mut edits: Vec<Replace>) -> Result<String, String> {
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for edit in edits {
        let gap = text.get(cursor..edit.range.start).ok_or("overlapping or invalid edit range")?;
        text.get(edit.range.clone()).ok_or("invalid edit range")?;
        out.push_str(gap);
        out.push_str(&edit.text);
        cursor = edit.range.end;
    }
    out.push_str(&text[cursor..]);
    Ok(out)
}

// A single live edit re-evaluates all the modules in its batch. Its error
// sink covers the whole batch, so restore every participating file together.
fn settle(cx: &mut Cx, ready: Vec<String>, errors: &[String]) -> bool {
    let rollback = !errors.is_empty() && !ready.is_empty();
    for file in ready {
        cx.finish_preview_change(&file, rollback);
    }
    if rollback { cx.redraw_all(); }
    rollback
}

pub fn apply_preview(cx: &mut Cx, file: &str, payload: PreviewPayload) -> Result<PreviewOutcome, String> {
    let file = cx.preview_source_file(file)?;
    let next = match payload {
        PreviewPayload::FullText(text) => text,
        PreviewPayload::Edits { base_hash, edits } => {
            let text = match cx.active_preview_text(&file) {
                Some(text) => text,
                None => disk(&file)?,
            };
            if locate::source_hash(&text) != base_hash { return Err("hash_mismatch".into()); }
            splice(&text, edits)?
        }
        PreviewPayload::Revert => {
            let file_hash = locate::source_hash(&disk(&file)?);
            let active = cx.active_preview_text(&file).is_some();
            let pending_reload = active && cx.revert_live_edit_file(&file) > 0;
            cx.redraw_all();
            return Ok(PreviewOutcome { file, file_hash, applied: active, pending_reload, errors: vec![] });
        }
    };

    cx.validate_live_edit_text(&file, &next)?;
    let ready = cx.previews_to_verify();
    if !ready.is_empty() {
        let errors = cx.take_live_edit_errors();
        if settle(cx, ready, &errors) {
            return Err(format!("previous preview rolled back: {}", errors.join("\n")));
        }
    }
    let file_hash = locate::source_hash(&next);
    cx.begin_preview_change(&file);
    cx.take_live_edit_errors();
    let result = cx.design_preview_callback.and_then(|hook| hook(cx, &file, &next));
    let pending_reload = match result {
        Some(result) => {
            let mut errors = cx.take_live_edit_errors();
            if let Err(err) = result { errors.push(err); }
            if !errors.is_empty() {
                cx.finish_preview_change(&file, true);
                cx.redraw_all();
                return Err(errors.join("\n"));
            }
            // A preceding queued preview must not overwrite this one on the next frame.
            cx.clear_preview_text(&file);
            false
        }
        None => {
            cx.queue_preview_file_change(&file, next.clone());
            true
        }
    };
    cx.install_preview_text(&file, next);
    cx.redraw_all();
    Ok(PreviewOutcome { file, file_hash, applied: true, pending_reload, errors: vec![] })
}

pub fn verify_preview(cx: &mut Cx, file: Option<&str>, path: Option<&str>) -> Result<String, String> {
    verify_in_window(cx, file, path, None)
}

pub(super) fn verify_in_window(cx: &mut Cx, file: Option<&str>, path: Option<&str>, window: Option<usize>)
    -> Result<String, String>
{
    let file = file.map(|f| cx.preview_source_file(f)).transpose()?;
    if cx.preview_change_queued(file.as_deref()) {
        return Ok(r#"{"ok":false,"pending_reload":true,"errors":[]}"#.into());
    }
    let ready = cx.previews_to_verify();
    let errors = cx.take_live_edit_errors();
    let rolled_back = settle(cx, ready, &errors);
    if !errors.is_empty() {
        return Ok(format!(r#"{{"ok":false,"rolled_back":{rolled_back},"errors":{}}}"#, errors.serialize_json()));
    }
    if let Some(path) = path {
        let widget = super::pick::resolve_target(cx, path, window)?;
        let locate = |cx: &mut Cx| {
            let w = locate::locate_widget_ast(cx, &widget, None, locate::LocateMode::Strict)?;
            if file.as_ref().is_some_and(|f| *f != w.file) {
                return Err("widget belongs to a different file".into());
            }
            Ok(format!(r#"{{"ok":true,"file":{},"file_hash":"{:016x}","open":{},"errors":[]}}"#,
                json_str(&w.file), w.file_hash, w.open_byte))
        };
        return super::with_widget_vm(cx, &widget, locate);
    }
    Ok(r#"{"ok":true,"errors":[]}"#.into())
}

fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    fn digit(c: u8) -> Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("invalid base64 character".into()),
        }
    }
    if data.len() % 4 != 0 { return Err("invalid base64 length".into()); }
    let mut out = Vec::with_capacity(data.len() / 4 * 3);
    let mut groups = data.as_bytes().chunks_exact(4).peekable();
    while let Some(g) = groups.next() {
        let (a, b) = (digit(g[0])?, digit(g[1])?);
        out.push((a << 2) | (b >> 4));
        if g[2] == b'=' {
            if g[3] != b'=' || b & 15 != 0 || groups.peek().is_some() { return Err("invalid base64 padding".into()); }
            break;
        }
        let c = digit(g[2])?;
        out.push((b << 4) | (c >> 2));
        if g[3] == b'=' {
            if c & 3 != 0 || groups.peek().is_some() { return Err("invalid base64 padding".into()); }
            break;
        }
        out.push((c << 6) | digit(g[3])?);
    }
    Ok(out)
}

fn decode_edits(value: &str) -> Result<Vec<Replace>, String> {
    let bytes = decode_base64(value)?;
    let json = JsonValue::deserialize_json_strict(std::str::from_utf8(&bytes).map_err(|_| "edits must be UTF-8")?)
        .map_err(|err| err.to_string())?;
    let offset = |v: &JsonValue| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or("invalid edit offset");
    json.as_array().ok_or("expected array of edits")?.iter().map(|item| {
        let [start, end, text] = item.as_array().ok_or("expected edit triple")?.as_slice() else {
            return Err("expected [start,end,text]".into());
        };
        Ok(Replace::new(offset(start)?..offset(end)?, text.as_str().ok_or("expected replacement text")?))
    }).collect()
}

struct Upload {
    file: String,
    id: String,
    next_seq: usize,
    bytes: Vec<u8>,
}

pub(super) fn chunk(cx: &mut Cx, args: &Args) -> Result<String, String> {
    let file = cx.preview_source_file(need(args, "file")?)?;
    let id = need(args, "id")?;
    let seq: usize = need(args, "seq")?.parse().map_err(|_| "invalid seq")?;
    let bytes = decode_base64(need(args, "data")?)?;
    let slot = cx.global::<Option<Upload>>();
    if slot.as_ref().is_none_or(|s| s.id != id) {
        if seq != 0 { return Err("expected seq=0".into()); }
        *slot = Some(Upload { file: file.clone(), id: id.into(), next_seq: 0, bytes: Vec::new() });
    }
    let upload = slot.as_mut().unwrap();
    if upload.file != file || upload.next_seq != seq { return Err("wrong upload file or sequence".into()); }
    upload.bytes.extend(bytes);
    upload.next_seq += 1;
    Ok(format!(r#"{{"ok":true,"next_seq":{}}}"#, upload.next_seq))
}

pub(super) fn remote(cx: &mut Cx, op: &str, args: &Args) -> Result<String, String> {
    let file = cx.preview_source_file(need(args, "file")?)?;
    let payload = if op == "design_revert" {
        PreviewPayload::Revert
    } else {
        match (arg(args, "text"), arg(args, "base_hash"), arg(args, "edits"), arg(args, "upload")) {
            (Some(text), None, None, None) => PreviewPayload::FullText(text.into()),
            (None, Some(hash), Some(edits), None) => PreviewPayload::Edits {
                base_hash: u64::from_str_radix(hash, 16).map_err(|_| "invalid base_hash")?,
                edits: decode_edits(edits)?,
            },
            (None, None, None, Some(id)) => {
                let slot = cx.global::<Option<Upload>>();
                if !slot.as_ref().is_some_and(|s| s.id == id && s.file == file) { return Err("unknown upload".into()); }
                PreviewPayload::FullText(String::from_utf8(slot.take().unwrap().bytes).map_err(|_| "upload must be UTF-8")?)
            }
            _ => return Err("supply text, base_hash with edits, or upload".into()),
        }
    };
    let result = apply_preview(cx, &file, payload)?;
    Ok(format!(r#"{{"ok":true,"file":{},"file_hash":"{:016x}","applied":{},"pending_reload":{},"errors":{}}}"#,
        json_str(&result.file), result.file_hash, result.applied, result.pending_reload, result.errors.serialize_json()))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod fixture {
        use crate::makepad_draw::*;
        script_mod! {
            let seed = #(1.0)
            seed + 9
        }
    }

    fn install(cx: &mut Cx, file: &str, text: &str) -> Option<Result<(), String>> {
        Some(cx.install_live_edit_text(file, text).map(|_| ()))
    }

    fn fixture() -> (Cx, String, String) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| fixture::script_mod(vm));
        let file = cx.preview_source_file(file!()).expect("registered fixture has a source file");
        let text = disk(&file).unwrap();
        cx.design_preview_callback = Some(install);
        (cx, file, text)
    }

    #[test]
    fn preview_edits_rejects_stale_base_hash() {
        let (mut cx, file, disk) = fixture();
        let old = disk.replacen("seed + 9", "seed + 8", 1);
        apply_preview(&mut cx, &file, PreviewPayload::FullText(old.clone())).unwrap();
        let overrides = cx.script_data.live_reload.script_mod_overrides.snapshot();
        let err = apply_preview(&mut cx, &file, PreviewPayload::Edits {
            base_hash: locate::source_hash(&disk), edits: vec![],
        }).unwrap_err();
        assert_eq!(err, "hash_mismatch", "stale edits request a full sync");
        assert_eq!(cx.active_preview_text(&file), Some(old), "stale edits preserve effective text");
        assert_eq!(cx.script_data.live_reload.script_mod_overrides.snapshot(), overrides, "stale edits preserve running code");
    }

    #[test]
    fn preview_validation_error_does_not_mutate_running_app() {
        let (mut cx, file, disk) = fixture();
        let good = disk.replacen("seed + 9", "seed + 8", 1);
        apply_preview(&mut cx, &file, PreviewPayload::FullText(good.clone())).unwrap();
        let overrides = cx.script_data.live_reload.script_mod_overrides.snapshot();
        for invalid in [disk.replacen("#(1.0)", "1", 1), disk.replacen("seed + 9", "seed + (", 1)] {
            let result = apply_preview(&mut cx, &file, PreviewPayload::FullText(invalid));
            assert!(result.is_err(), "validation must refuse the malformed block");
            assert_eq!(cx.active_preview_text(&file).as_ref(), Some(&good), "validation preserves the last full text");
            assert_eq!(cx.script_data.live_reload.script_mod_overrides.snapshot(), overrides, "validation preserves every override");
        }
        cx.design_preview_callback = Some(|cx, file, text| {
            cx.install_live_edit_text(file, text).unwrap();
            cx.live_edit_capture_begin();
            cx.with_vm(|vm| vm.with_reload(fixture::script_mod));
            cx.live_edit_capture_end();
            Some(Ok(()))
        });
        let fails_on_eval = disk.replacen("seed + 9", "missing_preview_function()", 1);
        assert!(apply_preview(&mut cx, &file, PreviewPayload::FullText(fails_on_eval)).is_err(), "evaluation errors roll back a validated preview");
        assert_eq!(cx.active_preview_text(&file).as_ref(), Some(&good), "rollback restores the previous full text");
        assert_eq!(cx.script_data.live_reload.script_mod_overrides.snapshot(), overrides, "rollback restores the previous overrides");
        assert_eq!(cx.with_vm(|vm| vm.with_reload(fixture::script_mod)).as_f64(), Some(9.0), "the restored block runs the previous expression");
        assert!(cx.process_live_edit(), "the synchronous failure queues a rollback reload");

        // Exercise the boundary between a frame's installed code and the
        // next verification request, with the same VM error capture as a reload.
        let fails_on_eval = disk.replacen("seed + 9", "unresolved_preview_name()", 1);
        cx.begin_preview_change(&file);
        cx.install_live_edit_text(&file, &fails_on_eval).unwrap();
        cx.install_preview_text(&file, fails_on_eval);
        cx.live_edit_capture_begin();
        cx.with_vm(|vm| vm.with_reload(fixture::script_mod));
        cx.live_edit_capture_end();
        let reply = verify_preview(&mut cx, Some(&file), None).unwrap();
        let reply = JsonValue::deserialize_json_strict(&reply).unwrap();
        assert_eq!(reply.get("rolled_back").and_then(JsonValue::as_bool), Some(true), "verify rolls back a failed reload");
        assert_eq!(reply.get("ok").and_then(JsonValue::as_bool), Some(false), "verify reports the failed preview");
        assert!(reply.get("errors").and_then(JsonValue::as_array).is_some_and(|e| !e.is_empty()), "verify returns the captured error");
        assert_eq!(cx.active_preview_text(&file), Some(good), "verify restores the last good preview");
        assert_eq!(cx.script_data.live_reload.script_mod_overrides.snapshot(), overrides, "verify restores only the previous code");
    }

    #[test]
    fn preview_revert_restores_original_compiled_code() {
        let (mut cx, file, disk) = fixture();
        let at = disk.find("seed + 9").unwrap() + 7;
        let changed = apply_preview(&mut cx, &file, PreviewPayload::Edits {
            base_hash: locate::source_hash(&disk), edits: vec![Replace::new(at..at + 1, "4")],
        }).unwrap();
        assert!(!changed.pending_reload, "synchronous installation needs no frame");
        assert_eq!(cx.with_vm(|vm| vm.with_reload(fixture::script_mod)).as_f64(), Some(5.0), "the preview changes the executed expression");
        apply_preview(&mut cx, &file, PreviewPayload::Revert).unwrap();
        assert_eq!(cx.active_preview_text(&file), None, "revert clears the full-file preview");
        assert_eq!(cx.live_edit_override_count(), 0, "revert removes the override");
        assert_eq!(cx.with_vm(|vm| vm.with_reload(fixture::script_mod)).as_f64(), Some(10.0), "revert executes the compiled expression");
        cx.design_preview_callback = None;
        let queued = apply_preview(&mut cx, &file, PreviewPayload::FullText(disk.clone())).unwrap();
        assert!(queued.pending_reload && cx.preview_change_queued(Some(&file)), "ordinary previews wait for live edit");
        cx.live_edit_capture_begin();
        cx.with_vm(|vm| { crate::script_eval!(vm, { unresolved_queued_error() }); });
        cx.live_edit_capture_end();
        assert!(verify_preview(&mut cx, Some(&file), None).unwrap().contains(r#""pending_reload":true"#), "verify before a frame keeps the rollback checkpoint");
        assert!(!cx.take_live_edit_errors().is_empty(), "verify leaves captured errors available while the preview is queued");
        apply_preview(&mut cx, &file, PreviewPayload::Revert).unwrap();
        assert!(!cx.preview_change_queued(Some(&file)), "reverting before the frame cancels the queued change");

        let edited = disk.replacen("seed + 9", "seed + 8", 1);
        apply_preview(&mut cx, &file, PreviewPayload::FullText(edited)).unwrap();
        let seq = cx.live_edit_seq();
        assert!(cx.process_live_edit(), "processing the queued preview runs a reload pass");
        assert_eq!(cx.live_edit_seq(), seq + 1, "the preview advances the completion counter once");
        assert!(cx.live_edit_override_count() > 0, "the queued edit installs an override");
        assert!(!cx.preview_change_queued(Some(&file)), "the installed preview is ready to verify");
        assert_eq!(verify_preview(&mut cx, Some(&file), None).unwrap(), r#"{"ok":true,"errors":[]}"#, "verify accepts the installed preview");

        let reverted = apply_preview(&mut cx, &file, PreviewPayload::Revert).unwrap();
        assert!(reverted.pending_reload && cx.preview_change_queued(Some(&file)), "reverting an installed override waits for its reload");
        assert!(cx.process_live_edit(), "processing the revert runs a reload pass");
        assert_eq!(cx.live_edit_seq(), seq + 2, "the revert advances the completion counter once");
        assert!(!cx.preview_change_queued(Some(&file)), "the completed revert is ready to verify");
        assert_eq!(verify_preview(&mut cx, Some(&file), None).unwrap(), r#"{"ok":true,"errors":[]}"#, "verify accepts the completed revert");
    }

    #[test]
    fn preview_chunked_upload_reassembles_large_file() {
        fn encode(input: &[u8]) -> String {
            const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            for group in input.chunks(3) {
                let bits = (u32::from(group[0]) << 16) | (u32::from(*group.get(1).unwrap_or(&0)) << 8) | u32::from(*group.get(2).unwrap_or(&0));
                for i in 0..4 {
                    out.push(if i > group.len() { '=' } else { ALPHABET[((bits >> (18 - 6 * i)) & 63) as usize] as char });
                }
            }
            out
        }
        let (mut cx, file, mut text) = fixture();
        text.push_str("\n//");
        text.push_str(&"a".repeat(200 * 1024));
        let full = apply_preview(&mut cx, &file, PreviewPayload::FullText(text.clone())).unwrap();
        let expected = cx.script_data.live_reload.script_mod_overrides.snapshot();
        apply_preview(&mut cx, &file, PreviewPayload::Revert).unwrap();
        let path = std::path::Path::new(&file);
        let alias = path.parent().unwrap().join(".").join(path.file_name().unwrap()).to_string_lossy().into_owned();
        let mut args = vec![("file".into(), alias.clone()), ("id".into(), "upload".into()), ("seq".into(), "0".into()), ("data".into(), String::new())];
        for malformed in ["a", "!!==", "abc=AAAA", "a===", "AB=="] {
            args[3].1 = malformed.into();
            assert!(chunk(&mut cx, &args).is_err(), "reject malformed base64 {malformed}");
        }
        for (seq, bytes) in text.as_bytes().chunks(48 * 1024).enumerate() {
            args[2].1 = seq.to_string();
            args[3].1 = encode(bytes);
            chunk(&mut cx, &args).expect("all spellings of the registered file must append to the same upload");
            assert!(chunk(&mut cx, &args).is_err(), "a duplicate chunk cannot append twice");
            args[0].1.clone_from(&file);
        }
        let unregistered = [("file", "unregistered-chunk.rs"), ("id", "other"), ("seq", "0"), ("data", "YQ==")]
            .map(|(key, value)| (key.into(), value.into()));
        assert_eq!(chunk(&mut cx, &unregistered).unwrap_err(), "no registered source file for unregistered-chunk.rs",
            "the first chunk for an unregistered file is rejected at arrival");
        let consume = [("file".into(), alias), ("upload".into(), "upload".into())];
        let outcome = remote(&mut cx, "design_preview", &consume)
            .expect("the same registered file can consume the buffered upload after an unregistered request");
        assert!(outcome.contains(&format!("{:016x}", full.file_hash)), "uploaded bytes hash exactly like a full preview");
        assert_eq!(cx.active_preview_text(&file), Some(text), "all chunks are installed in original order");
        assert_eq!(cx.script_data.live_reload.script_mod_overrides.snapshot(), expected, "chunked and full previews install the same code");
        assert!(remote(&mut cx, "design_preview", &consume).is_err(), "completed upload has been evicted");
    }
}
