//! Root-relative exclusions applied by every backend before traversal and
//! before probing: build outputs, version-control internals, caches and
//! recordings never produce events, are never walked and are never read.
//! This is distinct from what an analyser later deems eligible.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExcludePolicy {
    /// Directory names excluded at any depth below a root. A trailing `*`
    /// makes a name a prefix pattern (`target*` covers `target`,
    /// `target-wasm`, ...). Matched against one path component.
    pub dir_names: Vec<String>,
    /// Root-relative paths excluded with everything below them (`build/out`,
    /// `recordings`).
    pub paths: Vec<PathBuf>,
}

impl Default for ExcludePolicy {
    fn default() -> Self {
        Self {
            dir_names: vec![".git".to_string(), "target*".to_string()],
            paths: Vec::new(),
        }
    }
}

impl ExcludePolicy {
    /// Exclude nothing.
    pub fn none() -> Self {
        Self {
            dir_names: Vec::new(),
            paths: Vec::new(),
        }
    }

    pub fn dir_name(mut self, name: impl Into<String>) -> Self {
        self.dir_names.push(name.into());
        self
    }

    pub fn path(mut self, relative: impl Into<PathBuf>) -> Self {
        self.paths.push(relative.into());
        self
    }

    fn matches_name(&self, name: &OsStr) -> bool {
        let Some(name) = name.to_str() else {
            return false;
        };
        self.dir_names.iter().any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => !prefix.is_empty() && name.starts_with(prefix),
            None => name == pattern,
        })
    }

    fn relative_excluded(&self, relative: &Path, whole: bool) -> bool {
        let components: Vec<&OsStr> = relative.components().map(|c| c.as_os_str()).collect();
        let checked = if whole {
            &components[..]
        } else {
            &components[..components.len().saturating_sub(1)]
        };
        if checked.iter().any(|name| self.matches_name(name)) {
            return true;
        }
        self.paths.iter().any(|prefix| relative.starts_with(prefix))
    }

    /// Whether a directory (all its components count) is excluded.
    pub fn excludes_dir(&self, root: &Path, dir: &Path) -> bool {
        match dir.strip_prefix(root) {
            Ok(relative) => self.relative_excluded(relative, true),
            Err(_) => false,
        }
    }

    /// Whether a file is excluded because of the directories above it.
    pub fn excludes_file(&self, root: &Path, file: &Path) -> bool {
        match file.strip_prefix(root) {
            Ok(relative) => self.relative_excluded(relative, false),
            Err(_) => false,
        }
    }

    pub fn excludes(&self, root: &Path, path: &Path, is_dir: bool) -> bool {
        if is_dir {
            self.excludes_dir(root, path)
        } else {
            self.excludes_file(root, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_excludes_git_and_target_prefixes_at_any_depth() {
        let policy = ExcludePolicy::default();
        let root = Path::new("/w");
        assert!(policy.excludes_dir(root, Path::new("/w/target")));
        assert!(policy.excludes_dir(root, Path::new("/w/target-wasm")));
        assert!(policy.excludes_dir(root, Path::new("/w/apps/x/target/debug")));
        assert!(policy.excludes_dir(root, Path::new("/w/.git")));
        assert!(policy.excludes_file(root, Path::new("/w/.git/index")));
        assert!(policy.excludes_file(root, Path::new("/w/target/debug/build.log")));
        assert!(!policy.excludes_dir(root, Path::new("/w")), "the root itself never is");
        assert!(!policy.excludes_dir(root, Path::new("/w/src")));
        assert!(!policy.excludes_file(root, Path::new("/w/src/target")), "a file named target is a file");
        assert!(!policy.excludes_file(root, Path::new("/w/src/targets.rs")));
        assert!(!policy.excludes_dir(root, Path::new("/elsewhere/target")), "outside the root: not ours");
    }

    #[test]
    fn configured_paths_and_names_extend_the_policy() {
        let policy = ExcludePolicy::default()
            .dir_name("node_modules")
            .path("recordings")
            .path("build/out");
        let root = Path::new("/w");
        assert!(policy.excludes_file(root, Path::new("/w/web/node_modules/x/index.js")));
        assert!(policy.excludes_dir(root, Path::new("/w/recordings")));
        assert!(policy.excludes_file(root, Path::new("/w/recordings/take1.mp4")));
        assert!(policy.excludes_file(root, Path::new("/w/build/out/a.o")));
        assert!(!policy.excludes_file(root, Path::new("/w/build/src/a.rs")));
        assert!(!policy.excludes_dir(root, Path::new("/w/recordings-notes")));
        assert!(ExcludePolicy::none().excludes(root, Path::new("/w/target"), true) == false);
    }
}
