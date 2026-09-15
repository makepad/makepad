//! Local depth-1 clones with alternates, then commit and merge across the
//! clones — all on a repository built with the API, no git binary.
use makepad_git::test_support::{tempdir, write_pack};
use makepad_git::*;
use std::fs;
use std::path::Path;

fn sig() -> Signature {
    Signature {
        name: "Test".into(),
        email: "info@makepad.nl".into(),
        timestamp: 1700000000,
        tz_offset: "+0000".into(),
    }
}

fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    Repository::open(dir).unwrap()
}

fn entry(name: &str, mode: u32, oid: ObjectId) -> TreeEntry {
    TreeEntry {
        mode,
        name: name.into(),
        oid,
    }
}

/// A source repository: README.md, src/lib.rs (nested tree) and packed.txt
/// (stored only in a pack), committed on `main`.
fn make_source(dir: &Path) -> ObjectId {
    let repo = init_repo(dir);
    let readme = repo.write_blob(b"hello\n").unwrap();
    let lib = repo.write_blob(b"fn x() {}\n").unwrap();
    let packed = write_pack(&dir.join(".git"), &[(ObjectKind::Blob, b"packed\n".to_vec())]).unwrap()[0];
    let src = repo
        .write_tree(&Tree {
            entries: vec![entry("lib.rs", 0o100644, lib)],
        })
        .unwrap();
    let mut entries = vec![
        entry("README.md", 0o100644, readme),
        entry("packed.txt", 0o100644, packed),
        entry("src", 0o040000, src),
    ];
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let root = repo.write_tree(&Tree { entries }).unwrap();
    let commit = repo
        .write_commit(&Commit {
            tree: root,
            parents: vec![],
            author: sig(),
            committer: sig(),
            message: "initial\n".into(),
        })
        .unwrap();
    repo.create_branch("main", &commit).unwrap();
    commit
}

fn copy_loose_objects(src: &Path, dst: &Path) {
    if let Ok(entries) = fs::read_dir(src) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.len() == 2 && entry.path().is_dir() {
                let dst_sub = dst.join(&name);
                let _ = fs::create_dir_all(&dst_sub);
                if let Ok(sub_entries) = fs::read_dir(entry.path()) {
                    for sub in sub_entries.flatten() {
                        let dst_file = dst_sub.join(sub.file_name());
                        if !dst_file.exists() {
                            let _ = fs::copy(sub.path(), dst_file);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn test_local_clone_commit_merge() {
    let base = tempdir().unwrap();
    let root = base.path().join("source");
    fs::create_dir_all(&root).unwrap();
    let head = make_source(&root);
    let checkout1 = base.path().join("checkout1");
    let checkout2 = base.path().join("checkout2");

    // --- Clone into checkout1 and checkout2 ---
    let t1 = local_clone_depth1(&root, &checkout1, Some("main")).expect("clone checkout1 failed");
    assert_eq!(t1.num_files, 3);
    assert_eq!(fs::read_to_string(checkout1.join("README.md")).unwrap(), "hello\n");
    assert_eq!(fs::read_to_string(checkout1.join("src/lib.rs")).unwrap(), "fn x() {}\n");
    assert_eq!(fs::read_to_string(checkout1.join("packed.txt")).unwrap(), "packed\n");
    assert!(t1.bytes_written > 0);
    let alternates = fs::read_to_string(checkout1.join(".git/objects/info/alternates")).unwrap();
    assert!(alternates.contains("source"), "alternates: {}", alternates);

    let t2 = local_clone_depth1(&root, &checkout2, Some("main")).expect("clone checkout2 failed");
    assert_eq!(t2.num_files, 3);

    // --- Verify checkout1 with our own API ---
    let mut repo1 = Repository::open(&checkout1).unwrap();
    assert_eq!(repo1.head_oid().unwrap(), head);
    assert_eq!(repo1.current_branch().unwrap().as_deref(), Some("main"));
    assert_eq!(repo1.read_index().unwrap().entries.len(), 3);
    let status = repo1.status().unwrap();
    assert!(
        !status.entries.iter().any(|e| matches!(
            e.status,
            FileStatus::Untracked | FileStatus::StagedNew | FileStatus::StagedDeleted | FileStatus::Deleted
        )),
        "working tree should be clean: {:?}",
        status.entries
    );
    // HEAD is readable through the alternate
    assert_eq!(repo1.read_commit(&head).unwrap().message, "initial\n");

    // --- Make a commit in checkout2 ---
    fs::write(checkout2.join("test_from_checkout2.txt"), "hello from checkout2\n").unwrap();
    let mut repo2 = Repository::open(&checkout2).unwrap();
    repo2.stage_file("test_from_checkout2.txt").unwrap();
    let commit2_oid = repo2.commit("commit from checkout2\n", sig()).unwrap();
    assert_eq!(repo2.head_oid().unwrap(), commit2_oid);
    assert_eq!(repo2.read_commit(&commit2_oid).unwrap().parents, vec![head]);

    // --- Merge checkout2 into checkout1 ---
    // Copy new objects from checkout2 to checkout1 (just the loose ones from the commit)
    copy_loose_objects(&checkout2.join(".git/objects"), &checkout1.join(".git/objects"));
    // Create a branch in checkout1 for the merge source
    refs::write_ref(&checkout1.join(".git"), "refs/heads/_from_checkout2", &commit2_oid).unwrap();

    let mut repo1 = Repository::open(&checkout1).unwrap();
    let result = repo1.merge_branch("_from_checkout2", sig()).unwrap();
    assert!(!result.has_conflict(), "merge: {}", result.content());

    // --- Verify merge result ---
    assert!(checkout1.join("test_from_checkout2.txt").exists(), "merged file must exist");
    assert_eq!(
        fs::read_to_string(checkout1.join("test_from_checkout2.txt")).unwrap(),
        "hello from checkout2\n"
    );
    let head1 = repo1.head_oid().unwrap();
    // checkout1 had no commits of its own: a fast-forward to checkout2's commit
    assert_eq!(head1, commit2_oid);
    let log = repo1.log(&head1, 5).unwrap();
    assert_eq!(log.len(), 2);
    let status = repo1.status().unwrap();
    assert!(
        !status.entries.iter().any(|e| matches!(
            e.status,
            FileStatus::Untracked | FileStatus::StagedNew | FileStatus::StagedDeleted | FileStatus::Deleted
        )),
        "working tree should be clean after the merge: {:?}",
        status.entries
    );
}

#[test]
fn local_clone_reads_objects_through_source_alternates() {
    let base = tempdir().unwrap();
    // `other` holds a loose blob and a packed blob; `src` only refers to them
    // through its alternates file.
    let other = base.path().join("other");
    fs::create_dir_all(&other).unwrap();
    let other_repo = init_repo(&other);
    let loose_alt = other_repo.write_blob(b"from alternate\n").unwrap();
    let packed_alt = write_pack(&other.join(".git"), &[(ObjectKind::Blob, b"packed alternate\n".to_vec())]).unwrap()[0];

    let src = base.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let src_repo = init_repo(&src);
    fs::create_dir_all(src.join(".git/objects/info")).unwrap();
    fs::write(
        src.join(".git/objects/info/alternates"),
        format!("{}\n", other.join(".git/objects").display()),
    )
    .unwrap();
    let own = src_repo.write_blob(b"own\n").unwrap();
    let mut entries = vec![
        entry("own.txt", 0o100644, own),
        entry("loose.txt", 0o100644, loose_alt),
        entry("packed.txt", 0o100644, packed_alt),
    ];
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let tree = src_repo.write_tree(&Tree { entries }).unwrap();
    let commit = src_repo
        .write_commit(&Commit {
            tree,
            parents: vec![],
            author: sig(),
            committer: sig(),
            message: "uses alternates\n".into(),
        })
        .unwrap();
    src_repo.create_branch("main", &commit).unwrap();
    drop(src_repo);

    let dst = base.path().join("dst");
    let timings = local_clone_depth1(&src, &dst, Some("main")).expect("clone through alternates");
    assert_eq!(timings.num_files, 3);
    assert_eq!(fs::read_to_string(dst.join("own.txt")).unwrap(), "own\n");
    assert_eq!(fs::read_to_string(dst.join("loose.txt")).unwrap(), "from alternate\n");
    assert_eq!(fs::read_to_string(dst.join("packed.txt")).unwrap(), "packed alternate\n");

    // The clone lists every source store as its alternate, so a repository
    // opened on it reaches the alternate's objects in one hop.
    let alternates = fs::read_to_string(dst.join(".git/objects/info/alternates")).unwrap();
    assert_eq!(alternates.lines().count(), 2, "alternates: {}", alternates);
    let mut dst_repo = Repository::open(&dst).unwrap();
    assert_eq!(dst_repo.read_blob(&loose_alt).unwrap(), b"from alternate\n");
    assert_eq!(dst_repo.read_blob(&packed_alt).unwrap(), b"packed alternate\n");
    assert_eq!(dst_repo.head_oid().unwrap(), commit);
}
