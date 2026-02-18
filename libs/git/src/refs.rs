use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::error::GitError;
use crate::oid::ObjectId;

#[derive(Debug, Clone)]
pub enum RefTarget {
    Direct(ObjectId),
    Symbolic(String),
}

#[derive(Debug, Clone)]
pub struct Ref {
    pub name: String,
    pub target: RefTarget,
}

/// Read HEAD — returns Symbolic("refs/heads/main") or Direct(oid).
pub fn read_head(git_dir: &Path) -> Result<RefTarget, GitError> {
    let head_path = git_dir.join("HEAD");
    let content = fs::read_to_string(&head_path)?;
    let content = content.trim_end();

    if let Some(rest) = content.strip_prefix("ref: ") {
        Ok(RefTarget::Symbolic(rest.to_string()))
    } else {
        Ok(RefTarget::Direct(ObjectId::from_hex(content)?))
    }
}

/// Read a single ref by name (e.g. "refs/heads/main").
/// Checks loose refs first, then packed-refs.
pub fn read_ref(git_dir: &Path, name: &str) -> Result<Option<RefTarget>, GitError> {
    // Try loose ref first
    let loose_path = git_dir.join(name);
    if loose_path.is_file() {
        let content = fs::read_to_string(&loose_path)?;
        let content = content.trim_end();
        if let Some(rest) = content.strip_prefix("ref: ") {
            return Ok(Some(RefTarget::Symbolic(rest.to_string())));
        }
        return Ok(Some(RefTarget::Direct(ObjectId::from_hex(content)?)));
    }

    // Try packed-refs
    let packed = read_packed_refs(git_dir)?;
    if let Some(oid) = packed.get(name) {
        return Ok(Some(RefTarget::Direct(*oid)));
    }

    Ok(None)
}

/// Resolve a ref to a concrete ObjectId, following symbolic refs.
pub fn resolve_ref(git_dir: &Path, name: &str) -> Result<ObjectId, GitError> {
    let mut current = name.to_string();
    for _ in 0..10 {
        match read_ref(git_dir, &current)? {
            Some(RefTarget::Direct(oid)) => return Ok(oid),
            Some(RefTarget::Symbolic(target)) => current = target,
            None => return Err(GitError::RefNotFound(current)),
        }
    }
    Err(GitError::InvalidRef(format!(
        "too many levels of symbolic refs from {}",
        name
    )))
}

/// Resolve HEAD to a concrete ObjectId.
pub fn resolve_head(git_dir: &Path) -> Result<ObjectId, GitError> {
    match read_head(git_dir)? {
        RefTarget::Direct(oid) => Ok(oid),
        RefTarget::Symbolic(name) => resolve_ref(git_dir, &name),
    }
}

/// Parse .git/packed-refs file.
///
/// Format:
/// ```text
/// # pack-refs with: peeled fully-peeled sorted
/// <sha1> <refname>
/// ^<sha1>           (peeled tag — we skip these)
/// ```
pub fn read_packed_refs(git_dir: &Path) -> Result<HashMap<String, ObjectId>, GitError> {
    let packed_path = git_dir.join("packed-refs");
    let mut refs = HashMap::new();

    let content = match fs::read_to_string(&packed_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(refs),
        Err(e) => return Err(GitError::Io(e)),
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        // "<sha1> <refname>"
        let mut parts = line.splitn(2, ' ');
        let hex = parts
            .next()
            .ok_or_else(|| GitError::InvalidRef("packed-refs: empty line".into()))?;
        let name = parts
            .next()
            .ok_or_else(|| GitError::InvalidRef("packed-refs: no refname".into()))?;
        let oid = ObjectId::from_hex(hex)?;
        refs.insert(name.to_string(), oid);
    }

    Ok(refs)
}

/// Write a ref atomically using a lockfile.
pub fn write_ref(git_dir: &Path, name: &str, oid: &ObjectId) -> Result<(), GitError> {
    let ref_path = git_dir.join(name);
    let lock_path = git_dir.join(format!("{}.lock", name));

    // Ensure parent directory exists
    if let Some(parent) = ref_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = format!("{}\n", oid.to_hex());
    fs::write(&lock_path, content.as_bytes())?;
    fs::rename(&lock_path, &ref_path)?;
    Ok(())
}

/// Update HEAD — either to a symbolic ref or a direct OID.
pub fn update_head(git_dir: &Path, target: &RefTarget) -> Result<(), GitError> {
    let head_path = git_dir.join("HEAD");
    let lock_path = git_dir.join("HEAD.lock");

    let content = match target {
        RefTarget::Symbolic(name) => format!("ref: {}\n", name),
        RefTarget::Direct(oid) => format!("{}\n", oid.to_hex()),
    };

    fs::write(&lock_path, content.as_bytes())?;
    fs::rename(&lock_path, &head_path)?;
    Ok(())
}

/// List all refs under a given prefix (e.g. "refs/heads/").
/// Returns refs from both loose and packed-refs, with loose taking precedence.
pub fn list_refs(git_dir: &Path, prefix: &str) -> Result<Vec<Ref>, GitError> {
    let mut result: HashMap<String, RefTarget> = HashMap::new();

    // Read packed refs first (loose will override)
    let packed = read_packed_refs(git_dir)?;
    for (name, oid) in packed {
        if name.starts_with(prefix) {
            result.insert(name, RefTarget::Direct(oid));
        }
    }

    // Walk loose refs directory
    let prefix_dir = git_dir.join(prefix);
    if prefix_dir.is_dir() {
        collect_loose_refs(git_dir, &prefix_dir, prefix, &mut result)?;
    }

    let mut refs: Vec<Ref> = result
        .into_iter()
        .map(|(name, target)| Ref { name, target })
        .collect();
    refs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(refs)
}

fn collect_loose_refs(
    git_dir: &Path,
    dir: &Path,
    prefix: &str,
    result: &mut HashMap<String, RefTarget>,
) -> Result<(), GitError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(GitError::Io(e)),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_loose_refs(git_dir, &path, prefix, result)?;
        } else if path.is_file() {
            // Compute ref name relative to git_dir
            let rel = path
                .strip_prefix(git_dir)
                .map_err(|_| GitError::InvalidRef("cannot strip git_dir prefix".into()))?;
            let name = rel.to_string_lossy().to_string();
            if name.starts_with(prefix) {
                let content = fs::read_to_string(&path)?;
                let content = content.trim_end();
                let target = if let Some(rest) = content.strip_prefix("ref: ") {
                    RefTarget::Symbolic(rest.to_string())
                } else {
                    RefTarget::Direct(ObjectId::from_hex(content)?)
                };
                result.insert(name, target);
            }
        }
    }
    Ok(())
}

/// Delete a ref (loose only — does not update packed-refs).
pub fn delete_ref(git_dir: &Path, name: &str) -> Result<(), GitError> {
    let ref_path = git_dir.join(name);
    match fs::remove_file(&ref_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(GitError::RefNotFound(name.to_string()))
        }
        Err(e) => Err(GitError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_head_symbolic() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        match read_head(git_dir).unwrap() {
            RefTarget::Symbolic(s) => assert_eq!(s, "refs/heads/main"),
            _ => panic!("expected symbolic"),
        }
    }

    #[test]
    fn test_read_head_detached() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        fs::write(
            git_dir.join("HEAD"),
            "552e2a0e14dd313f5f572118a18fba67ad99699c\n",
        )
        .unwrap();

        match read_head(git_dir).unwrap() {
            RefTarget::Direct(oid) => {
                assert_eq!(oid.to_hex(), "552e2a0e14dd313f5f572118a18fba67ad99699c")
            }
            _ => panic!("expected direct"),
        }
    }

    #[test]
    fn test_write_and_read_ref() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        let oid = ObjectId::from_hex("552e2a0e14dd313f5f572118a18fba67ad99699c").unwrap();

        write_ref(git_dir, "refs/heads/main", &oid).unwrap();

        match read_ref(git_dir, "refs/heads/main").unwrap() {
            Some(RefTarget::Direct(read_oid)) => assert_eq!(read_oid, oid),
            other => panic!("expected Direct, got {:?}", other),
        }
    }

    #[test]
    fn test_packed_refs() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        fs::write(
            git_dir.join("packed-refs"),
            "# pack-refs with: peeled fully-peeled sorted\n\
             552e2a0e14dd313f5f572118a18fba67ad99699c refs/heads/main\n\
             0f60d4fc088e59c246bd3fadff73454cf236b472 refs/heads/feature\n\
             ^deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n",
        )
        .unwrap();

        let packed = read_packed_refs(git_dir).unwrap();
        assert_eq!(packed.len(), 2);
        assert_eq!(
            packed["refs/heads/main"].to_hex(),
            "552e2a0e14dd313f5f572118a18fba67ad99699c"
        );
        assert_eq!(
            packed["refs/heads/feature"].to_hex(),
            "0f60d4fc088e59c246bd3fadff73454cf236b472"
        );
    }
}
