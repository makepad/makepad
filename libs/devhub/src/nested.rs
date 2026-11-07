//! The repositories nested in a makepad checkout: each `apps/<name>` holding a git checkout of
//! its own (a clone, or a lane's worktree of one). makepad's `.gitignore` keeps them out of its
//! tree and its workspace takes their crates in. A pin, a lease's tree and its overlay name each
//! by its `name`, which is also the name of its store on a node.

use std::path::Path;

/// A nested repository's directory in the tree: `apps/<name>`.
pub fn dir(name: &str) -> String {
    format!("apps/{name}")
}

/// What a tree path of nested repository `name` starts with: `apps/<name>/`.
pub fn prefix(name: &str) -> String {
    format!("apps/{name}/")
}

/// Whether `name` can name a nested repository: lowercase letters, digits and dashes, and not
/// makepad's own name.
pub fn valid(name: &str) -> bool {
    !name.is_empty() && name != "makepad" && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The nested repositories of the checkout at `root`, by name.
pub fn names(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join("apps"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| valid(n))
        .collect();
    out.sort();
    out
}

/// The nested repository among `names` that tree path `path` is in, with the path inside it.
pub fn split<'a, 'n>(path: &'a str, names: impl IntoIterator<Item = &'n str>) -> Option<(&'n str, &'a str)> {
    names.into_iter().find_map(|n| path.strip_prefix("apps/")?.strip_prefix(n)?.strip_prefix('/').map(|rest| (n, rest)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_nested_repository_is_an_apps_dir_with_its_own_git() {
        let d = std::env::temp_dir().join(format!("devhub-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["apps/alpha/.git", "apps/beta-2/.git", "apps/plain/src", "apps/Upper/.git"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        // a worktree's `.git` is a file
        std::fs::create_dir_all(d.join("apps/gamma")).unwrap();
        std::fs::write(d.join("apps/gamma/.git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(super::names(&d), ["alpha", "beta-2", "gamma"]);
        assert!(super::names(&d.join("none")).is_empty());
        assert_eq!(super::split("apps/beta-2/src/lib.rs", ["alpha", "beta-2"]), Some(("beta-2", "src/lib.rs")));
        assert_eq!(super::split("apps/beta-20/x", ["beta-2"]), None);
        assert_eq!(super::split("apps/plain/x", ["alpha"]), None);
        assert!(!super::valid("makepad") && !super::valid("") && !super::valid("a/b") && super::valid("source-library"));
        assert_eq!((super::dir("alpha"), super::prefix("alpha")), ("apps/alpha".to_string(), "apps/alpha/".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
