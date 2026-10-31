//! Every path a tool touches goes through here: relative to the project
//! root, no `..` out of it, no symlink that leads out of it.

use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    pub fn new(root: &Path) -> Sandbox {
        Sandbox { root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()) }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path for `path` (relative to the root, or absolute inside
    /// it). The file need not exist yet.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let path = path.trim();
        let mut out = self.root.clone();
        let p = Path::new(path);
        let rel: PathBuf = if p.is_absolute() {
            match p.strip_prefix(&self.root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => return Err(format!("'{path}' is outside the project; use a path inside it")),
            }
        } else {
            p.to_path_buf()
        };
        for comp in rel.components() {
            match comp {
                Component::CurDir => {}
                Component::Normal(name) => out.push(name),
                Component::ParentDir => {
                    if out == self.root || !out.pop() || !out.starts_with(&self.root) {
                        return Err(format!("'{path}' leaves the project"));
                    }
                }
                _ => return Err(format!("'{path}' is not a project path")),
            }
        }
        // Symlinks: resolve the deepest ancestor that exists.
        let mut probe = out.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            match probe.canonicalize() {
                Ok(real) => {
                    if !real.starts_with(&self.root) {
                        return Err(format!("'{path}' resolves outside the project"));
                    }
                    let mut full = real;
                    for t in tail.iter().rev() {
                        full.push(t);
                    }
                    return Ok(full);
                }
                Err(_) => match probe.file_name().map(|n| n.to_os_string()) {
                    Some(name) => {
                        tail.push(name);
                        if !probe.pop() {
                            return Err(format!("'{path}' cannot be resolved"));
                        }
                    }
                    None => return Err(format!("'{path}' cannot be resolved")),
                },
            }
        }
    }

    /// `abs` as the model sees it: relative to the root.
    pub fn rel(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.root).unwrap_or(abs).to_string_lossy().replace('\\', "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("code-sandbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        dir
    }

    #[test]
    fn plain_paths_resolve_inside() {
        let dir = scratch("plain");
        let sb = Sandbox::new(&dir);
        assert_eq!(sb.resolve("sub/new.txt").unwrap(), sb.root().join("sub/new.txt"));
        assert_eq!(sb.resolve(".").unwrap(), sb.root());
        assert_eq!(sb.resolve("sub/../sub/a").unwrap(), sb.root().join("sub/a"));
        assert_eq!(sb.rel(&sb.root().join("sub/a")), "sub/a");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dotdot_and_absolute_outside_are_refused() {
        let dir = scratch("escape");
        let sb = Sandbox::new(&dir);
        assert!(sb.resolve("../x").is_err());
        assert!(sb.resolve("sub/../../x").is_err());
        assert!(sb.resolve("/etc/passwd").is_err());
        let inside = sb.root().join("sub").display().to_string();
        assert!(sb.resolve(&inside).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_refused() {
        let dir = scratch("link");
        let outside = scratch("link-outside");
        std::os::unix::fs::symlink(&outside, dir.join("sub/out")).unwrap();
        std::fs::write(outside.join("f"), "x").unwrap();
        let sb = Sandbox::new(&dir);
        assert!(sb.resolve("sub/out").is_err());
        assert!(sb.resolve("sub/out/f").is_err());
        assert!(sb.resolve("sub/out/new").is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
