//! A Tandem patch library on disk (PATCH-FORMAT.md section 7): roots
//! scanned in order (factory, third party, user), every directory below a
//! root a category named by its relative path, every `*.fxp` a patch named
//! by its file stem; natural, case-insensitive order.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct LibraryPatch {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    /// Which root (index into the scanned roots).
    pub group: usize,
    /// Relative path with '/' separators (`Jacky Ligon/Atmospheres`); empty
    /// for loose files in a root.
    pub name: String,
    pub patches: Vec<LibraryPatch>,
}

impl Category {
    /// The parent category's name (`Jacky Ligon` for `Jacky Ligon/Atmospheres`).
    pub fn parent(&self) -> Option<&str> {
        self.name.rsplit_once('/').map(|(p, _)| p)
    }
}

/// Natural, case-insensitive comparison: digit runs compare as numbers.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n.push(c);
                    x.next();
                }
                let mut m = String::new();
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    m.push(d);
                    y.next();
                }
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn scan_dir(group: usize, root: &Path, dir: &Path, out: &mut Vec<Category>, include_root: bool) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut patches = Vec::new();
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fxp")) {
            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            patches.push(LibraryPatch { name, path: p });
        }
    }
    patches.sort_by(|a, b| natural_cmp(&a.name, &b.name));
    let rel = dir.strip_prefix(root).map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")).unwrap_or_default();
    if !rel.is_empty() || (include_root && !patches.is_empty()) {
        out.push(Category { group, name: rel, patches });
    }
    for d in subdirs {
        scan_dir(group, root, &d, out, include_root);
    }
}

/// Scan the roots in order; `user_root` (the last, if any) also lists loose
/// files in the root itself.
pub fn scan(roots: &[PathBuf], user_root: Option<&Path>) -> Vec<Category> {
    let mut out = Vec::new();
    let mut all: Vec<(PathBuf, bool)> = roots.iter().map(|r| (r.clone(), false)).collect();
    if let Some(u) = user_root {
        all.push((u.to_path_buf(), true));
    }
    for (group, (root, user)) in all.iter().enumerate() {
        let mut cats = Vec::new();
        scan_dir(group, root, root, &mut cats, *user);
        cats.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        out.extend(cats);
    }
    out
}

/// The factory and third-party roots inside an unpacked data folder.
pub fn data_roots(data: &Path) -> Vec<PathBuf> {
    vec![data.join("patches_factory"), data.join("patches_3rdparty")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["Pad 10", "pad 2", "Pad 1", "Bass", "bass 02"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["Bass", "bass 02", "Pad 1", "pad 2", "Pad 10"]);
    }
}
