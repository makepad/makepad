//! Command-line cache builder and search client, using the same worker as Mail.
use makepad_mail::{
    mail_worker::{MailWorker, Reply, Request},
    source::LocalConfig,
};
use makepad_widgets::Cx;
use std::{path::PathBuf, sync::atomic::Ordering, time::Duration};
fn main() {
    if let Err(error) = run() {
        eprintln!("mail-index: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.iter().any(|a| a == "--help") {
        println!("mail-index [--mail-root DIR] [--mail-cache FILE] [--query QUERY] [--limit N]\nBuilds the local search cache; --query prints matching message metadata. Never modifies source mail.");
        return Ok(());
    }
    let arg = |key| {
        args.iter()
            .position(|a| a == key)
            .and_then(|i| args.get(i + 1))
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unavailable; use the Mail application")?;
    let config = LocalConfig::for_mail_root(
        arg("--mail-root").map(PathBuf::from).unwrap_or_else(|| home.join("Library/Mail")),
        arg("--mail-cache").map(PathBuf::from),
    );
    // This is a command-line process: no window or UI event loop is created.
    let mut cx = Cx::new(Box::new(|_, _| {}));
    let worker = MailWorker::spawn(&mut cx, config)?;
    let mut done = false;
    loop {
        match worker
            .rx
            .recv_timeout(Duration::from_secs(120))
            .map_err(|e| e.to_string())?
        {
            Reply::Error(error) => return Err(error),
            Reply::QueryError {
                generation: 1,
                message,
            } if done => return Err(message),
            Reply::ScanDone(status) => {
                println!("{status}");
                done = true;
                worker.generation.store(1, Ordering::Release);
                let query = Request::Search {
                    generation: 1,
                    text: arg("--query").cloned().unwrap_or_default(),
                    limit: arg("--limit").and_then(|s| s.parse().ok()).unwrap_or(20),
                };
                worker.send(query).map_err(|_| "Search queue is full")?;
            }
            Reply::Results {
                generation: 1,
                page,
            } if done => {
                println!("{} matches in {:.2} ms", page.total, page.elapsed_ms);
                if arg("--query").is_some() {
                    for row in &page.hits {
                        let json = makepad_strict_json::obj(vec![
                            ("id", makepad_strict_json::Value::Int(row.id as i64)),
                            (
                                "subject",
                                makepad_strict_json::Value::Str(row.subject.clone()),
                            ),
                            ("from", makepad_strict_json::Value::Str(row.from.clone())),
                            (
                                "preview",
                                makepad_strict_json::Value::Str(row.preview.clone()),
                            ),
                            (
                                "incomplete",
                                makepad_strict_json::Value::Bool(row.incomplete),
                            ),
                        ]);
                        println!("{}", json.to_json());
                    }
                }
                return Ok(());
            }
            _ => {}
        }
    }
}
