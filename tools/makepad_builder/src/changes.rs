//! "Send my changes": a person who changed an app's source with their
//! coding agent sends Makepad a change report, the changes told as
//! concepts (docs/agents/change-report.md), after reading it.
//!
//! The agent writes the report into `builder/changes/<app>-report/`
//! (report.json, REPORT.md, optionally changes.diff) or as
//! `builder/changes/<app>-report.zip`; `makepad-builder changes APP` gives
//! it the diff to describe. The Builder checks the report completely
//! (`makepad_change_report::open`: schema, sizes, text only, the
//! anonymisation scan), shows REPORT.md, and only on an explicit yes sends
//! the zip to the feedback endpoint (`POST /api/feedback/changes`, or
//! `MAKEPAD_FEEDBACK_URL` + `/changes`). The address the Builder knows goes
//! along only when the person chooses so. A sent report is renamed
//! `<app>-report-sent-<id>` so it is not offered again.
use crate::catalog::{self, Release};
use crate::curl_http::{self, Limits, Request};
use makepad_change_report as report;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use report::Package;

/// The app's release as it is installed here (the one its sources were
/// checked out for), else the one the catalog last offered.
pub fn release(root: &Path, app: &str) -> Option<Release> {
    let load = |path: PathBuf| -> Option<Release> {
        Release::parse(&makepad_strict_json::parse(&fs::read(path).ok()?).ok()?).ok()
    };
    load(root.join("installed").join(format!("{app}.json"))).or_else(|| load(root.join("available").join(format!("{app}.json"))))
}

/// Where the agent leaves the report for `app`: the folder, else the zip.
pub fn waiting(root: &Path, app: &str) -> Option<PathBuf> {
    let changes = root.join("changes");
    [changes.join(format!("{app}-report")), changes.join(format!("{app}-report.zip"))]
        .into_iter()
        .find(|path| path.exists())
}

/// Load and check a report for `app` (a folder or a zip).
pub fn load(path: &Path, app: &str) -> Result<Package, String> {
    let package = report::load(path)?;
    if package.report.app != app {
        return Err(format!("This report is about {}, not {app}", package.report.app));
    }
    Ok(package)
}

/// Send a checked report; returns the id the server gave it. `email` only
/// when the person chose to include it.
pub fn send(package: &Package, email: Option<&str>) -> Result<i64, String> {
    send_to(&report::endpoint(std::env::var("MAKEPAD_FEEDBACK_URL").ok().as_deref()), package, email)
}

/// [`send`] to a given endpoint.
pub fn send_to(url: &str, package: &Package, email: Option<&str>) -> Result<i64, String> {
    let mut request = Request::post(url)
        .limits(Limits { max_body_bytes: 64 * 1024, total_timeout: Duration::from_secs(60), ..Limits::default() })
        .header("Content-Type", "application/zip")?
        .header(report::FEEDBACK_HEADER, "1")?
        .body(package.zip.clone());
    if let Some(email) = email.filter(|e| !e.is_empty()) {
        request = request.header(report::REPLY_TO_HEADER, email)?;
    }
    let response = curl_http::request_no_redirect(request).map_err(|e| format!("Could not reach {url}: {e}"))?;
    match response.status {
        200 => report::response_id(&response.body),
        status => Err(match report::response_id(&response.body) {
            Err(reason) if !reason.starts_with("The server's answer") => format!("Not sent: {reason}"),
            _ => format!("Not sent: the server answered {status}"),
        }),
    }
}

/// After sending, the report is kept as `<app>-report-sent-<id>` (with its
/// `.zip` when it was one), so the Builder does not offer it again.
pub fn mark_sent(root: &Path, path: &Path, id: i64) -> Result<PathBuf, String> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or("Unnamed report")?;
    let sent = match name.strip_suffix(".zip") {
        Some(stem) => format!("{stem}-sent-{id}.zip"),
        None => format!("{name}-sent-{id}"),
    };
    let to = path.with_file_name(sent);
    // Only reports inside the Builder's own changes/ folder are renamed.
    if path.parent().is_some_and(|parent| parent == root.join("changes")) {
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        return Ok(to);
    }
    Ok(path.to_path_buf())
}

/// One line per file of the zip, for the review.
pub fn contents(package: &Package) -> String {
    let mut files = vec![report::REPORT_JSON, report::REPORT_MD];
    if package.diff.is_some() {
        files.push(report::CHANGES_DIFF);
    }
    format!("{} ({} KB zipped)", files.join(", "), package.zip.len().div_ceil(1024))
}

/// `makepad-builder changes APP [--files]`: the edits in APP's source
/// snapshot as a unified diff (or with --files one "M|A|D path" line per
/// file), for a coding agent writing a change report. Paths are relative
/// to the snapshot (makepad/…, makepad/apps/<repository>/…).
pub fn cli_changes() -> Result<(), String> {
    let mut app = None;
    let mut files = false;
    for arg in std::env::args().skip(2) {
        match arg.as_str() {
            "--files" => files = true,
            _ if app.is_none() && catalog::identifier(&arg) => app = Some(arg),
            _ => return Err(format!("Usage: makepad-builder changes APP [--files] (not {arg})")),
        }
    }
    let app = app.ok_or("Usage: makepad-builder changes APP [--files]")?;
    let root = crate::validate_install_root(&crate::default_root())?;
    let release = release(&root, &app).ok_or_else(|| format!("{app} is not in this installation"))?;
    if !release.installed(&root) {
        return Err(format!("The sources of {} are not downloaded yet", release.title));
    }
    let (diff, list) = catalog::current_changes(&root, &release)?;
    if list.is_empty() {
        eprintln!("No changes in the {} source ({}).", release.title, release.release);
        return Ok(());
    }
    print!("{}", if files { list } else { diff });
    Ok(())
}

/// `makepad-builder send-changes APP [PATH] [--yes] [--with-email]`: check
/// a change report (PATH, else `builder/changes/APP-report[.zip]`) and show
/// it; with `--yes`, given only after the person approved exactly this
/// list, send it and print its id. `--with-email` includes the email this
/// installation is logged in with (the agent never sees it).
pub fn cli_send() -> Result<(), String> {
    let mut app = None;
    let mut path = None;
    let mut yes = false;
    let mut with_email = false;
    for arg in std::env::args().skip(2) {
        match arg.as_str() {
            "--yes" => yes = true,
            "--with-email" => with_email = true,
            _ if app.is_none() && catalog::identifier(&arg) => app = Some(arg),
            _ if app.is_some() && path.is_none() => path = Some(PathBuf::from(arg)),
            _ => return Err(format!("Usage: makepad-builder send-changes APP [PATH] [--yes] [--with-email] (not {arg})")),
        }
    }
    let app = app.ok_or("Usage: makepad-builder send-changes APP [PATH] [--yes] [--with-email]")?;
    let root = crate::validate_install_root(&crate::default_root())?;
    let path = match path {
        Some(path) => path,
        None => waiting(&root, &app).ok_or_else(|| format!("No report for {app}: write it to {} first", crate::shown(&root.join("changes").join(format!("{app}-report")))))?,
    };
    let package = load(&path, &app)?;
    println!("{}", package.markdown.trim_end());
    println!("\n---\nWould send: {}", contents(&package));
    let email = if with_email {
        let email = makepad_loader_bundle::load_from(&root).ok().flatten().map(|b| b.email).filter(|e| !e.is_empty());
        if email.is_none() {
            return Err("This installation is not logged in, so there is no email to include".into());
        }
        println!("With the email this installation is logged in with, so Makepad can reply.");
        email
    } else {
        println!("Anonymously: no email or account goes with it.");
        None
    };
    if !yes {
        println!("Nothing was sent. Once the person has read and approved this list, run again with --yes.");
        return Ok(());
    }
    let id = send(&package, email.as_deref())?;
    let kept = mark_sent(&root, &path, id)?;
    println!("Sent. Makepad received report #{id}. Kept here as {}.", crate::shown(&kept));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn package() -> Package {
        let json = r#"{"format":"makepad-change-report","version":1,"app":{"id":"calculator","release":"r1"},
            "changes":[{"title":"Larger result text","kind":"ui","what":"The result uses a larger font."}],
            "redactions":[],"diff":false}"#;
        let markdown = report::parse_report(json.as_bytes()).unwrap().to_markdown();
        let mut zip = makepad_zip_file::ZipWriter::new();
        zip.add(report::REPORT_JSON, json.as_bytes(), makepad_zip_file::ZipMethod::Deflate).unwrap();
        zip.add(report::REPORT_MD, markdown.as_bytes(), makepad_zip_file::ZipMethod::Deflate).unwrap();
        report::open(&zip.finish().unwrap()).unwrap()
    }

    /// A one-request server: returns what it received, answers `status` and `body`.
    fn serve(status: &'static str, body: &'static str) -> (String, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api/feedback/changes", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buffer = [0u8; 65536];
            let (head, length) = loop {
                let n = stream.read(&mut buffer).unwrap();
                data.extend_from_slice(&buffer[..n]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&data[..end]).into_owned();
                    let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap();
                    data.drain(..end + 4);
                    break (head, length);
                }
            };
            while data.len() < length {
                let n = stream.read(&mut buffer).unwrap();
                data.extend_from_slice(&buffer[..n]);
            }
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            (head, data)
        });
        (url, handle)
    }

    #[test]
    fn a_report_is_posted_as_the_zip_with_the_address_only_when_chosen() {
        let package = package();
        let (url, server) = serve("200 OK", "{\"ok\":true,\"id\":7}");
        assert_eq!(send_to(&url, &package, None), Ok(7));
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /api/feedback/changes HTTP/1.1"), "{head}");
        assert!(head.contains("Content-Type: application/zip"), "{head}");
        assert!(head.contains("X-Makepad-Feedback: 1"), "{head}");
        assert!(!head.contains("X-Makepad-Reply-To"), "{head}");
        assert_eq!(body, package.zip);

        let (url, server) = serve("200 OK", "{\"ok\":true,\"id\":8}");
        assert_eq!(send_to(&url, &package, Some("someone@example.com")), Ok(8));
        assert!(server.join().unwrap().0.contains("X-Makepad-Reply-To: someone@example.com"));

        let (url, server) = serve("422 Unprocessable Content", "{\"error\":\"The report lists no changes\"}");
        assert_eq!(send_to(&url, &package, None), Err("Not sent: The report lists no changes".into()));
        server.join().unwrap();
        let (url, server) = serve("502 Bad Gateway", "<html>");
        assert_eq!(send_to(&url, &package, None), Err("Not sent: the server answered 502".into()));
        server.join().unwrap();
    }

    #[test]
    fn a_sent_report_is_renamed_only_inside_changes() {
        let root = std::env::temp_dir().join(format!("makepad-builder-changes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("changes").join("calculator-report")).unwrap();
        fs::write(root.join("changes").join("scope-report.zip"), package().zip).unwrap();
        assert_eq!(waiting(&root, "calculator"), Some(root.join("changes").join("calculator-report")));
        let kept = mark_sent(&root, &root.join("changes").join("calculator-report"), 3).unwrap();
        assert_eq!(kept, root.join("changes").join("calculator-report-sent-3"));
        assert_eq!(waiting(&root, "calculator"), None);
        let kept = mark_sent(&root, &root.join("changes").join("scope-report.zip"), 4).unwrap();
        assert_eq!(kept, root.join("changes").join("scope-report-sent-4.zip"));
        let elsewhere = root.join("report.zip");
        fs::write(&elsewhere, b"x").unwrap();
        assert_eq!(mark_sent(&root, &elsewhere, 5).unwrap(), elsewhere);
        let _ = fs::remove_dir_all(&root);
    }
}
