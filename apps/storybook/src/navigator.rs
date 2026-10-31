//! The navigator: the registry drawn as a tree of category, component and
//! story, with a dot on every story that is new since the baseline.
//!
//! The tree is rebuilt from the registry on every draw, filtered by the
//! search text and the new-only switch. Folders open on the first draw
//! unless the person closed them last time: a click on a folder's row
//! folds it or unfolds it, and the set of closed folders is handed to the
//! app to keep, so the tree comes back the way it was left. A folder
//! holding the story on the canvas opens itself, whatever it was told:
//! a selected row nobody can see is no selection. Clicking a story row
//! raises [`NavigatorAction::Open`] with the story's key.
//!
//! The search text is read two ways. **Filtering** hides everything that
//! does not match, which is what the tree has always done, and holds
//! every folder open while it does so: something is being hidden, so
//! nothing else may be, or a match sits under a closed folder where
//! nobody can see it. **Finding** leaves the tree whole and walks the
//! matches one at a time; the folders on the way open as each match is
//! selected. Either way the same set of stories matches, so the count
//! means the same thing in both and the arrows step through the same
//! list. What the switches keep out of that list is reported too, so a
//! search that finds nothing says why. The new-only switch hides rows in
//! both modes, so it holds every folder open the way filtering does: a
//! folder closed earlier would otherwise keep the new stories folded away.
//! The starred-only switch works the same way over the stories the person
//! starred, whose rows carry a star in front of their name.
use crate::makepad_widgets::file_tree::*;
use crate::makepad_widgets::*;
use crate::registry::{self, Story};
use crate::settings;
use std::collections::{BTreeSet, HashMap, HashSet};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.storybook.StoryNavigatorBase = #(StoryNavigator::register_widget(vm))
    mod.storybook.StoryNavigator = set_type_default() do mod.storybook.StoryNavigatorBase{
        file_tree: FileTree{}
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum NavigatorAction {
    /// A story row was clicked; the payload is the story key.
    Open(String),
    /// A folder row was clicked and the set of closed folders changed.
    Folded,
    #[default]
    None,
}

/// What the two switches keep out of the tree while there is search
/// text. Each number is what turning that switch off would bring back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hidden {
    /// Matches the New-only switch keeps out: they match the text but
    /// their widget predates the baseline.
    pub by_new_only: usize,
    /// Stories the Filter switch keeps out: the ones that do not match.
    pub by_filter: usize,
}

impl Hidden {
    /// From the four counts a walk of the registry yields: how many
    /// stories match the text, how many of those are new, how many are
    /// new at all, and how many there are.
    pub fn count(
        matched: usize,
        matched_new: usize,
        new: usize,
        total: usize,
        new_only: bool,
        filtering: bool,
    ) -> Hidden {
        let by_new_only = if new_only { matched - matched_new } else { 0 };
        // With both switches on, the filter can only hide what the
        // new-only switch let through.
        let by_filter = match (filtering, new_only) {
            (false, _) => 0,
            (true, true) => new - matched_new,
            (true, false) => total - matched,
        };
        Hidden { by_new_only, by_filter }
    }

    /// The note under the tree, one line for each switch that keeps
    /// something out, so it never leans on wrapping in a narrow pane;
    /// empty when nothing is kept out.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        match self.by_new_only {
            0 => {}
            1 => parts.push("1 match hidden by New only".to_string()),
            n => parts.push(format!("{n} matches hidden by New only")),
        }
        match self.by_filter {
            0 => {}
            1 => parts.push("1 story hidden by Filter".to_string()),
            n => parts.push(format!("{n} stories hidden by Filter")),
        }
        parts.join("\n")
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryNavigator {
    #[uid]
    uid: WidgetUid,
    #[redraw]
    #[find]
    #[live]
    file_tree: FileTree,
    #[rust]
    filter: String,
    /// Whether the search text hides the rows that miss (filtering) or
    /// only picks them out of a whole tree (finding).
    #[rust(true)]
    filtering: bool,
    /// Which match the arrows are on, once they have been used. None
    /// means the search has not been walked yet, which is why the
    /// count reads "n matches" before the first step and "i of n"
    /// after it.
    #[rust]
    cursor: Option<usize>,
    #[rust]
    new_only: bool,
    /// The stories the person starred, by live key, and whether only they
    /// are listed.
    #[rust]
    starred: BTreeSet<String>,
    #[rust]
    starred_only: bool,
    #[rust(registry::DEFAULT_BASELINE.to_string())]
    baseline: String,
    /// Row id to story key.
    #[rust]
    keys: HashMap<LiveId, &'static str>,
    /// Folder row id to the folder's slug path, the name it is kept by.
    #[rust]
    folders: HashMap<LiveId, String>,
    /// The folders the person closed, by slug path. What the app keeps
    /// between runs; everything else is open.
    #[rust]
    closed: BTreeSet<String>,
    /// Folders drawn at least once, so a folder's first draw is the one
    /// that sets it open or closed and the later ones leave it alone.
    #[rust]
    seen: HashSet<LiveId>,
    /// The switches and the text the last draw was made under. A change
    /// puts every folder back to what the new state wants.
    #[rust]
    drawn_under: Option<(bool, bool, bool, String)>,
    #[rust]
    selected: Option<String>,
    /// How many stories the last draw listed, so a log line can say so.
    #[rust]
    listed: usize,
}

struct Component {
    name: &'static str,
    /// `category/component` in slug form, from the stories' keys.
    path: String,
    stories: Vec<&'static Story>,
    any_new: bool,
}

struct Category {
    name: &'static str,
    /// The category in slug form, from the stories' keys.
    path: String,
    components: Vec<Component>,
    any_new: bool,
}

/// The row id of the folder kept under this slug path. A path has one
/// or two segments and a story key has three, so no folder shares a row
/// id with a story.
fn folder_id(path: &str) -> LiveId {
    LiveId::from_str(path)
}

/// The slug paths of the folders a story sits under: its category, and
/// its component when the component has a folder at all. A component
/// with one page has none: the row IS the page. A key that moved sits
/// under the folders of the page it moved to, so the path is split from
/// the live key rather than from the key asked about.
fn folders_of(key: &str) -> Vec<String> {
    let Some(story) = registry::find(key) else {
        return Vec::new();
    };
    let mut segments = story.key.splitn(3, '/');
    let (Some(category), Some(component)) = (segments.next(), segments.next()) else {
        return Vec::new();
    };
    let mut out = vec![category.to_string()];
    let siblings = registry::all()
        .filter(|s| s.category == story.category && s.component == story.component)
        .count();
    if siblings > 1 {
        out.push(format!("{category}/{component}"));
    }
    out
}

/// Every folder the tree draws, by slug path: each category, and each
/// component with more than one page.
fn live_folders() -> BTreeSet<String> {
    registry::all().flat_map(|story| folders_of(story.key)).collect()
}

impl StoryNavigator {
    pub fn set_filter(&mut self, cx: &mut Cx, filter: &str) {
        if self.filter != filter {
            // A new query is a new list, so the walk starts over.
            self.cursor = None;
        }
        self.filter = filter.to_string();
        self.file_tree.redraw(cx);
    }

    /// True while the search text hides the rows that miss.
    pub fn filtering(&self) -> bool {
        self.filtering
    }

    pub fn set_filtering(&mut self, cx: &mut Cx, on: bool) {
        self.filtering = on;
        self.file_tree.redraw(cx);
    }

    /// Every story the search text picks out, in tree order. The
    /// new-only switch still applies: a row the arrows cannot reach
    /// has no business being counted.
    pub fn match_keys(&self) -> Vec<&'static str> {
        if self.filter.is_empty() {
            return Vec::new();
        }
        registry::all()
            .filter(|s| registry::matches(s, &self.filter))
            .filter(|s| !self.new_only || registry::is_new(s, &self.baseline))
            .filter(|s| !self.starred_only || self.starred.contains(s.key))
            .map(|s| s.key)
            .collect()
    }

    /// Step to the next match (`1`) or the previous one (`-1`) and
    /// return its key. Wraps, because a find bar that stops at the end
    /// makes the person work out where the end was.
    pub fn step_match(&mut self, cx: &mut Cx, delta: isize) -> Option<&'static str> {
        let keys = self.match_keys();
        if keys.is_empty() {
            self.cursor = None;
            return None;
        }
        let n = keys.len() as isize;
        let next = match self.cursor {
            // The first press lands on the first match going down and
            // on the last one going up, rather than stepping off a
            // position nobody chose.
            None if delta >= 0 => 0,
            None => n - 1,
            Some(at) => (at as isize + delta).rem_euclid(n),
        };
        self.cursor = Some(next as usize);
        self.file_tree.redraw(cx);
        keys.get(next as usize).copied()
    }

    /// What to print beside the arrows: how many matched, and which
    /// one the walk is on once it has started.
    pub fn match_report(&self) -> String {
        if self.filter.is_empty() {
            return String::new();
        }
        let n = self.match_keys().len();
        match (n, self.cursor) {
            (0, _) => "no matches".to_string(),
            (n, None) if n == 1 => "1 match".to_string(),
            (n, None) => format!("{n} matches"),
            (n, Some(at)) => format!("{} of {}", at + 1, n),
        }
    }

    /// What the switches keep out of the tree for the search text in
    /// the box. Nothing while the box is empty: there is no search for
    /// them to be keeping anything out of.
    pub fn hidden(&self) -> Hidden {
        if self.filter.is_empty() {
            return Hidden::default();
        }
        let (mut matched, mut matched_new, mut new, mut total) = (0, 0, 0, 0);
        for story in registry::all() {
            let is_match = registry::matches(story, &self.filter);
            let is_new = registry::is_new(story, &self.baseline);
            total += 1;
            new += usize::from(is_new);
            matched += usize::from(is_match);
            matched_new += usize::from(is_match && is_new);
        }
        Hidden::count(matched, matched_new, new, total, self.new_only, self.filtering)
    }

    pub fn set_new_only(&mut self, cx: &mut Cx, on: bool) {
        self.new_only = on;
        self.file_tree.redraw(cx);
    }

    /// The starred stories, as the settings keep them (keys, comma
    /// separated). Keys that moved are carried to the page they went to;
    /// keys that name nothing are dropped.
    pub fn set_starred(&mut self, cx: &mut Cx, line: &str) {
        self.starred = settings::parse_folded(line)
            .into_iter()
            .filter_map(|key| registry::find(&key).map(|story| story.key.to_string()))
            .collect();
        self.file_tree.redraw(cx);
    }

    pub fn starred_line(&self) -> String {
        settings::format_folded(&self.starred)
    }

    pub fn is_starred(&self, key: &str) -> bool {
        let key = registry::find(key).map_or(key, |story| story.key);
        self.starred.contains(key)
    }

    /// Stars the story, or takes its star away; true when it is starred now.
    pub fn toggle_starred(&mut self, cx: &mut Cx, key: &str) -> bool {
        let key = registry::find(key).map_or(key, |story| story.key).to_string();
        let on = !self.starred.remove(&key);
        if on {
            self.starred.insert(key);
        }
        self.file_tree.redraw(cx);
        on
    }

    pub fn set_starred_only(&mut self, cx: &mut Cx, on: bool) {
        self.starred_only = on;
        self.file_tree.redraw(cx);
    }

    /// Matches of the search text the starred-only switch keeps out (after
    /// the new-only switch has had its say).
    pub fn hidden_by_starred(&self) -> usize {
        if self.filter.is_empty() || !self.starred_only {
            return 0;
        }
        registry::all()
            .filter(|s| registry::matches(s, &self.filter))
            .filter(|s| !self.new_only || registry::is_new(s, &self.baseline))
            .filter(|s| !self.starred.contains(s.key))
            .count()
    }

    pub fn set_baseline(&mut self, cx: &mut Cx, baseline: &str) {
        self.baseline = baseline.to_string();
        self.file_tree.redraw(cx);
    }

    pub fn baseline(&self) -> &str {
        &self.baseline
    }

    /// The closed folders, as the settings keep them.
    pub fn folded(&self) -> String {
        settings::format_folded(&self.closed)
    }

    /// Close the folders a previous run left closed. Told before the
    /// first draw, which is the draw that sets every folder. A folder
    /// whose pages moved is closed where they went, and one the tree no
    /// longer draws is forgotten; [`Self::folded`] then says what is
    /// really closed, for the app to write down.
    pub fn set_folded(&mut self, cx: &mut Cx, folded: &str) {
        self.closed = settings::carry_folded(
            &settings::parse_folded(folded),
            registry::MOVED,
            &live_folders(),
        );
        self.seen.clear();
        self.file_tree.redraw(cx);
    }

    /// Highlight the row of this story and open the folders over it.
    /// Returns whether a closed folder had to be opened to show it, so
    /// the app knows the closed set moved.
    pub fn select(&mut self, cx: &mut Cx, key: &str) -> bool {
        // A key that moved selects the row of the page it moved to: row
        // ids are built from live keys.
        let key = registry::find(key).map_or(key, |story| story.key);
        self.selected = Some(key.to_string());
        let mut unfolded = false;
        for path in folders_of(key) {
            unfolded |= self.closed.remove(&path);
            self.file_tree.set_folder_is_open(cx, folder_id(&path), true, Animate::No);
        }
        self.file_tree.select_node(cx, LiveId::from_str(key));
        unfolded
    }

    /// The first story the search text picks out, whichever way the
    /// text is being read.
    pub fn first_match(&self) -> Option<&'static Story> {
        registry::all().find(|s| {
            registry::matches(s, &self.filter)
                && (!self.new_only || registry::is_new(s, &self.baseline))
                && (!self.starred_only || self.starred.contains(s.key))
        })
    }

    pub fn listed(&self) -> usize {
        self.listed
    }

    /// How many stories are new against the baseline, filter aside.
    pub fn new_count(&self) -> usize {
        registry::all().filter(|s| registry::is_new(s, &self.baseline)).count()
    }

    fn lists(&self, story: &Story) -> bool {
        // Finding leaves the tree whole: the search text picks a row
        // out rather than taking the others away.
        (!self.filtering || registry::matches(story, &self.filter))
            && (!self.new_only || registry::is_new(story, &self.baseline))
            && (!self.starred_only || self.starred.contains(story.key))
    }

    /// True while the new-only or starred-only switch or the search text is
    /// taking rows away, which holds every folder open.
    fn hiding(&self) -> bool {
        self.new_only || self.starred_only || (self.filtering && !self.filter.is_empty())
    }

    /// A story row's name: starred ones wear a star in front.
    fn row_name(&self, key: &str, name: &str) -> String {
        if self.starred.contains(key) {
            format!("\u{2605} {name}")
        } else {
            name.to_string()
        }
    }

    fn outline(&self) -> Vec<Category> {
        let mut categories: Vec<Category> = Vec::new();
        for story in registry::all() {
            if !self.lists(story) {
                continue;
            }
            let is_new = registry::is_new(story, &self.baseline);
            let category = match categories.iter_mut().find(|c| c.name == story.category) {
                Some(c) => c,
                None => {
                    let path = story.key.split('/').next().unwrap_or_default().to_string();
                    categories.push(Category { name: story.category, path, components: Vec::new(), any_new: false });
                    categories.last_mut().unwrap()
                }
            };
            category.any_new |= is_new;
            let component = match category.components.iter_mut().find(|c| c.name == story.component) {
                Some(c) => c,
                None => {
                    let path = story.key.rsplit_once('/').map(|(p, _)| p).unwrap_or_default().to_string();
                    category.components.push(Component { name: story.component, path, stories: Vec::new(), any_new: false });
                    category.components.last_mut().unwrap()
                }
            };
            component.any_new |= is_new;
            component.stories.push(story);
        }
        categories
    }

    /// Set a folder open or closed on the draws that decide it: its
    /// first, and the first after the switches or the text changed.
    /// Every other draw leaves it as the person left it.
    fn fold(&mut self, cx: &mut Cx2d, id: LiveId, path: &str, reopen: bool) {
        self.folders.insert(id, path.to_string());
        if self.seen.insert(id) || reopen {
            let open = self.hiding() || !self.closed.contains(path);
            self.file_tree.set_folder_is_open(cx, id, open, Animate::No);
        }
    }

    fn draw_tree(&mut self, cx: &mut Cx2d) {
        let outline = self.outline();
        // The switches or the text changing puts every folder back to
        // what the new state wants: open while New only or the text is
        // hiding rows, and the way the person left it otherwise.
        let under = (self.new_only, self.starred_only, self.filtering, self.filter.clone());
        let reopen = self.drawn_under.as_ref() != Some(&under);
        self.drawn_under = Some(under);
        self.keys.clear();
        self.folders.clear();
        self.listed = 0;
        for category in &outline {
            let cat_id = folder_id(&category.path);
            self.fold(cx, cat_id, &category.path, reopen);
            let dot = if category.any_new { StatusDotKind::New } else { StatusDotKind::None };
            if self.file_tree.begin_folder_with_status(cx, cat_id, category.name, dot).is_err() {
                continue;
            }
            for component in &category.components {
                // A component with one page IS that page. A folder holding a
                // single row called "Overview" costs a click to learn nothing,
                // so the component row opens the page directly and carries the
                // component's own name.
                if let [only] = component.stories[..] {
                    let id = LiveId::from_str(only.key);
                    self.keys.insert(id, only.key);
                    let dot = if registry::is_new(only, &self.baseline) {
                        StatusDotKind::New
                    } else {
                        StatusDotKind::None
                    };
                    let name = self.row_name(only.key, component.name);
                    self.file_tree.file_with_status(cx, id, &name, dot);
                    self.listed += 1;
                    continue;
                }
                let comp_id = folder_id(&component.path);
                self.fold(cx, comp_id, &component.path, reopen);
                let dot = if component.any_new { StatusDotKind::New } else { StatusDotKind::None };
                if self.file_tree.begin_folder_with_status(cx, comp_id, component.name, dot).is_err() {
                    continue;
                }
                for story in &component.stories {
                    let id = LiveId::from_str(story.key);
                    self.keys.insert(id, story.key);
                    let dot = if registry::is_new(story, &self.baseline) {
                        StatusDotKind::New
                    } else {
                        StatusDotKind::None
                    };
                    let name = self.row_name(story.key, story.name);
                    self.file_tree.file_with_status(cx, id, &name, dot);
                    self.listed += 1;
                }
                self.file_tree.end_folder();
            }
            self.file_tree.end_folder();
        }
    }
}

impl Widget for StoryNavigator {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.draw_tree(cx);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.file_tree.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if let Some(item) = actions.find_widget_action(self.file_tree.widget_uid()) {
                match item.cast() {
                    FileTreeAction::FileClicked(id) => {
                        if let Some(key) = self.keys.get(&id) {
                            let key = key.to_string();
                            self.selected = Some(key.clone());
                            cx.widget_action(self.uid, NavigatorAction::Open(key));
                        }
                    }
                    // The tree has already folded or unfolded the row by
                    // the time the click is reported; what is left is to
                    // remember which way it went.
                    FileTreeAction::FolderClicked(id) => {
                        if let Some(path) = self.folders.get(&id) {
                            let changed = if self.file_tree.is_folder_open(id) {
                                self.closed.remove(path)
                            } else {
                                self.closed.insert(path.clone())
                            };
                            if changed {
                                cx.widget_action(self.uid, NavigatorAction::Folded);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

impl StoryNavigatorRef {
    /// The story key a click on a row raised this pass, if any.
    pub fn opened(&self, actions: &Actions) -> Option<String> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let NavigatorAction::Open(key) = item.cast() {
                return Some(key);
            }
        }
        None
    }

    /// True when a click folded or unfolded a folder this pass.
    pub fn folded_changed(&self, actions: &Actions) -> bool {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let NavigatorAction::Folded = item.cast() {
                return true;
            }
        }
        false
    }

    pub fn set_filter(&self, cx: &mut Cx, filter: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_filter(cx, filter);
        }
    }

    pub fn set_new_only(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_new_only(cx, on);
        }
    }

    pub fn set_baseline(&self, cx: &mut Cx, baseline: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_baseline(cx, baseline);
        }
    }

    pub fn set_starred(&self, cx: &mut Cx, line: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_starred(cx, line);
        }
    }

    pub fn starred_line(&self) -> String {
        self.borrow().map(|inner| inner.starred_line()).unwrap_or_default()
    }

    pub fn is_starred(&self, key: &str) -> bool {
        self.borrow().is_some_and(|inner| inner.is_starred(key))
    }

    /// Stars the story or takes its star away; true when it is starred now.
    pub fn toggle_starred(&self, cx: &mut Cx, key: &str) -> bool {
        match self.borrow_mut() {
            Some(mut inner) => inner.toggle_starred(cx, key),
            None => false,
        }
    }

    pub fn set_starred_only(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_starred_only(cx, on);
        }
    }

    pub fn folded(&self) -> String {
        self.borrow().map(|inner| inner.folded()).unwrap_or_default()
    }

    pub fn set_folded(&self, cx: &mut Cx, folded: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_folded(cx, folded);
        }
    }

    /// Select the story's row; true when a closed folder was opened
    /// to show it.
    pub fn select(&self, cx: &mut Cx, key: &str) -> bool {
        if let Some(mut inner) = self.borrow_mut() {
            // Named explicitly: the library's `Select` widget generates a
            // `select(cx, ids)` accessor that is otherwise a better match
            // for this call than the navigator's own method.
            return StoryNavigator::select(&mut inner, cx, key);
        }
        false
    }

    pub fn first_match(&self) -> Option<&'static Story> {
        self.borrow().and_then(|inner| inner.first_match())
    }

    pub fn set_filtering(&self, cx: &mut Cx, on: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_filtering(cx, on);
        }
    }

    pub fn filtering(&self) -> bool {
        self.borrow().map(|inner| inner.filtering()).unwrap_or(true)
    }

    /// Step the walk and hand back the story to open, if there is one.
    pub fn step_match(&self, cx: &mut Cx, delta: isize) -> Option<&'static str> {
        let mut inner = self.borrow_mut()?;
        inner.step_match(cx, delta)
    }

    pub fn match_report(&self) -> String {
        self.borrow().map(|inner| inner.match_report()).unwrap_or_default()
    }

    /// The line under the tree saying what the switches keep out.
    pub fn hidden_line(&self) -> String {
        self.borrow()
            .map(|inner| {
                let mut line = inner.hidden().line();
                let starred = match inner.hidden_by_starred() {
                    0 => String::new(),
                    1 => "1 match hidden by Starred only".to_string(),
                    n => format!("{n} matches hidden by Starred only"),
                };
                if !starred.is_empty() {
                    if !line.is_empty() {
                        line.push('\n');
                    }
                    line.push_str(&starred);
                }
                line
            })
            .unwrap_or_default()
    }

    pub fn match_count(&self) -> usize {
        self.borrow().map(|inner| inner.match_keys().len()).unwrap_or(0)
    }

    pub fn new_count(&self) -> usize {
        self.borrow().map(|inner| inner.new_count()).unwrap_or(0)
    }

    pub fn listed(&self) -> usize {
        self.borrow().map(|inner| inner.listed()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_switch_hides_what_turning_it_off_would_bring_back() {
        // Two matches, neither new; twelve new; a hundred and twenty.
        assert_eq!(
            Hidden::count(2, 0, 12, 120, true, true),
            Hidden { by_new_only: 2, by_filter: 12 }
        );
        assert_eq!(
            Hidden::count(2, 1, 12, 120, true, true),
            Hidden { by_new_only: 1, by_filter: 11 }
        );
        assert_eq!(
            Hidden::count(2, 1, 12, 120, false, true),
            Hidden { by_new_only: 0, by_filter: 118 }
        );
        assert_eq!(
            Hidden::count(2, 1, 12, 120, true, false),
            Hidden { by_new_only: 1, by_filter: 0 }
        );
        assert_eq!(Hidden::count(2, 1, 12, 120, false, false), Hidden::default());
        // Every match is new: the new-only switch hides none of them.
        assert_eq!(Hidden::count(3, 3, 12, 120, true, false), Hidden::default());
    }

    #[test]
    fn the_hidden_line_names_the_switch_and_counts_in_english() {
        assert_eq!(Hidden::default().line(), "");
        assert_eq!(Hidden { by_new_only: 1, by_filter: 0 }.line(), "1 match hidden by New only");
        assert_eq!(Hidden { by_new_only: 3, by_filter: 0 }.line(), "3 matches hidden by New only");
        assert_eq!(Hidden { by_new_only: 0, by_filter: 1 }.line(), "1 story hidden by Filter");
        assert_eq!(Hidden { by_new_only: 0, by_filter: 118 }.line(), "118 stories hidden by Filter");
        assert_eq!(
            Hidden { by_new_only: 3, by_filter: 118 }.line(),
            "3 matches hidden by New only\n118 stories hidden by Filter"
        );
    }

    #[test]
    fn a_story_sits_under_its_category_and_its_component_folder() {
        for story in registry::all() {
            let folders = folders_of(story.key);
            let mut segments = story.key.split('/');
            let category = segments.next().unwrap();
            let component = segments.next().unwrap();
            assert_eq!(folders[0], category, "{}", story.key);
            let siblings = registry::all()
                .filter(|s| s.category == story.category && s.component == story.component)
                .count();
            if siblings > 1 {
                assert_eq!(folders.len(), 2, "{}", story.key);
                assert_eq!(folders[1], format!("{category}/{component}"));
            } else {
                assert_eq!(folders.len(), 1, "{}: a component with one page has no folder", story.key);
            }
            // No folder shares a row id with a story.
            for path in &folders {
                assert_ne!(folder_id(path), LiveId::from_str(story.key));
            }
        }
        assert!(folders_of("no/such/story").is_empty());
    }

    #[test]
    fn an_old_key_sits_under_the_folders_of_the_page_it_moved_to() {
        for (old, new) in registry::MOVED {
            assert!(!folders_of(old).is_empty(), "{old}");
            assert_eq!(folders_of(old), folders_of(new), "{old} -> {new}");
        }
    }

    #[test]
    fn closed_folders_from_before_the_regroup_close_where_their_pages_went() {
        let live = live_folders();
        let carry = |line: &str| {
            settings::format_folded(&settings::carry_folded(
                &settings::parse_folded(line),
                registry::MOVED,
                &live,
            ))
        };
        assert_eq!(carry("containers/layout"), "layout/layout");
        assert_eq!(carry("containers/glasssurfaces"), "containers/glass");
        assert_eq!(carry("inputs/glass"), "containers/glass");
        assert_eq!(carry("data-display/svg,data-display/vector"), "media/svg");
        assert_eq!(carry("data-display/tree"), "collections/tree");
        // Components that are a single row now have no folder to close.
        assert_eq!(carry("actions/button,media/icon"), "");
        // CheckBox was a single row and is two since it grew an icon-toggle
        // page, so an old fold of it is a folder again and carries over.
        assert_eq!(carry("inputs/checkbox"), "selection/checkbox");
        // DataGrid has an Overview page and a List page, so its folder is
        // still one to close.
        assert_eq!(carry("data-display/datagrid"), "collections/datagrid");
        // Folders that did not move stay closed, and so do categories.
        assert_eq!(carry("containers,foundations/colour,inputs/rotary"), "containers,foundations/colour,inputs/rotary");
        // Nothing an earlier build wrote can close a new category, so they
        // start open; and every folder carried over is one the tree draws.
        assert_eq!(carry(""), "");
        let everything: Vec<String> = registry::MOVED
            .iter()
            .map(|(old, _)| old.rsplit_once('/').unwrap().0.to_string())
            .collect();
        for path in settings::parse_folded(&carry(&everything.join(","))) {
            assert!(live.contains(&path), "{path} is no folder");
        }
    }
}
