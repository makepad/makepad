//! Command line and on-disk state. Everything here is `Cx`-free so it can be
//! unit-tested without a window.
//!
//! Layout of the state directory (`--state-dir <dir>`, default
//! `~/.makepad/<app>`):
//!
//! - `settings.ron` — appearance choice (`Settings`)
//! - `dock.ron`     — the dock layout (`HashMap<LiveId, DockItem>`)

use makepad_widgets::dock::DockItem;
use makepad_widgets::makepad_micro_serde::*;
use makepad_widgets::makepad_platform::home::makepad_home;
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What the command line asked for. Unknown arguments (`--remote`, the WM's
/// `--stdin-loop`, ...) belong to the platform and are ignored here.
#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    /// The app's name under `~/.makepad/` when no `--state-dir` is given.
    pub app: &'static str,
    /// Where settings and the dock layout live; `None` = the user's default.
    pub state_dir: Option<PathBuf>,
    /// Working directory for new terminals; `None` = the process cwd.
    pub cwd: Option<PathBuf>,
    /// Optional standalone window size in layout points.
    pub window_size: Option<(u32, u32)>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            app: "studio",
            state_dir: None,
            cwd: None,
            window_size: None,
        }
    }
}

impl Args {
    pub fn from_env() -> Self {
        parse_args(std::env::args().skip(1))
    }

    /// The command line for an app whose default state directory is
    /// `~/.makepad/<app>`.
    pub fn from_env_named(app: &'static str) -> Self {
        let mut args = parse_args(std::env::args().skip(1));
        args.app = app;
        args
    }

    /// The directory settings and layout are read from and written to.
    pub fn state_dir(&self) -> PathBuf {
        self.state_dir
            .clone()
            .unwrap_or_else(|| makepad_home().join(self.app))
    }
}

pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Args {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state-dir" => out.state_dir = args.next().map(PathBuf::from),
            "--cwd" => out.cwd = args.next().map(PathBuf::from),
            "--size" => out.window_size = args.next().and_then(|s| parse_size(&s)),
            _ => {
                if let Some(v) = arg.strip_prefix("--state-dir=") {
                    out.state_dir = Some(PathBuf::from(v));
                } else if let Some(v) = arg.strip_prefix("--size=") {
                    out.window_size = parse_size(v);
                } else if let Some(v) = arg.strip_prefix("--cwd=") {
                    out.cwd = Some(PathBuf::from(v));
                }
            }
        }
    }
    out
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    if (360..=8192).contains(&w) && (300..=8192).contains(&h) {
        Some((w, h))
    } else {
        None
    }
}

/// Persisted user settings. Kept deliberately small for the shell slice.
#[derive(Clone, Debug, Default, PartialEq, SerRon, DeRon)]
pub struct Settings {
    /// Style family id (`"macos"`, `"windows-2000"`, ...); `None` follows the
    /// host OS family and native appearance where available.
    pub style: Option<String>,
    /// Manual dark preference for an explicit family, or for a platform that
    /// cannot report its native appearance.
    pub dark: bool,
    /// Display cells per four source indentation cells in Architecture.
    pub architecture_indent_cells: Option<u32>,
}

const SETTINGS_FILE: &str = "settings.ron";
const DOCK_FILE: &str = "dock.ron";

impl Settings {
    pub fn code_indent_cells(&self) -> u32 {
        self.architecture_indent_cells.unwrap_or(2).clamp(1, 8)
    }
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|text| Self::deserialize_ron(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        write_atomic(dir, SETTINGS_FILE, &self.serialize_ron())
    }
}

#[derive(SerRon, DeRon)]
struct DockStateRon {
    items: HashMap<LiveId, DockItem>,
}

/// The dock layout as last saved, or `None` when absent or unreadable.
pub fn load_dock(dir: &Path) -> Option<HashMap<LiveId, DockItem>> {
    let text = std::fs::read_to_string(dir.join(DOCK_FILE)).ok()?;
    decode_dock(&text)
}

pub fn save_dock(dir: &Path, items: &HashMap<LiveId, DockItem>) -> std::io::Result<()> {
    write_atomic(dir, DOCK_FILE, &encode_dock(items))
}

pub fn encode_dock(items: &HashMap<LiveId, DockItem>) -> String {
    DockStateRon {
        items: items.clone(),
    }
    .serialize_ron()
}

pub fn decode_dock(text: &str) -> Option<HashMap<LiveId, DockItem>> {
    if text.len() > 131072 {
        return None;
    }
    DockStateRon::deserialize_ron(text).ok().map(|s| s.items)
}

fn write_atomic(dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!(
        ".{name}.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        drop(file);
        std::fs::rename(&path, dir.join(name))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Require a bounded rooted tree: no cycles, multiple parents, orphan nodes,
/// dangling references, invalid selections or unsupported terminal/tab kinds.
pub fn dock_state_is_usable(items: &HashMap<LiveId, DockItem>, kinds: &[LiveId]) -> bool {
    if items.len() > 512 {
        return false;
    }
    let mut visited = HashSet::new();
    let mut pending = vec![(id!(root), false)];
    let mut tabs_seen = 0;
    while let Some((id, must_be_tab)) = pending.pop() {
        if !visited.insert(id) {
            return false;
        }
        let Some(item) = items.get(&id) else {
            return false;
        };
        if must_be_tab != matches!(item, DockItem::Tab { .. }) {
            return false;
        }
        match item {
            DockItem::Splitter { a, b, align, .. } => {
                use makepad_widgets::splitter::SplitterAlign;
                let valid = match align {
                    SplitterAlign::Weighted(v) => v.is_finite() && (0.0..=1.0).contains(v),
                    SplitterAlign::FromA(v) | SplitterAlign::FromB(v) => v.is_finite() && *v >= 0.0,
                };
                if !valid {
                    return false;
                }
                pending.push((*a, false));
                pending.push((*b, false));
            }
            DockItem::Tabs { tabs, selected, .. } => {
                tabs_seen += 1;
                if (!tabs.is_empty() && *selected >= tabs.len())
                    || (tabs.is_empty() && *selected != 0)
                {
                    return false;
                }
                pending.extend(tabs.iter().map(|id| (*id, true)));
            }
            DockItem::Tab { kind, .. } => {
                if !kinds.contains(kind) {
                    return false;
                }
            }
        }
    }
    tabs_seen > 0 && visited.len() == items.len()
}

/// Older theme reloads could reinsert deleted default containers beside the
/// live tree. Recover only a fully valid rooted layout; never repair missing
/// children, cycles, unsupported tabs or invalid selections in visible panes.
pub fn recover_rooted_dock(
    items: &HashMap<LiveId, DockItem>,
    kinds: &[LiveId],
) -> Option<HashMap<LiveId, DockItem>> {
    if items.len() > 512 {
        return None;
    }
    let mut reachable = HashSet::new();
    let mut pending = vec![id!(root)];
    while let Some(id) = pending.pop() {
        if !reachable.insert(id) {
            continue;
        }
        match items.get(&id)? {
            DockItem::Splitter { a, b, .. } => pending.extend([*a, *b]),
            DockItem::Tabs { tabs, .. } => pending.extend(tabs.iter().copied()),
            DockItem::Tab { .. } => {}
        }
    }
    let rooted: HashMap<_, _> = items
        .iter()
        .filter(|(id, _)| reachable.contains(id))
        .map(|(id, item)| (*id, item.clone()))
        .collect();
    dock_state_is_usable(&rooted, kinds).then_some(rooted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::splitter::{SplitterAlign, SplitterAxis};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "makepad-studio-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir
    }

    #[test]
    fn args_parse_both_spellings() {
        let a = parse_args(["--state-dir", "/tmp/s", "--cwd=/tmp/w", "--remote"].map(String::from));
        assert_eq!(a.state_dir, Some(PathBuf::from("/tmp/s")));
        assert_eq!(a.cwd, Some(PathBuf::from("/tmp/w")));
        let b = parse_args(["--state-dir=/x", "--cwd", "/y"].map(String::from));
        assert_eq!(b.state_dir, Some(PathBuf::from("/x")));
        assert_eq!(b.cwd, Some(PathBuf::from("/y")));
        assert_eq!(parse_args(Vec::<String>::new()), Args::default());
        assert!(Args::default().state_dir().ends_with("studio"));
    }

    #[test]
    fn settings_roundtrip_and_missing_file_is_default() {
        let dir = temp_dir("settings");
        assert_eq!(Settings::load(&dir), Settings::default());
        assert_eq!(Settings::deserialize_ron("(dark: false)").unwrap().code_indent_cells(), 2);
        let s = Settings {
            style: Some("windows-2000".into()),
            dark: true,
            architecture_indent_cells: Some(2),
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        std::fs::write(dir.join(SETTINGS_FILE), "not ron").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn layout() -> HashMap<LiveId, DockItem> {
        let mut m = HashMap::new();
        m.insert(
            id!(root),
            DockItem::splitter(
                SplitterAxis::Horizontal,
                SplitterAlign::Weighted(0.5),
                id!(left),
                id!(right),
            ),
        );
        m.insert(id!(left), DockItem::tabs(vec![id!(t1)], 0, true));
        m.insert(id!(right), DockItem::tabs(vec![id!(s1)], 0, true));
        m.insert(
            id!(t1),
            DockItem::tab("Terminal".into(), id!(TerminalTab), id!(CloseableTab)),
        );
        m.insert(
            id!(s1),
            DockItem::tab("Settings".into(), id!(SettingsTab), id!(CloseableTab)),
        );
        m
    }

    #[test]
    fn dock_roundtrip_preserves_layout() {
        let dir = temp_dir("dock");
        assert!(load_dock(&dir).is_none());
        let items = layout();
        save_dock(&dir, &items).unwrap();
        let back = load_dock(&dir).unwrap();
        assert_eq!(back.len(), items.len());
        assert!(matches!(
            back.get(&id!(root)),
            Some(DockItem::Splitter { .. })
        ));
        assert!(matches!(
            back.get(&id!(t1)),
            Some(DockItem::Tab { name, kind, .. }) if name == "Terminal" && *kind == id!(TerminalTab)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_theme_default_orphans_without_accepting_broken_visible_layouts() {
        let kinds = [id!(TerminalTab), id!(SettingsTab)];
        let mut items = layout();
        // A deleted default group reappeared on reload with a stale selection
        // and an ID already used by the live group. Neither is reachable.
        items.insert(id!(old_default), DockItem::tabs(vec![id!(t1)], 1, true));
        assert!(!dock_state_is_usable(&items, &kinds));
        let recovered = recover_rooted_dock(&items, &kinds).unwrap();
        assert_eq!(recovered.len(), layout().len());
        assert!(!recovered.contains_key(&id!(old_default)));
        assert!(
            matches!(&recovered[&id!(left)], DockItem::Tabs { tabs, selected: 0, .. } if tabs == &[id!(t1)])
        );
        items.remove(&id!(t1));
        assert!(recover_rooted_dock(&items, &kinds).is_none());
        let mut invalid = layout();
        invalid.insert(id!(left), DockItem::tabs(vec![id!(t1)], 1, true));
        assert!(recover_rooted_dock(&invalid, &kinds).is_none());
        invalid.insert(
            id!(left),
            DockItem::splitter(
                SplitterAxis::Horizontal,
                SplitterAlign::Weighted(0.5),
                id!(root),
                id!(right),
            ),
        );
        assert!(recover_rooted_dock(&invalid, &kinds).is_none());
    }

    #[test]
    fn dock_state_validation() {
        let kinds = [id!(TerminalTab), id!(SettingsTab)];
        assert!(dock_state_is_usable(&layout(), &kinds));

        let mut no_root = layout();
        no_root.remove(&id!(root));
        assert!(!dock_state_is_usable(&no_root, &kinds));

        let mut dangling = layout();
        dangling.remove(&id!(s1));
        assert!(!dock_state_is_usable(&dangling, &kinds));

        let mut unknown_kind = layout();
        unknown_kind.insert(
            id!(t1),
            DockItem::tab("X".into(), id!(GoneTab), id!(CloseableTab)),
        );
        assert!(!dock_state_is_usable(&unknown_kind, &kinds));

        let mut cycle = layout();
        cycle.insert(
            id!(left),
            DockItem::splitter(
                SplitterAxis::Horizontal,
                SplitterAlign::Weighted(0.5),
                id!(root),
                id!(right),
            ),
        );
        assert!(!dock_state_is_usable(&cycle, &kinds));
        let mut orphan = layout();
        orphan.insert(id!(orphan), DockItem::tabs(vec![], 0, true));
        assert!(!dock_state_is_usable(&orphan, &kinds));
        let mut shared = layout();
        shared.insert(id!(right), DockItem::tabs(vec![id!(t1)], 0, true));
        assert!(!dock_state_is_usable(&shared, &kinds));

        let mut bad_selected = layout();
        bad_selected.insert(id!(left), DockItem::tabs(vec![id!(t1)], 3, true));
        assert!(!dock_state_is_usable(&bad_selected, &kinds));
    }
}
