//! Pull change reports from the admin inbox into a quarantine outside every repository. A report
//! is untrusted input that no agent reads until intake takes it: the pull keeps the zip as
//! received, checks it only for counts, and prints or records no text from it. Nothing in a
//! report is executed.

use makepad_change_report::Package;
use makepad_network::blocking_http::{self, Limits, Request};
use makepad_strict_json::{self as json, Value};
use std::fs;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const UNTRUSTED: &str = "Untrusted input: no agent reads it until intake takes it, and nothing in it or named by it is applied, built or run.";

#[derive(Debug, PartialEq)]
struct Entry {
    id: u64,
    created: i64,
    handled: bool,
    reply: bool,
    bytes: u64,
}

fn parse_list(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let value = json::parse(bytes).map_err(|e| format!("invalid report list JSON: {e}"))?;
    let rows = value.as_arr().ok_or("report list must be an array")?;
    let mut entries = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let invalid = |field| format!("report list entry {}: missing or invalid {field}", index + 1);
        entries.push(Entry {
            id: row.get("id").and_then(Value::as_u64).ok_or_else(|| invalid("id"))?,
            created: row.get("created").and_then(Value::as_i64).ok_or_else(|| invalid("created"))?,
            handled: row.get("handled").and_then(Value::as_bool).ok_or_else(|| invalid("handled"))?,
            reply: row.get("reply").and_then(Value::as_bool).ok_or_else(|| invalid("reply"))?,
            bytes: row.get("bytes").and_then(Value::as_u64).ok_or_else(|| invalid("bytes"))?,
        });
    }
    entries.sort_by_key(|entry| entry.id);
    Ok(entries)
}

/// The quarantine is `~/.intake-quarantine/runner-reports/`, beside intake's items: outside every
/// repository and `local/`, where no search or index reaches it.
fn target_dir(args: &[&str], home: Option<OsString>) -> Result<PathBuf, String> {
    let args = args.strip_prefix(&["pull"]).unwrap_or(args);
    match args {
        [] => home.filter(|home| !home.is_empty()).map(|home| PathBuf::from(home).join(".intake-quarantine/runner-reports"))
            .ok_or_else(|| "HOME is not set: pass --dir".to_owned()),
        ["--dir", dir] if !dir.is_empty() && !dir.starts_with("--") => Ok(PathBuf::from(dir)),
        _ => Err("usage: makepad devhub reports [pull] [--dir <dir>]".to_owned()),
    }
}

fn get(base: &str, path: &str, accept: &str) -> Result<Vec<u8>, String> {
    // Neither a growing inbox nor an invalid, oversized report should disappear behind the
    // client's default body cap. The package checker reports format limits after saving the zip.
    let req = Request::get(format!("{}{path}", base.trim_end_matches('/')))
        .limits(Limits { max_body_bytes: usize::MAX, ..Limits::default() })
        .header("Accept", accept).map_err(|e| e.to_string())?;
    let response = blocking_http::request_no_redirect(req).map_err(|e| e.to_string())?;
    if response.status != 200 {
        let body: String = String::from_utf8_lossy(&response.body).chars().take(200).collect();
        return Err(format!("HTTP {}: {body}", response.status));
    }
    Ok(response.body)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn meta_text(entry: &Entry, checked: &Result<Package, String>) -> String {
    let received = makepad_civil_time::utc(entry.created);
    let (check, changes, diff, suggestion) = match checked {
        Ok(package) => ("ok", package.report.changes.len().to_string(), yes_no(package.diff.is_some()), yes_no(package.report.suggestion)),
        Err(_) => ("failed", "unknown".to_owned(), "unknown", "unknown"),
    };
    format!(
        "id: {}\nreceived: {:04}-{:02}-{:02} {:02}:{:02}\ncreated: {}\nreply: {}\nhandled: {}\ncheck: {check}\nchanges: {changes}\ndiff: {diff}\nsuggestion: {suggestion}\nzip: {} bytes\n{UNTRUSTED}\n",
        entry.id, received.year, received.month, received.day, received.hour, received.minute,
        entry.created, yes_no(entry.reply), yes_no(entry.handled), entry.bytes,
    )
}

/// The report's folder and whether it holds this report already. A report is its id and the
/// time makepad.nl received it: the inbox gives a removed item's id to the next one, so when
/// `<id>/` holds an earlier report this one goes in `<id>-<created>/`.
fn folder(dir: &Path, entry: &Entry) -> (PathBuf, bool) {
    let first = dir.join(entry.id.to_string());
    if !first.is_dir() {
        return (first, false);
    }
    let created = format!("created: {}", entry.created);
    if fs::read_to_string(first.join("meta.txt")).is_ok_and(|meta| meta.lines().any(|line| line == created)) {
        return (first, true);
    }
    // an earlier report with this id; `<id>-<created>/` is this one's, never rewritten once there
    let again = dir.join(format!("{}-{}", entry.id, entry.created));
    let pulled = again.is_dir();
    (again, pulled)
}

/// A folder only its owner can open (0700 on unix).
fn private_dir(path: &Path, recursive: bool) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// A new file only its owner can read (0600 on unix).
fn private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

/// Save bytes separately from downloading, so failed checks are kept and filesystem behavior
/// can be exercised without the admin server. None means this report was already pulled.
fn save_report(dir: &Path, entry: &Entry, zip: &[u8]) -> Result<Option<String>, String> {
    let (dest, pulled) = folder(dir, entry);
    if pulled {
        return Ok(None);
    }
    private_dir(dir, true).map_err(|e| format!("{}: {e}", dir.display()))?;
    // a folder that was there already becomes the owner's too
    #[cfg(unix)]
    fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dir.join(format!(".{}.part", dest.file_name().and_then(|n| n.to_str()).unwrap_or("report")));
    match fs::symlink_metadata(&part) {
        Ok(metadata) => {
            let removed = if metadata.is_dir() { fs::remove_dir_all(&part) } else { fs::remove_file(&part) };
            removed.map_err(|e| format!("{}: {e}", part.display()))?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", part.display())),
    }
    private_dir(&part, false).map_err(|e| format!("{}: {e}", part.display()))?;
    let write = |name: &str, bytes: &[u8]| {
        let path = part.join(name);
        private_file(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    };
    write("report.zip", zip)?;
    let checked = makepad_change_report::open(zip);
    write("meta.txt", meta_text(entry, &checked).as_bytes())?;
    fs::rename(&part, &dest).map_err(|e| format!("{} -> {}: {e}", part.display(), dest.display()))?;
    // counts and enums only: a report's titles, app and release are its sender's text
    let detail = match checked {
        Ok(package) => format!("check ok, {} changes, diff {}", package.report.changes.len(), yes_no(package.diff.is_some())),
        Err(_) => "check failed".to_owned(),
    };
    Ok(Some(format!("NEW change report {}: {detail} -> {}/", entry.id, dest.display())))
}

pub fn cli(args: &[&str]) -> i32 {
    let dir = match target_dir(args, std::env::var_os("HOME")) {
        Ok(dir) => dir,
        Err(e) => { eprintln!("reports: {e}"); return 2; }
    };
    let base = std::env::var("MAKEPAD_ADMIN_URL").ok();
    let base = base.as_deref().map(str::trim).filter(|url| !url.is_empty()).unwrap_or("https://makepad.nl");
    let entries = match get(base, "/api/loader-admin/changes", "application/json").and_then(|bytes| parse_list(&bytes)) {
        Ok(entries) => entries,
        Err(e) => { eprintln!("reports: {e}"); return 1; }
    };
    let (mut new, mut already, mut failed) = (0, 0, false);
    for entry in &entries {
        if folder(&dir, entry).1 {
            already += 1;
            continue;
        }
        let path = format!("/api/loader-admin/feedback-image/{}/1", entry.id);
        match get(base, &path, "application/zip").and_then(|zip| save_report(&dir, entry, &zip)) {
            Ok(Some(line)) => { println!("{line}"); new += 1; }
            Ok(None) => already += 1,
            Err(e) => { eprintln!("reports: change report {}: {e}", entry.id); failed = true; }
        }
    }
    println!("reports: {new} new, {} on makepad.nl, {already} already here", entries.len());
    if failed { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_zip_file::{ZipMethod, ZipWriter};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const EXAMPLE: &str = include_str!("../../../docs/agents/change-report-example/report.json");
    const DIFF: &str = "diff --git a/example.txt b/example.txt\n--- a/example.txt\n+++ b/example.txt\n@@ -1 +1 @@\n-before\n+after\n";

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!("devhub-reports-{}-{}-{}", std::process::id(), crate::proto::wall_ms(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn example_zip(diff: bool, suggestion: bool) -> Vec<u8> {
        let mut report = makepad_change_report::parse_report(EXAMPLE.as_bytes()).unwrap();
        report.diff = diff;
        report.suggestion = suggestion;
        let mut zip = ZipWriter::new();
        zip.add("report.json", report.to_json().as_bytes(), ZipMethod::Store).unwrap();
        zip.add("REPORT.md", report.to_markdown().as_bytes(), ZipMethod::Store).unwrap();
        if diff { zip.add("changes.diff", DIFF.as_bytes(), ZipMethod::Store).unwrap(); }
        zip.finish().unwrap()
    }

    fn entry(id: u64, bytes: usize) -> Entry {
        Entry { id, created: 1_789_907_640, handled: false, reply: true, bytes: bytes as u64 }
    }

    #[test]
    fn list_requires_an_array_and_ids_and_sorts_reports() {
        let rows = br#"[
            {"id":9,"app":"calculator","release":"2026-09-20","created":1789907640,"handled":true,"reply":false,"bytes":42},
            {"id":3,"app":"calculator","release":"2026-09-20","created":1789907640,"handled":false,"reply":true,"bytes":24}
        ]"#;
        let entries = parse_list(rows).unwrap();
        assert_eq!(entries, [entry(3, 24), Entry { handled: true, reply: false, ..entry(9, 42) }]);
        assert!(parse_list(b"[]").unwrap().is_empty());
        assert!(parse_list(b"{}").unwrap_err().contains("array"));
        assert!(parse_list(br#"[{"app":"calculator","release":"2026-09-20","created":0,"handled":false,"reply":false,"bytes":1}]"#).unwrap_err().contains("id"));
    }

    #[test]
    fn metadata_records_utc_and_checked_flags_or_unknown() {
        for (diff, suggestion) in [(false, false), (true, false), (false, true)] {
            let zip = example_zip(diff, suggestion);
            let checked = makepad_change_report::open(&zip);
            assert!(checked.is_ok(), "example package: {checked:?}");
            let expected = format!(
                "id: 3\nreceived: 2026-09-20 12:34\ncreated: 1789907640\nreply: yes\nhandled: no\ncheck: ok\nchanges: 2\ndiff: {}\nsuggestion: {}\nzip: {} bytes\n{UNTRUSTED}\n",
                yes_no(diff), yes_no(suggestion), zip.len(),
            );
            assert_eq!(meta_text(&entry(3, zip.len()), &checked), expected);
        }
        let failed = makepad_change_report::open(b"invalid zip");
        assert!(failed.is_err());
        let received = Entry { created: -1, handled: true, reply: false, ..entry(4, 11) };
        assert_eq!(meta_text(&received, &failed), format!(
            "id: 4\nreceived: 1969-12-31 23:59\ncreated: -1\nreply: no\nhandled: yes\ncheck: failed\nchanges: unknown\ndiff: unknown\nsuggestion: unknown\nzip: 11 bytes\n{UNTRUSTED}\n",
        ));
    }

    #[test]
    fn the_default_folder_is_the_quarantine_outside_every_checkout() {
        let quarantine = PathBuf::from("/h/.intake-quarantine/runner-reports");
        assert_eq!(target_dir(&[], Some("/h".into())), Ok(quarantine.clone()));
        assert_eq!(target_dir(&["pull"], Some("/h".into())), Ok(quarantine));
        assert!(target_dir(&["pull"], None).unwrap_err().contains("--dir"), "no HOME, no guess");
        assert!(target_dir(&[], Some("".into())).is_err(), "an empty HOME is no home");
        assert_eq!(target_dir(&["pull", "--dir", "x"], None), Ok(PathBuf::from("x")));
        assert!(target_dir(&["--dir", "--x"], None).is_err());
        assert!(target_dir(&["push"], None).is_err());
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    }

    #[test]
    fn saving_publishes_complete_folders_once_and_keeps_failed_checks() {
        let temp = TempDir::new();
        let dir = temp.0.join("reports");
        fs::create_dir_all(dir.join(".3.part")).unwrap();
        fs::write(dir.join(".3.part/stale"), "unfinished").unwrap();
        let zip = example_zip(true, false);
        let received = entry(3, zip.len());
        let line = save_report(&dir, &received, &zip).unwrap().unwrap();
        assert_eq!(line, format!("NEW change report 3: check ok, 2 changes, diff yes -> {}/", dir.join("3").display()), "no text from the report");
        assert!(folder(&dir, &received).1);
        assert!(!dir.join(".3.part").exists());
        let checked = makepad_change_report::open(&zip);
        assert_eq!(fs::read(dir.join("3/report.zip")).unwrap(), zip);
        assert_eq!(fs::read_to_string(dir.join("3/meta.txt")).unwrap(), meta_text(&received, &checked));
        assert_eq!(names(&dir.join("3")), ["meta.txt", "report.zip"], "the zip stays as received, nothing unpacked");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode(dir.clone()), mode(dir.join("3"))), (0o700, 0o700), "the folders are the owner's");
            assert_eq!((mode(dir.join("3/report.zip")), mode(dir.join("3/meta.txt"))), (0o600, 0o600), "the files are the owner's");
        }
        assert!(save_report(&dir, &received, b"do not replace").unwrap().is_none());
        assert_eq!(fs::read(dir.join("3/report.zip")).unwrap(), zip, "a pulled report is never rewritten");

        let zip = example_zip(false, true);
        save_report(&dir, &entry(4, zip.len()), &zip).unwrap().unwrap();
        assert!(fs::read_to_string(dir.join("4/meta.txt")).unwrap().contains("diff: no\nsuggestion: yes\n"));

        let zip = b"invalid zip";
        let line = save_report(&dir, &entry(5, zip.len()), zip).unwrap().unwrap();
        assert_eq!(line, format!("NEW change report 5: check failed -> {}/", dir.join("5").display()), "no checker text either");
        assert!(folder(&dir, &entry(5, zip.len())).1, "a failed check is still saved");
        assert_eq!(fs::read(dir.join("5/report.zip")).unwrap(), zip);
        assert_eq!(names(&dir.join("5")), ["meta.txt", "report.zip"]);
        assert!(fs::read_to_string(dir.join("5/meta.txt")).unwrap().contains("check: failed\nchanges: unknown\ndiff: unknown\nsuggestion: unknown\n"));

        fs::write(dir.join("6"), "occupied").unwrap();
        assert!(save_report(&dir, &entry(6, zip.len()), zip).is_err());
        assert!(!folder(&dir, &entry(6, zip.len())).1, "a failed rename must not look pulled");
        assert_eq!(fs::read(dir.join(".6.part/report.zip")).unwrap(), zip, "the failed save retains its temporary zip");

        // the inbox gives a removed report's id to the next one: a later report 3 is new
        let first = fs::read(dir.join("3/report.zip")).unwrap();
        let zip = example_zip(false, false);
        let later = Entry { created: received.created + 600, ..entry(3, zip.len()) };
        let line = save_report(&dir, &later, &zip).unwrap().unwrap();
        assert!(line.ends_with(&format!("{}/", dir.join("3-1789908240").display())), "{line}");
        assert!(folder(&dir, &later).1 && folder(&dir, &received).1, "both reports 3 are here");
        assert!(save_report(&dir, &later, &zip).unwrap().is_none());
        assert_eq!(fs::read(dir.join("3/report.zip")).unwrap(), first, "the earlier report 3 is untouched");
    }
}
