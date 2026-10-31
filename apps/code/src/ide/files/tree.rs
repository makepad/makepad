//! The workspace file tree: directories are listed the first time they are
//! opened, never ahead of time, so a big checkout costs nothing until the
//! person (or the agent touching a file) goes there. What git ignores is
//! not listed (the same rules as the file index), and `.git` never is.

use makepad_git::ignore::GitIgnore;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Node {
    /// Path relative to the root, with `/` separators.
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub dir: bool,
    pub open: bool,
}

/// Never listed, ignored or not.
const HIDDEN: &[&str] = &[".git", ".DS_Store"];

#[derive(Default)]
pub struct FileTree {
    root: PathBuf,
    /// Flat, in display order; an open directory is followed by its children.
    nodes: Vec<Node>,
    ignore: Option<GitIgnore>,
}

impl FileTree {
    pub fn new(root: &Path) -> Self {
        let mut tree = FileTree { root: root.to_path_buf(), nodes: Vec::new(), ignore: GitIgnore::with_user_excludes(root).ok() };
        tree.nodes = tree.list("", 0);
        tree
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    fn list(&mut self, rel: &str, depth: usize) -> Vec<Node> {
        let dir = if rel.is_empty() { self.root.clone() } else { self.root.join(rel) };
        let Ok(read) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut out: Vec<Node> = Vec::new();
        for e in read.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if HIDDEN.contains(&name.as_str()) {
                continue;
            }
            let dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if let Some(ig) = self.ignore.as_mut() {
                if ig.ignored(&path, dir).unwrap_or(false) {
                    continue;
                }
            }
            out.push(Node { path, name, depth, dir, open: false });
        }
        out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        out
    }

    /// Open or close the directory at `index`.
    pub fn toggle(&mut self, index: usize) {
        let Some(node) = self.nodes.get(index) else { return };
        if !node.dir {
            return;
        }
        if node.open {
            let end = self.subtree_end(index);
            self.nodes.drain(index + 1..end);
            self.nodes[index].open = false;
        } else {
            let (path, depth) = (node.path.clone(), node.depth);
            let children = self.list(&path, depth + 1);
            self.nodes[index].open = true;
            self.nodes.splice(index + 1..index + 1, children);
        }
    }

    /// One past the last row under the row at `index`.
    pub fn subtree_end(&self, index: usize) -> usize {
        let depth = self.nodes[index].depth;
        self.nodes[index + 1..].iter().position(|n| n.depth <= depth).map_or(self.nodes.len(), |p| index + 1 + p)
    }

    /// The row of the folder holding the row at `index`.
    pub fn parent(&self, index: usize) -> Option<usize> {
        let depth = self.nodes.get(index)?.depth;
        if depth == 0 {
            return None;
        }
        self.nodes[..index].iter().rposition(|n| n.depth + 1 == depth)
    }

    /// Open every directory above `path`, so it shows in the tree; its row.
    pub fn reveal(&mut self, path: &str) -> Option<usize> {
        let mut prefix = String::new();
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if let Some(i) = self.nodes.iter().position(|n| n.dir && n.path == prefix) {
                if !self.nodes[i].open {
                    self.toggle(i);
                }
            }
        }
        self.position(path)
    }

    /// List everything again (after files changed), keeping the folders
    /// that were open open; ignore rules are read afresh.
    pub fn refresh(&mut self) {
        let open: Vec<String> = self.nodes.iter().filter(|n| n.open).map(|n| n.path.clone()).collect();
        self.ignore = GitIgnore::with_user_excludes(&self.root).ok();
        self.nodes = self.list("", 0);
        // Parents come before children in `open`, so each reopens in place.
        for path in open {
            if let Some(i) = self.nodes.iter().position(|n| n.dir && n.path == path) {
                if !self.nodes[i].open {
                    self.toggle(i);
                }
            }
        }
    }

    /// Index of the node for `path`, if it is currently listed.
    pub fn position(&self, path: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("code-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\nout/\n*.tmp\n").unwrap();
        std::fs::write(dir.join("README.md"), "x").unwrap();
        std::fs::write(dir.join("scratch.tmp"), "x").unwrap();
        std::fs::write(dir.join("src/main.rs"), "x").unwrap();
        std::fs::write(dir.join("src/deep/a.rs"), "x").unwrap();
        dir
    }

    #[test]
    fn directories_list_lazily_dirs_first_and_skip_ignored() {
        let dir = scratch("lazy");
        let mut tree = FileTree::new(&dir);
        let names: Vec<_> = tree.nodes().iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["src", ".gitignore", "README.md"]);
        tree.toggle(0);
        let names: Vec<_> = tree.nodes().iter().map(|n| n.path.as_str()).collect();
        assert_eq!(names, ["src", "src/deep", "src/main.rs", ".gitignore", "README.md"]);
        assert_eq!(tree.parent(2), Some(0));
        assert_eq!(tree.subtree_end(0), 3);
        tree.toggle(0);
        assert_eq!(tree.nodes().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_opens_the_ancestors_and_refresh_keeps_them_open() {
        let dir = scratch("reveal");
        let mut tree = FileTree::new(&dir);
        assert_eq!(tree.reveal("src/deep/a.rs"), Some(2));
        assert_eq!(tree.nodes()[2].depth, 2);
        std::fs::write(dir.join("src/deep/b.rs"), "x").unwrap();
        tree.refresh();
        assert_eq!(tree.position("src/deep/b.rs"), Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
