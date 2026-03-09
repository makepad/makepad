#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
mod native {
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

static LOG_FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

fn init_log() {
    let path = std::env::var("TUI_LOG").unwrap_or_else(|_| "/tmp/tui_test.log".into());
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .expect("open log");
    LOG_FILE.set(Mutex::new(f)).ok();
}

macro_rules! tlog {
    ($($arg:tt)*) => {
        if let Some(m) = LOG_FILE.get() {
            if let Ok(mut f) = m.lock() {
                let _ = writeln!(f, $($arg)*);
                let _ = f.flush();
            }
        }
    };
}

// ─── Terminal size via ioctl ─────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

unsafe extern "C" {
    fn ioctl(fd: i32, request: usize, ...) -> i32;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const TIOCGWINSZ: usize = 0x5413;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
const TIOCGWINSZ: usize = 0x4008_7468;

fn terminal_size(fd: RawFd) -> (usize, usize) {
    let mut ws = WinSize::default();
    if unsafe { ioctl(fd, TIOCGWINSZ, &mut ws) } == 0 && ws.ws_row > 0 && ws.ws_col > 0 {
        (ws.ws_row as usize, ws.ws_col as usize)
    } else {
        (24, 80)
    }
}

// ─── Raw terminal mode (termios) ────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod raw_term {
    use std::os::fd::RawFd;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
        pub c_iflag: u64,
        pub c_oflag: u64,
        pub c_cflag: u64,
        pub c_lflag: u64,
        pub c_cc: [u8; 20],
        pub c_ispeed: u64,
        pub c_ospeed: u64,
    }

    unsafe extern "C" {
        fn tcgetattr(fd: i32, t: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, a: i32, t: *const Termios) -> i32;
    }

    pub fn get(fd: RawFd) -> Termios {
        unsafe {
            let mut t: Termios = std::mem::zeroed();
            tcgetattr(fd, &mut t);
            t
        }
    }

    pub fn set(fd: RawFd, t: &Termios) {
        unsafe {
            tcsetattr(fd, 0, t);
        }
    }

    pub fn make_raw(t: &Termios) -> Termios {
        let mut r = *t;
        r.c_iflag &= !(0x0000_0100 | 0x0000_0040 | 0x0000_0080 | 0x0000_0200);
        r.c_lflag &= !(0x0000_0008 | 0x0000_0100 | 0x0000_0080 | 0x0000_0400);
        r.c_cc[16] = 1;
        r.c_cc[17] = 0;
        r
    }
}

#[cfg(target_os = "linux")]
mod raw_term {
    use std::os::fd::RawFd;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
        pub c_iflag: u32,
        pub c_oflag: u32,
        pub c_cflag: u32,
        pub c_lflag: u32,
        pub c_line: u8,
        pub c_cc: [u8; 32],
        pub c_ispeed: u32,
        pub c_ospeed: u32,
    }

    unsafe extern "C" {
        fn tcgetattr(fd: i32, t: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, a: i32, t: *const Termios) -> i32;
    }

    pub fn get(fd: RawFd) -> Termios {
        unsafe {
            let mut t: Termios = std::mem::zeroed();
            tcgetattr(fd, &mut t);
            t
        }
    }

    pub fn set(fd: RawFd, t: &Termios) {
        unsafe {
            tcsetattr(fd, 0, t);
        }
    }

    pub fn make_raw(t: &Termios) -> Termios {
        let mut r = *t;
        r.c_iflag &= !(0x0000_0100 | 0x0000_0040 | 0x0000_0080 | 0x0000_0200);
        r.c_lflag &= !(0x0000_0008 | 0x0000_0002 | 0x0000_0001 | 0x0000_8000);
        r.c_cc[6] = 1;
        r.c_cc[5] = 0;
        r
    }
}

// ─── Raw byte I/O ───────────────────────────────────────────────────────────

mod posix {
    unsafe extern "C" {
        pub fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    }
}

fn read_byte(fd: RawFd) -> Option<u8> {
    let mut buf = [0u8; 1];
    if unsafe { posix::read(fd, buf.as_mut_ptr(), 1) } == 1 {
        Some(buf[0])
    } else {
        None
    }
}

fn spawn_stdin_reader(fd: RawFd) -> mpsc::Receiver<u8> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            match read_byte(fd) {
                Some(b) => {
                    if tx.send(b).is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    });
    rx
}

// ─── Input parsing ──────────────────────────────────────────────────────────

#[derive(Debug)]
enum InputEvent {
    Char(char),
    Enter,
    Backspace,
    CtrlC,
    Escape,
    #[allow(dead_code)]
    CursorReport(usize, usize),
    Unknown,
}

fn parse_input(first: u8, rx: &mpsc::Receiver<u8>) -> InputEvent {
    match first {
        0x03 => InputEvent::CtrlC,
        0x0a | 0x0d => InputEvent::Enter,
        0x7f | 0x08 => InputEvent::Backspace,
        0x1b => parse_escape(rx),
        0x20..=0x7e => InputEvent::Char(first as char),
        0xc0..=0xdf => read_utf8(first, 1, rx),
        0xe0..=0xef => read_utf8(first, 2, rx),
        0xf0..=0xf7 => read_utf8(first, 3, rx),
        _ => InputEvent::Unknown,
    }
}

fn read_utf8(first: u8, extra: usize, rx: &mpsc::Receiver<u8>) -> InputEvent {
    let mut bytes = vec![first];
    for _ in 0..extra {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(b) => bytes.push(b),
            Err(_) => return InputEvent::Unknown,
        }
    }
    match String::from_utf8(bytes) {
        Ok(s) => s.chars().next().map_or(InputEvent::Unknown, InputEvent::Char),
        Err(_) => InputEvent::Unknown,
    }
}

fn parse_escape(rx: &mpsc::Receiver<u8>) -> InputEvent {
    match rx.recv_timeout(Duration::from_millis(50)) {
        Ok(b'[') => parse_csi(rx),
        Ok(b'O') => {
            let _ = rx.recv_timeout(Duration::from_millis(50));
            InputEvent::Unknown
        }
        Ok(_) => InputEvent::Unknown,
        Err(_) => InputEvent::Escape,
    }
}

fn parse_csi(rx: &mpsc::Receiver<u8>) -> InputEvent {
    let mut params = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(b) if (0x30..=0x3f).contains(&b) => params.push(b),
            Ok(b) if (0x20..=0x2f).contains(&b) => {
                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(b) if (0x40..=0x7e).contains(&b) => return InputEvent::Unknown,
                        Ok(_) => continue,
                        Err(_) => return InputEvent::Unknown,
                    }
                }
            }
            Ok(b'R') => {
                let s = String::from_utf8_lossy(&params);
                if let Some((r, c)) = s.split_once(';') {
                    let row = r.parse().unwrap_or(1);
                    let col = c.parse().unwrap_or(1);
                    return InputEvent::CursorReport(row, col);
                }
                return InputEvent::Unknown;
            }
            Ok(b) if (0x40..=0x7e).contains(&b) => {
                return InputEvent::Unknown;
            }
            _ => return InputEvent::Unknown,
        }
    }
}

// ─── CWD display ────────────────────────────────────────────────────────────

fn short_cwd() -> String {
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".to_string());
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().to_string();
        if cwd.starts_with(&home) {
            return format!("~{}", &cwd[home.len()..]);
        }
    }
    cwd
}

// ─── Rendering constants ────────────────────────────────────────────────────

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const NO_DIM: &str = "\x1b[22m";
const RST: &str = "\x1b[0m";

const PINNED_ROWS: usize = 4;

fn header_lines(cols: usize, cwd: &str) -> Vec<String> {
    let box_w = 53.min(cols);
    let inner = box_w.saturating_sub(2);

    let mut out = Vec::new();
    out.push(format!("{DIM}╭{}╮{RST}", "─".repeat(inner)));

    let title_vis = 4 + 8 + 9;
    let pad = inner.saturating_sub(title_vis);
    out.push(format!(
        "{DIM}│ >_ {NO_DIM}{BOLD}TUI Test{NO_DIM}{DIM} (v0.1.0){}{DIM}│{RST}",
        " ".repeat(pad)
    ));

    out.push(format!("{DIM}│{}│{RST}", " ".repeat(inner)));

    let model_pre = " model:     ";
    let model_val = "fake-model";
    let model_mid = "   ";
    let model_cmd = "/model";
    let model_suf = " to change ";
    let model_vis =
        model_pre.len() + model_val.len() + model_mid.len() + model_cmd.len() + model_suf.len();
    let mpad = inner.saturating_sub(model_vis);
    out.push(format!(
        "{DIM}│{model_pre}{NO_DIM}{model_val}{DIM}{model_mid}{NO_DIM}{RST}{model_cmd}{DIM}{RST} to change {}{DIM}│{RST}",
        " ".repeat(mpad)
    ));

    let dir_pre = " directory: ";
    let dir_vis = dir_pre.len() + cwd.len();
    let dpad = inner.saturating_sub(dir_vis);
    out.push(format!(
        "{DIM}│{dir_pre}{NO_DIM}{cwd}{DIM}{}│{RST}",
        " ".repeat(dpad)
    ));

    out.push(format!("{DIM}╰{}╯{RST}", "─".repeat(inner)));
    out
}

// ─── App state ──────────────────────────────────────────────────────────────

struct TranscriptEntry {
    user_input: String,
    responses: Vec<String>,
}

struct App {
    rows: usize,
    cols: usize,

    scroll_bottom: usize,
    content_row: usize,
    header_committed: bool,
    printed_lines: usize,

    input: String,
    transcript: Vec<TranscriptEntry>,
    working: bool,
    started: Instant,
    cwd: String,
}

impl App {
    fn new(rows: usize, cols: usize, start_row: usize) -> Self {
        Self {
            rows,
            cols,
            scroll_bottom: rows.saturating_sub(PINNED_ROWS).max(1),
            content_row: start_row,
            header_committed: false,
            printed_lines: 0,
            input: String::new(),
            transcript: Vec::new(),
            working: false,
            started: Instant::now(),
            cwd: short_cwd(),
        }
    }

    fn content_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for entry in &self.transcript {
            lines.push(format!("{DIM}› {NO_DIM}{}{RST}", entry.user_input));
            for resp in &entry.responses {
                lines.push(format!("{DIM}• {NO_DIM}{}{RST}", resp));
            }
        }
        lines
    }

    fn emit_scroll_line(&mut self, buf: &mut Vec<u8>, line: &str) {
        if self.content_row <= self.scroll_bottom {
            write!(buf, "\x1b[{};1H\x1b[;m\x1b[K{}", self.content_row, line).unwrap();
            self.content_row += 1;
        } else {
            write!(buf, "\x1b[{};1H\n\x1b[;m\x1b[K{}", self.scroll_bottom, line).unwrap();
        }
    }

    fn init(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();

        tlog!("init: rows={} cols={} content_row={} scroll_bottom={}", self.rows, self.cols, self.content_row, self.scroll_bottom);

        let header_height = 7;
        let needed_below = header_height + PINNED_ROWS;
        let available_below = self.rows.saturating_sub(self.content_row);
        tlog!("init: needed_below={} available_below={}", needed_below, available_below);
        if available_below < needed_below {
            let deficit = needed_below - available_below;
            tlog!("init: pushing {} newlines to make room", deficit);
            write!(stdout, "\x1b[{};1H", self.rows)?;
            for _ in 0..deficit {
                write!(stdout, "\r\n")?;
            }
            stdout.flush()?;
            self.content_row = self.content_row.saturating_sub(deficit).max(1);
            tlog!("init: content_row adjusted to {}", self.content_row);
        }

        // Clear the pinned area rows (don't touch the scroll content area)
        for row in (self.rows - PINNED_ROWS + 1)..=self.rows {
            write!(stdout, "\x1b[{};1H\x1b[K", row)?;
        }

        // Set scroll region (rows 1..scroll_bottom), pinned area below
        write!(stdout, "\x1b[1;{}r", self.scroll_bottom)?;
        stdout.flush()?;

        self.render()?;
        Ok(())
    }

    fn render(&mut self) -> io::Result<()> {
        let mut buf: Vec<u8> = Vec::with_capacity(4096);

        buf.extend_from_slice(b"\x1b[?2026h");

        // Ensure scroll region is active
        write!(buf, "\x1b[1;{}r", self.scroll_bottom)?;

        // Commit header to scroll region (once)
        if !self.header_committed {
            let header = header_lines(self.cols, &self.cwd);
            for line in &header {
                self.emit_scroll_line(&mut buf, line);
            }
            self.emit_scroll_line(&mut buf, "");
            self.header_committed = true;
        }

        // Commit new transcript lines to scroll region
        let all_content = self.content_lines();
        for i in self.printed_lines..all_content.len() {
            self.emit_scroll_line(&mut buf, &all_content[i]);
        }
        self.printed_lines = all_content.len();

        // Temporarily reset scroll region to draw pinned area
        buf.extend_from_slice(b"\x1b[r");
        self.draw_pinned(&mut buf);
        write!(buf, "\x1b[1;{}r", self.scroll_bottom)?;

        // Position cursor at prompt input (must be after scroll region restore)
        let prompt_row = self.rows - 2;
        let cursor_col = 3 + self.input.len();
        write!(buf, "\x1b[?25h\x1b[{prompt_row};{cursor_col}H")?;

        buf.extend_from_slice(b"\x1b[?2026l");

        let mut stdout = io::stdout();
        stdout.write_all(&buf)?;
        stdout.flush()?;
        Ok(())
    }

    fn draw_pinned(&self, buf: &mut Vec<u8>) {
        let working_row = self.rows - 3;
        let prompt_row = self.rows - 2;
        let blank_row = self.rows - 1;
        let status_row = self.rows;

        // Working indicator row
        write!(buf, "\x1b[{working_row};1H\x1b[0m\x1b[m\x1b[K").unwrap();
        if self.working {
            let secs = self.started.elapsed().as_secs();
            write!(
                buf,
                "\u{2022} {DIM}Working ({secs}s \u{2022} esc to interrupt){RST}"
            )
            .unwrap();
        }

        // Prompt
        write!(buf, "\x1b[{prompt_row};1H\x1b[0m\x1b[m\x1b[K").unwrap();
        if self.input.is_empty() && !self.working {
            write!(
                buf,
                "{BOLD}\u{203a}{NO_DIM} {DIM}Enter a prompt...{RST}"
            )
            .unwrap();
        } else {
            write!(buf, "{BOLD}\u{203a}{NO_DIM} {}{RST}", self.input).unwrap();
        }

        // Blank
        write!(buf, "\x1b[{blank_row};1H\x1b[0m\x1b[m\x1b[K").unwrap();

        // Status
        write!(buf, "\x1b[{status_row};1H\x1b[0m\x1b[m\x1b[K").unwrap();
        write!(
            buf,
            "  fake-model \u{00b7} 100% left \u{00b7} {}",
            self.cwd
        )
        .unwrap();

    }

    fn handle_event(&mut self, event: InputEvent) -> io::Result<bool> {
        match event {
            InputEvent::CtrlC | InputEvent::Escape => return Ok(true),
            InputEvent::Enter => {
                let text = self.input.clone();
                if text == "quit" || text == "exit" {
                    return Ok(true);
                }
                if text.is_empty() {
                    return Ok(false);
                }
                self.input.clear();
                self.transcript.push(TranscriptEntry {
                    user_input: text.clone(),
                    responses: Vec::new(),
                });
                self.working = true;
                self.started = Instant::now();
                self.render()?;

                for i in 1..=10 {
                    thread::sleep(Duration::from_millis(15));
                    if let Some(entry) = self.transcript.last_mut() {
                        entry.responses.push(format!("[{i}/10] {text}"));
                    }
                    tlog!("RESP: [{}/10] {}", i, text);
                    self.render()?;
                }

                self.working = false;
                tlog!("RESP: done");
                self.render()?;
            }
            InputEvent::Backspace => {
                self.input.pop();
                self.render()?;
            }
            InputEvent::Char(ch) => {
                self.input.push(ch);
                self.render()?;
            }
            InputEvent::CursorReport(_, _) => {}
            InputEvent::Unknown => {}
        }
        Ok(false)
    }
}

// ─── Terminal guard (setup + cleanup) ───────────────────────────────────────

struct TermGuard {
    fd: RawFd,
    orig: raw_term::Termios,
}

impl TermGuard {
    fn enter(fd: RawFd) -> io::Result<Self> {
        let orig = raw_term::get(fd);
        raw_term::set(fd, &raw_term::make_raw(&orig));

        let mut stdout = io::stdout();
        write!(stdout, "\x1b[?2004h\x1b[>7u\x1b[?1004h")?;
        stdout.flush()?;

        Ok(Self { fd, orig })
    }

    fn cleanup(&self) -> io::Result<()> {
        let mut stdout = io::stdout();
        // Reset scroll region, move to bottom, newline to leave cursor below content
        write!(stdout, "\x1b[r")?;
        write!(stdout, "\x1b[999;1H\r\n")?;
        write!(
            stdout,
            "\x1b[<1u\x1b[?2004l\x1b[?1004l\x1b[0m\x1b[?25h"
        )?;
        stdout.flush()?;
        Ok(())
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
        raw_term::set(self.fd, &self.orig);
    }
}

// ─── Cursor position query ──────────────────────────────────────────────────

fn query_cursor_position(rx: &mpsc::Receiver<u8>) -> io::Result<(usize, usize)> {
    let mut stdout = io::stdout();
    write!(stdout, "\x1b[6n")?;
    stdout.flush()?;

    let mut state = 0u8;
    let mut params: Vec<u8> = Vec::new();

    loop {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(b) => match state {
                0 => {
                    if b == 0x1b {
                        state = 1;
                    }
                }
                1 => {
                    if b == b'[' {
                        state = 2;
                        params.clear();
                    } else {
                        state = 0;
                    }
                }
                2 => {
                    if b == b'R' {
                        let s = String::from_utf8_lossy(&params);
                        if let Some((r, c)) = s.split_once(';') {
                            let row = r.parse().unwrap_or(1);
                            let col = c.parse().unwrap_or(1);
                            return Ok((row, col));
                        }
                        return Ok((1, 1));
                    }
                    params.push(b);
                }
                _ => {}
            },
            Err(_) => return Ok((1, 1)),
        }
    }
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn run() -> io::Result<()> {
    init_log();
    let stdin_fd = io::stdin().as_raw_fd();
    let (rows, cols) = terminal_size(stdin_fd);
    tlog!("startup: terminal_size={}x{} stdin_fd={}", rows, cols, stdin_fd);

    let _guard = TermGuard::enter(stdin_fd)?;
    let rx = spawn_stdin_reader(stdin_fd);

    let (start_row, _start_col) = query_cursor_position(&rx)?;
    tlog!("startup: DSR cursor_pos=row:{} col:{}", start_row, _start_col);

    let mut app = App::new(rows, cols, start_row);
    tlog!("startup: scroll_bottom={} content_row={} PINNED_ROWS={}", app.scroll_bottom, app.content_row, PINNED_ROWS);
    app.init()?;
    tlog!("startup: init done, content_row after init={}", app.content_row);

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(byte) => {
                let event = parse_input(byte, &rx);
                if app.handle_event(event)? {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let (new_rows, new_cols) = terminal_size(stdin_fd);
        if new_rows != app.rows || new_cols != app.cols {
            tlog!("RESIZE: {}x{} -> {}x{}", app.rows, app.cols, new_rows, new_cols);
            app.rows = new_rows;
            app.cols = new_cols;
            app.scroll_bottom = new_rows.saturating_sub(PINNED_ROWS).max(1);
            if app.content_row > app.scroll_bottom {
                app.content_row = app.scroll_bottom;
            } else if app.content_row == 0 {
                app.content_row = 1;
            }
            // Force a deterministic full redraw on resize, like real TUIs.
            app.header_committed = false;
            app.printed_lines = 0;
            app.content_row = 1;
            let mut stdout = io::stdout();
            write!(stdout, "\x1b[1;{}r", app.scroll_bottom)?;
            stdout.flush()?;
            app.render()?;
            tlog!("RESIZE: render done, content_row={}", app.content_row);
        }

        if app.working {
            app.render()?;
        }
    }

    Ok(())
}

pub(crate) fn main() {
    if let Err(err) = run() {
        let _ = writeln!(io::stderr(), "tui_test: {err}");
        std::process::exit(1);
    }
}
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::main();
}
