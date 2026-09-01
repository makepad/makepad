//! Shared subprocess runner for reference-tier image backends.
//!
//! Explicit reference-only capabilities may carry an operator-configured
//! command template (for example `MAKEPAD_DEPTH_CMD`):
//! the template's `{in}`/`{out}` tokens are replaced with temp PNG paths and
//! the command is split on whitespace — paths with spaces need the operator
//! to place temp dirs accordingly; on the boxes everything lives under C:\ai.
//!
//! Two run shapes:
//! - [`run_blocking`]: a small legacy/test utility using `Command::status()`
//!   with inherited stdio, no cancel, and no timeout. No canonical character
//!   pipeline backend calls it.
//! - [`run_cancellable`]: spawns with piped stdout, polls the child, kills it
//!   when the job's [`CancelToken`] is raised or the deadline passes (the
//!   world_backend pattern). Child stdout lines `@P <fraction> <stage...>`
//!   become progress callbacks; everything else is passed through to our
//!   stdout so reference prints stay visible in the service log. Stderr
//!   inherits the service's stream, so python tracebacks land in svc.log.
//!
//! Output contract: the command must write `{out}`; it may also write a
//! metadata sidecar at `{out}.json` (the depth backend requires one). Both
//! are read and cleaned up here.

use crate::backend::CancelToken;
use crate::child_process;
use makepad_micro_serde::*;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// What one subprocess run produced.
#[derive(Debug)]
pub struct SubprocOutput {
    /// Bytes of the `{out}` file the command wrote.
    pub out_bytes: Vec<u8>,
    /// Contents of the optional `{out}.json` metadata sidecar.
    pub sidecar_json: Option<String>,
}

/// Why a cancellable run did not produce output.
#[derive(Debug)]
pub enum SubprocError {
    /// The job's cancel flag was raised; the child was killed.
    Cancelled,
    /// The deadline passed; the child was killed.
    TimedOut(Duration),
    /// Anything else: spawn/io failure or a non-zero exit.
    Failed(String),
}

impl std::fmt::Display for SubprocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubprocError::Cancelled => write!(f, "cancelled"),
            SubprocError::TimedOut(t) => write!(f, "timed out after {t:?}"),
            SubprocError::Failed(m) => write!(f, "{m}"),
        }
    }
}

/// True when this MACHINE carries the command template's provisioned pieces:
/// the executable (first token) exists, and so does every `.py` script
/// argument. This is the per-box `backend_provisioned` probe for subprocess
/// reference backends — `/models` must not advertise capabilities the
/// scheduler would route jobs to only to fail at generate time.
pub fn cmd_provisioned(cmd_template: &str) -> bool {
    let mut parts = cmd_template.split_whitespace();
    let Some(exe) = parts.next() else {
        return false;
    };
    if !Path::new(exe).exists() {
        return false;
    }
    parts
        .filter(|part| part.ends_with(".py"))
        .all(|script| Path::new(script).exists())
}

/// Unique in/out temp paths for one run: `<tmp>/<tag>_in_<pid>_<millis>.<ext>`
/// and `<tag>_out_...` (the naming the rembg path always used, ext "png";
/// the rig/motion GLB backends pass "glb" so box-side python can trust the
/// suffix).
fn temp_paths(tmp_dir: &Path, tag: &str, ext: &str) -> (PathBuf, PathBuf) {
    let unique = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    (
        tmp_dir.join(format!("{tag}_in_{unique}.{ext}")),
        tmp_dir.join(format!("{tag}_out_{unique}.{ext}")),
    )
}

/// Splits the template on whitespace and substitutes the `{in}`/`{out}`
/// tokens. Errors with "empty command" on a blank template.
fn expand_template(
    cmd_template: &str,
    in_path: &Path,
    out_path: &Path,
) -> Result<(String, Vec<String>), String> {
    let parts: Vec<String> = cmd_template
        .split_whitespace()
        .map(|part| {
            part.replace("{in}", &in_path.to_string_lossy())
                .replace("{out}", &out_path.to_string_lossy())
        })
        .collect();
    let (exe, args) = parts
        .split_first()
        .ok_or_else(|| "empty command".to_string())?;
    Ok((exe.clone(), args.to_vec()))
}

fn sidecar_path(out_path: &Path) -> PathBuf {
    let mut os = out_path.as_os_str().to_os_string();
    os.push(".json");
    PathBuf::from(os)
}

/// Per-run parameters beyond raw input bytes (prompt, clip names, seed...)
/// travel as a JSON sidecar at `{in}.json` — the template stays `{in}/{out}`
/// only, so free-text params never fight the whitespace-split command line.
fn input_sidecar_path(in_path: &Path) -> PathBuf {
    let mut os = in_path.as_os_str().to_os_string();
    os.push(".json");
    PathBuf::from(os)
}

fn read_outputs(out_path: &Path) -> Result<SubprocOutput, String> {
    let out_bytes = std::fs::read(out_path).map_err(|e| format!("read out: {e}"))?;
    let sidecar_json = std::fs::read_to_string(sidecar_path(out_path)).ok();
    Ok(SubprocOutput {
        out_bytes,
        sidecar_json,
    })
}

fn cleanup(in_path: &Path, out_path: &Path) {
    let _ = std::fs::remove_file(in_path);
    let _ = std::fs::remove_file(out_path);
    let _ = std::fs::remove_file(sidecar_path(out_path));
    let _ = std::fs::remove_file(input_sidecar_path(in_path));
}

/// Runs the command template to completion with inherited stdio — the exact
/// behavior the trellis rembg preprocess always had (no cancel, no timeout;
/// the caller brackets the call with its own cancel checks). Error strings
/// match the original: "tmp dir:", "write:", "empty command", "spawn {exe}:",
/// "exit {status}", "read out:".
pub fn run_blocking(
    cmd_template: &str,
    tmp_dir: &Path,
    tag: &str,
    input: &[u8],
) -> Result<SubprocOutput, String> {
    std::fs::create_dir_all(tmp_dir).map_err(|e| format!("tmp dir: {e}"))?;
    let (in_path, out_path) = temp_paths(tmp_dir, tag, "png");
    let result = (|| {
        std::fs::write(&in_path, input).map_err(|e| format!("write: {e}"))?;
        let (exe, args) = expand_template(cmd_template, &in_path, &out_path)?;
        let status = child_process::status(Command::new(&exe).args(&args))
            .map_err(|e| format!("spawn {exe}: {e}"))?;
        if !status.success() {
            return Err(format!("exit {status}"));
        }
        read_outputs(&out_path)
    })();
    cleanup(&in_path, &out_path);
    result
}

/// Runs the command template with cancel + deadline supervision: the child
/// is spawned with piped stdout and polled; a raised cancel flag or an
/// elapsed deadline kills it (partial temp files discarded). Stdout lines
/// `@P <fraction 0..1> <stage text>` are forwarded to `progress`; other
/// lines pass through to our stdout (service log).
pub fn run_cancellable(
    cmd_template: &str,
    tmp_dir: &Path,
    tag: &str,
    input: &[u8],
    timeout: Duration,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<SubprocOutput, SubprocError> {
    run_cancellable_req(
        &SubprocRequest {
            cmd_template,
            tmp_dir,
            tag,
            ext: "png",
            input,
            input_sidecar_json: None,
            timeout,
        },
        cancel,
        progress,
    )
}

/// One cancellable subprocess run, fully specified. [`run_cancellable`] is
/// the historical PNG-shaped wrapper; the GLB backends (rig/motion) use this
/// directly for the `.glb` temp suffix and the `{in}.json` params sidecar.
pub struct SubprocRequest<'a> {
    pub cmd_template: &'a str,
    pub tmp_dir: &'a Path,
    pub tag: &'a str,
    /// Extension for both temp files (no dot): "png", "glb", ...
    pub ext: &'a str,
    pub input: &'a [u8],
    /// When set, written to `{in}.json` before spawn — how prompts and other
    /// free-text params reach the script without fighting the
    /// whitespace-split command template.
    pub input_sidecar_json: Option<&'a str>,
    pub timeout: Duration,
}

/// [`run_cancellable`] generalized over temp-file extension + params sidecar.
pub fn run_cancellable_req(
    req: &SubprocRequest,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<SubprocOutput, SubprocError> {
    let fail = |m: String| SubprocError::Failed(m);
    std::fs::create_dir_all(req.tmp_dir).map_err(|e| fail(format!("tmp dir: {e}")))?;
    let (in_path, out_path) = temp_paths(req.tmp_dir, req.tag, req.ext);
    let result = run_cancellable_at(req, &in_path, &out_path, cancel, progress);
    cleanup(&in_path, &out_path);
    result
}

fn run_cancellable_at(
    req: &SubprocRequest,
    in_path: &Path,
    out_path: &Path,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<SubprocOutput, SubprocError> {
    let (cmd_template, input, timeout) = (req.cmd_template, req.input, req.timeout);
    let fail = |m: String| SubprocError::Failed(m);
    std::fs::write(in_path, input).map_err(|e| fail(format!("write: {e}")))?;
    if let Some(json) = req.input_sidecar_json {
        std::fs::write(input_sidecar_path(in_path), json)
            .map_err(|e| fail(format!("write params: {e}")))?;
    }
    let (exe, args) = expand_template(cmd_template, in_path, out_path).map_err(fail)?;
    let mut child = child_process::spawn(
        Command::new(&exe)
        .args(&args)
        .env("PYTHONUNBUFFERED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // stderr inherits the service's stream -> tracebacks in svc.log.
    )
        .map_err(|e| fail(format!("spawn {exe}: {e}")))?;

    // Reader thread: ends (and disconnects the channel) when the child
    // closes stdout, i.e. exits or crashes.
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let started = Instant::now();
    let forward = |line: String, progress: &mut dyn FnMut(&str, f64)| {
        // "@P 0.42 load model" -> progress("load model", 0.42)
        if let Some(rest) = line.strip_prefix("@P ") {
            if let Some((frac, stage)) = rest.split_once(' ') {
                if let Ok(frac) = frac.trim().parse::<f64>() {
                    progress(stage.trim(), frac.clamp(0.0, 1.0));
                    return;
                }
            }
        }
        // Reference prints stay visible in the service log.
        println!("{line}");
    };
    let kill = |child: &mut std::process::Child| {
        let _ = child_process::kill_tree(child);
        let _ = child.wait();
    };
    loop {
        if cancel.is_cancelled() {
            kill(&mut child);
            return Err(SubprocError::Cancelled);
        }
        if started.elapsed() > timeout {
            kill(&mut child);
            return Err(SubprocError::TimedOut(timeout));
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => forward(line, progress),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // stdout closed: reap the child and check how it went.
    let status = child.wait().map_err(|e| fail(format!("wait {exe}: {e}")))?;
    if !status.success() {
        return Err(fail(format!("exit {status}")));
    }
    read_outputs(out_path).map_err(fail)
}

// ---------------------------------------------------------------------------
// PNG header introspection
// ---------------------------------------------------------------------------

/// The IHDR fields subprocess output contracts are checked against — enough
/// to validate "16-bit grayscale" / "RGBA" without a full decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PngHeader {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    /// PNG color type: 0 grayscale, 2 RGB, 3 palette, 4 gray+alpha, 6 RGBA.
    pub color_type: u8,
}

/// Parses the PNG signature + IHDR chunk. `None` when `bytes` is not a PNG.
pub fn png_header(bytes: &[u8]) -> Option<PngHeader> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    if bytes.len() < 33 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some(PngHeader {
        width: u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        height: u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
        bit_depth: bytes[24],
        color_type: bytes[25],
    })
}

// ---------------------------------------------------------------------------
// GLB header introspection
// ---------------------------------------------------------------------------

/// The JSON chunk of a structurally valid GLB (binary glTF v2), or `None`.
/// This validates the container version, declared byte length, and aligned
/// chunk bounds before returning a borrowed chunk.
pub fn glb_json_chunk(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 12
        || &bytes[0..4] != b"glTF"
        || u32::from_le_bytes(bytes[4..8].try_into().ok()?) != 2
        || u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize != bytes.len()
    {
        return None;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        if len % 4 != 0 {
            return None;
        }
        let kind = &bytes[at + 4..at + 8];
        let data = bytes.get(at + 8..at + 8 + len)?;
        if kind == b"JSON" {
            return Some(data);
        }
        at += 8 + len;
    }
    None
}

/// Parse the GLB JSON chunk into a real JSON tree. Contract validators use
/// this instead of searching raw bytes, so a string/comment-like payload or
/// a nested key cannot masquerade as a top-level glTF field.
pub fn glb_json_value(bytes: &[u8]) -> Option<JsonValue> {
    let chunk = glb_json_chunk(bytes)?;
    let json = std::str::from_utf8(chunk).ok()?;
    JsonValue::deserialize_json(json.trim_end_matches(|c| c == ' ' || c == '\0')).ok()
}

/// True when the GLB's JSON header contains a top-level-looking `"key":`
/// (substring probe — good enough to tell a rigged GLB from a bare mesh,
/// mirroring the depth backend's sidecar substring checks).
pub fn glb_has_key(bytes: &[u8], key: &str) -> bool {
    let Some(json) = glb_json_chunk(bytes) else {
        return false;
    };
    let needle = format!("\"{key}\":");
    json.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Test helper (also used by the rig/motion backend tests): a minimal GLB
/// container around the given JSON text.
#[cfg(test)]
pub fn fake_glb(json: &str) -> Vec<u8> {
    let mut json_bytes = json.as_bytes().to_vec();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    out
}

/// Test helper (also used by the matte/depth backend tests): fabricates a
/// PNG signature + IHDR — all the header checks look at.
#[cfg(test)]
pub fn fake_png(width: u32, height: u32, bit_depth: u8, color_type: u8) -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.push(bit_depth);
    png.push(color_type);
    png.extend_from_slice(&[0, 0, 0]); // compression/filter/interlace
    png.extend_from_slice(&[0, 0, 0, 0]); // crc (unchecked)
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_header_reads_ihdr() {
        // A real encoder-produced RGBA8 png.
        let rgba = vec![255u8; 4 * 4 * 4];
        let png = crate::testpattern::encode_png_rgba(&rgba, 4, 4).unwrap();
        let header = png_header(&png).unwrap();
        assert_eq!(header.width, 4);
        assert_eq!(header.height, 4);
        assert_eq!(header.bit_depth, 8);
        assert_eq!(header.color_type, 6);
        // 16-bit grayscale (the depth output contract).
        let header = png_header(&fake_png(640, 480, 16, 0)).unwrap();
        assert_eq!((header.bit_depth, header.color_type), (16, 0));
        assert_eq!((header.width, header.height), (640, 480));
        // Not a png.
        assert!(png_header(b"not a png").is_none());
        assert!(png_header(&[]).is_none());
    }

    #[test]
    fn cmd_provisioned_probes_exe_and_scripts() {
        // Nothing configured / nothing on disk.
        assert!(!cmd_provisioned(""));
        assert!(!cmd_provisioned(r"C:\nope\python.exe C:\nope\x.py {in} {out}"));
        // An executable that certainly exists on the test machine.
        let exe = std::env::current_exe().unwrap();
        let exe = exe.to_string_lossy();
        assert!(cmd_provisioned(&format!("{exe} {{in}} {{out}}")));
        // ... but a missing .py script argument fails the probe.
        assert!(!cmd_provisioned(&format!(
            "{exe} /definitely/not/there.py {{in}} {{out}}"
        )));
    }

    #[test]
    fn glb_json_chunk_and_keys() {
        let glb = fake_glb(r#"{"asset":{"version":"2.0"},"skins":[{"joints":[1]}]}"#);
        assert!(glb_json_chunk(&glb).is_some());
        let root = glb_json_value(&glb).unwrap();
        assert!(matches!(root.key("skins"), Some(JsonValue::Array(v)) if v.len() == 1));
        assert!(glb_has_key(&glb, "skins"));
        assert!(glb_has_key(&glb, "asset"));
        assert!(!glb_has_key(&glb, "animations"));
        // Not a GLB.
        assert!(glb_json_chunk(b"not a glb").is_none());
        assert!(glb_json_chunk(&[]).is_none());
        assert!(!glb_has_key(b"\x89PNG", "skins"));
        // Truncated chunk is rejected, not read out of bounds.
        let mut cut = glb.clone();
        cut.truncate(24);
        assert!(glb_json_chunk(&cut).is_none());
        // Header version and declared length are part of the GLB contract.
        let mut bad_version = glb.clone();
        bad_version[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(glb_json_value(&bad_version).is_none());
        let mut bad_length = glb.clone();
        bad_length[8..12].copy_from_slice(&12u32.to_le_bytes());
        assert!(glb_json_value(&bad_length).is_none());
        // A byte substring is not a JSON contract.
        let bait = fake_glb(r#"{"extras":{"skins":[]}}"#);
        assert!(glb_has_key(&bait, "skins"));
        assert!(glb_json_value(&bait).unwrap().key("skins").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn run_cancellable_req_glb_ext_and_params_sidecar() {
        let tmp = std::env::temp_dir().join(format!("subproc_test_g_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // The "backend" asserts the .glb suffix and echoes the params sidecar
        // into the output — proving both reached the child.
        let script_path = tmp.join("glb_backend.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\ncase \"$1\" in *.glb) ;; *) exit 3;; esac\n\
             test -f \"$1.json\" || exit 4\ncat \"$1\" \"$1.json\" > \"$2\"\n",
        )
        .unwrap();
        let cmd = format!("/bin/sh {} {{in}} {{out}}", script_path.display());
        let out = run_cancellable_req(
            &SubprocRequest {
                cmd_template: &cmd,
                tmp_dir: &tmp,
                tag: "t",
                ext: "glb",
                input: b"GLBBYTES",
                input_sidecar_json: Some(r#"{"prompt":"knight"}"#),
                timeout: Duration::from_secs(30),
            },
            &CancelToken::new(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(out.out_bytes, b"GLBBYTES{\"prompt\":\"knight\"}");
        // Params sidecar cleaned up alongside the temp files.
        let leftovers: Vec<_> = std::fs::read_dir(&tmp)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "glb_backend.sh")
            .collect();
        assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn run_blocking_round_trips_bytes() {
        let tmp = std::env::temp_dir().join(format!("subproc_test_{}", std::process::id()));
        let out = run_blocking("/bin/cp {in} {out}", &tmp, "t", b"PAYLOAD").unwrap();
        assert_eq!(out.out_bytes, b"PAYLOAD");
        assert!(out.sidecar_json.is_none());
        // Temp files cleaned up.
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn run_blocking_failure_paths() {
        let tmp = std::env::temp_dir().join(format!("subproc_test_f_{}", std::process::id()));
        // Non-zero exit.
        let err = run_blocking("/bin/cp {in} /no/such/dir/x", &tmp, "t", b"x").unwrap_err();
        assert!(err.starts_with("exit "), "{err}");
        // Command produced no output file.
        let err = run_blocking("/bin/ls {in}", &tmp, "t", b"x").unwrap_err();
        assert!(err.starts_with("read out: "), "{err}");
        // Empty template.
        let err = run_blocking("", &tmp, "t", b"x").unwrap_err();
        assert_eq!(err, "empty command");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn run_cancellable_progress_sidecar_and_cancel() {
        let tmp = std::env::temp_dir().join(format!("subproc_test_c_{}", std::process::id()));
        // A shell "backend" that reports progress and writes output + sidecar
        // (a script file: split_whitespace can't carry inline spaced args).
        std::fs::create_dir_all(&tmp).unwrap();
        let script_path = tmp.join("backend.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\necho '@P 0.5 halfway'\ncp \"$1\" \"$2\"\nprintf '{\"k\":1}' > \"$2.json\"\n",
        )
        .unwrap();
        let cmd = format!("/bin/sh {} {{in}} {{out}}", script_path.display());
        let mut stages = Vec::new();
        let out = run_cancellable(
            &cmd,
            &tmp,
            "t",
            b"BYTES",
            Duration::from_secs(30),
            &CancelToken::new(),
            &mut |stage, frac| stages.push((stage.to_string(), frac)),
        )
        .unwrap();
        assert_eq!(out.out_bytes, b"BYTES");
        assert_eq!(out.sidecar_json.as_deref(), Some("{\"k\":1}"));
        assert_eq!(stages, vec![("halfway".to_string(), 0.5)]);

        // A pre-raised cancel kills a long-running child promptly.
        let token = CancelToken::new();
        token.cancel();
        let started = std::time::Instant::now();
        let err = run_cancellable(
            "/bin/sleep 30",
            &tmp,
            "t",
            b"x",
            Duration::from_secs(60),
            &token,
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(matches!(err, SubprocError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));

        // Deadline kills too.
        let started = std::time::Instant::now();
        let err = run_cancellable(
            "/bin/sleep 30",
            &tmp,
            "t",
            b"x",
            Duration::from_millis(300),
            &CancelToken::new(),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(matches!(err, SubprocError::TimedOut(_)));
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
