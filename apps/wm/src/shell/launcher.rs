//! The `apps` provider — omarchy's launcher.
//!
//! In omarchy-shell the launcher is not a separate surface: `Super+Space`
//! opens the menu at its root and `Super+Alt+Space` opens it on the `apps`
//! provider, which lists desktop entries alphabetically
//! (`services/AppLibrary.qml` + `AppSearch.js`) minus the ids in
//! `default/omarchy/launcher.hides` and anything marked Hidden/NoDisplay.
//! Ours lists the app registry (`clients.rs`) — the same contract, with our
//! binaries in place of `.desktop` files: an entry appears only when its
//! binary is actually next to us, which is our version of NoDisplay.
//!
//! The launcher's own token section (`[launcher]` in shell.toml.tpl: a card
//! at α 0.95 over a scrim at α 0.5) is what the brief asks the launcher to
//! wear, so the menu surface takes its colors from `shell.launcher` when it
//! is opened on this provider and from `shell.menu` otherwise.

use crate::apps::Launchable;
use crate::clients;

use super::menu::{MenuItem, MenuKind};
use super::ui::Ico;

/// Ids hidden from the launcher, one per line, matched EXACTLY and
/// case-sensitively against the entry id — omarchy's `launcher.hides`
/// semantics (no globs, no substring).
pub fn hides() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let path = crate::theme::makepad_home().join("wm/launcher.hides");
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            out.push(line.to_string());
        }
    }
    out
}

/// True when this id is hidden (exact, case-sensitive).
pub fn is_hidden(id: &str, hides: &[String]) -> bool {
    hides.iter().any(|h| h == id)
}

/// An icon for every app id. The user's rule: EVERY row carries one, so
/// the label column starts at the same x on every line — our icon set is
/// small, so a few are stand-ins rather than literal pictograms.
fn icon_for(id: &str) -> Option<Ico> {
    Some(match id {
        "terminal" => Ico::Keyboard,
        "browser" => Ico::Search,
        "files" => Ico::Menu,
        "task" => Ico::Pulse,
        "sheets" => Ico::Calendar,
        "score" => Ico::Bell,
        "image" => Ico::Monitor,
        "video" => Ico::Play,
        "pdf" => Ico::Check,
        "route" => Ico::Globe,
        "mixer" => Ico::Speaker,
        "amp" => Ico::Play,
        "code" => Ico::Prompt,
        "photos" | "camera" => Ico::Photo,
        "clock" => Ico::Calendar,
        "weather" => Ico::Sun,
        "finance" => Ico::Pulse,
        "fabric" => Ico::Shirt,
        "fab" => Ico::Refresh,
        // Never None: an app row without an icon would shift its label.
        _ => Ico::Dot,
    })
}

/// The `apps` provider rows: every registry app this build can start
/// (see `Launchable`), not hidden, in the CURATED registry order (the
/// user's: browser/files/terminal first, then by rarity — a deliberate
/// deviation from omarchy's alphabetical provider). The live filter never
/// reorders.
/// The listed foreign app that replaces our browser app in the dock and launcher.
pub const BROWSER_REPLACEMENT: &str = "apps.wl.chromium";

/// How long a scan of the apps stands: the scan reads the disk (does each app's
/// crate exist, is it built, the hides file, the foreign catalog), and the dock
/// asks for the list on every frame it draws. A newly built app shows its
/// "compiles when opened" note go within this long.
const APPS_TTL: std::time::Duration = std::time::Duration::from_secs(2);

thread_local! {
    static APPS_SCAN: std::cell::RefCell<Option<(std::time::Instant, Launchable, Vec<MenuItem>)>> =
        const { std::cell::RefCell::new(None) };
}

pub fn apps(launchable: &Launchable) -> Vec<MenuItem> {
    APPS_SCAN.with(|scan| {
        let mut scan = scan.borrow_mut();
        match scan.as_ref() {
            Some((at, for_launchable, items)) if for_launchable == launchable && at.elapsed() < APPS_TTL => items.clone(),
            _ => {
                let items = scan_apps(launchable);
                *scan = Some((std::time::Instant::now(), launchable.clone(), items.clone()));
                items
            }
        }
    })
}

fn scan_apps(launchable: &Launchable) -> Vec<MenuItem> {
    let hides = hides();
    let items: Vec<MenuItem> = clients::registry()
        .iter()
        // Launchable: a package this checkout can run where processes
        // exist, or a module this build links (the only kind a build
        // without processes has).
        .filter(|app| launchable.allows(app))
        .filter(|app| !is_hidden(&app.id, &hides))
        .map(|app| MenuItem {
            id: format!("apps.{}", app.id),
            label: app.label.clone(),
            icon: icon_for(&app.id),
            kind: MenuKind::App,
            checked: false,
            divider: false,
            disabled: false,
            // Opening it compiles it first (clients.rs spawn_build).
            description: if app.is_built() { String::new() } else { "compiles when opened".into() },
            aliases: vec![app.id.clone(), app.package.clone()],
        })
        .collect();
    // Foreign Wayland apps (installed .desktop entries and the user's wayland_apps.txt).
    let foreign = crate::wayland_host::menu_apps().into_iter().filter(|(id, _)| !is_hidden(id, &hides)).map(|(id, label)| MenuItem {
        id: format!("apps.{id}"),
        label,
        icon: Some(Ico::Dot),
        kind: MenuKind::App,
        checked: false,
        divider: false,
        disabled: false,
        description: String::new(),
        aliases: if id == "wl.chromium" { vec![id, "browser".into()] } else { vec![id] },
    });
    place_foreign(items, foreign.collect())
}

/// Chromium takes the browser's slot (our own browser app leaves the list; it stays in the repo).
/// A foreign app listed under one of our apps' ids (`code` = VS Code) sits right after that app.
/// The rest follow our apps.
fn place_foreign(mut items: Vec<MenuItem>, foreign: Vec<MenuItem>) -> Vec<MenuItem> {
    let mut rest = Vec::new();
    for f in foreign {
        if f.id == BROWSER_REPLACEMENT {
            let slot = items.iter().position(|i| i.id == "apps.browser").unwrap_or(0).min(items.len());
            items.retain(|i| i.id != "apps.browser");
            items.insert(slot.min(items.len()), f);
        } else if let Some(ours) = f.id.strip_prefix("apps.wl.").and_then(|bare| items.iter().position(|i| i.id == format!("apps.{bare}"))) {
            items.insert(ours + 1, f);
        } else {
            rest.push(f);
        }
    }
    items.extend(rest);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> MenuItem {
        MenuItem { id: id.into(), label: id.into(), icon: None, kind: MenuKind::App, checked: false, divider: false, disabled: false, description: String::new(), aliases: vec![] }
    }

    #[test]
    fn vscode_sits_next_to_code_and_chromium_takes_the_browser_slot() {
        let ours = ["apps.files", "apps.browser", "apps.code", "apps.stage"].map(row).to_vec();
        let foreign = ["apps.wl.code", "apps.wl.gimp", "apps.wl.chromium"].map(row).to_vec();
        let ids: Vec<String> = place_foreign(ours, foreign).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, ["apps.files", "apps.wl.chromium", "apps.code", "apps.wl.code", "apps.stage", "apps.wl.gimp"]);
    }

    #[test]
    fn hides_match_exactly_and_case_sensitively() {
        let hides = vec!["btop".to_string(), "libreoffice-base".to_string()];
        assert!(is_hidden("btop", &hides));
        // No substring, no case folding, no globs.
        assert!(!is_hidden("btopper", &hides));
        assert!(!is_hidden("BTOP", &hides));
        assert!(!is_hidden("libreoffice", &hides));
    }

    #[test]
    fn the_apps_provider_keeps_the_curated_order() {
        let items = apps(&Launchable::default());
        // Rows appear in registry order (available subset preserves it).
        // Foreign Wayland apps (`apps.wl.*`) come after the registry rows and depend on the machine.
        let labels: Vec<String> = items.iter().filter(|i| !i.id.starts_with("apps.wl.")).map(|i| i.label.clone()).collect();
        let registry_order: Vec<String> = clients::registry()
            .iter()
            .filter(|a| labels.contains(&a.label))
            .map(|a| a.label.clone())
            .collect();
        assert_eq!(labels, registry_order);
        // Every row is an app row under the `apps` parent.
        assert!(items
            .iter()
            .all(|i| i.kind == MenuKind::App && i.id.starts_with("apps.")));
    }
}
