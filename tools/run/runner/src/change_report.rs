//! `makepad change-report guide|check|send`: a change report (docs/agents/change-report.md), what
//! someone changed in a Makepad app told as concepts, for sending to Makepad after they read it.
//!
//! A coding agent writes the report folder (report.json, REPORT.md, optionally changes.diff), reads
//! the format with `guide` and checks its work with `check` (the same checks makepad.nl runs, the
//! anonymisation scan included). Sending is the person's: in the Makepad Runner they read it and
//! press Send in the app's panel; elsewhere they run `send` at a terminal, read it there and type
//! the app's id. `send` never sends without a terminal, so an agent's tool call cannot send.

use makepad_change_report as report;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::time::Duration;

/// The format and the rules, for the coding agent that writes a report (`guide`).
const GUIDE: &str = include_str!("../../../../docs/agents/change-report.md");

fn usage() -> i32 {
    eprintln!("usage: makepad change-report guide                     the format and the rules (for the agent writing one)");
    eprintln!("       makepad change-report check <folder|zip>        check a report exactly as makepad.nl will");
    eprintln!("       makepad change-report send <folder|zip> [--reply-to <email>]");
    eprintln!("                                                       the person sends it, at a terminal, after reading it");
    2
}

pub fn main(args: &[String]) -> i32 {
    match (args.first().map(String::as_str), &args[args.len().min(1)..]) {
        (Some("guide"), []) => {
            print!("{GUIDE}");
            0
        }
        (Some("check"), [path]) => match report::load(Path::new(path)) {
            Ok(package) => {
                print!("{}", summary(&package));
                println!("The report passes every check. The person reads it and sends it (an agent never sends it).");
                0
            }
            Err(reason) => {
                eprintln!("makepad change-report: {}", reason);
                1
            }
        },
        (Some("send"), [path, rest @ ..]) => {
            let reply_to = match rest {
                [] => None,
                [flag, address] if flag == "--reply-to" => Some(address.trim().to_string()),
                _ => return usage(),
            };
            match send(Path::new(path), reply_to.as_deref()) {
                Ok(id) => {
                    println!("Sent to Makepad as report {id}. Thank you.");
                    0
                }
                Err(reason) => {
                    eprintln!("makepad change-report: {reason}");
                    1
                }
            }
        }
        _ => usage(),
    }
}

/// What a report holds, in a few lines: the app, its changes by title, what was taken out, the diff.
pub fn summary(package: &report::Package) -> String {
    let r = &package.report;
    let mut out = format!(
        "{} for {} (release {}), {} KB zipped:\n",
        if r.suggestion { "Suggestions" } else { "Changes" },
        r.app,
        r.release,
        package.zip.len().div_ceil(1024)
    );
    for (n, change) in r.changes.iter().enumerate() {
        out.push_str(&format!("  {}. {} ({})\n", n + 1, change.title, change.kind));
    }
    let taken: Vec<String> = r.redactions.iter().map(|x| format!("{} \u{d7} {}", x.what, x.count)).collect();
    out.push_str(&format!("Taken out: {}\n", if taken.is_empty() { "nothing".to_string() } else { taken.join(", ") }));
    out.push_str(&match &package.diff {
        Some(diff) => format!("Code: changes.diff, {} lines (sent only when the person chooses to include it)\n", diff.lines().count()),
        None => "Code: no diff\n".to_string(),
    });
    out
}

/// One line typed at the terminal, trimmed (None at its end).
fn ask(question: &str) -> Option<String> {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

/// `send`: check it, show it to the person at this terminal, ask about the diff (default: leave
/// it out) and for the app's id as the go-ahead, then POST it.
fn send(path: &Path, reply_to: Option<&str>) -> Result<i64, String> {
    if std::env::var_os(report::RUNNER_AGENT_TAB).is_some() {
        return Err("this runs in a Makepad Runner agent tab: the Runner shows the report to the person, who sends it from the app's panel (Send my changes to Makepad). A coding agent never sends a report itself.".into());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err("sending needs the person at a terminal: they run `makepad change-report send <folder>` themselves, read the report and confirm. A coding agent never sends a report.".into());
    }
    let mut package = report::load(path)?;
    println!("{}", package.markdown.trim_end());
    println!();
    print!("{}", summary(&package));
    if let Some(diff) = &package.diff {
        let lines = diff.lines().count();
        let answer = ask(&format!("Include your code (changes.diff, {lines} lines)? Type yes to include it, Enter to send the concepts only: "));
        if answer.as_deref() != Some("yes") {
            package = package.without_diff()?;
            println!("The diff stays here.");
        }
    }
    if let Some(address) = reply_to {
        println!("Makepad may answer you at {address}.");
    }
    let app = package.report.app.clone();
    match ask(&format!("Send this to makepad.nl? Type {app} to send it, anything else to stop: ")) {
        Some(answer) if answer == app => {}
        _ => return Err("not sent".into()),
    }
    let url = report::endpoint(std::env::var("MAKEPAD_FEEDBACK_URL").ok().as_deref());
    let mut headers = vec![(report::FEEDBACK_HEADER, "1"), ("Content-Type", "application/zip")];
    if let Some(address) = reply_to {
        headers.push((report::REPLY_TO_HEADER, address));
    }
    let request = relax_light_http::Request {
        headers: &headers,
        timeout: Duration::from_secs(120),
        max_body: 64 * 1024,
        ..relax_light_http::Request::post(&url, &package.zip)
    };
    let (response, body) = relax_light_http::fetch(&request).map_err(|e| format!("could not reach {url}: {e}"))?;
    report::answer(response.status, &body)
}
