//! The foreign apps the launcher offers: only those listed in the user's config (opt-in; nothing is
//! discovered), each with its own env (an unpacked browser needs `LD_LIBRARY_PATH`, say).
//!
//! `~/.makepad/wm/wayland_apps.txt`, one app per line (`#` starts a comment):
//!
//! ```text
//! id | Label | VAR=value VAR2=value | command and arguments
//! chromium | Chromium | LD_LIBRARY_PATH=$HOME/wlwm/root/usr/lib | $HOME/wlwm/root/usr/lib/chromium/chromium --ozone-platform=wayland --user-data-dir=$HOME/wlwm/chrome-profile
//! ```
//!
//! `$HOME` and a leading `~` expand.
//! Launcher ids are `wl.<id>`.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignApp {
    /// `wl.<slug>`: the launcher and remote id.
    pub id: String,
    pub label: String,
    pub env: Vec<(String, String)>,
    pub argv: Vec<String>,
}

pub fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    s.trim_matches('-').to_string()
}

/// Splits an `Exec=` line like a shell would for the simple cases (quotes, backslashes), dropping
/// the field codes (`%f`, `%U`, ...) and keeping `%%` as `%`.
pub fn split_exec(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut have = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', _) => { if let Some(n) = chars.next() { cur.push(n); have = true; } }
            ('"' | '\'', None) => { quote = Some(c); have = true; }
            (q, Some(open)) if q == open => quote = None,
            ('%', _) => match chars.next() {
                Some('%') => { cur.push('%'); have = true; }
                Some(_) | None => {}
            },
            (c, None) if c.is_whitespace() => { if have { out.push(std::mem::take(&mut cur)); have = false; } }
            (c, _) => { cur.push(c); have = true; }
        }
    }
    if have { out.push(cur); }
    out
}

fn expand(s: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    let s = s.replace("$HOME", &h);
    match s.strip_prefix("~/") { Some(r) => format!("{h}/{r}"), None => s }
}

pub fn parse_config(text: &str, home: &Path) -> Vec<ForeignApp> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let parts: Vec<&str> = line.splitn(4, '|').map(str::trim).collect();
        let [id, label, env, cmd] = parts[..] else { continue };
        let id = slug(id);
        let argv: Vec<String> = split_exec(cmd).into_iter().map(|a| expand(&a, home)).collect();
        if id.is_empty() || argv.is_empty() { continue; }
        let env = env.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), expand(v, home))).collect();
        out.push(ForeignApp { id: format!("wl.{id}"), label: if label.is_empty() { id.clone() } else { label.to_string() }, env, argv });
    }
    out
}

/// The catalog: exactly the apps listed in the config (opt-in; nothing is discovered).
pub fn load(home: &Path) -> Vec<ForeignApp> {
    std::fs::read_to_string(home.join(".makepad/wm/wayland_apps.txt")).map(|t| parse_config(&t, home)).unwrap_or_default()
}

/// The app's icon: a hicolor PNG of the program's own icon theme (an unpacked package carries
/// `usr/share/icons/hicolor` above its binary) or the system one, largest size first.
pub fn icon_path(app: &ForeignApp) -> Option<PathBuf> {
    let program = Path::new(app.argv.first()?);
    let stem = program.file_stem()?.to_str()?.to_string();
    let mut roots: Vec<PathBuf> = program.ancestors().skip(1).map(|a| a.join("usr/share/icons/hicolor")).collect();
    roots.push(PathBuf::from("/usr/share/icons/hicolor"));
    for root in roots {
        for size in ["256x256", "128x128", "64x64", "48x48"] {
            let f = root.join(size).join("apps").join(format!("{stem}.png"));
            if f.is_file() { return Some(f); }
        }
    }
    None
}

/// A window `app_id` or a program name reduced to what names the application: lowercase, without
/// a `.desktop` suffix or a reverse-DNS prefix (`org.chromium.Chromium` is `chromium`), as a slug,
/// without a trailing packaging word (`chromium-browser`, `foo-bin`, `google-chrome-stable`).
/// Windows, entries and programs of one application then compare equal whichever way the toolkit
/// spelled its id.
pub fn app_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    let lower = lower.strip_suffix(".desktop").unwrap_or(&lower);
    let parts: Vec<&str> = lower.split('.').filter(|p| !p.is_empty()).collect();
    let reverse_dns = parts.len() >= 3
        || (parts.len() == 2 && matches!(parts[0], "org" | "com" | "net" | "io" | "dev" | "app" | "de" | "fr" | "me"));
    let base = if reverse_dns { parts[parts.len() - 1] } else { lower };
    let mut key = slug(base);
    while let Some(stripped) = ["-browser", "-bin", "-stable", "-beta", "-nightly"].iter().find_map(|s| key.strip_suffix(s)) {
        if stripped.is_empty() { break; }
        key = stripped.to_string();
    }
    key
}

/// The catalog entry a window belongs to, by its `app_id`: the entry's id, else the name of the
/// program it starts (both compared by `app_key`). Windows of one entry group under the entry's
/// launcher id, like windows of one native app.
pub fn for_window<'a>(apps: &'a [ForeignApp], app_id: &str) -> Option<&'a ForeignApp> {
    let want = app_key(app_id);
    if want.is_empty() { return None; }
    apps.iter().find(|a| a.id.strip_prefix("wl.").is_some_and(|id| app_key(id) == want))
        .or_else(|| apps.iter().find(|a| {
            a.argv.first().and_then(|p| Path::new(p).file_name()).and_then(|n| n.to_str()).is_some_and(|n| app_key(n) == want)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_lines_split_like_a_shell_without_field_codes() {
        assert_eq!(split_exec("firefox %u"), ["firefox"]);
        assert_eq!(split_exec(r#"env FOO="a b" /usr/bin/app --flag %F"#), ["env", "FOO=a b", "/usr/bin/app", "--flag"]);
        assert_eq!(split_exec("app 100%% x"), ["app", "100%", "x"]);
        assert_eq!(split_exec(r"app a\ b"), ["app", "a b"]);
    }
    #[test]
    fn config_lines_carry_env_and_expand_home() {
        let text = "# c\nchromium | Chromium | LD_LIBRARY_PATH=$HOME/lib A=b | $HOME/bin/chromium --ozone-platform=wayland\nbad line\n";
        let apps = parse_config(text, Path::new("/home/u"));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].id, "wl.chromium");
        assert_eq!(apps[0].env, [("LD_LIBRARY_PATH".to_string(), "/home/u/lib".to_string()), ("A".to_string(), "b".to_string())]);
        assert_eq!(apps[0].argv, ["/home/u/bin/chromium", "--ozone-platform=wayland"]);
    }
    #[test]
    fn a_window_belongs_to_the_entry_of_its_app_id_or_program() {
        let apps = parse_config("chromium | Chromium | | /opt/x/chromium --ozone-platform=wayland\nweb | Web | | /usr/bin/epiphany\n", Path::new("/h"));
        assert_eq!(for_window(&apps, "chromium").map(|a| a.id.as_str()), Some("wl.chromium"));
        assert_eq!(for_window(&apps, "Chromium").map(|a| a.id.as_str()), Some("wl.chromium"));
        assert_eq!(for_window(&apps, "epiphany").map(|a| a.id.as_str()), Some("wl.web"));
        // Chromium's ids: whatever the build and package call the window, it is the entry's.
        for id in ["chromium-browser", "Chromium-browser", "org.chromium.Chromium", "chromium.desktop", "Chromium", "chromium-bin"] {
            assert_eq!(for_window(&apps, id).map(|a| a.id.as_str()), Some("wl.chromium"), "{id}");
        }
        assert_eq!(for_window(&apps, "org.gnome.Epiphany").map(|a| a.id.as_str()), Some("wl.web"));
        // The program's own name, normalised the same way.
        let apps2 = parse_config("browser | B | | /opt/google/chrome/google-chrome-stable\n", Path::new("/h"));
        assert_eq!(for_window(&apps2, "google-chrome").map(|a| a.id.as_str()), Some("wl.browser"));
        assert!(for_window(&apps, "").is_none());
        assert!(for_window(&apps, "other").is_none());
    }
    #[test]
    fn app_keys_drop_the_packaging_around_a_name() {
        assert_eq!(app_key("org.chromium.Chromium"), "chromium");
        assert_eq!(app_key("com.mitchellh.ghostty"), "ghostty");
        assert_eq!(app_key("chromium-browser"), "chromium");
        assert_eq!(app_key("google-chrome-stable"), "google-chrome");
        assert_eq!(app_key("firefox.desktop"), "firefox");
        assert_eq!(app_key("foot"), "foot");
        assert_eq!(app_key("-browser"), "browser");
        assert_eq!(app_key(""), "");
    }
    #[test]
    fn only_the_config_is_read() {
        let dir = std::env::temp_dir().join(format!("wl-catalog-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("home/.makepad/wm")).unwrap();
        std::fs::create_dir_all(dir.join("home/.local/share/applications")).unwrap();
        std::fs::write(dir.join("home/.local/share/applications/zed.desktop"), "[Desktop Entry]\nType=Application\nName=Zed\nExec=zed\n").unwrap();
        assert!(load(&dir.join("home")).is_empty());
        std::fs::write(dir.join("home/.makepad/wm/wayland_apps.txt"), "foot | My Foot | | /opt/foot\n").unwrap();
        assert_eq!(load(&dir.join("home")).iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["wl.foot"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn icon_comes_from_the_programs_own_theme() {
        let dir = std::env::temp_dir().join(format!("wl-icon-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("usr/lib/chromium")).unwrap();
        std::fs::create_dir_all(dir.join("usr/share/icons/hicolor/128x128/apps")).unwrap();
        std::fs::write(dir.join("usr/share/icons/hicolor/128x128/apps/chromium.png"), b"x").unwrap();
        let app = ForeignApp { id: "wl.chromium".into(), label: "C".into(), env: vec![], argv: vec![dir.join("usr/lib/chromium/chromium").to_string_lossy().into_owned()] };
        assert!(icon_path(&app).unwrap().ends_with("128x128/apps/chromium.png"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
