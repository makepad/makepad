//! The navigator: the registry drawn as a tree of category, component and
//! story, with a dot on every story that is new since the baseline.
//!
//! The tree is rebuilt from the registry on every draw, filtered by the
//! search text and the new-only switch. Folders open on the first draw, and
//! every folder is held open while a filter is in force so a match is never
//! hidden under a closed folder. Clicking a story row raises
//! [`NavigatorAction::Open`] with the story's key.
//!
//! The search text is read two ways. **Filtering** hides everything that
//! does not match, which is what the tree has always done. **Finding**
//! leaves the tree whole and walks the matches one at a time, opening
//! only the folders on the way to the one being visited. Either way the
//! same set of stories matches, so the count means the same thing in
//! both and the arrows step through the same list.
use crate::makepad_widgets::file_tree::*;
use crate::makepad_widgets::*;
use crate::registry::{self, Story};
use std::collections::{HashMap, HashSet};

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
    #[default]
    None,
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
    #[rust(registry::DEFAULT_BASELINE.to_string())]
    baseline: String,
    /// Row id to story key.
    #[rust]
    keys: HashMap<LiveId, &'static str>,
    /// Folders already opened once; a folder the user closed stays closed.
    #[rust]
    opened: HashSet<LiveId>,
    #[rust]
    selected: Option<String>,
    /// How many stories the last draw listed, so a log line can say so.
    #[rust]
    listed: usize,
}

struct Component {
    name: &'static str,
    stories: Vec<&'static Story>,
    any_new: bool,
}

struct Category {
    name: &'static str,
    components: Vec<Component>,
    any_new: bool,
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
            .map(|s| s.key)
            .collect()
    }

    /// The story the arrows are standing on, if they have moved.
    fn current_match(&self) -> Option<&'static str> {
        let keys = self.match_keys();
        keys.get(self.cursor?).copied()
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

    pub fn set_new_only(&mut self, cx: &mut Cx, on: bool) {
        self.new_only = on;
        self.file_tree.redraw(cx);
    }

    pub fn set_baseline(&mut self, cx: &mut Cx, baseline: &str) {
        self.baseline = baseline.to_string();
        self.file_tree.redraw(cx);
    }

    pub fn baseline(&self) -> &str {
        &self.baseline
    }

    /// Highlight the row of this story.
    pub fn select(&mut self, cx: &mut Cx, key: &str) {
        self.selected = Some(key.to_string());
        self.file_tree.select_node(cx, LiveId::from_str(key));
    }

    /// The first story the search text picks out, whichever way the
    /// text is being read.
    pub fn first_match(&self) -> Option<&'static Story> {
        registry::all().find(|s| {
            registry::matches(s, &self.filter)
                && (!self.new_only || registry::is_new(s, &self.baseline))
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
    }

    /// The folders that have to be open for the match being visited to
    /// be on screen. Empty while filtering, which opens everything.
    fn on_the_way(&self, outline: &[Category]) -> HashSet<LiveId> {
        let mut open = HashSet::new();
        let Some(key) = self.current_match() else {
            return open;
        };
        for category in outline {
            for component in &category.components {
                if !component.stories.iter().any(|s| s.key == key) {
                    continue;
                }
                open.insert(LiveId::from_str(&format!("category:{}", category.name)));
                // A component with one page has no folder of its own:
                // the row IS the page, and the category above it is the
                // only thing in the way.
                if component.stories.len() > 1 {
                    open.insert(LiveId::from_str(&format!(
                        "component:{}/{}",
                        category.name, component.name
                    )));
                }
            }
        }
        open
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
                    categories.push(Category { name: story.category, components: Vec::new(), any_new: false });
                    categories.last_mut().unwrap()
                }
            };
            category.any_new |= is_new;
            let component = match category.components.iter_mut().find(|c| c.name == story.component) {
                Some(c) => c,
                None => {
                    category.components.push(Component { name: story.component, stories: Vec::new(), any_new: false });
                    category.components.last_mut().unwrap()
                }
            };
            component.any_new |= is_new;
            component.stories.push(story);
        }
        categories
    }

    fn folder_open(&mut self, cx: &mut Cx2d, id: LiveId, on_the_way: &HashSet<LiveId>) {
        // Something is being hidden, so nothing else may be: a match
        // under a closed folder is a match nobody can see. Finding
        // hides nothing, so it opens only what stands between the
        // person and the match they asked to be taken to.
        let hiding = self.new_only || (self.filtering && !self.filter.is_empty());
        let force = hiding || on_the_way.contains(&id);
        if force || self.opened.insert(id) {
            self.file_tree.set_folder_is_open(cx, id, true, Animate::No);
        }
    }

    fn draw_tree(&mut self, cx: &mut Cx2d) {
        let outline = self.outline();
        let on_the_way = self.on_the_way(&outline);
        self.keys.clear();
        self.listed = 0;
        for category in &outline {
            let cat_id = LiveId::from_str(&format!("category:{}", category.name));
            self.folder_open(cx, cat_id, &on_the_way);
            let dot = if category.any_new { GitStatusDotKind::New } else { GitStatusDotKind::None };
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
                        GitStatusDotKind::New
                    } else {
                        GitStatusDotKind::None
                    };
                    self.file_tree.file_with_status(cx, id, component.name, dot);
                    self.listed += 1;
                    continue;
                }
                let comp_id = LiveId::from_str(&format!("component:{}/{}", category.name, component.name));
                self.folder_open(cx, comp_id, &on_the_way);
                let dot = if component.any_new { GitStatusDotKind::New } else { GitStatusDotKind::None };
                if self.file_tree.begin_folder_with_status(cx, comp_id, component.name, dot).is_err() {
                    continue;
                }
                for story in &component.stories {
                    let id = LiveId::from_str(story.key);
                    self.keys.insert(id, story.key);
                    let dot = if registry::is_new(story, &self.baseline) {
                        GitStatusDotKind::New
                    } else {
                        GitStatusDotKind::None
                    };
                    self.file_tree.file_with_status(cx, id, story.name, dot);
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
                if let FileTreeAction::FileClicked(id) = item.cast() {
                    if let Some(key) = self.keys.get(&id) {
                        let key = key.to_string();
                        self.selected = Some(key.clone());
                        cx.widget_action(self.uid, NavigatorAction::Open(key));
                    }
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

    pub fn select(&self, cx: &mut Cx, key: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            // Named explicitly: the library's `Select` widget generates a
            // `select(cx, ids)` accessor that is otherwise a better match
            // for this call than the navigator's own method.
            StoryNavigator::select(&mut inner, cx, key);
        }
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
