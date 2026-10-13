//! The browser's tree, apart from any widget: folders and the items under
//! them, from any source (the MilkDrop pack, the person's own presets, a
//! host's effect list), the live filter and the rows it leaves.
//!
//! [`TreeIndex::build`] does the string work once (folding every item's
//! title, detail and folder names for search), so a keystroke only runs
//! [`TreeIndex::matches`] over the folded strings and [`TreeIndex::rows`]
//! over the tree: plain arithmetic, well under a millisecond for the
//! 10,000-preset pack. Build the index off the UI thread for a big source.

use makepad_search::fold_words;
use std::collections::HashSet;

/// One thing a browser lists: a preset, an effect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserItem {
    /// Its stable name (a preset's path under the pack, an effect's id).
    pub key: String,
    /// The folders it sits in, `/`-separated; empty for the top level.
    pub folder: String,
    pub title: String,
    /// The second line: authors, a description. Shown muted and searched.
    pub detail: String,
    /// The person's own (made by them or their agent), marked in the list.
    pub own: bool,
}

/// A folder of the tree. Folder 0 is the invisible root.
#[derive(Clone, Debug, Default)]
pub struct TreeFolder {
    pub name: String,
    /// The folder names from the top, `/`-joined: what stays the same
    /// when the source is read again, so the folds survive a reload.
    pub path: String,
    pub parent: usize,
    /// 0 for a top-level folder.
    pub depth: usize,
    folders: Vec<usize>,
    items: Vec<usize>,
}

/// What a row of the list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowKey {
    Folder(usize),
    Item(usize),
}

/// One row of the list, in the order drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeRow {
    pub key: RowKey,
    /// Indent level: 0 at the top.
    pub depth: usize,
    /// A folder's fold state as drawn.
    pub open: bool,
    /// A folder's items the filter keeps, all the way down.
    pub count: usize,
}

/// The rows a filter and a fold state leave, and every kept item in list
/// order (the order next and previous walk, folded or not).
#[derive(Clone, Debug, Default)]
pub struct TreeRows {
    pub rows: Vec<TreeRow>,
    pub shown: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct TreeIndex {
    folders: Vec<TreeFolder>,
    /// Per item: its folder.
    item_folder: Vec<usize>,
    /// Per item: title, detail and folder names, folded for search.
    item_search: Vec<String>,
}

impl TreeIndex {
    /// Index items given as (folder, title, detail), in the source's
    /// order; item `n` of the index is the source's `n`th. The index keeps
    /// the folded search text, not the strings.
    pub fn build<F: AsRef<str>, T: AsRef<str>, D: AsRef<str>>(items: impl IntoIterator<Item = (F, T, D)>) -> Self {
        let mut index = TreeIndex { folders: vec![TreeFolder::default()], ..Default::default() };
        let mut by_path: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (at, (folder, title, detail)) in items.into_iter().enumerate() {
            let (folder, title, detail) = (folder.as_ref(), title.as_ref(), detail.as_ref());
            let mut parent = 0usize;
            let mut path = String::new();
            for name in folder.split('/').filter(|n| !n.is_empty()) {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str(name);
                parent = match by_path.get(&path) {
                    Some(&f) => f,
                    None => {
                        let f = index.folders.len();
                        let depth = if parent == 0 { 0 } else { index.folders[parent].depth + 1 };
                        index.folders.push(TreeFolder { name: name.to_string(), path: path.clone(), parent, depth, ..Default::default() });
                        index.folders[parent].folders.push(f);
                        by_path.insert(path.clone(), f);
                        f
                    }
                };
            }
            index.folders[parent].items.push(at);
            index.item_folder.push(parent);
            index.item_search.push(fold_words(&format!("{title} {detail} {}", folder.replace('/', " "))));
        }
        // Folders by name, case aside; items keep the source's order.
        let names: Vec<String> = index.folders.iter().map(|f| f.name.to_lowercase()).collect();
        for folder in &mut index.folders {
            folder.folders.sort_by(|&a, &b| names[a].cmp(&names[b]));
        }
        index
    }

    /// The index of a host's list.
    pub fn of_items(items: &[BrowserItem]) -> Self {
        Self::build(items.iter().map(|i| (i.folder.as_str(), i.title.as_str(), i.detail.as_str())))
    }

    pub fn item_count(&self) -> usize {
        self.item_folder.len()
    }

    pub fn folder(&self, at: usize) -> Option<&TreeFolder> {
        self.folders.get(at).filter(|_| at != 0)
    }

    /// The folder an item sits in (0 = the top).
    pub fn item_folder(&self, item: usize) -> usize {
        self.item_folder.get(item).copied().unwrap_or(0)
    }

    /// The folders an item sits in, innermost first.
    pub fn ancestors(&self, item: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = self.item_folder(item);
        while at != 0 {
            out.push(at);
            at = self.folders[at].parent;
        }
        out
    }

    /// Whether `item` has every word of a folded query (substrings, so
    /// "frac" finds Fractal and "geis" finds Geiss).
    pub fn matches(&self, item: usize, words: &[&str]) -> bool {
        let text = &self.item_search[item];
        words.iter().all(|w| text.contains(w))
    }

    /// The rows for the items `keep` answers true for. `open` says whether
    /// a folder (by its path) is unfolded. Folders with nothing kept are
    /// left out; folders come before items at each level.
    pub fn rows(&self, keep: impl Fn(usize) -> bool, open: impl Fn(&TreeFolder) -> bool) -> TreeRows {
        let mut count = vec![0usize; self.folders.len()];
        let mut kept = vec![false; self.item_folder.len()];
        for (item, &folder) in self.item_folder.iter().enumerate() {
            if keep(item) {
                kept[item] = true;
                let mut at = folder;
                loop {
                    count[at] += 1;
                    if at == 0 {
                        break;
                    }
                    at = self.folders[at].parent;
                }
            }
        }
        let mut out = TreeRows::default();
        self.walk(0, true, &count, &kept, &open, &mut out);
        out
    }

    fn walk(&self, at: usize, drawn: bool, count: &[usize], kept: &[bool], open: &impl Fn(&TreeFolder) -> bool, out: &mut TreeRows) {
        let folder = &self.folders[at];
        let unfolded = at == 0 || open(folder);
        if at != 0 && drawn {
            out.rows.push(TreeRow { key: RowKey::Folder(at), depth: folder.depth, open: unfolded, count: count[at] });
        }
        let inner = drawn && unfolded;
        for &sub in &folder.folders {
            if count[sub] > 0 {
                self.walk(sub, inner, count, kept, open, out);
            }
        }
        let depth = if at == 0 { 0 } else { folder.depth + 1 };
        for &item in &folder.items {
            if kept[item] {
                out.shown.push(item);
                if inner {
                    out.rows.push(TreeRow { key: RowKey::Item(item), depth, open: false, count: 0 });
                }
            }
        }
    }
}

/// The fold state of one list: what the person unfolded, and what they
/// folded while a filter unfolds every match.
#[derive(Clone, Debug, Default)]
pub struct Folds {
    pub open: HashSet<String>,
    pub shut_while_filtering: HashSet<String>,
}

impl Folds {
    pub fn is_open(&self, folder: &TreeFolder, filtering: bool) -> bool {
        if filtering {
            !self.shut_while_filtering.contains(&folder.path)
        } else {
            self.open.contains(&folder.path)
        }
    }

    pub fn set_open(&mut self, folder: &TreeFolder, filtering: bool, open: bool) {
        let path = folder.path.clone();
        if filtering {
            if open {
                self.shut_while_filtering.remove(&path);
            } else {
                self.shut_while_filtering.insert(path);
            }
        } else if open {
            self.open.insert(path);
        } else {
            self.open.remove(&path);
        }
    }
}
