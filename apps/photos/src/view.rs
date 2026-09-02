//! The picture wall: a `TileGrid` over the chosen library, a status line
//! under it, and the two things the assistant asks of it — search the
//! words the pictures carry, and show one.
//!
//! The grid opens on the widget's first event or draw, whichever comes
//! first, with the library `library::find` resolved — the same in a
//! standalone window and in a module's isolate, where no `Startup` event
//! ever arrives. Wheel zooms around the cursor, drag pans, a click logs
//! the picture; `show` glides the camera onto one.

use crate::library;
use makepad_image_tiles::library::ItemId;
use makepad_image_tiles::{Library, TileGrid, TileGridAction};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PhotosViewBase = #(PhotosView::register_widget(vm))

    mod.widgets.PhotosView = set_type_default() do mod.widgets.PhotosViewBase{
        width: Fill
        height: Fill
        flow: Down
        grid_wrap := View{
            width: Fill
            height: Fill
            grid := TileGrid{}
        }
        status := Label{
            width: Fill
            height: Fit
            padding: Inset{left: 12 right: 12 top: 5 bottom: 5}
            text: ""
            draw_text +: {
                color: theme.color_text_meta
                text_style: theme.font_regular{font_size: 8.5}
            }
        }
    }
}

/// The most matches a search reports.
pub const SEARCH_LIMIT: usize = 12;

#[derive(Script, ScriptHook, Widget)]
pub struct PhotosView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The collection to open (`None` = the default); set before the first
    /// event by the module's `create` or the standalone window.
    #[rust]
    collection: Option<String>,
    #[rust]
    opened: bool,
    /// What the grid reported when it opened: the count, or the error.
    #[rust]
    pictures: usize,
    #[rust]
    library_root: String,
    #[rust]
    status_text: String,
}

impl PhotosView {
    /// Which collection to open. Before the first event only.
    pub fn set_collection(&mut self, collection: Option<String>) {
        self.collection = collection;
    }

    /// Open the library once. The grid owns its draw list so a sibling
    /// redraw never re-uploads the whole wall.
    fn ensure_open(&mut self, cx: &mut Cx) {
        if self.opened {
            return;
        }
        self.opened = true;
        if let Some(mut wrap) = self.view.view(cx, ids!(grid_wrap)).borrow_mut() {
            wrap.set_optimize(cx, ViewOptimize::DrawList);
        }
        match library::find(self.collection.as_deref()) {
            Some(lib) => self.open_library(cx, lib),
            None => self.set_status(cx, library::how_to_bake()),
        }
    }

    fn open_library(&mut self, cx: &mut Cx, lib: Library) {
        self.library_root = lib.root.to_string_lossy().to_string();
        if let Some(mut grid) = self.view.widget(cx, ids!(grid)).borrow_mut::<TileGrid>() {
            grid.open(cx, lib);
        }
    }

    fn set_status(&mut self, cx: &mut Cx, text: String) {
        if self.status_text != text {
            self.status_text = text.clone();
            self.view.label(cx, ids!(status)).set_text(cx, &text);
        }
    }

    /// Search the words the pictures carry (title and link), case-insensitive,
    /// every word of the query somewhere; at most [`SEARCH_LIMIT`] hits.
    pub fn search(&self, cx: &mut Cx, query: &str) -> Vec<(ItemId, String, String)> {
        let items = self
            .view
            .widget(cx, ids!(grid))
            .borrow::<TileGrid>()
            .map(|grid| grid.items())
            .unwrap_or_default();
        search_items(&items, query)
    }

    /// Glide onto one picture; false when the id is not on the wall.
    pub fn show(&mut self, cx: &mut Cx, item: ItemId) -> bool {
        self.view
            .widget(cx, ids!(grid))
            .borrow_mut::<TileGrid>()
            .map(|mut grid| grid.show_item(cx, item))
            .unwrap_or(false)
    }

    /// One line about the wall: how many pictures, from where.
    pub fn summary(&self) -> String {
        if self.library_root.is_empty() {
            return self.status_text.clone();
        }
        format!("{} pictures from {}", self.pictures, self.library_root)
    }

    pub fn pictures(&self) -> usize {
        self.pictures
    }
}

/// The search over an item list: every query word must appear in the
/// title or the link, case-insensitively; empty queries match nothing.
pub fn search_items(items: &[(ItemId, String, String)], query: &str) -> Vec<(ItemId, String, String)> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .filter(|(_, title, link)| {
            let hay = format!("{} {}", title.to_lowercase(), link.to_lowercase());
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .take(SEARCH_LIMIT)
        .cloned()
        .collect()
}

impl Widget for PhotosView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_open(cx);
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            for action in actions {
                let Some(widget_action) = action.as_widget_action() else { continue };
                match widget_action.cast::<TileGridAction>() {
                    TileGridAction::Opened { count, error: None } => {
                        self.pictures = count;
                        let text = format!("{count} pictures — {}", self.library_root);
                        self.set_status(cx, text);
                    }
                    TileGridAction::Opened { error: Some(e), .. } => {
                        self.pictures = 0;
                        self.set_status(cx, format!("{e} — {}", library::how_to_bake()));
                    }
                    TileGridAction::Clicked { item, title, link, .. } => {
                        let text = format!("#{item}  {}  {link}", title.chars().take(140).collect::<String>());
                        self.set_status(cx, text);
                    }
                    TileGridAction::None => {}
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_open(cx);
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<(ItemId, String, String)> {
        vec![
            (1, "2024-01-02: A robot learns to love".into(), "https://smbc/1".into()),
            (2, "2024-01-03: Physics of a cat".into(), "https://smbc/2".into()),
            (3, "2024-01-04: The Robot uprising, again".into(), "https://smbc/3".into()),
        ]
    }

    #[test]
    fn every_query_word_must_appear_case_insensitively() {
        let hits = search_items(&items(), "robot");
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(search_items(&items(), "ROBOT uprising").len(), 1);
        assert!(search_items(&items(), "dog").is_empty());
        assert!(search_items(&items(), "   ").is_empty());
        // The link counts too.
        assert_eq!(search_items(&items(), "smbc/2")[0].0, 2);
    }

    #[test]
    fn hits_are_capped() {
        let many: Vec<(ItemId, String, String)> = (0..40).map(|i| (i, format!("robot {i}"), String::new())).collect();
        assert_eq!(search_items(&many, "robot").len(), SEARCH_LIMIT);
    }
}
