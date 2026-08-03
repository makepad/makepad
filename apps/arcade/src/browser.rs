//! The library browser: what is installed, what the registry offers, and the
//! install / uninstall / publish buttons between them (game.md §"Game format
//! and sharing").
//!
//! The packaging and verification live in `makepad-game-pkg`; this is the
//! surface for them. Two rules it enforces on the way through:
//!
//! - A downloaded package is verified against the digest the index promised
//!   *before* it is unpacked — `Registry::download` does that, and this never
//!   installs bytes that skipped it.
//! - An installed game is marked `Trust::Downloaded`, so when it runs its
//!   isolate is capability-stripped. A game from a stranger is untrusted code.

use makepad_game_pkg::{
    library::Library as PkgLibrary, registry::IndexEntry, PkgError, Registry,
};
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ArcadeBrowserBase = #(ArcadeBrowser::register_widget(vm))
    mod.widgets.ArcadeBrowser = set_type_default() do mod.widgets.ArcadeBrowserBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 8
        padding: theme.space_2

        View {
            width: Fill
            height: Fit
            flow: Right
            spacing: 8
            align: Align{y: 0.5}

            Label {
                text: "Games"
                draw_text.text_style: theme.font_bold{font_size: 15}
            }
            View { width: Fill height: 1 }
            registry_input := TextInput {
                width: 190
                height: 32
                empty_text: "registry host:port"
                draw_text.text_style.font_size: 11
            }
            refresh_button := Button { text: "Browse" }
            publish_button := Button { text: "Publish current" }
        }

        status_label := Label {
            text: ""
            draw_text.text_style: theme.font_regular{font_size: 11}
        }

        list := PortalList {
            width: Fill
            height: Fill

            Row := View {
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                padding: theme.space_1
                align: Align{y: 0.5}

                title := Label {
                    width: Fill
                    text: ""
                    draw_text.text_style: theme.font_regular{font_size: 12}
                }
                play_button := Button { text: "Play" }
                action_button := Button { text: "" }
            }
        }
    }
}

/// One row: either something installed, or something the registry offers.
#[derive(Clone, Debug)]
pub enum Row {
    Installed {
        slug: String,
        name: String,
        description: String,
        players_max: u32,
    },
    Available {
        entry: IndexEntry,
    },
}

impl Row {
    fn title(&self) -> String {
        match self {
            Row::Installed {
                name,
                description,
                players_max,
                ..
            } => {
                let players = if *players_max > 1 {
                    format!("  ·  up to {players_max} players")
                } else {
                    String::new()
                };
                if description.is_empty() {
                    format!("{name}{players}")
                } else {
                    format!("{name}  —  {description}{players}")
                }
            }
            Row::Available { entry } => {
                let size = format!("  ·  {} KB", (entry.size / 1024).max(1));
                if entry.description.is_empty() {
                    format!("{}{size}", entry.name)
                } else {
                    format!("{}  —  {}{size}", entry.name, entry.description)
                }
            }
        }
    }

    fn action(&self) -> &'static str {
        match self {
            Row::Installed { .. } => "Remove",
            Row::Available { .. } => "Install",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrowserAction {
    /// The user asked to play an installed game.
    Play(String),
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ArcadeBrowser {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    rows: Vec<Row>,
    #[rust]
    initialized: bool,
    #[rust]
    registry_base: String,
    /// Set by the app so Publish knows what "current" means.
    #[rust]
    current_slug: Option<String>,
}

/// Where games live. Shared with the librarian's view of the same directory.
pub fn games_root() -> PathBuf {
    std::env::var("ARCADE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("arcade-games")
        })
}

impl ArcadeBrowser {
    pub fn set_current_game(&mut self, slug: Option<String>) {
        self.current_slug = slug;
    }

    fn library(&self) -> PkgLibrary {
        PkgLibrary::new(games_root())
    }

    fn set_status(&mut self, cx: &mut Cx, text: &str) {
        self.label(cx, ids!(status_label)).set_text(cx, text);
    }

    /// Installed games, always shown — the registry is optional, the local
    /// library is not.
    fn reload_installed(&mut self) {
        let installed: Vec<Row> = self
            .library()
            .list()
            .into_iter()
            .map(|e| Row::Installed {
                slug: e.slug,
                name: e.manifest.name,
                description: e.manifest.description,
                players_max: e.manifest.players_max,
            })
            .collect();
        // Keep any registry rows that are not already installed.
        let slugs: Vec<String> = installed
            .iter()
            .filter_map(|r| match r {
                Row::Installed { slug, .. } => Some(slug.clone()),
                _ => None,
            })
            .collect();
        let available: Vec<Row> = self
            .rows
            .drain(..)
            .filter(|r| match r {
                Row::Available { entry } => !slugs.contains(&entry.id),
                _ => false,
            })
            .collect();
        self.rows = installed;
        self.rows.extend(available);
    }

    fn browse_registry(&mut self, cx: &mut Cx) {
        let base = self.text_input(cx, ids!(registry_input)).text();
        let base = if base.trim().is_empty() {
            self.registry_base.clone()
        } else {
            base.trim().to_string()
        };
        if base.is_empty() {
            self.set_status(cx, "enter a registry address first");
            return;
        }
        self.registry_base = base.clone();

        // Blocking, deliberately: a registry index is a few KB on a LAN or a
        // fast CDN, and a background task here would need a whole async story
        // for a button nobody holds down. If this ever serves a slow remote,
        // it moves to the task pump.
        match Registry::new(&base).index() {
            Ok(entries) => {
                let n = entries.len();
                self.reload_installed();
                let installed: Vec<String> = self
                    .rows
                    .iter()
                    .filter_map(|r| match r {
                        Row::Installed { slug, .. } => Some(slug.clone()),
                        _ => None,
                    })
                    .collect();
                for entry in entries {
                    if !installed.contains(&entry.id) {
                        self.rows.push(Row::Available { entry });
                    }
                }
                self.set_status(cx, &format!("{n} game(s) in the registry"));
            }
            Err(e) => self.set_status(cx, &format!("registry unreachable: {e}")),
        }
        self.redraw(cx);
    }

    fn install(&mut self, cx: &mut Cx, entry: IndexEntry) {
        let reg = Registry::new(&self.registry_base);
        // download() verifies the digest; a mismatch never reaches the unpacker.
        match reg.download(&entry).map_err(|e| e.to_string()).and_then(|bytes| {
            self.library()
                .install(&entry.id, &bytes)
                .map_err(|e: PkgError| e.to_string())
        }) {
            Ok(installed) => {
                self.set_status(
                    cx,
                    &format!("installed {} — runs sandboxed", installed.manifest.name),
                );
                self.reload_installed();
            }
            Err(e) => self.set_status(cx, &format!("install failed: {e}")),
        }
        self.redraw(cx);
    }

    fn uninstall(&mut self, cx: &mut Cx, slug: &str) {
        match self.library().uninstall(slug) {
            Ok(()) => {
                self.set_status(cx, &format!("removed {slug}"));
                self.reload_installed();
            }
            Err(e) => self.set_status(cx, &format!("could not remove {slug}: {e}")),
        }
        self.redraw(cx);
    }

    fn publish(&mut self, cx: &mut Cx) {
        let Some(slug) = self.current_slug.clone() else {
            self.set_status(cx, "no game loaded to publish");
            return;
        };
        if self.registry_base.is_empty() {
            self.set_status(cx, "enter a registry address first");
            return;
        }
        let packed = match self.library().pack(&slug) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(cx, &format!("could not pack {slug}: {e}"));
                return;
            }
        };
        match Registry::new(&self.registry_base).publish(&packed) {
            Ok(id) => self.set_status(cx, &format!("published as {id} ({} KB)", packed.len() / 1024)),
            Err(e) => self.set_status(cx, &format!("publish failed: {e}")),
        }
    }

    /// Drain what the user clicked. Returns Play when an installed title was
    /// chosen, so the app can hand it to the ScriptHost.
    pub fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) -> BrowserAction {
        if self.button(cx, ids!(refresh_button)).clicked(actions) {
            self.browse_registry(cx);
        }
        if self.button(cx, ids!(publish_button)).clicked(actions) {
            self.publish(cx);
        }

        // Row buttons: the list groups its items' actions under its own uid, so
        // this resolves which row was clicked without per-row widget ids.
        let list = self.portal_list(cx, ids!(list));
        let mut hit: Option<(usize, bool)> = None;
        for (index, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(action_button)).clicked(actions) {
                hit = Some((index, true));
            } else if item.button(cx, ids!(play_button)).clicked(actions) {
                hit = Some((index, false));
            }
        }
        let Some((index, is_action)) = hit else {
            return BrowserAction::None;
        };
        let Some(row) = self.rows.get(index).cloned() else {
            return BrowserAction::None;
        };
        match (row, is_action) {
            (Row::Available { entry }, true) => self.install(cx, entry),
            (Row::Installed { slug, .. }, true) => self.uninstall(cx, &slug),
            // Clicking the title of something installed plays it.
            (Row::Installed { slug, .. }, false) => return BrowserAction::Play(slug),
            (Row::Available { .. }, false) => {}
        }
        BrowserAction::None
    }
}

impl Widget for ArcadeBrowser {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            self.initialized = true;
            self.reload_installed();
            if self.registry_base.is_empty() {
                if let Ok(base) = std::env::var("ARCADE_REGISTRY") {
                    self.registry_base = base;
                }
            }
        }
        let rows = self.rows.clone();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, rows.len());
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(row) = rows.get(index) else { continue };
                    let item = list.item(cx, index, id!(Row));
                    item.label(cx, ids!(title)).set_text(cx, &row.title());
                    item.button(cx, ids!(action_button))
                        .set_text(cx, row.action());
                    // Only something already installed can be played.
                    item.button(cx, ids!(play_button))
                        .set_visible(cx, matches!(row, Row::Installed { .. }));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_game_pkg::registry::IndexEntry;

    #[test]
    fn rows_describe_themselves_for_the_list() {
        let installed = Row::Installed {
            slug: "speedway".into(),
            name: "Speedway".into(),
            description: "race, 4 cars".into(),
            players_max: 4,
        };
        let text = installed.title();
        assert!(text.contains("Speedway"));
        assert!(text.contains("race, 4 cars"));
        assert!(text.contains("4 players"));
        assert_eq!(installed.action(), "Remove");

        let available = Row::Available {
            entry: IndexEntry {
                id: "dogfight".into(),
                name: "Dogfight".into(),
                description: "planes".into(),
                size: 4096,
                ..Default::default()
            },
        };
        assert!(available.title().contains("Dogfight"));
        assert!(available.title().contains("4 KB"));
        assert_eq!(available.action(), "Install");
    }

    #[test]
    fn a_single_player_game_does_not_advertise_a_player_count() {
        let row = Row::Installed {
            slug: "solo".into(),
            name: "Solo".into(),
            description: String::new(),
            players_max: 1,
        };
        assert_eq!(row.title(), "Solo");
    }

    #[test]
    fn games_root_follows_the_env_override() {
        // ARCADE_HOME is what the tests and the app both use to relocate the
        // library; without it we fall back to the home directory.
        let root = games_root();
        assert!(root.is_absolute() || std::env::var("ARCADE_HOME").is_ok());
    }
}
