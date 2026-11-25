//! The master's menu-bar item on the Mac, shown by a helper process: this app's own executable
//! with [`FLAG`], a child of the master that never execs ([`makepad_devhub::helper`]). macOS 27
//! takes no status item from a process that exec'd: the master's self-updates exec in place, and
//! after the first one its item was an empty pill (MenuBarAgent keeps the first image "pending
//! exit" by pid and cancels every later image's connection, 11-17). Each image of the master takes
//! the same helper on, hands it the glyph and the menu, and hears the items chosen in it.
//!
//! The helper also shows the devhub panel's window ([`crate::panel_window`]): devhub is one app
//! with one item, and a process that never execs keeps the panel on screen through the master's
//! self-updates. A self-update to other code starts the next helper beside this one
//! ([`makepad_devhub::helper::Spec::ready`]): it opens the panel where this one shows it and says
//! it is ready once its panel shows a fleet ([`Answer::Ready`]), and the master ends the old one.
//! The new one shows the item once the old one let go of it (the item's lock): never two items, and
//! the panel never leaves the screen.

use makepad_strict_json::{obj, s, Value};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The argument that makes this binary the menu-bar item's helper.
pub const FLAG: &str = "--menu-bar-item";

/// What carries the helper over the master's exec ([`makepad_devhub::helper::Spec::env`]).
#[cfg(target_os = "macos")]
const ENV: &str = "DEVHUB_MENU_ITEM";

/// The master's side: its helper, and the items chosen in its menu not taken yet.
pub struct ItemLink {
    #[cfg(target_os = "macos")]
    helper: makepad_devhub::helper::Helper,
    chosen: Arc<Mutex<Vec<LiveId>>>,
}

impl ItemLink {
    /// The helper the master's last image left it, else a new one (the Mac only: elsewhere the
    /// master sets its tray itself).
    pub fn start() -> Option<ItemLink> {
        #[cfg(target_os = "macos")]
        {
            let chosen: Arc<Mutex<Vec<LiveId>>> = Arc::default();
            let c = chosen.clone();
            let helper = makepad_devhub::helper::Helper::start(makepad_devhub::helper::Spec {
                env: ENV,
                version: image_id(),
                // (the bundle's executable when the master runs from it, as its self-update and its
                // LaunchAgent start it: the item has the app's identity)
                command: Box::new(|| {
                    let exe = std::env::current_exe().map_err(|e| format!("its executable: {e}"))?;
                    let mut cmd = std::process::Command::new(exe);
                    cmd.arg(FLAG).env_remove("MAKEPAD_DEVHUB_OPEN");
                    Ok(cmd)
                }),
                answer: Box::new(move |line| match Answer::parse(&line) {
                    Some(Answer::Item(item)) => {
                        c.lock().unwrap().push(item);
                        makepad_widgets::makepad_platform::thread::SignalToUI::set_ui_signal();
                    }
                    Some(Answer::Ready) => {}
                    Some(Answer::Panel(open)) => eprintln!("devhub: the panel {}", if open { "opened" } else { "closed" }),
                    None => eprintln!("devhub: the menu-bar item's helper said {line:?}"),
                }),
                say: Box::new(|line| eprintln!("devhub: the menu-bar item: {line}")),
                pause: std::time::Duration::from_secs(2),
                ready: Some(Box::new(|line| Answer::parse(line) == Some(Answer::Ready))),
            });
            Some(ItemLink { helper, chosen })
        }
        #[cfg(not(target_os = "macos"))]
        None
    }

    /// Shows `icon` in the menu bar: the helper gets it, or the newest state after it should it
    /// fall behind. `open_ms`: when the panel was last asked open from outside the menu (a launch of
    /// devhub beside the master), 0 for never. Never waits.
    pub fn show(&self, icon: &TrayIcon, open_ms: u64) {
        #[cfg(target_os = "macos")]
        self.helper.send(encode(icon, open_ms));
        #[cfg(not(target_os = "macos"))]
        let _ = (icon, open_ms);
    }

    /// The items chosen in the menu since the last call.
    pub fn chosen(&self) -> Vec<LiveId> {
        std::mem::take(&mut *self.chosen.lock().unwrap())
    }
}

/// The linker's id of this executable (its LC_UUID: it changes with the code, not with a copy or a
/// signature), as hex: a self-update to other code replaces the helper too.
#[cfg(target_os = "macos")]
fn image_id() -> String {
    extern "C" {
        fn _dyld_get_image_header(index: u32) -> *const u8;
    }
    // SAFETY: dyld's image 0 is the main executable, its mach_header_64 mapped with its load commands
    unsafe {
        let h = _dyld_get_image_header(0);
        let rd = |p: *const u8| std::ptr::read_unaligned(p as *const u32);
        if !h.is_null() && rd(h) == 0xfeed_facf {
            let mut c = h.add(32);
            for _ in 0..rd(h.add(16)) {
                // LC_UUID
                if rd(c) == 0x1b {
                    return std::slice::from_raw_parts(c.add(8), 16).iter().map(|b| format!("{b:02x}")).collect();
                }
                c = c.add(rd(c.add(4)) as usize);
            }
        }
    }
    "unknown".into()
}

/// What a helper tells its master, one line each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// an item chosen in its menu
    Item(LiveId),
    /// what it showed at its start shows (the panel, kept open): the helper it replaces may go
    Ready,
    /// the panel's window opened (true) or closed
    Panel(bool),
}

impl Answer {
    #[cfg(any(unix, test))]
    fn line(self) -> String {
        match self {
            Answer::Item(item) => obj(vec![("item", s(item.0.to_string()))]),
            Answer::Ready => obj(vec![("ready", Value::Bool(true))]),
            Answer::Panel(open) => obj(vec![("panel", Value::Bool(open))]),
        }
        .to_json()
    }

    #[cfg(any(target_os = "macos", test))]
    fn parse(line: &str) -> Option<Answer> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        if let Some(item) = v.get("item") {
            return Some(Answer::Item(LiveId(item.as_str()?.parse().ok()?)));
        }
        if v.get("ready").and_then(Value::as_bool) == Some(true) {
            return Some(Answer::Ready);
        }
        v.get("panel").and_then(Value::as_bool).map(Answer::Panel)
    }
}

/// The helper's side: the newest state its master sent, whether it holds the item yet, and where
/// its answers go.
pub struct Host {
    latest: Arc<Mutex<Option<String>>>,
    /// it holds the item's lock: the helper before it let go of the item
    holds: Arc<AtomicBool>,
    /// the newest ask to open the panel it answered (at first its start: an older ask was the
    /// helper's before it)
    opened_ms: AtomicU64,
    #[cfg(unix)]
    answers: Mutex<std::fs::File>,
    #[cfg(unix)]
    _lock: std::fs::File,
}

impl Host {
    /// Keeps stdout for the answers (anything else written there goes to the log), reads the
    /// master's states until its pipe ends, when the helper ends too, and waits for the one
    /// helper's lock on a thread of its own: the helper this one replaces holds it until it ends,
    /// and only then this one shows the item.
    #[cfg(unix)]
    pub fn start() -> Result<Host, String> {
        use std::io::BufRead;
        use std::os::fd::AsFd;
        extern "C" {
            fn flock(fd: i32, op: i32) -> i32;
            fn dup2(from: i32, to: i32) -> i32;
        }
        const LOCK_EX: i32 = 2;
        const LOCK_NB: i32 = 4;
        let path = makepad_devhub::proto::home().join("menu-item.lock");
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let fd = std::os::fd::AsRawFd::as_raw_fd(&lock);
        let holds = Arc::new(AtomicBool::new(false));
        // SAFETY: flock on a descriptor this Host owns for the life of the process
        if unsafe { flock(fd, LOCK_EX | LOCK_NB) } == 0 {
            holds.store(true, Ordering::Release);
        } else {
            eprintln!("devhub: menu-bar item: another helper holds {}: its item stays until it ends", path.display());
            let h = holds.clone();
            std::thread::Builder::new().name("devhub-menu-item-lock".into()).spawn(move || {
                // SAFETY: as above; a blocking flock, off the UI thread
                while unsafe { flock(fd, LOCK_EX) } != 0 {
                    if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                        return eprintln!("devhub: menu-bar item: its lock: {}", std::io::Error::last_os_error());
                    }
                }
                eprintln!("devhub: menu-bar item: the helper before this one let go of the item");
                h.store(true, Ordering::Release);
                makepad_widgets::makepad_platform::thread::SignalToUI::set_ui_signal();
            }).map_err(|e| format!("its lock's thread: {e}"))?;
        }
        let answers = std::io::stdout().as_fd().try_clone_to_owned().map_err(|e| format!("its answers: {e}"))?;
        // SAFETY: descriptors only: stray output goes to the log, never into the answers
        unsafe { dup2(2, 1) };
        let latest: Arc<Mutex<Option<String>>> = Arc::default();
        let l = latest.clone();
        std::thread::Builder::new().name("devhub-menu-item".into()).spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if line.is_empty() { continue; }
                *l.lock().unwrap() = Some(line);
                makepad_widgets::makepad_platform::thread::SignalToUI::set_ui_signal();
            }
            // the master ended (or let it go): so does its item
            std::process::exit(0);
        }).map_err(|e| format!("its reader: {e}"))?;
        Ok(Host { latest, holds, opened_ms: AtomicU64::new(makepad_devhub::proto::wall_ms()), answers: Mutex::new(std::fs::File::from(answers)), _lock: lock })
    }

    #[cfg(not(unix))]
    pub fn start() -> Result<Host, String> {
        Err("the menu-bar item's helper runs on the Mac only".into())
    }

    /// The newest state the master sent, as tray `id`, once this helper holds the item; and
    /// whether it asks the panel open (an ask since the last one this helper answered).
    pub fn take(&self, id: LiveId) -> Option<(TrayIcon, bool)> {
        if !self.holds.load(Ordering::Acquire) {
            return None;
        }
        let line = self.latest.lock().unwrap().take()?;
        let Some((icon, open_ms)) = decode(&line, id) else {
            eprintln!("devhub: the menu-bar item: no state in {line:?}");
            return None;
        };
        let open = open_ms > self.opened_ms.fetch_max(open_ms, Ordering::Relaxed);
        Some((icon, open))
    }

    /// Tells the master `a`.
    pub fn say(&self, a: Answer) {
        #[cfg(unix)]
        {
            use std::io::Write;
            let mut out = self.answers.lock().unwrap();
            let _ = writeln!(out, "{}", a.line()).and_then(|()| out.flush());
        }
        #[cfg(not(unix))]
        let _ = a;
    }
}

/// `icon` as one line for the helper: its glyph's alpha in hex, its menu items; and `open_ms`.
#[cfg(any(target_os = "macos", test))]
pub fn encode(icon: &TrayIcon, open_ms: u64) -> String {
    let alpha: String = icon.image.alpha.iter().map(|b| format!("{b:02x}")).collect();
    let menu = icon.menu.iter().map(|m| match m {
        TrayMenuItem::Item { id, label, enabled, checked } => obj(vec![("id", s(id.0.to_string())), ("label", s(label.as_str())),
            ("enabled", Value::Bool(*enabled)), ("checked", checked.map_or(Value::Null, Value::Bool))]),
        TrayMenuItem::Line => obj(vec![("line", Value::Bool(true))]),
    }).collect();
    obj(vec![("w", Value::Int(icon.image.width as i64)), ("h", Value::Int(icon.image.height as i64)), ("alpha", s(alpha)),
        ("tooltip", s(icon.tooltip.as_str())), ("dropdown", Value::Bool(icon.dropdown)), ("menu", Value::Arr(menu)), ("open_ms", Value::Int(open_ms as i64))]).to_json()
}

/// [`encode`]'s line as tray `id`, and its `open_ms` (0 when it has none).
pub fn decode(line: &str, id: LiveId) -> Option<(TrayIcon, u64)> {
    let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
    let hex = v.get("alpha")?.as_str()?;
    let alpha = (0..hex.len() / 2).map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>()?;
    let menu = v.get("menu")?.as_arr()?.iter().map(|m| {
        if m.get("line").and_then(Value::as_bool) == Some(true) {
            return Some(TrayMenuItem::Line);
        }
        Some(TrayMenuItem::Item { id: LiveId(m.get("id")?.as_str()?.parse().ok()?), label: m.get("label")?.as_str()?.to_string(),
            enabled: m.get("enabled")?.as_bool()?, checked: m.get("checked").and_then(Value::as_bool) })
    }).collect::<Option<Vec<_>>>()?;
    let icon = TrayIcon { id, image: TrayImage { width: v.get("w")?.as_u64()? as u32, height: v.get("h")?.as_u64()? as u32, alpha },
        tooltip: v.get("tooltip")?.as_str()?.to_string(), menu, dropdown: v.get("dropdown")?.as_bool()? };
    Some((icon, v.get("open_ms").and_then(Value::as_u64).unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glyph, the whole menu and the panel's last open ask cross the pipe to the helper as
    /// they were; the helper's answers come back as they were said.
    #[test]
    fn the_menu_bar_item_crosses_to_its_helper_whole() {
        let id = LiveId(7);
        let icon = TrayIcon { id, image: TrayImage { width: 2, height: 2, alpha: vec![0, 255, 16, 1] }, tooltip: String::new(), dropdown: false,
            menu: vec![TrayMenuItem::Item { id: LiveId(u64::MAX - 3), label: "Dev on a \u{b7} \"in use\"\n".into(), enabled: false, checked: None },
                TrayMenuItem::Line, TrayMenuItem::check(LiveId(9), "Pause landing", true)] };
        let line = encode(&icon, 1_800_000_000_123);
        assert!(!line.contains('\n'), "one line: {line}");
        assert_eq!(decode(&line, id), Some((icon, 1_800_000_000_123)));
        assert_eq!(decode("{\"w\":2}", id), None, "a state with parts missing shows nothing");
        for a in [Answer::Item(LiveId(u64::MAX - 3)), Answer::Ready, Answer::Panel(true), Answer::Panel(false)] {
            assert_eq!(Answer::parse(&a.line()), Some(a));
        }
        assert_eq!(Answer::parse("{\"said\":1}"), None);
    }
}
