//! Plain full-screen views in the terminal's own colours: a header, sections
//! of rows, one status line (questions and progress live there too) and a
//! key-hint footer. Details go to the log file, never onto the screen.
use super::{clean, console, line, Key};
use crate::progress;
use std::{
    cell::{Cell as StdCell, RefCell},
    env, fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) const PLAIN: &str = "0";
pub(super) const DIM: &str = "2";
pub(super) const BOLD: &str = "1";
pub(super) const OK: &str = "32";
pub(super) const WARN: &str = "33";
pub(super) const ACC: &str = "36";
const INV: &str = "7";

#[derive(Clone)]
pub(super) struct Span(pub String, pub &'static str);
pub(super) type Text = Vec<Span>;
pub(super) fn text(value: impl Into<String>, style: &'static str) -> Text {
    vec![Span(value.into(), style)]
}
/// A green check mark followed by plain text.
pub(super) fn done(value: impl Into<String>) -> Text {
    vec![Span("✓".into(), OK), Span(format!(" {}", value.into()), PLAIN)]
}
pub(super) fn plain_text(value: &Text) -> String {
    value.iter().map(|s| s.0.as_str()).collect()
}

#[derive(Clone)]
pub(super) struct Item {
    pub id: String,
    pub name: String,
    /// "commercial", "beta", "free" or empty (no license column).
    pub license: &'static str,
    pub status: Text,
    /// Shown on the selected row only, as `<action> ⏎`.
    pub action: String,
}
#[derive(Clone)]
pub(super) enum Row {
    Head(String),
    Note(Text),
    Item(Item),
}
#[derive(Clone, Default)]
pub(super) struct View {
    pub crumb: String,
    pub subtitle: String,
    pub email: String,
    pub rows: Vec<Row>,
    /// Sub-screens: Escape goes back.
    pub back: bool,
    /// Name column width (15 unless a screen needs more).
    pub name_width: usize,
    /// Replaces the key-hint footer (a consent screen cancels, not quits).
    pub footer: Option<&'static str>,
}
pub(super) enum Nav {
    Select(String),
    Back,
    Quit,
    /// Something changed in the background (an app exited, disk measured):
    /// rebuild the view and call `menu` again.
    Refresh,
}

thread_local! {
    static VIEW: RefCell<View> = RefCell::new(View::default());
    static SELECTED: StdCell<Option<usize>> = const { StdCell::new(None) };
    static SCROLL: StdCell<usize> = const { StdCell::new(0) };
    static MESSAGE: RefCell<Text> = const { RefCell::new(Vec::new()) };
    static CHOICE: RefCell<Text> = const { RefCell::new(Vec::new()) };
    static FOOTER: StdCell<Option<&'static str>> = const { StdCell::new(None) };
    static LOG_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static COLOR: StdCell<bool> = const { StdCell::new(false) };
}

/// Setup detail (stages, compiler output summaries, errors) goes to this file.
pub(super) fn set_log(path: PathBuf) {
    // One session's worth: start over when an older log grew large.
    if fs::metadata(&path).is_ok_and(|m| m.len() > 4 * 1024 * 1024) {
        let _ = fs::remove_file(&path);
    }
    LOG_PATH.with(|log| *log.borrow_mut() = Some(path));
}
pub(super) fn log_path() -> Option<PathBuf> {
    LOG_PATH.with(|log| log.borrow().clone())
}
pub(super) fn activity(value: &str) {
    let lines: String = value.lines().map(clean).filter(|s| !s.is_empty()).map(|s| s + "\n").collect();
    if lines.is_empty() {
        return;
    }
    if let Some(path) = log_path() {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(lines.as_bytes());
        }
    }
    if !COLOR.with(StdCell::get) && !progress::active() {
        print!("{lines}");
    }
}
/// The status line shown until the next key press.
pub(super) fn message(value: Text) {
    if !COLOR.with(StdCell::get) {
        println!("{}", plain_text(&value));
    }
    MESSAGE.with(|m| *m.borrow_mut() = value);
}
pub(super) fn warn(value: &str) {
    activity(value);
    message(text(clean(value.lines().next().unwrap_or_default()), WARN));
}
/// Show short blocking work on the status line.
pub(super) fn busy(label: &str) {
    MESSAGE.with(|m| *m.borrow_mut() = vec![Span("⠋".into(), ACC), Span(format!(" {label}"), PLAIN)]);
    if COLOR.with(StdCell::get) {
        draw();
    } else {
        println!("{label}");
    }
}
/// Work started from a row: show it on that row of the current screen
/// ("installing…", "compiling…") and switch the footer until the work ends.
pub(super) fn working(id: &str, label: &str) {
    VIEW.with(|v| {
        for row in v.borrow_mut().rows.iter_mut() {
            if let Row::Item(item) = row {
                if item.id == id {
                    item.status = text(label, WARN);
                    item.action.clear();
                }
            }
        }
    });
    MESSAGE.with(|m| m.borrow_mut().clear());
    FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
    draw();
}
pub(super) fn set_view(view: View) {
    VIEW.with(|v| *v.borrow_mut() = view);
    SELECTED.with(|s| s.set(None));
}

fn ansi(style: &str) -> String {
    format!("\x1b[0;{style}m")
}
#[derive(Clone, Copy, PartialEq)]
struct Cell { ch: char, style: &'static str }
#[derive(Default)]
struct Canvas { width: usize, cells: Vec<Cell>, shown: Vec<Cell> }
thread_local! {
    static CANVAS: RefCell<Canvas> = RefCell::new(Canvas::default());
    static SCREEN_DEPTH: StdCell<usize> = const { StdCell::new(0) };
}
fn invalidate() { CANVAS.with(|c| c.borrow_mut().shown.clear()); }
fn begin_frame(cols: usize, rows: usize) {
    CANVAS.with(|c| {
        let mut c = c.borrow_mut();
        if c.width != cols || c.cells.len() != cols * rows { c.shown.clear(); }
        c.width = cols;
        c.cells.clear();
        c.cells.resize(cols * rows, Cell { ch: ' ', style: PLAIN });
    });
}
fn present() -> io::Result<()> {
    use std::fmt::Write as _;
    CANVAS.with(|canvas| {
        let mut c = canvas.borrow_mut();
        let mut out = String::from("\x1b[?2026h\x1b[?25l\x1b[?7l");
        let width = c.width;
        if width == 0 { return Ok(()); }
        for (y, cells) in c.cells.chunks(width).enumerate() {
            let start = y * width;
            if c.shown.get(start..start + width) == Some(cells) { continue; }
            let _ = write!(out, "\x1b[{};1H", y + 1);
            let mut style = "";
            for cell in cells {
                if style != cell.style { out.push_str(&ansi(cell.style)); style = cell.style; }
                out.push(cell.ch);
            }
        }
        out.push_str("\x1b[0m\x1b[?7h\x1b[?2026l");
        io::stdout().write_all(out.as_bytes())?;
        io::stdout().flush()?;
        c.shown = c.cells.clone();
        Ok(())
    })
}
/// Write `value` at 1-based row `y`, 0-based column `x`; returns the column after it.
fn put(y: usize, x: usize, value: &str, style: &'static str) -> usize {
    CANVAS.with(|canvas| {
        let mut c = canvas.borrow_mut();
        let width = c.width;
        let mut x = x;
        for ch in value.chars() {
            if ch.is_control() { continue; }
            if x < width {
                if let Some(cell) = c.cells.get_mut((y - 1) * width + x) { *cell = Cell { ch, style }; }
            }
            x += 1;
        }
        x
    })
}
fn put_text(y: usize, x: usize, value: &Text) -> usize {
    value.iter().fold(x, |x, span| put(y, x, &span.0, span.1))
}
fn padded(value: &str, width: usize) -> String {
    let n = value.chars().count();
    format!("{value}{}", " ".repeat(width.saturating_sub(n)))
}

/// One body line: its spans, and the item index it shows (if any).
fn body_lines(view: &View, selected: Option<usize>) -> (Vec<(Text, Option<usize>)>, Option<usize>) {
    let mut lines = Vec::new();
    let mut item = 0;
    let mut selected_line = None;
    let name_width = view.name_width.max(15);
    for (i, row) in view.rows.iter().enumerate() {
        match row {
            Row::Head(title) => {
                if i > 0 || !view.back { lines.push((Vec::new(), None)); }
                lines.push((vec![Span("    ".into(), PLAIN), Span(title.clone(), DIM)], None));
            }
            Row::Note(note) => {
                let mut spans = vec![Span("    ".into(), PLAIN)];
                spans.extend(note.iter().cloned());
                lines.push((spans, None));
            }
            Row::Item(entry) => {
                if item == 0 && view.back {
                    lines.push((Vec::new(), None));
                }
                let chosen = selected == Some(item);
                let mut spans = if chosen {
                    vec![Span("  ".into(), PLAIN), Span("›".into(), ACC), Span(" ".into(), PLAIN), Span(padded(&entry.name, name_width), BOLD), Span(" ".into(), PLAIN)]
                } else {
                    vec![Span(format!("    {} ", padded(&entry.name, name_width)), PLAIN)]
                };
                let license = match entry.license {
                    "commercial" => Some(("commercial license", BOLD)),
                    "beta" => Some(("beta access", WARN)),
                    "free" => Some(("free, open source", DIM)),
                    _ => None,
                };
                if let Some((label, style)) = license {
                    spans.push(Span(padded(label, 20), style));
                }
                spans.extend(entry.status.iter().cloned());
                if chosen && entry.action == "⏎" {
                    spans.push(Span(" ⏎".into(), ACC));
                } else if chosen && !entry.action.is_empty() {
                    let gap = if entry.status.is_empty() && entry.license.is_empty() { "" } else { "  " };
                    spans.push(Span(format!("{gap}{} ⏎", entry.action), ACC));
                }
                if chosen { selected_line = Some(lines.len()); }
                lines.push((spans, Some(item)));
                item += 1;
            }
        }
    }
    (lines, selected_line)
}

fn draw() {
    if !COLOR.with(StdCell::get) { return; }
    let (cols, rows) = console::size();
    if cols < 40 || rows < 14 {
        print!("\x1b[0m\x1b[2J\x1b[HResize the terminal to at least 40 × 14.");
        let _ = io::stdout().flush();
        invalidate();
        return;
    }
    begin_frame(cols, rows);
    let width = cols.min(80);
    VIEW.with(|view| {
        let view = view.borrow();
        let mut x = put(2, 2, "Makepad", BOLD);
        x = put(2, x, " commercial apps", PLAIN);
        x = put(2, x, &view.crumb, PLAIN);
        let email = view.email.chars().count();
        if !view.email.is_empty() && width >= email + 2 && width - 2 - email > x {
            put(2, width - 2 - email, &view.email, DIM);
        }
        put(3, 2, &view.subtitle, DIM);
        let (lines, selected_line) = body_lines(&view, SELECTED.with(StdCell::get));
        let top = 4;
        let mut room = rows.saturating_sub(top + 4 + usize::from(view.back));
        let mut offset = SCROLL.with(StdCell::get);
        if lines.len() <= room {
            offset = 0;
        } else {
            if !view.back { room -= 1; } // the "more" indicator
            if let Some(line) = selected_line {
                // Keep the heading above the first item in view.
                let first_item = lines.iter().position(|l| l.1 == Some(0)).unwrap_or(0);
                if line == first_item { offset = 0; }
                if line < offset { offset = line; }
                if line >= offset + room { offset = line + 1 - room; }
            }
            offset = offset.min(lines.len() - room);
        }
        SCROLL.with(|s| s.set(offset));
        let mut y = top;
        for (spans, _) in lines.iter().skip(offset).take(room) {
            put_text(y, 0, spans);
            y += 1;
        }
        // Sub-screens keep a line for the scroll hint, as the main screen
        // does when it has to scroll.
        if lines.len() > room || view.back {
            y += 1;
        }
        if lines.len() > room {
            let below = lines[(offset + room).min(lines.len())..].iter().filter(|l| l.1.is_some()).count();
            let above = lines[..offset].iter().filter(|l| l.1.is_some()).count();
            let more = if below > 0 { format!("↓ {below} more") } else if above > 0 { format!("↑ {above} above") } else { String::new() };
            put(y - 1, 4, &more, DIM);
        }
        // The rule, status, choice and footer follow the rows directly.
        let y = (y + 1).min(rows - 3);
        put(y, 2, &"─".repeat(width.saturating_sub(4)), DIM);
        MESSAGE.with(|m| put_text(y + 1, 2, &m.borrow()));
        CHOICE.with(|c| put_text(y + 2, 2, &c.borrow()));
        let footer = FOOTER.with(StdCell::get).or(view.footer).unwrap_or(if view.back {
            "↑↓ move   ⏎ select   esc back   q quit"
        } else {
            "↑↓ move   ⏎ select   q quit"
        });
        put(y + 3, 2, footer, DIM);
    });
    let _ = present();
}

/// Run one screen until a row is chosen or the person leaves it.
/// `selected` is the item index, kept by the caller across calls.
pub(super) fn menu(view: View, selected: &mut usize, changed: &dyn Fn() -> bool) -> Result<Nav, String> {
    let ids: Vec<String> = view.rows.iter().filter_map(|r| if let Row::Item(i) = r { Some(i.id.clone()) } else { None }).collect();
    if !COLOR.with(StdCell::get) {
        println!();
        for row in &view.rows {
            match row {
                Row::Head(title) => println!("\n{title}"),
                Row::Note(note) => println!("  {}", plain_text(note)),
                Row::Item(item) => {
                    let number = ids.iter().position(|id| *id == item.id).unwrap_or(0) + 1;
                    println!("{number:>3}. {}  {}  [{}]", item.name, plain_text(&item.status), item.action);
                }
            }
        }
        let Ok(answer) = line(if view.back { "Choose a number, Enter to go back, q to quit: " } else { "Choose a number, or q to quit: " }) else {
            return Ok(Nav::Quit);
        };
        return Ok(match answer.to_lowercase().as_str() {
            "q" => Nav::Quit,
            "" if view.back => Nav::Back,
            choice => match choice.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| ids.get(n)) {
                Some(id) => { *selected = ids.iter().position(|i| i == id).unwrap_or(0); Nav::Select(id.clone()) }
                None => Nav::Refresh,
            },
        });
    }
    if ids.is_empty() {
        return Ok(Nav::Back);
    }
    VIEW.with(|v| *v.borrow_mut() = view);
    FOOTER.with(|f| f.set(None));
    let mut sel = (*selected).min(ids.len() - 1);
    let _input = console::Input::enter()?;
    loop {
        SELECTED.with(|s| s.set(Some(sel)));
        *selected = sel;
        draw();
        let key = console::key()?;
        if !matches!(key, Key::Other) {
            MESSAGE.with(|m| m.borrow_mut().clear());
        }
        match key {
            Key::Up | Key::Char('k') => sel = sel.saturating_sub(1),
            Key::Down | Key::Char('j') => sel = (sel + 1).min(ids.len() - 1),
            Key::PageUp => sel = sel.saturating_sub(10),
            Key::PageDown => sel = (sel + 10).min(ids.len() - 1),
            Key::Home => sel = 0,
            Key::End => sel = ids.len() - 1,
            #[cfg(not(windows))]
            Key::WheelUp => sel = sel.saturating_sub(3),
            #[cfg(not(windows))]
            Key::WheelDown => sel = (sel + 3).min(ids.len() - 1),
            Key::Enter => return Ok(Nav::Select(ids[sel].clone())),
            Key::Back => return Ok(Nav::Back),
            Key::Quit | Key::Char('q') | Key::Char('Q') => return Ok(Nav::Quit),
            Key::Other => {
                if super::reap_apps() || changed() {
                    return Ok(Nav::Refresh);
                }
            }
            _ => {}
        }
    }
}

/// An inline question on the status line with its options on the next line
/// and a dim note after them: ←→ or a first letter picks, ⏎ accepts, Escape
/// backs out (None). Closed input and Ctrl-C also return None.
pub(super) fn choose(question: &str, note: &str, options: &[&str], default: usize) -> Result<Option<String>, String> {
    if options.is_empty() {
        return Ok(None);
    }
    if !COLOR.with(StdCell::get) {
        println!("\n{}", clean(question));
        if !note.is_empty() { println!("({note})"); }
        loop {
            let Ok(answer) = line(&format!("[{}] ", options.join("/"))) else { return Ok(None) };
            if answer.contains('\u{1b}') { return Ok(None); }
            let answer = answer.to_lowercase();
            if answer.is_empty() { return Ok(Some(options[default.min(options.len() - 1)].to_owned())); }
            if let Some(option) = options.iter().find(|o| o.to_lowercase().starts_with(&answer)) { return Ok(Some((*option).to_owned())); }
            println!("Please answer {}.", options.join(" or "));
        }
    }
    let mut pick = default.min(options.len() - 1);
    let _input = console::Input::enter()?;
    let saved = SELECTED.with(StdCell::get);
    let result = loop {
        MESSAGE.with(|m| *m.borrow_mut() = text(clean(question), PLAIN));
        CHOICE.with(|c| {
            let mut spans = Vec::new();
            for (i, option) in options.iter().enumerate() {
                spans.push(if i == pick { Span(format!(" {option} "), INV) } else { Span(format!(" {option} "), PLAIN) });
                spans.push(Span(" ".into(), PLAIN));
            }
            if !note.is_empty() {
                spans.push(Span(format!(" {note}"), DIM));
            }
            *c.borrow_mut() = spans;
        });
        FOOTER.with(|f| f.set(Some("←→ choose   ⏎ accept   esc back")));
        draw();
        match console::key()? {
            Key::Left | Key::Up => pick = pick.saturating_sub(1),
            Key::Right | Key::Down => pick = (pick + 1).min(options.len() - 1),
            Key::Enter => break Some(options[pick].to_owned()),
            Key::Back | Key::Quit => break None,
            Key::Char(c) => {
                let c = c.to_ascii_lowercase();
                if let Some(option) = options.iter().find(|o| o.to_lowercase().starts_with(c)) {
                    break Some((*option).to_owned());
                }
            }
            Key::Other => { super::reap_apps(); }
            _ => {}
        }
    };
    SELECTED.with(|s| s.set(saved));
    MESSAGE.with(|m| m.borrow_mut().clear());
    CHOICE.with(|c| c.borrow_mut().clear());
    FOOTER.with(|f| f.set(None));
    Ok(result)
}

/// A one-line editor on the status line. Escape or closed input: None.
pub(super) fn edit(prompt: &str, hint: &str, footer: &'static str) -> Result<Option<String>, String> {
    if !COLOR.with(StdCell::get) {
        return Ok(line(&format!("{prompt} ")).ok());
    }
    let mut value = String::new();
    let _input = console::Input::enter()?;
    let result = loop {
        MESSAGE.with(|m| {
            *m.borrow_mut() = vec![
                Span(format!("{prompt} "), PLAIN),
                Span(value.clone(), BOLD),
                Span(" ".into(), INV),
            ];
        });
        CHOICE.with(|c| *c.borrow_mut() = text(hint, DIM));
        FOOTER.with(|f| f.set(Some(footer)));
        draw();
        match console::key()? {
            Key::Enter => break Some(value.trim().to_owned()),
            Key::Back | Key::Quit => break None,
            Key::Backspace => { value.pop(); }
            Key::Char(c) if !c.is_control() && value.chars().count() < 200 => value.push(c),
            _ => {}
        }
    };
    MESSAGE.with(|m| m.borrow_mut().clear());
    CHOICE.with(|c| c.borrow_mut().clear());
    FOOTER.with(|f| f.set(None));
    Ok(result)
}

pub(super) struct Screen {
    color: bool,
}
impl Screen {
    pub(super) fn enter() -> Self {
        let color = io::stdout().is_terminal()
            && io::stdin().is_terminal()
            && env::var_os("NO_COLOR").is_none();
        if color {
            console::enable();
            SCREEN_DEPTH.with(|depth| {
                if depth.get() == 0 { print!("\x1b[?1049h\x1b[?25l"); invalidate(); COLOR.with(|c| c.set(true)); }
                depth.set(depth.get() + 1);
            });
        }
        Self { color }
    }
    pub(super) fn pause() -> Pause {
        let active = SCREEN_DEPTH.with(|d| d.get() > 0);
        if active {
            print!("\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = io::stdout().flush();
            COLOR.with(|c| c.set(false));
        }
        Pause(active)
    }
}
pub(super) struct Pause(bool);
impl Drop for Pause {
    fn drop(&mut self) {
        if self.0 {
            print!("\x1b[?1049h\x1b[?25l");
            invalidate();
            COLOR.with(|c| c.set(true));
            let _ = io::stdout().flush();
        }
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        if self.color {
            SCREEN_DEPTH.with(|depth| {
                depth.set(depth.get().saturating_sub(1));
                if depth.get() == 0 {
                    print!("\x1b[0m\x1b[?25h\x1b[?1049l");
                    invalidate();
                    COLOR.with(|c| c.set(false));
                }
            });
            let _ = io::stdout().flush();
        }
    }
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Run blocking setup work with its progress as one self-rewriting bar on the
/// status line: a short label, the bar, a percentage and a dim amount. Every
/// phase change is written to the log file; per-file events never are.
pub fn with_progress<R>(work: impl FnOnce() -> R) -> R {
    let _screen = Screen::enter();
    let color = COLOR.with(StdCell::get);
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut identity = String::new();
    let mut started = Instant::now();
    let mut baseline = 0;
    let begun = Instant::now();
    let result = progress::scope(
        move |p| {
            let phase = format!("{}:{}:{}", p.package.group, p.package.index, p.stage);
            let changed = identity != phase;
            if changed {
                identity = phase;
                started = Instant::now();
                baseline = p.loaded;
                let group = if p.package.group.is_empty() { String::new() } else { format!("{} {}: ", p.package.group, p.package.name) };
                activity(&format!("{group}{}: {}", p.stage, p.detail));
            } else if p.stage == "Compiling Rust" && !p.detail.trim().is_empty() {
                activity(&p.detail);
            }
            if !changed && last.elapsed() < Duration::from_millis(100) && !(p.total > 0 && p.loaded >= p.total) {
                return;
            }
            last = Instant::now();
            let label = if p.package.group.is_empty() { p.stage.clone() } else { p.package.group.clone() };
            let fraction = if p.total > 0 {
                Some((p.loaded as f64 / p.total as f64).clamp(0.0, 1.0))
            } else if p.stage == "Ready" {
                Some(1.0)
            } else if p.package.count > 0 {
                Some(p.package.index.saturating_sub(1) as f64 / p.package.count as f64)
            } else {
                None
            };
            let mut amount = match p.unit {
                progress::Unit::Bytes if p.total > 0 => format!("{:.1} / {:.1} MB", p.loaded as f64 / 1048576., p.total as f64 / 1048576.),
                progress::Unit::Bytes if p.loaded > 0 => format!("{:.1} MB", p.loaded as f64 / 1048576.),
                progress::Unit::Files | progress::Unit::Blocks | progress::Unit::Objects if p.loaded > 0 => format!(
                    "{}{} {}",
                    p.loaded,
                    if p.total > 0 { format!(" / {}", p.total) } else { String::new() },
                    match p.unit { progress::Unit::Files => "files", progress::Unit::Objects => "objects", _ => "blocks" }
                ),
                _ => String::new(),
            };
            if p.stage == "Download" && p.loaded > baseline {
                let elapsed = started.elapsed().as_secs_f64();
                if elapsed > 0.25 {
                    amount.push_str(&format!(" · {:.1} MB/s", (p.loaded - baseline) as f64 / elapsed / 1048576.));
                }
            }
            // One short word or two about what is happening, never a path.
            let what = if p.package.count > 0 {
                format!("{} · {} of {}", p.stage.to_lowercase(), p.package.index.max(1), p.package.count)
            } else if p.stage == "Compiling Rust" {
                p.detail.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
            } else if matches!(p.unit, progress::Unit::None) {
                p.detail.clone()
            } else {
                p.stage.to_lowercase()
            };
            let what = [amount, clean(&what)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            if !color {
                if changed { println!("{label}: {what}"); }
                return;
            }
            let label: String = label.chars().take(22).collect();
            let mut spans = vec![Span(format!("{} ", padded(&label, 13)), PLAIN)];
            match fraction {
                Some(f) => {
                    let filled = (f * 30.).round() as usize;
                    spans.push(Span("━".repeat(filled), ACC));
                    spans.push(Span("─".repeat(30 - filled), DIM));
                    spans.push(Span(format!(" {:3.0}%", f * 100.), PLAIN));
                }
                None => {
                    let frame = (begun.elapsed().as_millis() / 100) as usize % SPINNER.len();
                    spans.insert(0, Span(format!("{} ", SPINNER[frame]), ACC));
                }
            }
            if !what.is_empty() {
                spans.push(Span(format!("  {what}"), DIM));
            }
            MESSAGE.with(|m| *m.borrow_mut() = spans);
            FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
            draw();
        },
        work,
    );
    FOOTER.with(|f| f.set(None));
    MESSAGE.with(|m| m.borrow_mut().clear());
    result
}
