//! A real git store with no extracted source files, shared by VFS and Host tests.
use crate::gitstore::{self as store, Oid, Store};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub struct Fixture {
    pub store: Arc<Store>,
    pub commit: Oid,
    dir: PathBuf,
}

impl Fixture {
    pub fn new(files: &[(&str, &[u8])]) -> Self {
        fn object(objects: &mut Vec<(Oid, u8, Vec<u8>)>, kind: &str, tag: u8, data: Vec<u8>) -> Oid {
            let mut bytes = format!("{kind} {}\0", data.len()).into_bytes();
            bytes.extend_from_slice(&data);
            let oid = store::sha1(&bytes);
            objects.push((oid, tag, data));
            oid
        }
        fn tree(objects: &mut Vec<(Oid, u8, Vec<u8>)>, files: &[(&str, &[u8])]) -> Oid {
            let mut children = BTreeMap::<&str, Vec<(&str, &[u8])>>::new();
            let mut entries = BTreeMap::new();
            for &(path, data) in files {
                if let Some((dir, rest)) = path.split_once('/') {
                    children.entry(dir).or_default().push((rest, data));
                } else {
                    entries.insert(path, ("100644", object(objects, "blob", store::BLOB, data.to_vec())));
                }
            }
            for (dir, files) in children {
                entries.insert(dir, ("40000", tree(objects, &files)));
            }
            let mut bytes = Vec::new();
            for (name, (mode, oid)) in entries {
                bytes.extend_from_slice(format!("{mode} {name}\0").as_bytes());
                bytes.extend_from_slice(&oid);
            }
            object(objects, "tree", store::TREE, bytes)
        }
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("relax-vfs-spelling-{}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let mut objects = Vec::new();
        let root = tree(&mut objects, files);
        let commit = object(&mut objects, "commit", store::COMMIT,
            format!("tree {}\nauthor test <test> 0 +0000\ncommitter test <test> 0 +0000\n\nvfs\n", store::hex(&root)).into_bytes());
        let path = dir.join("git-test-00000000.store");
        store::append(&path, store::Append { objects: &objects,
            reference: store::Ref { key: "test".into(), commit, time: 1 } }, &|refs| refs.to_vec()).unwrap();
        Self { store: Arc::new(Store::open(&path).unwrap().unwrap()), commit, dir }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
}
