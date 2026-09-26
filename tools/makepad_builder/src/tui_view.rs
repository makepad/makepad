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
    /// The row last worked on; `menu` starts its selection there once.
    static WORKED_ON: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Setup detail (stages, compiler output summaries, errors) goes to this file.
pub(super) fn set_log(path: PathBuf) {
    // Start the log over (truncate, never delete) once it grew large.
    if fs::metadata(&path).is_ok_and(|m| m.len() > 4 * 1024 * 1024) {
        let _ = fs::File::create(&path);
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
        // Seconds since the Builder started, so the log shows where time goes.
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let at = START.get_or_init(Instant::now).elapsed().as_secs_f64();
        let stamped: String = lines.lines().map(|line| format!("{at:8.2} {line}\n")).collect();
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(stamped.as_bytes());
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
    // The selection moves to the row being worked on and stays there
    // afterwards (the next `menu` call starts on it).
    VIEW.with(|v| {
        let mut index = 0;
        for row in v.borrow_mut().rows.iter_mut() {
            if let Row::Item(item) = row {
                if item.id == id {
                    item.status = text(label, WARN);
                    item.action.clear();
                    SELECTED.with(|s| s.set(Some(index)));
                }
                index += 1;
            }
        }
    });
    WORKED_ON.with(|w| *w.borrow_mut() = Some(id.to_owned()));
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
    // No verbatim `\\?\` path prefix ever reaches the screen.
    let plain;
    let value = if value.contains(r"\\?\") {
        plain = crate::plain_paths(value);
        plain.as_str()
    } else {
        value
    };
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
/// `value` in at most `max` lines of `width` columns, broken at spaces and
/// keeping each span's style; the last line ends in "…" when text is left.
fn wrap_text(value: &Text, width: usize, max: usize) -> Vec<Text> {
    let total: usize = value.iter().map(|s| crate::plain_paths(&s.0).chars().count()).sum();
    if total <= width || width < 8 || max == 0 {
        return vec![value.clone()];
    }
    // Words with their style; a word never spans two styles' boundary.
    // `glued`: no space before it (it continues the previous span's word).
    let mut words: Vec<(String, &'static str, bool)> = Vec::new();
    let mut after_space = true;
    for span in value {
        let text = crate::plain_paths(&span.0);
        for (i, part) in text.split(' ').enumerate() {
            if i > 0 {
                after_space = true;
            }
            if part.is_empty() {
                continue;
            }
            words.push((part.to_string(), span.1, !after_space));
            after_space = false;
        }
    }
    // A leading mark ("✓", "✗") keeps the text beside it: later lines
    // start under the text, not under the mark.
    let hang = match words.first() {
        Some((mark, _, _)) if mark.chars().count() <= 2 && !mark.chars().any(char::is_alphanumeric) => mark.chars().count() + 1,
        _ => 0,
    };
    let mut lines: Vec<Text> = vec![Vec::new()];
    let mut used = 0;
    for (word, style, glued) in words {
        let n = word.chars().count();
        let gap = usize::from(!glued && used > 0);
        if used > 0 && used + gap + n > width {
            if lines.len() == max {
                let last = lines.last_mut().unwrap();
                if used + 1 <= width {
                    last.push(Span("…".into(), style));
                } else if let Some(span) = last.last_mut() {
                    span.0.pop();
                    span.0.push('…');
                }
                return lines;
            }
            lines.push(vec![Span(" ".repeat(hang), PLAIN)]);
            used = hang;
            let line = lines.last_mut().unwrap();
            line.push(Span(word, style));
            used += n;
            continue;
        }
        let line = lines.last_mut().unwrap();
        let text = if used > 0 && !glued { format!(" {word}") } else { word };
        used += text.chars().count();
        match line.last_mut() {
            Some(span) if span.1 == style => span.0.push_str(&text),
            _ => line.push(Span(text, style)),
        }
    }
    lines
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
        let work = work_rows().map(|rows| View { rows, back: true, ..View::default() });
        let (lines, selected_line) = match &work {
            Some(page) => body_lines(page, None),
            None => body_lines(&view, SELECTED.with(StdCell::get)),
        };
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
        // A status message longer than the line wraps onto the choice line
        // when that is free; what still does not fit ends in "…".
        let room = width.saturating_sub(4);
        let choice = CHOICE.with(|c| c.borrow().clone());
        let lines = MESSAGE.with(|m| wrap_text(&m.borrow(), room, if choice.is_empty() { 2 } else { 1 }));
        for (i, line) in lines.iter().enumerate() {
            put_text(y + 1 + i, 2, line);
        }
        put_text(y + 2, 2, &choice);
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
    // Only the screen that has the worked-on row takes it over.
    let worked = WORKED_ON.with(|w| w.borrow().as_ref().and_then(|id| ids.iter().position(|i| i == id)));
    if let Some(index) = worked {
        WORKED_ON.with(|w| w.borrow_mut().take());
        *selected = index;
    }
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
            // The list wraps around: Up on the first row goes to the last.
            Key::Up | Key::Char('k') => sel = (sel + ids.len() - 1) % ids.len(),
            Key::Down | Key::Char('j') => sel = (sel + 1) % ids.len(),
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
/// `initial` pre-fills the text with the cursor at its end (a rejected email
/// to correct); `hint` is the line under it.
pub(super) fn edit(prompt: &str, initial: &str, hint: Text, footer: &'static str) -> Result<Option<String>, String> {
    if !COLOR.with(StdCell::get) {
        if !hint.is_empty() { println!("{}", plain_text(&hint)); }
        return Ok(line(&format!("{prompt} ")).ok().map(|v| if v.is_empty() { initial.to_owned() } else { v }));
    }
    let mut value = initial.to_owned();
    let _input = console::Input::enter()?;
    let result = loop {
        MESSAGE.with(|m| {
            *m.borrow_mut() = vec![
                Span(format!("{prompt} "), PLAIN),
                Span(value.clone(), BOLD),
                Span(" ".into(), INV),
            ];
        });
        CHOICE.with(|c| *c.borrow_mut() = hint.clone());
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

// ---- Work page ---------------------------------------------------------------

/// While work runs (installing, downloading, compiling, updating) a page of
/// its own shows the steps, one per line: ✓ done, ● now with its bar, ○ next,
/// ✗ failed with the reason. Components that install side by side are all
/// "now" at once, each with its own bar.
#[derive(Clone, PartialEq)]
enum StepState {
    Next,
    Now,
    Done,
    Failed(String),
    /// Stopped because another step failed, or waiting to be retried.
    Held(String),
}
struct Step {
    name: String,
    state: StepState,
    /// Its bar and amount (running), or its summary (done).
    line: Text,
}
struct Work {
    steps: Vec<Step>,
}
thread_local! {
    static WORK: RefCell<Option<Work>> = const { RefCell::new(None) };
}
const RED: &str = "31";

/// Open the work page. `row` is the menu row the work belongs to; the menu
/// selects it afterwards.
pub(super) fn work_begin(crumb: &str, subtitle: &str, email: &str, row: &str, steps: &[&str]) {
    set_view(View { crumb: format!(" › {crumb}"), subtitle: subtitle.into(), email: email.into(), back: true, ..View::default() });
    WORK.with(|w| *w.borrow_mut() = Some(Work { steps: steps.iter().map(|s| Step { name: s.to_string(), state: StepState::Next, line: Vec::new() }).collect() }));
    WORKED_ON.with(|w| *w.borrow_mut() = Some(row.to_owned()));
    MESSAGE.with(|m| m.borrow_mut().clear());
    CHOICE.with(|c| c.borrow_mut().clear());
    FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
    if COLOR.with(StdCell::get) {
        draw();
    } else {
        println!("{crumb}: {}", steps.join(", "));
    }
}
/// Make `name` the current step; the ones before it are done.
pub(super) fn work_step(name: &str) {
    let changed = WORK.with(|w| {
        let mut w = w.borrow_mut();
        let Some(work) = w.as_mut() else { return false };
        let Some(index) = work.steps.iter().position(|s| s.name == name) else { return false };
        if work.steps[index].state == StepState::Now {
            return false;
        }
        for (i, step) in work.steps.iter_mut().enumerate() {
            if i < index {
                step.state = StepState::Done;
                step.line.clear();
            } else if i == index {
                step.state = StepState::Now;
                step.line.clear();
            }
        }
        true
    });
    if changed {
        if COLOR.with(StdCell::get) { draw(); } else { println!("{name}…"); }
    }
}
/// The next menu keeps its own selection instead of the last work's row.
pub(super) fn forget_worked_on() {
    WORKED_ON.with(|w| w.borrow_mut().take());
}
pub(super) fn working_page() -> bool {
    WORK.with(|w| w.borrow().is_some())
}
/// Close the work page. A failure stays on screen, the step marked ✗ with
/// the short reason, until Return or Escape.
pub(super) fn work_end<T>(result: Result<T, String>) -> Result<T, String> {
    if let Err(error) = &result {
        let reason = clean(error.lines().next().unwrap_or_default());
        WORK.with(|w| {
            if let Some(work) = w.borrow_mut().as_mut() {
                // Side-by-side components mark their own failures; a single
                // step fails where the work stood.
                if !work.steps.iter().any(|s| matches!(s.state, StepState::Failed(_) | StepState::Held(_))) {
                    let index = work.steps.iter().position(|s| s.state == StepState::Now)
                        .or_else(|| work.steps.iter().position(|s| s.state == StepState::Next))
                        .unwrap_or(0);
                    if let Some(step) = work.steps.get_mut(index) {
                        step.state = StepState::Failed(reason.clone());
                        step.line.clear();
                    }
                }
            }
        });
        if COLOR.with(StdCell::get) {
            FOOTER.with(|f| f.set(Some("⏎ back")));
            draw();
            if let Ok(_input) = console::Input::enter() {
                while !matches!(console::key(), Ok(Key::Enter | Key::Back | Key::Quit) | Err(_)) {}
            }
        } else {
            println!("✗ {reason}");
        }
    }
    WORK.with(|w| *w.borrow_mut() = None);
    FOOTER.with(|f| f.set(None));
    result
}
/// The work page's body, in place of the view's rows.
fn work_rows() -> Option<Vec<Row>> {
    WORK.with(|w| {
        let w = w.borrow();
        let work = w.as_ref()?;
        let mut rows = vec![Row::Note(Vec::new())];
        for step in &work.steps {
            let name = &step.name;
            let mut spans = match &step.state {
                StepState::Done => vec![Span("✓ ".into(), OK), Span(padded(name, 16), PLAIN)],
                StepState::Now => vec![Span("● ".into(), ACC), Span(padded(name, 16), BOLD)],
                StepState::Next => vec![Span("○ ".into(), DIM), Span(padded(name, 16), DIM)],
                StepState::Failed(_) => vec![Span("✗ ".into(), RED), Span(padded(name, 16), PLAIN)],
                StepState::Held(_) => vec![Span("◌ ".into(), WARN), Span(padded(name, 16), PLAIN)],
            };
            match &step.state {
                StepState::Now | StepState::Done => spans.extend(step.line.iter().cloned()),
                StepState::Failed(reason) => spans.push(Span(reason.clone(), RED)),
                StepState::Held(reason) => spans.push(Span(reason.clone(), DIM)),
                StepState::Next => {}
            }
            rows.push(Row::Note(spans));
        }
        Some(rows)
    })
}
/// A step's bar and amount, from the progress hook: the current step's
/// when `name` is None.
fn work_line(name: Option<&str>, line: Text) {
    WORK.with(|w| {
        if let Some(work) = w.borrow_mut().as_mut() {
            let step = match name {
                Some(name) => work.steps.iter_mut().find(|s| s.name == name),
                None => work.steps.iter_mut().find(|s| s.state == StepState::Now),
            };
            if let Some(step) = step {
                step.line = line;
            }
        }
    });
}
/// Show a side-by-side component's row: its state and its line.
fn work_row(row: &progress::Row, line: Text) {
    WORK.with(|w| {
        let mut w = w.borrow_mut();
        let Some(work) = w.as_mut() else { return };
        let step = match work.steps.iter_mut().find(|s| s.name == row.label) {
            Some(step) => step,
            None => {
                work.steps.push(Step { name: row.label.clone(), state: StepState::Next, line: Vec::new() });
                work.steps.last_mut().unwrap()
            }
        };
        let detail = progress::Rows::detail(row);
        step.state = match &row.state {
            progress::RowState::Running => StepState::Now,
            progress::RowState::Done => StepState::Done,
            progress::RowState::Failed(reason) => StepState::Failed(clean(reason)),
            progress::RowState::Stopped | progress::RowState::Waiting => StepState::Held(detail.clone()),
        };
        step.line = match &row.state {
            progress::RowState::Done => vec![Span(detail, DIM)],
            _ => line,
        };
    });
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
    // Cargo's last line comes again with every crate counted; log it once.
    let mut logged_line = String::new();
    let mut started = Instant::now();
    let mut baseline = 0;
    let begun = Instant::now();
    // Components installing side by side: one row each on the work page,
    // redrawn at most every 66 ms (and at once when one starts or ends).
    let mut rows = progress::Rows::default();
    let mut printer = (!color).then(progress::printer);
    let mut logged: Vec<(String, String)> = Vec::new();
    let mut drawn = Instant::now() - Duration::from_secs(1);
    let result = progress::scope(
        move |p| {
            if !p.row.is_empty() {
                // The log gets each component's notes and its one-line end
                // (the timing lines follow the install); stages flip with
                // every payload on many threads and are not logged.
                if p.stage == "Working" && !logged.iter().any(|(row, note)| *row == p.row && *note == p.detail) {
                    activity(&format!("{}: {}", p.row, p.detail.trim()));
                    logged.push((p.row.clone(), p.detail.clone()));
                }
                let changed = rows.update(&p);
                if let Some(row) = rows.rows.iter().find(|r| r.label == p.row && r.state != progress::RowState::Running) {
                    if changed {
                        activity(&format!("{}: {}", row.label, progress::Rows::end_line(row)));
                    }
                }
                if let Some(print) = printer.as_mut() {
                    print(p);
                    return;
                }
                if !changed && drawn.elapsed() < Duration::from_millis(66) {
                    return;
                }
                drawn = Instant::now();
                let width = console::size().0.min(80);
                for row in &rows.rows {
                    let line = row_line(row, &begun, width);
                    if working_page() {
                        work_row(row, line);
                    } else if row.state == progress::RowState::Running {
                        let mut spans = vec![Span(format!("{} ", padded(&row.label, 13)), PLAIN)];
                        spans.extend(line);
                        MESSAGE.with(|m| *m.borrow_mut() = spans);
                        CHOICE.with(|c| c.borrow_mut().clear());
                    }
                }
                FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
                draw();
                return;
            }
            let phase = format!("{}:{}:{}", p.package.group, p.package.index, p.stage);
            let changed = identity != phase;
            if changed {
                identity = phase;
                started = Instant::now();
                baseline = p.loaded;
                let group = if p.package.group.is_empty() { String::new() } else { format!("{} {}: ", p.package.group, p.package.name) };
                activity(&format!("{group}{}: {}", p.stage, p.detail));
            } else if p.stage == "Compiling Rust" && !p.detail.trim().is_empty() && p.detail != logged_line {
                activity(&p.detail);
            }
            if p.stage == "Compiling Rust" {
                logged_line = p.detail.clone();
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
                progress::Unit::Crates if p.total > 0 => format!("{} / {} crates", p.loaded, p.total),
                progress::Unit::Crates => format!("{} crates compiled", p.loaded),
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
                // The count says it all; crate names flickering past do not.
                String::new()
            } else if matches!(p.unit, progress::Unit::None) {
                p.detail.clone()
            } else {
                p.stage.to_lowercase()
            };
            let what = [amount, clean(&what)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            // A component with a known total shows one forward-only bar for all of
            // it, the MB done of its payload and what it is doing now.
            let (label, fraction, what) = match &p.overall {
                Some(overall) => {
                    let mb = |bytes: f64| bytes / 1048576.;
                    let doing = match p.stage.as_str() {
                        "Download" => "downloading",
                        "Verify cache" => "checking downloads",
                        _ => "unpacking",
                    };
                    let amount = format!("{:.0} / {:.0} MB", mb(overall.fraction * overall.bytes as f64), mb(overall.bytes as f64));
                    (overall.label.clone(), Some(overall.fraction), format!("{amount} · {doing}"))
                }
                None => (label, fraction, what),
            };
            if !color {
                if changed { println!("{label}: {what}"); }
                return;
            }
            let label: String = label.chars().take(22).collect();
            let mut spans = Vec::new();
            spans.push(Span(format!("{} ", padded(&label, 13)), PLAIN));
            let width = 30;
            match fraction {
                Some(f) => {
                    let filled = (f * width as f64).round() as usize;
                    spans.push(Span("━".repeat(filled), ACC));
                    spans.push(Span("─".repeat(width - filled), DIM));
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
            if working_page() {
                // The page lists the steps itself: the bar goes on the current
                // step's line, without the label that line already has. A
                // component is current from its first package, before its
                // total is known (Rust reads its manifest first).
                match &p.overall {
                    Some(overall) => work_step(&overall.label),
                    None if !p.package.group.is_empty() => work_step(&p.package.group),
                    None => (),
                }
                let label_span = format!("{} ", padded(&label, 13));
                let name = p.overall.as_ref().map(|o| o.label.clone());
                work_line(name.as_deref(), spans.into_iter().filter(|s| s.0 != label_span).collect());
            } else {
                MESSAGE.with(|m| *m.borrow_mut() = spans);
                CHOICE.with(|c| c.borrow_mut().clear());
            }
            FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
            draw();
        },
        work,
    );
    FOOTER.with(|f| f.set(None));
    MESSAGE.with(|m| m.borrow_mut().clear());
    CHOICE.with(|c| c.borrow_mut().clear());
    result
}

/// A side-by-side component's line after its name: its bar, percentage
/// and figures, cut to the width.
fn row_line(row: &progress::Row, begun: &Instant, width: usize) -> Text {
    const BAR: usize = 16;
    let mut spans = Vec::new();
    match (&row.state, row.fraction) {
        (progress::RowState::Running, Some(f)) => {
            let filled = (f * BAR as f64).round() as usize;
            spans.push(Span("━".repeat(filled), ACC));
            spans.push(Span("─".repeat(BAR - filled), DIM));
            spans.push(Span(format!(" {:3.0}%", f * 100.), PLAIN));
        }
        (progress::RowState::Running, None) => {
            let frame = (begun.elapsed().as_millis() / 100) as usize % SPINNER.len();
            spans.push(Span(SPINNER[frame].to_string(), ACC));
        }
        _ => {}
    }
    // Rows are indented 4, then the mark and the 16-wide name.
    let room = width.saturating_sub(4 + 2 + 16 + BAR + 5 + 2 + 1);
    // Whole figures only: drop the last ones that do not fit.
    let mut detail = String::new();
    for part in progress::Rows::detail(row).split(" · ") {
        let next = if detail.is_empty() { part.to_string() } else { format!("{detail} · {part}") };
        if next.chars().count() > room {
            if detail.is_empty() {
                detail = part.chars().take(room).collect();
            }
            break;
        }
        detail = next;
    }
    if !detail.is_empty() {
        spans.push(Span(format!("  {detail}"), DIM));
    }
    spans
}
