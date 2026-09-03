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
use makepad_ai_services::wire::ToolResult;
use makepad_image_tiles::library::ItemId;
use makepad_image_tiles::{Library, TileGrid, TileGridAction};
use makepad_widgets::makepad_platform::thread::{Lane, TaskHandle};
use makepad_widgets::*;
use std::path::Path;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PhotosViewBase = #(PhotosView::register_widget(vm))

    mod.widgets.PhotosView = set_type_default() do mod.widgets.PhotosViewBase{
        width: Fill
        height: Fill
        flow: Down
        // The as-you-type search: every keystroke re-hangs the wall to the
        // matches, the pictures flying to their new places.
        search_row := View{
            width: Fill
            height: Fit
            flow: Right
            align: Align{y: 0.5}
            padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
            spacing: 8
            search := TextInput{
                width: Fill
                height: Fit
                empty_text: "Search the wall  (⌘F, Esc clears)"
            }
        }
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
    /// Pictures being baked into the wall right now (`photos.add`), each
    /// answered through `reply` when its bake finishes.
    #[rust]
    adds: Vec<PendingAdd>,
    /// Where a finished `add` sends its result: the standalone window's
    /// port, or the module host's reply sink. Set by whoever seats the view.
    #[rust]
    reply: Option<Box<dyn Fn(ToolResult)>>,
    /// The picture to glide onto once the wall has re-opened after an add
    /// — the item id only exists after the reload.
    #[rust]
    show_when_opened: Option<String>,
}

/// One `photos.add` in flight: the blocking tape bake is a heavy pool job.
pub struct PendingAdd {
    call_id: String,
    path: String,
    done: TaskHandle<Result<makepad_image_tiles::bake::BakeSummary, String>>,
}

impl PhotosView {
    /// Filter the wall to `query` as the person (or the assistant) types:
    /// the matches fly into a fresh packing, the rest shrink away, an
    /// empty query brings everything back. The box shows the query too.
    pub fn set_query(&mut self, cx: &mut Cx, query: &str) {
        let input = self.view.text_input(cx, ids!(search));
        if input.text().trim() != query.trim() {
            input.set_text(cx, query.trim());
        }
        self.apply_query(cx, query);
    }

    /// The query the wall is filtered by right now.
    pub fn query(&self, cx: &mut Cx) -> String {
        self.view.widget(cx, ids!(grid)).borrow::<TileGrid>().map(|g| g.query().to_string()).unwrap_or_default()
    }

    fn apply_query(&mut self, cx: &mut Cx, query: &str) {
        let counts = self.view.widget(cx, ids!(grid)).borrow_mut::<TileGrid>().map(|mut grid| {
            grid.set_query(cx, query);
            (grid.visible_count(), grid.count(), grid.query().to_string())
        });
        if let Some((shown, total, q)) = counts {
            let text = filter_status(shown, total, &q, &self.library_root);
            self.set_status(cx, text);
        }
    }

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

    /// The picture whose link is exactly `link` (an added file's path).
    fn search_link(&self, cx: &mut Cx, link: &str) -> Option<ItemId> {
        self.view
            .widget(cx, ids!(grid))
            .borrow::<TileGrid>()
            .and_then(|grid| grid.items().into_iter().find(|(_, _, l)| l == link).map(|(id, _, _)| id))
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

    /// How many pictures the wall shows under the current filter.
    pub fn visible(&self, cx: &mut Cx) -> usize {
        self.view.widget(cx, ids!(grid)).borrow::<TileGrid>().map(|g| g.visible_count()).unwrap_or(0)
    }

    /// Where finished adds report. The standalone window routes it to its
    /// port; the module host to its reply sink.
    pub fn set_reply(&mut self, reply: Box<dyn Fn(ToolResult)>) {
        self.reply = Some(reply);
    }

    /// The library this wall is showing, if any.
    pub fn library_root(&self) -> &str {
        &self.library_root
    }

    /// Bake one picture on disk into the open library, off the UI thread;
    /// the wall re-opens and glides onto it when the bake reports. The
    /// answer goes out through `reply` — `Err` here means nothing was
    /// started and the caller answers now.
    pub fn start_add(
        &mut self,
        cx: &mut Cx,
        call_id: &str,
        path: &Path,
        title: &str,
    ) -> Result<(), String> {
        if self.library_root.is_empty() {
            return Err("no library is open on this wall".to_string());
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (cx, call_id, path, title);
            Err("adding pictures needs the native app".to_string())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use makepad_image_tiles::bake::{bake, BakeOptions, Source};
            let root = std::path::PathBuf::from(&self.library_root);
            let path_text = path.to_string_lossy().to_string();
            let source = Source { url: path_text.clone(), title: title.to_string(), link: path_text.clone() };
            let pool = cx.task_pool();
            let bake_pool = pool.clone();
            let done = pool
                .submit(Lane::Heavy, move || {
                    // One picture: one fetch, one encode. The library's own
                    // failed items stay failed — an add is about this file.
                    let options = BakeOptions { fetch_threads: 1, encode_threads: 1, retry_failed: false };
                    bake(
                        &root,
                        &[source],
                        &options,
                        &mut |line| log!("photos add: {line}"),
                        &bake_pool,
                    )
                })
                .map_err(|e| format!("could not queue the bake: {e}"))?;
            self.adds.push(PendingAdd { call_id: call_id.to_string(), path: path_text, done });
            Ok(())
        }
    }

    /// Finished adds: re-open the wall, remember which picture to show,
    /// and answer the call.
    fn poll_adds(&mut self, cx: &mut Cx) {
        if self.adds.is_empty() {
            return;
        }
        let mut finished = Vec::new();
        self.adds.retain_mut(|add| match add.done.try_take() {
            Some(Ok(result)) => {
                finished.push((add.call_id.clone(), add.path.clone(), result));
                false
            }
            Some(Err(error)) => {
                finished.push((add.call_id.clone(), add.path.clone(), Err(format!("bake task failed: {error}"))));
                false
            }
            None => true,
        });
        for (call_id, path, result) in finished {
            let answer = match result {
                Ok(summary) if summary.baked > 0 || summary.skipped > 0 => {
                    self.show_when_opened = Some(path.clone());
                    let root = Library::new(&self.library_root);
                    self.open_library(cx, root);
                    ToolResult::ok(&call_id, format!("{path} is on the wall now; showing it"), "added")
                        .with_data(format!("{{\"path\":{}}}", makepad_strict_json::s(path.clone()).to_json()))
                }
                Ok(_) => ToolResult::failed(&call_id, format!("{path} could not be baked (not a picture the wall can decode?)")),
                Err(e) => ToolResult::failed(&call_id, format!("adding {path} failed: {e}")),
            };
            match &self.reply {
                Some(reply) => reply(answer),
                None => log!("photos: an add finished with nobody to answer ({call_id})"),
            }
        }
    }
}

/// The status line for a filtered wall: how many of the pictures show,
/// and for which words; the plain count with the library when there is
/// no query.
pub fn filter_status(shown: usize, total: usize, query: &str, root: &str) -> String {
    if query.trim().is_empty() {
        format!("{total} pictures — {root}")
    } else if shown == 0 {
        format!("nothing of {total} matches · {}", query.trim())
    } else {
        format!("{shown} of {total} · {}", query.trim())
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
        self.poll_adds(cx);
        // ⌘F (or Ctrl+F, or a bare `/` while the wall has the keys) puts the
        // caret in the search box; Esc in the box clears the filter.
        if let Event::KeyDown(ke) = event {
            let input = self.view.text_input(cx, ids!(search));
            let boxed = cx.has_key_focus(input.area());
            let find = ke.key_code == KeyCode::KeyF && (ke.modifiers.logo || ke.modifiers.control);
            let slash = ke.key_code == KeyCode::Slash && !boxed && !ke.modifiers.logo && !ke.modifiers.control;
            if find || slash {
                input.set_key_focus(cx);
            }
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let input = self.view.text_input(cx, ids!(search));
            if let Some(text) = input.changed(actions) {
                self.apply_query(cx, &text);
            }
            if input.escaped(actions) {
                self.set_query(cx, "");
            }
            for action in actions {
                let Some(widget_action) = action.as_widget_action() else { continue };
                match widget_action.cast::<TileGridAction>() {
                    TileGridAction::Opened { count, error: None } => {
                        self.pictures = count;
                        let text = format!("{count} pictures — {}", self.library_root);
                        self.set_status(cx, text);
                        // A re-opened wall (an add) keeps the words in the box.
                        let kept = self.view.text_input(cx, ids!(search)).text();
                        if !kept.trim().is_empty() {
                            self.apply_query(cx, &kept);
                        }
                        // A freshly added picture: the wall knows it now.
                        if let Some(path) = self.show_when_opened.take() {
                            let wanted = self.search_link(cx, &path);
                            if let Some(item) = wanted {
                                self.show(cx, item);
                            }
                        }
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
    fn the_status_line_counts_the_filter() {
        assert_eq!(filter_status(293, 293, "", "/lib"), "293 pictures — /lib");
        assert_eq!(filter_status(12, 293, " robots ", "/lib"), "12 of 293 · robots");
        assert_eq!(filter_status(0, 293, "dog", "/lib"), "nothing of 293 matches · dog");
    }

    #[test]
    fn hits_are_capped() {
        let many: Vec<(ItemId, String, String)> = (0..40).map(|i| (i, format!("robot {i}"), String::new())).collect();
        assert_eq!(search_items(&many, "robot").len(), SEARCH_LIMIT);
    }
}
