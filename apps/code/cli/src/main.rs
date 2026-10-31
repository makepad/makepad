//! `makepad-code`: Makepad Code in a terminal.
//!
//! ```text
//! makepad-code [DIR] [--root DIR] [--approve ask|allowlist|auto] [--context N]
//!              [--ref PATH]... [--resume [ID|last]]
//! makepad-code -p "task" [same options]     one task, streamed, then exit
//! ```
//!
//! Interactive when stdin and stdout are a terminal: a prompt at the
//! bottom, the conversation scrolling above it. Otherwise (pipes, or `-p`)
//! it reads lines from stdin: prompts, and answers to approvals and
//! questions. Sessions are the window's (`~/.makepad/code/sessions`).
//!
//! Modules: `term` (raw mode, size, signals, keys), `line` (the prompt's
//! editor), `render` (style, screen, rows), `text` (widths, wrapping),
//! `app` (the session: events, keys, slash commands).

mod app;
mod line;
mod render;
mod term;
mod text;

use makepad_code_agent::event::{AgentConfig, ApprovalMode};
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "usage: makepad-code [DIR] [--root DIR] [--approve ask|allowlist|auto] [--context N]
                    [--ref PATH]... [--resume [ID|last]] [-p TASK...]

  -p, --print TASK   run one task, print what happens, exit (no TASK: read stdin)
  --root DIR         the project folder (default: DIR or the current folder)
  --approve MODE     ask: everything asks; allowlist: risky actions ask (default); auto: nothing asks
  --context N        context window in tokens (default 16384)
  --ref PATH         attach a file to the first request (repeatable)
  --resume [ID]      continue a saved session (no ID or 'last': the newest)

In the session: /help lists the commands. Esc or Ctrl+C interrupts; Ctrl+D or
Ctrl+C twice at an empty prompt exits. CODE_MOCK=1 runs a scripted agent.";

pub struct Options {
    pub config: AgentConfig,
    pub print: Option<String>,
    pub refs: Vec<String>,
    pub resume: Option<String>,
}

fn parse_args() -> Result<Options, String> {
    let mut root: Option<PathBuf> = None;
    let mut approval = ApprovalMode::Allowlist;
    let mut context_tokens = 16384u32;
    let mut refs = Vec::new();
    let mut resume = None;
    let mut print: Option<Vec<String>> = None;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--root" => root = Some(args.next().ok_or("--root needs a folder")?.into()),
            "--approve" => {
                approval = app::parse_mode(&args.next().unwrap_or_default()).ok_or("--approve is ask, allowlist or auto")?
            }
            "--context" => {
                context_tokens = args.next().and_then(|v| app::parse_tokens(&v)).ok_or("--context needs a size, e.g. 32768 or 32k")?
            }
            "--ref" => refs.push(args.next().ok_or("--ref needs a path")?),
            "--resume" => {
                resume = Some(match args.peek() {
                    Some(v) if !v.starts_with('-') => args.next().unwrap_or_default(),
                    _ => "last".into(),
                })
            }
            "-p" | "--print" => print = Some(Vec::new()),
            other if other.starts_with('-') && other.len() > 1 => return Err(format!("unknown option {other}")),
            other => match &mut print {
                Some(words) => words.push(other.to_string()),
                None if root.is_none() => root = Some(other.into()),
                None => return Err(format!("unexpected argument {other} (a task needs -p)")),
            },
        }
    }
    let root = root.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    let print = match print {
        Some(words) if words.is_empty() => {
            let mut task = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut task).map_err(|e| e.to_string())?;
            Some(task.trim().to_string())
        }
        Some(words) => Some(words.join(" ")),
        None => None,
    };
    if print.as_deref() == Some("") {
        return Err("-p needs a task".into());
    }
    Ok(Options {
        config: AgentConfig {
            root: root.canonicalize().unwrap_or(root),
            context_tokens,
            model: None,
            approval,
            session_dir: home().map(|h| h.join(".makepad/code/sessions")),
        },
        print,
        refs,
        resume,
    })
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

fn main() {
    let options = match parse_args() {
        Ok(o) => o,
        Err(why) => {
            if !why.is_empty() {
                eprintln!("makepad-code: {why}");
            }
            eprintln!("{USAGE}");
            std::process::exit(if why.is_empty() { 0 } else { 2 });
        }
    };
    let interactive = options.print.is_none() && term::is_tty(0) && term::is_tty(1);
    let code = if interactive { run_interactive(options) } else { run_lines(options) };
    std::process::exit(code);
}

/// The full session in a terminal: raw keys, the live prompt.
fn run_interactive(options: Options) -> i32 {
    // The agent's and the hub's diagnostics go to a log, not over the prompt.
    let dir = home().map(|h| h.join(".makepad/code"));
    let old_stderr = dir.as_ref().and_then(|d| {
        let _ = std::fs::create_dir_all(d);
        term::redirect_stderr(&d.join("cli.log"))
    });
    let Some(raw) = term::RawMode::enable(0) else {
        eprintln!("makepad-code: cannot put the terminal in raw mode");
        return 1;
    };
    install_panic_hook(old_stderr);
    term::install_signals();
    // Bracketed paste on: a multi-line paste arrives as one piece.
    print!("\x1b[?2004h");
    let style = render::Style::detect(true);
    let history = dir.map(|d| d.join("cli-history"));
    let mut app = app::App::new(options, render::Screen::new(style, true), Some(history).flatten());
    let mut decoder = term::Decoder::default();
    let mut quiet_since = std::time::Instant::now();
    while !app.quit {
        if term::terminated() {
            break;
        }
        if term::take_interrupted() {
            app.ctrl_c();
        }
        if term::take_resized() {
            app.resized();
        }
        let wait = if decoder.pending() { Duration::from_millis(15) } else { Duration::from_millis(40) };
        match term::read_input(0, wait) {
            None => break,
            Some(bytes) if bytes.is_empty() => {
                if decoder.pending() && quiet_since.elapsed() >= Duration::from_millis(15) {
                    for key in decoder.flush() {
                        app.key(key);
                    }
                }
            }
            Some(bytes) => {
                quiet_since = std::time::Instant::now();
                for key in decoder.feed(&bytes) {
                    app.key(key);
                }
            }
        }
        app.pump();
        app.draw();
    }
    app.finish();
    print!("\x1b[?2004l");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    drop(raw);
    0
}

/// Lines in, plain rows out: pipes, scripts and `-p`.
fn run_lines(options: Options) -> i32 {
    let tty_out = term::is_tty(1);
    let style = render::Style::detect(tty_out);
    let print = options.print.is_some();
    let mut app = app::App::new(options, render::Screen::new(style, false), None);
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(None);
                    return;
                }
                Ok(_) => {
                    if tx.send(Some(line.trim_end_matches(['\n', '\r']).to_string())).is_err() {
                        return;
                    }
                }
            }
        }
    });
    term::install_signals();
    let mut eof = false;
    let mut queued: std::collections::VecDeque<String> = Default::default();
    loop {
        if term::terminated() {
            break;
        }
        if term::take_interrupted() {
            if app.busy() {
                app.ctrl_c();
            } else {
                break;
            }
        }
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Some(line) => queued.push_back(line),
                None => eof = true,
            }
        }
        app.pump();
        if app.wants_line() {
            match queued.pop_front() {
                Some(line) => app.line(&line),
                None if eof => app.no_more_input(),
                None => {}
            }
        }
        app.draw();
        if app.quit || (!app.busy() && !app.waiting() && ((print && app.done_one()) || (!print && eof && queued.is_empty()))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let code = if app.failed() { 1 } else { 0 };
    app.finish();
    code
}

/// A panic puts the terminal back before the message.
fn install_panic_hook(old_stderr: Option<i32>) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        term::restore();
        print!("\x1b[?2004l\x1b[?25h\r\n");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        if let Some(fd) = old_stderr {
            use std::os::unix::io::FromRawFd;
            let mut err = unsafe { std::fs::File::from_raw_fd(fd) };
            let _ = writeln!(err, "makepad-code: {info}");
            std::mem::forget(err);
        }
        default(info);
    }));
}
