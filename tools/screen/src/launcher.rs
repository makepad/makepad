//! Keyboard session browser. Shell launchers only build and exec this binary.
use crate::{
    client,
    protocol::{self, SessionLocation},
    server::{self, StartOptions},
    theme::Theme,
};
use makepad_strict_json::Value;
use std::{
    ffi::OsString,
    fmt::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    time::{Duration, Instant},
};

#[path = "launcher_io.rs"]
mod terminal_io;

#[derive(Clone)]
struct Session {
    id: String,
    instance: String,
    provider: String,
    cwd: String,
    clients: u64,
    cols: u64,
    rows: u64,
}

impl Session {
    fn parse(value: &Value) -> Option<Self> {
        let id = value.get("session_id")?.as_str()?.to_owned();
        let instance = value.get("instance")?.as_str()?.to_owned();
        if !protocol::valid_session(&id)
            || instance.len() != 32
            || !instance.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        let program = value.get("program")?.as_str()?;
        let identity = format!("{id} {program}").to_lowercase();
        let provider = if identity.contains("codex") {
            "Codex".into()
        } else if identity.contains("claude") || identity.contains("fable") {
            "Claude".into()
        } else {
            Path::new(program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        Some(Self {
            id,
            instance,
            provider,
            cwd: value.get("cwd")?.as_str()?.to_owned(),
            clients: value.get("clients")?.as_u64()?,
            cols: value.get("cols")?.as_u64()?,
            rows: value.get("rows")?.as_u64()?,
        })
    }
}

enum Job {
    List,
    Stop(Session),
}
enum Reply {
    List(Result<Vec<Session>, String>),
    Stop(String, Result<(), String>),
}

struct Worker {
    commands: SyncSender<Job>,
    replies: Receiver<Reply>,
}
impl Worker {
    fn new(state: PathBuf) -> Result<Self, String> {
        let (commands, jobs) = mpsc::sync_channel(1);
        let (results, replies) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("screen-launcher-sessions".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let reply = match job {
                        Job::List => Reply::List(server::list(&state).and_then(|value| {
                            let values = value.as_arr().ok_or("Invalid screen session list")?;
                            values
                                .iter()
                                .map(|v| {
                                    Session::parse(v)
                                        .ok_or_else(|| "Invalid screen session identity".to_owned())
                                })
                                .collect()
                        })),
                        Job::Stop(session) => {
                            let result =
                                server::stop_instance(&state, &session.id, &session.instance)
                                    .map(|_| ());
                            Reply::Stop(session.id, result)
                        }
                    };
                    if results.send(reply).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { commands, replies })
    }
}

#[derive(Clone, Copy)]
enum Provider {
    Codex,
    CodexYolo,
    Claude,
    ClaudeYolo,
    CodexResumeYolo,
    ClaudeResumeYolo,
}
impl Provider {
    const ALL: [Self; 6] = [
        Self::Codex,
        Self::CodexYolo,
        Self::Claude,
        Self::ClaudeYolo,
        Self::CodexResumeYolo,
        Self::ClaudeResumeYolo,
    ];

    fn command(self) -> &'static str {
        match self {
            Self::Codex | Self::CodexYolo | Self::CodexResumeYolo => "codex",
            Self::Claude | Self::ClaudeYolo | Self::ClaudeResumeYolo => "claude",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::CodexYolo | Self::CodexResumeYolo => "Codex --yolo",
            Self::Claude => "Claude",
            Self::ClaudeYolo | Self::ClaudeResumeYolo => "Claude --dangerously-skip-permissions",
        }
    }

    fn is_resume(self) -> bool {
        matches!(self, Self::CodexResumeYolo | Self::ClaudeResumeYolo)
    }

    fn args(self, resume: Option<&str>) -> Result<Vec<OsString>, String> {
        Ok(match self {
            Self::Codex | Self::Claude => Vec::new(),
            Self::CodexYolo => vec!["--yolo".into()],
            Self::ClaudeYolo => vec!["--dangerously-skip-permissions".into()],
            Self::CodexResumeYolo => vec![
                "resume".into(),
                "--yolo".into(),
                resume_token(resume.unwrap_or_default())?.into(),
            ],
            Self::ClaudeResumeYolo => vec![
                "--dangerously-skip-permissions".into(),
                "--resume".into(),
                resume_token(resume.unwrap_or_default())?.into(),
            ],
        })
    }
}

fn resume_token(input: &str) -> Result<&str, String> {
    let token = input.trim();
    if token.len() != 36
        || !token.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err("Paste the session UUID only (not the resume command).".into());
    }
    Ok(token)
}

enum Choice {
    Quit,
    Attach(Session),
    New(Provider, Option<String>),
}
struct Confirmation {
    session: Session,
    yes: bool,
}
struct ResumePrompt {
    provider: Provider,
    token: String,
    overflowed: bool,
    error: String,
}
impl ResumePrompt {
    fn new(provider: Provider) -> Self {
        Self {
            provider,
            token: String::new(),
            overflowed: false,
            error: String::new(),
        }
    }
}
struct Menu {
    sessions: Vec<Session>,
    selected: usize,
    top: usize,
    confirmation: Option<Confirmation>,
    resume: Option<ResumePrompt>,
    pending_stop: Option<Session>,
    busy: bool,
    stopping: bool,
    refresh: Instant,
    message: String,
}
impl Default for Menu {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            top: 0,
            confirmation: None,
            resume: None,
            pending_stop: None,
            busy: false,
            stopping: false,
            refresh: Instant::now(),
            message: "Loading running agents…".into(),
        }
    }
}

pub fn run(state: Option<PathBuf>, cwd: Option<PathBuf>) -> Result<(), String> {
    let cwd = cwd
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?)
        .canonicalize()
        .map_err(|e| format!("Agent working directory: {e}"))?;
    let state = state
        .or_else(|| std::env::var_os("MAKEPAD_SCREEN_STATE_DIR").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|home| PathBuf::from(home).join(".makepad/studio/agent_sessions"))
        })
        .ok_or("Provide --state-dir for the screen launcher")?;
    let state = if state.is_absolute() {
        state
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(state)
    };
    let worker = Worker::new(state.clone())?;
    let mut menu = Menu::default();
    loop {
        let choice = menu.show(&worker, &state, &cwd)?;
        let result = match choice {
            Choice::Quit => return Ok(()),
            Choice::Attach(session) => {
                menu.message = format!("Returned from {}", session.id);
                SessionLocation::open(&state, &session.id, false)
                    .and_then(|location| client::attach(&location, false))
            }
            Choice::New(provider, resume) => (|| {
                let (program, args) = provider_command(provider, resume.as_deref())?;
                let id = format!(
                    "{}-{}",
                    provider.command(),
                    &protocol::random_token()?[..12]
                );
                menu.message = format!("Returned from {id}");
                client::start_attached(
                    StartOptions {
                        state_dir: state.clone(),
                        session_id: id,
                        cwd: cwd.clone(),
                        program,
                        args,
                        cols: 120,
                        rows: 40,
                        theme: Theme::default(),
                    },
                    false,
                )
            })(),
        };
        if let Err(error) = result {
            menu.message = error;
        }
        menu.refresh = Instant::now();
    }
}

fn provider_command(
    provider: Provider,
    resume: Option<&str>,
) -> Result<(OsString, Vec<OsString>), String> {
    let provider_args = provider.args(resume)?;
    let name = provider.command();
    let paths: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    #[cfg(unix)]
    let paths = paths
        .into_iter()
        .chain(std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin")));
    #[cfg(windows)]
    let candidates = [
        format!("{name}.exe"),
        format!("{name}.cmd"),
        format!("{name}.bat"),
    ];
    #[cfg(unix)]
    let candidates = [name.to_owned()];
    for directory in paths {
        for candidate in &candidates {
            let path = directory.join(candidate);
            if !path.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if path
                    .metadata()
                    .map_err(|e| e.to_string())?
                    .permissions()
                    .mode()
                    & 0o111
                    == 0
                {
                    continue;
                }
            }
            #[cfg(windows)]
            if path.extension().is_some_and(|ext| ext != "exe") {
                let mut args = vec!["/d".into(), "/c".into(), name.into()];
                args.extend(provider_args);
                return Ok((
                    std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()),
                    args,
                ));
            }
            return Ok((path.into_os_string(), provider_args));
        }
    }
    Err(format!(
        "{name} is not installed or is not on PATH; no agent was started"
    ))
}

impl Menu {
    fn show(&mut self, worker: &Worker, state: &Path, cwd: &Path) -> Result<Choice, String> {
        let mut console = terminal_io::Console::new()?;
        let result = self.event_loop(&mut console, worker, state, cwd);
        let restored = console.close();
        restored.and(result)
    }

    fn event_loop(
        &mut self,
        console: &mut terminal_io::Console,
        worker: &Worker,
        state: &Path,
        cwd: &Path,
    ) -> Result<Choice, String> {
        let mut keys = Keys::default();
        let mut last_frame = String::new();
        loop {
            if console.interrupted() {
                return Ok(Choice::Quit);
            }
            match worker.replies.try_recv() {
                Ok(Reply::List(result)) => {
                    self.busy = false;
                    match result {
                        Ok(sessions) => {
                            let id = self.sessions.get(self.selected).map(|s| s.id.clone());
                            let creation = (self.message != "Loading running agents…")
                                .then(|| self.selected.checked_sub(self.sessions.len()))
                                .flatten();
                            self.selected = id
                                .and_then(|id| sessions.iter().position(|s| s.id == id))
                                .unwrap_or_else(|| {
                                    creation
                                        .map(|n| sessions.len() + n)
                                        .unwrap_or(self.selected)
                                        .min(sessions.len() + Provider::ALL.len() - 1)
                                });
                            self.sessions = sessions;
                            if self.message == "Loading running agents…" {
                                self.message =
                                    "Select an agent to connect, or start a new one below.".into();
                            }
                        }
                        Err(e) => self.message = format!("Refresh failed: {e}"),
                    }
                    self.refresh = Instant::now() + Duration::from_secs(2);
                }
                Ok(Reply::Stop(id, result)) => {
                    self.busy = false;
                    self.stopping = false;
                    self.message = match result {
                        Ok(()) => format!("Stopped {id}; saved session files retained"),
                        Err(e) => format!("Could not stop {id}: {e}"),
                    };
                    self.refresh = Instant::now();
                }
                Err(TryRecvError::Disconnected) => return Err("Session worker stopped".into()),
                Err(TryRecvError::Empty) => {}
            }
            if !self.busy {
                let job = self
                    .pending_stop
                    .take()
                    .map(Job::Stop)
                    .or_else(|| (Instant::now() >= self.refresh).then_some(Job::List));
                if let Some(job) = job {
                    worker
                        .commands
                        .try_send(job)
                        .map_err(|_| "Session worker is unavailable")?;
                    self.busy = true;
                }
            }
            let size = console.size()?;
            let frame = self.draw(size, state, cwd);
            if frame != last_frame {
                console.write(frame.as_bytes())?;
                last_frame = frame;
            }
            if let Some(input) = console.read()? {
                if input.is_empty() {
                    return Ok(Choice::Quit);
                }
                keys.push(&input);
            }
            while let Some(key) = keys.next(self.resume.is_some()) {
                if size.0 < 44 || size.1 < 12 {
                    if matches!(key, Key::Quit) {
                        return Ok(Choice::Quit);
                    }
                    continue;
                }
                if self.stopping {
                    continue;
                }
                if let Some(prompt) = &mut self.resume {
                    match key {
                        Key::Text(ch) => {
                            if prompt.token.len() < 128 {
                                prompt.token.push(ch);
                            } else {
                                prompt.overflowed = true;
                            }
                            prompt.error.clear();
                        }
                        Key::Delete => {
                            prompt.token.pop();
                            prompt.error.clear();
                            if prompt.token.is_empty() {
                                prompt.overflowed = false;
                            }
                        }
                        Key::Clear => *prompt = ResumePrompt::new(prompt.provider),
                        Key::Enter => {
                            if prompt.overflowed {
                                prompt.error = "Token too long. Ctrl+U clears the field.".into();
                            } else {
                                match resume_token(&prompt.token) {
                                    Ok(token) => {
                                        let token = token.to_owned();
                                        let provider = prompt.provider;
                                        self.resume = None;
                                        return Ok(Choice::New(provider, Some(token)));
                                    }
                                    Err(error) => prompt.error = error,
                                }
                            }
                        }
                        Key::Quit => {
                            self.resume = None;
                            keys.clear();
                            break;
                        }
                        _ => {}
                    }
                    continue;
                }
                if let Some(confirm) = &mut self.confirmation {
                    match key {
                        Key::Up | Key::Down | Key::Tab => confirm.yes = !confirm.yes,
                        Key::Enter | Key::Yes => {
                            let confirm = self.confirmation.take().unwrap();
                            if confirm.yes || matches!(key, Key::Yes) {
                                self.message = format!("Stopping {}…", confirm.session.id);
                                self.pending_stop = Some(confirm.session);
                                self.stopping = true;
                            }
                            keys.clear();
                            break;
                        }
                        Key::Quit | Key::No => {
                            self.confirmation = None;
                            keys.clear();
                            break;
                        }
                        _ => {}
                    }
                    continue;
                }
                match key {
                    Key::Up => self.selected = self.selected.saturating_sub(1),
                    Key::Down => {
                        self.selected =
                            (self.selected + 1).min(self.sessions.len() + Provider::ALL.len() - 1)
                    }
                    Key::Enter => {
                        if let Some(session) = self.sessions.get(self.selected) {
                            return Ok(Choice::Attach(session.clone()));
                        }
                        if let Some(provider) =
                            Provider::ALL.get(self.selected - self.sessions.len())
                        {
                            if provider.is_resume() {
                                self.resume = Some(ResumePrompt::new(*provider));
                                keys.clear();
                                break;
                            }
                            return Ok(Choice::New(*provider, None));
                        }
                    }
                    Key::Codex => return Ok(Choice::New(Provider::Codex, None)),
                    Key::Claude => return Ok(Choice::New(Provider::Claude, None)),
                    Key::Delete => {
                        if let Some(session) = self.sessions.get(self.selected) {
                            self.confirmation = Some(Confirmation {
                                session: session.clone(),
                                yes: false,
                            });
                            keys.clear();
                            break;
                        }
                    }
                    Key::Refresh => self.refresh = Instant::now(),
                    Key::Quit => return Ok(Choice::Quit),
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn draw(&mut self, (cols, rows): (usize, usize), state: &Path, cwd: &Path) -> String {
        let mut out = String::from("\x1b[?2026h\x1b[H\x1b[0m\x1b[2J");
        if cols < 44 || rows < 12 {
            line(
                &mut out,
                1,
                cols,
                "",
                "Make the terminal at least 44 × 12 to browse agents.",
            );
            out.push_str("\x1b[?2026l");
            return out;
        }
        line(
            &mut out,
            1,
            cols,
            "\x1b[1m",
            &format!(
                "Agents   {} running{}",
                self.sessions.len(),
                if self.stopping { " · stopping" } else { "" }
            ),
        );
        line(&mut out, 2, cols, "\x1b[2m", &state.display().to_string());
        line(
            &mut out,
            4,
            cols,
            "\x1b[2m",
            "   SESSION                AGENT      VIEWS   SIZE       DIRECTORY",
        );
        let visible = rows - 9;
        if self.selected < self.top {
            self.top = self.selected;
        }
        if self.selected >= self.top + visible {
            self.top = self.selected + 1 - visible;
        }
        for (row, index) in (self.top..self.sessions.len() + Provider::ALL.len())
            .take(visible)
            .enumerate()
        {
            let selected = index == self.selected;
            let marker = if selected { ">" } else { " " };
            let text = if let Some(session) = self.sessions.get(index) {
                format!(
                    "{marker}  {} {} {:>3}    {:>3}×{:<3}  {}",
                    column(&session.id, 22),
                    column(&session.provider, 10),
                    session.clients,
                    session.cols,
                    session.rows,
                    session.cwd
                )
            } else {
                let provider = Provider::ALL[index - self.sessions.len()];
                if provider.is_resume() {
                    format!("{marker}  Resume {} (paste resume token)", provider.label())
                } else {
                    format!("{marker}  + New {}", provider.label())
                }
            };
            line(
                &mut out,
                row + 5,
                cols,
                if selected { "\x1b[7m" } else { "" },
                &text,
            );
        }
        if let Some(session) = self.sessions.get(self.selected) {
            line(&mut out, rows - 4, cols, "\x1b[2m", &session.id);
        }
        line(
            &mut out,
            rows - 3,
            cols,
            "\x1b[2m",
            &format!("New agents start in {}", cwd.display()),
        );
        line(&mut out, rows - 2, cols, "", &self.message);
        line(
            &mut out,
            rows,
            cols,
            "\x1b[2m",
            "↑↓ move · Enter select · Del stop · C Codex · A Claude · R reload · Q quit",
        );
        if let Some(confirm) = &self.confirmation {
            let y = rows / 2;
            line(
                &mut out,
                y - 1,
                cols,
                "\x1b[1m",
                &format!(
                    "Stop {} ({})?",
                    confirm.session.id, confirm.session.provider
                ),
            );
            line(
                &mut out,
                y,
                cols,
                "",
                "Ends this agent and disconnects its views. Saved files remain.",
            );
            line(
                &mut out,
                y + 1,
                cols,
                "",
                if confirm.yes {
                    "  Keep running     > STOP SESSION"
                } else {
                    "> Keep running       Stop session"
                },
            );
            line(
                &mut out,
                y + 2,
                cols,
                "\x1b[2m",
                "↑↓ choose · Enter confirm · Escape cancel",
            );
        }
        if let Some(prompt) = &self.resume {
            let y = rows / 2;
            line(
                &mut out,
                y - 1,
                cols,
                "\x1b[1m",
                &format!("Resume {}", prompt.provider.label()),
            );
            line(
                &mut out,
                y,
                cols,
                "",
                "Paste the session UUID of the conversation to resume.",
            );
            let visible = cols.saturating_sub(5);
            let start = prompt.token.len().saturating_sub(visible);
            line(
                &mut out,
                y + 1,
                cols,
                "\x1b[7m",
                &format!("> {}_", &prompt.token[start..]),
            );
            line(&mut out, y + 2, cols, "", &prompt.error);
            line(
                &mut out,
                y + 3,
                cols,
                "\x1b[2m",
                "Enter resume · Escape cancel · Ctrl+U clear",
            );
        }
        out.push_str("\x1b[0m\x1b[?2026l");
        out
    }
}

fn column(text: &str, cols: usize) -> String {
    let mut result = String::new();
    let mut width = 0;
    for ch in text.chars().filter(|c| !c.is_control()) {
        let advance = usize::from(crate::term::unicode::char_width(ch as u32));
        if width + advance > cols {
            break;
        }
        result.push(ch);
        width += advance;
    }
    result.extend(std::iter::repeat(' ').take(cols - width));
    result
}

fn line(out: &mut String, row: usize, cols: usize, style: &str, text: &str) {
    let _ = write!(out, "\x1b[{row};1H\x1b[0m\x1b[2K{style}");
    let mut width = 0;
    for ch in text.chars().filter(|c| !c.is_control()) {
        let advance = usize::from(crate::term::unicode::char_width(ch as u32));
        if width + advance >= cols {
            break;
        }
        out.push(ch);
        width += advance;
    }
    out.push_str("\x1b[0m");
}

#[derive(Clone, Copy)]
enum Key {
    Up,
    Down,
    Tab,
    Enter,
    Delete,
    Codex,
    Claude,
    Refresh,
    Quit,
    Yes,
    No,
    Text(char),
    Clear,
    Ignore,
}
#[derive(Default)]
struct Keys {
    bytes: Vec<u8>,
    escape: Option<Instant>,
    paste: bool,
}
impl Keys {
    fn push(&mut self, bytes: &[u8]) {
        if self.bytes.len() + bytes.len() <= 8192 {
            self.bytes.extend_from_slice(bytes);
        }
    }
    fn clear(&mut self) {
        self.bytes.clear();
        self.escape = None;
    }
    fn next(&mut self, text_input: bool) -> Option<Key> {
        let byte = *self.bytes.first()?;
        let (count, key) = if byte == 27 {
            let started = *self.escape.get_or_insert_with(Instant::now);
            if self.bytes.len() == 1 {
                if started.elapsed() < Duration::from_millis(35) {
                    return None;
                }
                (1, Key::Quit)
            } else if matches!(self.bytes[1], b'[' | b'O') {
                if let Some(end) = self
                    .bytes
                    .iter()
                    .enumerate()
                    .skip(2)
                    .find(|(_, b)| (0x40..=0x7e).contains(*b))
                    .map(|(i, _)| i)
                {
                    let sequence = &self.bytes[..=end];
                    let key = match sequence {
                        b"\x1b[A" | b"\x1bOA" => Key::Up,
                        b"\x1b[B" | b"\x1bOB" => Key::Down,
                        b"\x1b[3~" => Key::Delete,
                        b"\x1b[200~" => {
                            self.paste = true;
                            Key::Ignore
                        }
                        b"\x1b[201~" => {
                            self.paste = false;
                            Key::Ignore
                        }
                        _ => Key::Ignore,
                    };
                    (end + 1, key)
                } else if b"\x1b[200~".starts_with(&self.bytes)
                    || b"\x1b[201~".starts_with(&self.bytes)
                    || started.elapsed() < Duration::from_millis(35)
                {
                    // Paste delimiters may arrive across reads, especially
                    // over SSH. Do not turn a delayed delimiter into text.
                    return None;
                } else {
                    (self.bytes.len(), Key::Ignore)
                }
            } else {
                (2, Key::Ignore)
            }
        } else {
            (
                1,
                match byte {
                    b' '..=b'~' if text_input => Key::Text(byte as char),
                    b'\r' | b'\n' | b'\t' if text_input && self.paste => Key::Text(' '),
                    21 if text_input => Key::Clear,
                    b'\r' | b'\n' => Key::Enter,
                    b'\t' => Key::Tab,
                    8 | 127 => Key::Delete,
                    3 | 4 | b'q' | b'Q' => Key::Quit,
                    b'c' | b'C' => Key::Codex,
                    b'a' | b'A' => Key::Claude,
                    b'r' | b'R' => Key::Refresh,
                    b'y' | b'Y' => Key::Yes,
                    b'n' | b'N' => Key::No,
                    _ => Key::Ignore,
                },
            )
        };
        self.bytes.drain(..count);
        self.escape = None;
        Some(if self.paste && !matches!(key, Key::Text(_)) {
            Key::Ignore
        } else {
            key
        })
    }
}
