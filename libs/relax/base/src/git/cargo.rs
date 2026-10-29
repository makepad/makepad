//! Cargo's local git sources. Scan the ident prefix; do not guess Cargo's URL hash.
use super::{Object, ObjectId, ObjectKind};
use crate::gitstore::{self as store, Oid};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    std::env::var_os("CARGO_HOME").filter(|h| !h.is_empty()).map(|h| cwd.join(h)).unwrap_or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or(cwd).join(".cargo")
    })
}

pub fn offline(dir: &Path) -> Result<bool, String> {
    if std::env::var_os("MAKEPAD_RUN_OFFLINE").is_some_and(|v| v != "0") {
        return Ok(true);
    }
    if let Some(v) = std::env::var_os("CARGO_NET_OFFLINE") {
        return match v.to_str() {
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            _ => Err("CARGO_NET_OFFLINE must be true or false".into()),
        };
    }
    let dir = std::env::current_dir().map_err(|e| e.to_string())?.join(dir);
    offline_config(&dir, &home())
}

fn offline_config(dir: &Path, home: &Path) -> Result<bool, String> {
    for config_dir in dir.ancestors().map(|p| p.join(".cargo")).chain(std::iter::once(home.to_path_buf())) {
        // Cargo prefers the extensionless file when both exist.
        for name in ["config", "config.toml"] {
            let path = config_dir.join(name);
            if !crate::vfs::is_file(&path) { continue; }
            let text = crate::vfs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let config = crate::toml::parse_toml(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if let Some(v) = config.get_path(&["net", "offline"]) {
                return v.as_bool().ok_or_else(|| format!("{}: net.offline must be a boolean", path.display()));
            }
            break;
        }
    }
    Ok(false)
}

fn repositories(dir: &Path, url: &str) -> Vec<PathBuf> {
    let url = url.trim_end_matches('/');
    let name = url.rsplit('/').next().unwrap_or("");
    let name = name.strip_suffix(".git").unwrap_or(name);
    let prefix = format!("{name}-").to_ascii_lowercase();
    let mut paths: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with(&prefix) && e.path().is_dir())
        .map(|e| e.path()).collect();
    paths.sort();
    paths
}

fn git_dir(checkout: &Path) -> Option<PathBuf> {
    let git = checkout.join(".git");
    if git.is_dir() { return Some(git); }
    let text = std::fs::read_to_string(&git).ok()?;
    Some(checkout.join(text.trim().strip_prefix("gitdir: ")?))
}

fn head(git: &Path) -> Option<Oid> {
    let text = std::fs::read_to_string(git.join("HEAD")).ok()?;
    if let Some(oid) = store::from_hex(text.trim()) { return Some(oid); }
    let reference = text.trim().strip_prefix("ref: ")?;
    let common = std::fs::read_to_string(git.join("commondir")).map(|p| git.join(p.trim())).unwrap_or_else(|_| git.to_path_buf());
    if let Ok(text) = std::fs::read_to_string(common.join(reference)) { return store::from_hex(text.trim()); }
    let packed = std::fs::read_to_string(common.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (oid, name) = line.split_once(' ')?;
        (name == reference).then(|| store::from_hex(oid)).flatten()
    })
}

pub fn checkout(home: &Path, url: &str, commit: &Oid) -> Option<PathBuf> {
    let rev = store::hex(commit);
    for repo in repositories(&home.join("git/checkouts"), url) {
        for entry in std::fs::read_dir(repo).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let short = name.to_string_lossy();
            let path = entry.path();
            // A short directory name alone is not a pin (nor is an unfinished checkout).
            if !short.is_empty() && rev.starts_with(short.as_ref()) && path.join(".cargo-ok").is_file()
                && git_dir(&path).and_then(|git| head(&git)) == Some(*commit) {
                return Some(path);
            }
        }
    }
    None
}

pub fn missing(home: &Path, url: &str, revision: &str) -> String {
    format!("{url}: git revision {revision} is not cached (offline); looked in {} and {}",
        home.join("git/checkouts").display(), home.join("git/db").display())
}

/// Only the bare-db fallback inflates packs. Completed checkouts are read directly.
struct Objects {
    dir: PathBuf,
    packs: Vec<PathBuf>,
    unpacked: HashMap<ObjectId, Object>,
}

fn loose(dir: &Path, oid: &ObjectId) -> Option<Object> {
    let hex = oid.to_hex();
    let data = std::fs::read(dir.join(&hex[..2]).join(&hex[2..])).ok()?;
    let data = crate::inflate::zlib_decompress_vec(&data).ok()?;
    let nul = data.iter().position(|b| *b == 0)?;
    let (kind, size) = std::str::from_utf8(&data[..nul]).ok()?.split_once(' ')?;
    let kind = match kind { "commit" => ObjectKind::Commit, "tree" => ObjectKind::Tree, "blob" => ObjectKind::Blob, "tag" => ObjectKind::Tag, _ => return None };
    let body = &data[nul + 1..];
    if size.parse::<usize>().ok()? != body.len() || super::hash_object(kind, body) != *oid { return None; }
    Some(Object { kind, data: body.to_vec() })
}

impl Objects {
    fn new(git: &Path) -> Self {
        let dir = git.join("objects");
        let packs = std::fs::read_dir(dir.join("pack")).into_iter().flatten().flatten()
            .map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pack")).collect();
        Self { dir, packs, unpacked: HashMap::new() }
    }

    fn get(&mut self, oid: &Oid) -> Option<(u8, Vec<u8>)> {
        let oid = ObjectId(*oid);
        if let Some(o) = loose(&self.dir, &oid) { return Some((o.kind as u8, o.data)); }
        loop {
            if let Some(o) = self.unpacked.get(&oid) { return Some((o.kind as u8, o.data.clone())); }
            let path = self.packs.pop()?;
            let Ok(pack) = std::fs::read(path) else { continue };
            let Ok(objects) = super::unpack_pack(&pack, &mut |id| self.unpacked.get(id).cloned().or_else(|| loose(&self.dir, id))) else { continue };
            self.unpacked.extend(objects);
        }
    }
}

/// Import only the pinned tree, not the repository's history, into the existing Relax store.
pub fn import(home: &Path, url: &str, commit: Oid, path: &Path, key: &str) -> Result<bool, String> {
    let mut repos = repositories(&home.join("git/db"), url);
    if let Some(git) = checkout(home, url, &commit).and_then(|p| git_dir(&p)) { repos.push(git); }
    for repo in repos {
        let mut reader = Objects::new(&repo);
        if !reader.get(&commit).is_some_and(|(k, _)| k == store::COMMIT) { continue; }
        let mut objects = HashMap::new();
        let table = store::path_table(&commit, &mut |oid| {
            let object = reader.get(oid)?;
            objects.insert(*oid, object.clone());
            Some(object)
        }).map_err(|e| format!("{}: {e}", repo.display()))?;
        for entry in table {
            if entry.mode == 0o160000 || objects.contains_key(&entry.oid) { continue; }
            let object = reader.get(&entry.oid).ok_or_else(|| format!("{}: missing {} for {}", repo.display(), store::hex(&entry.oid), entry.path))?;
            objects.insert(entry.oid, object);
        }
        drop(reader);
        let objects: Vec<_> = objects.into_iter().map(|(oid, (kind, data))| (oid, kind, data)).collect();
        store::append(path, store::Append { objects: &objects, reference: store::Ref { key: key.into(), commit, time: super::fetch::now() } }, &super::fetch::keep_two)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("relax-cargo-{name}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn cargo_checkout_requires_complete_and_exact_head() {
        let home = temp("checkout");
        let url = "https://example.org/team/project.git";
        let rev = "0123456789abcdef0123456789abcdef01234567";
        let commit = store::from_hex(rev).unwrap();
        let root = home.join("git/checkouts/project-unrelated-url-hash/0123456");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = 'project'\nversion = '0.1.0'\n").unwrap();
        std::fs::write(root.join(".git/HEAD"), rev).unwrap();
        assert_eq!(checkout(&home, url, &commit), None);
        std::fs::write(root.join(".cargo-ok"), "").unwrap();
        assert_eq!(checkout(&home, url, &commit), Some(root.clone()));
        // Same short prefix, different full commit: never accept it.
        std::fs::write(root.join(".git/HEAD"), "0123456789abcdef0123456789abcdef01234568").unwrap();
        assert_eq!(checkout(&home, url, &commit), None);
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(root.join(".git/packed-refs"), format!("{rev} refs/heads/main\n")).unwrap();
        assert_eq!(checkout(&home, url, &commit), Some(root.clone()));
        let linked = root.parent().unwrap().join("metadata");
        std::fs::rename(root.join(".git"), &linked).unwrap();
        std::fs::write(root.join(".git"), "gitdir: ../metadata\n").unwrap();
        assert_eq!(checkout(&home, url, &commit), Some(root));
        let missing = missing(&home, url, rev);
        for text in [url, rev, "git/checkouts", "git/db"] { assert!(missing.contains(text)); }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cargo_bare_db_loose_and_packed() {
        let home = temp("db");
        let url = "https://example.org/team/project";
        let db = home.join("git/db/project-url-hash");
        let blob = b"[package]\nname = 'project'\nversion = '0.1.0'\n".to_vec();
        let blob_id = super::super::hash_object(ObjectKind::Blob, &blob);
        let mut tree = b"100644 Cargo.toml\0".to_vec();
        tree.extend_from_slice(&blob_id.0);
        let tree_id = super::super::hash_object(ObjectKind::Tree, &tree);
        let commit = format!("tree {tree_id}\n\nlocal\n").into_bytes();
        let commit_id = super::super::hash_object(ObjectKind::Commit, &commit);
        let objects = [(ObjectKind::Blob, blob.clone()), (ObjectKind::Tree, tree), (ObjectKind::Commit, commit)];
        for (kind, data) in &objects {
            let oid = super::super::hash_object(*kind, data).to_hex();
            let mut raw = format!("{} {}\0", kind.as_str(), data.len()).into_bytes();
            raw.extend_from_slice(data);
            let dir = db.join("objects").join(&oid[..2]);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(&oid[2..]), crate::inflate::zlib_compress(&raw, 6)).unwrap();
        }
        assert!(import(&home, url, commit_id.0, &home.join("loose.store"), "HEAD").unwrap());
        std::fs::remove_dir_all(db.join("objects")).unwrap();
        // A pack with no .idx: the existing pack reader verifies object ids and checksum.
        let mut pack = b"PACK\0\0\0\x02".to_vec();
        pack.extend_from_slice(&(objects.len() as u32).to_be_bytes());
        for (kind, data) in &objects {
            let mut size = data.len();
            let mut byte = (*kind as u8) << 4 | (size & 15) as u8;
            size >>= 4;
            while size != 0 {
                pack.push(byte | 0x80);
                byte = (size & 127) as u8;
                size >>= 7;
            }
            pack.push(byte);
            pack.extend_from_slice(&crate::inflate::zlib_compress(data, 6));
        }
        let checksum = store::sha1(&pack);
        pack.extend_from_slice(&checksum);
        std::fs::create_dir_all(db.join("objects/pack")).unwrap();
        std::fs::write(db.join("objects/pack/local.pack"), pack).unwrap();
        assert!(import(&home, url, commit_id.0, &home.join("packed.store"), "HEAD").unwrap());
        assert!(!import(&home, url, [0; 20], &home.join("missing.store"), "HEAD").unwrap());
        for name in ["loose.store", "packed.store"] {
            let s = store::Store::open(&home.join(name)).unwrap().unwrap();
            assert_eq!(s.index.table(&commit_id.0).unwrap()[0].path, "Cargo.toml");
            assert_eq!(s.object(&blob_id.0).unwrap().unwrap().1, blob);
        }
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cargo_offline_config_precedence() {
        let root = temp("config");
        let home = root.join("home");
        let dir = root.join("project/member");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(dir.join(".cargo")).unwrap();
        std::fs::write(home.join("config.toml"), "[net]\noffline = true\n").unwrap();
        assert!(offline_config(&dir, &home).unwrap());
        std::fs::write(dir.join(".cargo/config.toml"), "net.offline = false\n").unwrap();
        assert!(!offline_config(&dir, &home).unwrap());
        std::fs::write(dir.join(".cargo/config"), "[net]\noffline = true\n").unwrap();
        assert!(offline_config(&dir, &home).unwrap());
        for flag in ["--offline", "--frozen"] {
            assert!(crate::resolve::parse_flags(&[flag.into()]).unwrap().offline);
        }
        assert!(!crate::resolve::parse_flags(&[]).unwrap().offline);
        std::fs::remove_dir_all(root).unwrap();
    }
}
