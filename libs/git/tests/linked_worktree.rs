//! Linked worktrees (`.git` file + `commondir`) opened and used without a
//! git binary: private HEAD/index, shared objects, packs, refs, packed-refs
//! and alternates.
use makepad_git::test_support::{build_pack_bytes, tempdir, write_pack, PackEntry, TempDir};
use makepad_git::*;
use std::fs;
use std::path::{Path, PathBuf};

fn sig() -> Signature {
    Signature {
        name: "Test".into(),
        email: "test@example.org".into(),
        timestamp: 0,
        tz_offset: "+0000".into(),
    }
}

/// An ordinary repository laid out by hand: `.git/{HEAD,objects,refs}`.
fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::create_dir_all(git.join("refs/tags")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    Repository::open(dir).unwrap()
}

fn commit(repo: &mut Repository, files: &[(&str, &str)], parents: Vec<ObjectId>, message: &str) -> (ObjectId, ObjectId) {
    let mut entries: Vec<TreeEntry> = files
        .iter()
        .map(|(name, content)| TreeEntry {
            mode: 0o100644,
            name: name.to_string(),
            oid: repo.write_blob(content.as_bytes()).unwrap(),
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let tree = repo.write_tree(&Tree { entries }).unwrap();
    let commit = repo
        .write_commit(&Commit {
            tree,
            parents,
            author: sig(),
            committer: sig(),
            message: message.to_string(),
        })
        .unwrap();
    (commit, tree)
}

/// Register `wt` as a linked worktree of `main` the way `git worktree add`
/// lays it out, with either an absolute or a relative gitfile pointer.
fn link_worktree(main: &Path, wt: &Path, name: &str, head: &str, relative: bool) -> PathBuf {
    let private = main.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("HEAD"), head).unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    fs::write(private.join("gitdir"), format!("{}\n", wt.join(".git").display())).unwrap();
    fs::create_dir_all(wt).unwrap();
    let pointer = if relative {
        format!("../{}/.git/worktrees/{}", main.file_name().unwrap().to_str().unwrap(), name)
    } else {
        private.display().to_string()
    };
    fs::write(wt.join(".git"), format!("gitdir: {}\n", pointer)).unwrap();
    private
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

/// A base directory holding `main` (with one commit on `main`) and `wt`.
fn fixture(relative: bool) -> (TempDir, PathBuf, PathBuf, ObjectId, PathBuf) {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("file.txt", "hello\n")], vec![], "initial\n");
    repo.create_branch("main", &first).unwrap();
    let (feature, _) = commit(&mut repo, &[("file.txt", "feature\n")], vec![first], "feature\n");
    repo.create_branch("feature", &feature).unwrap();
    let wt = base.path().join("wt");
    let private = link_worktree(&main, &wt, "wt", "ref: refs/heads/feature\n", relative);
    (base, main, wt, feature, private)
}

#[test]
fn absolute_gitfile_pointer_resolves_private_and_common_dirs() {
    let (_base, main, wt, feature, private) = fixture(false);
    let mut repo = Repository::open(&wt).unwrap();
    assert_eq!(canon(&repo.git_dir), canon(&private));
    assert_eq!(canon(&repo.common_dir), canon(&main.join(".git")));
    assert_eq!(canon(&repo.workdir), canon(&wt));
    assert!(matches!(repo.head().unwrap(), RefTarget::Symbolic(ref name) if name == "refs/heads/feature"));
    assert_eq!(repo.head_oid().unwrap(), feature);
    assert_eq!(repo.current_branch().unwrap().as_deref(), Some("feature"));
    let commit = repo.read_commit(&feature).unwrap();
    assert_eq!(commit.message, "feature\n");
    let tree = repo.read_tree(&commit.tree).unwrap();
    assert_eq!(repo.read_blob(&tree.entries[0].oid).unwrap(), b"feature\n");
}

#[test]
fn relative_gitfile_pointer_resolves_the_same_way() {
    let (_base, main, wt, feature, private) = fixture(true);
    let repo = Repository::open(&wt).unwrap();
    assert_eq!(canon(&repo.git_dir), canon(&private));
    assert_eq!(canon(&repo.common_dir), canon(&main.join(".git")));
    assert_eq!(repo.head_oid().unwrap(), feature);
    let paths = repository_paths(&wt).unwrap().unwrap();
    assert_eq!(canon(&paths.git_dir), canon(&private));
    assert_eq!(canon(&paths.common_dir), canon(&main.join(".git")));
}

#[test]
fn explicit_dot_git_paths_open() {
    let (_base, main, wt, feature, _) = fixture(false);
    let from_gitfile = Repository::open(&wt.join(".git")).unwrap();
    assert_eq!(from_gitfile.head_oid().unwrap(), feature);
    assert_eq!(canon(&from_gitfile.workdir), canon(&wt));
    let from_dir = Repository::open(&main.join(".git")).unwrap();
    assert_eq!(canon(&from_dir.git_dir), canon(&main.join(".git")));
    assert_eq!(canon(&from_dir.common_dir), canon(&main.join(".git")));
    assert!(matches!(from_dir.head().unwrap(), RefTarget::Symbolic(ref name) if name == "refs/heads/main"));
    // A nested directory inside the worktree walks up to the gitfile.
    fs::create_dir_all(wt.join("src/deep")).unwrap();
    let nested = Repository::open(&wt.join("src/deep")).unwrap();
    assert_eq!(nested.head_oid().unwrap(), feature);
}

#[test]
fn open_git_dir_reads_commondir() {
    let (_base, main, wt, feature, private) = fixture(false);
    let repo = Repository::open_git_dir(private.clone(), wt.clone()).unwrap();
    assert_eq!(canon(&repo.common_dir), canon(&main.join(".git")));
    assert_eq!(repo.head_oid().unwrap(), feature);
    assert_eq!(canon(&resolve_commondir(&private).unwrap()), canon(&main.join(".git")));
    assert_eq!(resolve_commondir(&main.join(".git")).unwrap(), main.join(".git"));
}

#[test]
fn malformed_pointers_are_errors_and_never_fall_through_to_an_ancestor() {
    let (_base, main, _wt, _feature, _) = fixture(false);
    // Nested inside the main repository's working directory on purpose.
    let bad = main.join("nested-bad");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join(".git"), "nonsense\n").unwrap();
    assert!(Repository::open(&bad).is_err());
    assert!(repository_paths(&bad).is_err());
    assert!(resolve_gitfile(&bad.join(".git")).is_err());

    let missing = main.join("nested-missing");
    fs::create_dir_all(&missing).unwrap();
    fs::write(missing.join(".git"), "gitdir: ../.git/worktrees/does-not-exist\n").unwrap();
    assert!(Repository::open(&missing).is_err());

    let empty = main.join("nested-empty");
    fs::create_dir_all(&empty).unwrap();
    fs::write(empty.join(".git"), "gitdir:   \n").unwrap();
    assert!(Repository::open(&empty).is_err());

    // A pointer at a plain directory that is not a git directory.
    let not_git = main.join("nested-not-git");
    fs::create_dir_all(not_git.join("target")).unwrap();
    fs::write(not_git.join(".git"), "gitdir: target\n").unwrap();
    assert!(Repository::open(&not_git).is_err());

    // A self-referencing pointer.
    let cyclic = main.join("nested-cyclic");
    fs::create_dir_all(&cyclic).unwrap();
    fs::write(cyclic.join(".git"), "gitdir: .\n").unwrap();
    assert!(Repository::open(&cyclic).is_err());

    // A chained gitfile (pointer at another gitfile).
    let chained = main.join("nested-chained");
    fs::create_dir_all(&chained).unwrap();
    fs::write(chained.join(".git"), format!("gitdir: {}\n", _wt.join(".git").display())).unwrap();
    assert!(Repository::open(&chained).is_err());

    // The main repository itself still opens fine.
    assert!(Repository::open(&main).is_ok());
}

#[test]
fn detached_private_head_is_independent_of_the_main_head() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("a", "1\n")], vec![], "one\n");
    let (second, _) = commit(&mut repo, &[("a", "2\n")], vec![first], "two\n");
    repo.create_branch("main", &second).unwrap();
    let wt = base.path().join("wt");
    link_worktree(&main, &wt, "wt", &format!("{}\n", first.to_hex()), false);
    let wt_repo = Repository::open(&wt).unwrap();
    assert!(matches!(wt_repo.head().unwrap(), RefTarget::Direct(oid) if oid == first));
    assert_eq!(wt_repo.head_oid().unwrap(), first);
    assert_eq!(wt_repo.current_branch().unwrap(), None);
    assert_eq!(Repository::open(&main).unwrap().head_oid().unwrap(), second);
}

#[test]
fn shared_loose_and_packed_refs_resolve_through_the_common_dir() {
    let (_base, main, wt, feature, _) = fixture(false);
    let mut main_repo = Repository::open(&main).unwrap();
    let (packed_commit, _) = commit(&mut main_repo, &[("p", "packed\n")], vec![feature], "packed\n");
    fs::write(
        main.join(".git/packed-refs"),
        format!(
            "# pack-refs with: peeled fully-peeled sorted\n{} refs/heads/packed\n{} refs/tags/v1\n",
            packed_commit.to_hex(),
            feature.to_hex()
        ),
    )
    .unwrap();
    // A symbolic branch pointing at another branch: resolution hops through
    // the routing twice.
    fs::write(main.join(".git/refs/heads/alias"), "ref: refs/heads/feature\n").unwrap();

    let wt_repo = Repository::open(&wt).unwrap();
    assert_eq!(wt_repo.resolve_ref("refs/heads/packed").unwrap(), packed_commit);
    assert_eq!(wt_repo.resolve_ref("refs/heads/alias").unwrap(), feature);
    assert_eq!(wt_repo.resolve_ref("refs/tags/v1").unwrap(), feature);
    let branches: Vec<String> = wt_repo.list_branches().unwrap().into_iter().map(|r| r.name).collect();
    assert_eq!(
        branches,
        ["refs/heads/alias", "refs/heads/feature", "refs/heads/main", "refs/heads/packed"]
    );
    let tags: Vec<String> = wt_repo.list_tags().unwrap().into_iter().map(|r| r.name).collect();
    assert_eq!(tags, ["refs/tags/v1"]);

    // Branches created from the worktree land in the shared refs; HEAD
    // changes stay private.
    wt_repo.create_branch("from-wt", &feature).unwrap();
    assert!(main.join(".git/refs/heads/from-wt").is_file());
    wt_repo.set_head_branch("from-wt").unwrap();
    assert_eq!(wt_repo.current_branch().unwrap().as_deref(), Some("from-wt"));
    assert_eq!(main_repo.current_branch().unwrap().as_deref(), Some("main"));
    wt_repo.delete_branch("from-wt").unwrap();
    assert!(!main.join(".git/refs/heads/from-wt").exists());
}

#[test]
fn shared_packed_objects_and_loose_root_tree_with_packed_descendants() {
    let (_base, main, wt, _feature, _) = fixture(false);
    let blob = b"packed blob\n".to_vec();
    let blob_oid = makepad_git::oid::hash_object("blob", &blob);
    let subtree = Tree {
        entries: vec![TreeEntry {
            mode: 0o100644,
            name: "inner.txt".into(),
            oid: blob_oid,
        }],
    };
    let subtree_bytes = makepad_git::tree::serialize_tree(&subtree);
    let ids = write_pack(
        &main.join(".git"),
        &[(ObjectKind::Blob, blob), (ObjectKind::Tree, subtree_bytes)],
    )
    .unwrap();
    assert_eq!(ids[0], blob_oid);
    let subtree_oid = ids[1];
    // The root tree is loose in the common dir and refers to the packed subtree.
    let main_repo = Repository::open(&main).unwrap();
    let root_oid = main_repo
        .write_tree(&Tree {
            entries: vec![TreeEntry {
                mode: 0o040000,
                name: "dir".into(),
                oid: subtree_oid,
            }],
        })
        .unwrap();

    let mut wt_repo = Repository::open(&wt).unwrap();
    assert_eq!(wt_repo.read_blob(&blob_oid).unwrap(), b"packed blob\n");
    let root = wt_repo.read_tree(&root_oid).unwrap();
    assert_eq!(root.entries[0].oid, subtree_oid);
    let inner = wt_repo.read_tree(&subtree_oid).unwrap();
    assert_eq!(inner.entries[0].name, "inner.txt");
    // Tree diffs walk packed descendants as well.
    let empty = main_repo.write_tree(&Tree { entries: vec![] }).unwrap();
    let changes = wt_repo.diff_trees(&empty, &root_oid).unwrap();
    assert_eq!(changes.len(), 1);
}

#[test]
fn alternates_are_resolved_relative_to_the_common_objects_dir() {
    let (_base, main, wt, _feature, _) = fixture(false);
    let other = _base.path().join("other");
    fs::create_dir_all(&other).unwrap();
    let other_repo = init_repo(&other);
    let only_in_other = other_repo.write_blob(b"from the alternate\n").unwrap();
    let far = _base.path().join("far");
    fs::create_dir_all(&far).unwrap();
    let far_repo = init_repo(&far);
    let only_in_far = far_repo.write_blob(b"from the absolute alternate\n").unwrap();
    fs::create_dir_all(main.join(".git/objects/info")).unwrap();
    fs::write(
        main.join(".git/objects/info/alternates"),
        format!("../../../other/.git/objects\n{}\n", far.join(".git/objects").display()),
    )
    .unwrap();
    let mut wt_repo = Repository::open(&wt).unwrap();
    assert_eq!(wt_repo.read_blob(&only_in_other).unwrap(), b"from the alternate\n");
    assert_eq!(wt_repo.read_blob(&only_in_far).unwrap(), b"from the absolute alternate\n");
}

#[test]
fn index_is_private_while_staged_blobs_are_shared() {
    let (_base, main, wt, _feature, private) = fixture(false);
    fs::write(wt.join("new.txt"), "staged in the worktree\n").unwrap();
    let mut wt_repo = Repository::open(&wt).unwrap();
    wt_repo.stage_file("new.txt").unwrap();
    assert!(private.join("index").is_file());
    assert!(!main.join(".git/index").exists());
    let index = wt_repo.read_index().unwrap();
    assert_eq!(index.entries.len(), 1);
    assert_eq!(index.entries[0].path, "new.txt");
    let blob = index.entries[0].oid;
    let (dir, file) = blob.loose_path_components();
    assert!(main.join(".git/objects").join(dir).join(file).is_file());
    // The main repository sees the object but not the worktree's index.
    let mut main_repo = Repository::open(&main).unwrap();
    assert_eq!(main_repo.read_blob(&blob).unwrap(), b"staged in the worktree\n");
    assert!(main_repo.read_index().map(|i| i.entries.is_empty()).unwrap_or(true));
}

// ===== Review findings: object sources, sync, refs, path resolution =====

/// Read back the raw bytes of an object written loose into a scratch repo, so
/// tests can put the same object into a pack elsewhere.
fn raw_object(repo: &mut Repository, oid: &ObjectId) -> (ObjectKind, Vec<u8>) {
    let object = repo.read_object(oid).unwrap();
    (object.kind, object.data)
}

#[test]
fn fresh_handles_read_packed_descendants_in_diff_status_checkout_and_merge_base() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let repo = init_repo(&main);
    // The subtree and its blob live only in a pack; the root tree and the
    // commit are loose.
    let blob = b"deep\n".to_vec();
    let blob_oid = makepad_git::oid::hash_object("blob", &blob);
    let subtree_bytes = makepad_git::tree::serialize_tree(&Tree {
        entries: vec![TreeEntry {
            mode: 0o100644,
            name: "inner.txt".into(),
            oid: blob_oid,
        }],
    });
    let ids = write_pack(
        &main.join(".git"),
        &[(ObjectKind::Blob, blob), (ObjectKind::Tree, subtree_bytes)],
    )
    .unwrap();
    let subtree_oid = ids[1];
    let top = repo.write_blob(b"top\n").unwrap();
    let root = repo
        .write_tree(&Tree {
            entries: vec![
                TreeEntry {
                    mode: 0o040000,
                    name: "dir".into(),
                    oid: subtree_oid,
                },
                TreeEntry {
                    mode: 0o100644,
                    name: "top.txt".into(),
                    oid: top,
                },
            ],
        })
        .unwrap();
    let first = repo
        .write_commit(&Commit {
            tree: root,
            parents: vec![],
            author: sig(),
            committer: sig(),
            message: "packed subtree\n".into(),
        })
        .unwrap();
    repo.create_branch("main", &first).unwrap();
    drop(repo);

    // Checkout on a fresh handle that never read a packed object.
    let mut fresh = Repository::open(&main).unwrap();
    fresh.checkout_branch("main").unwrap();
    assert_eq!(fs::read_to_string(main.join("dir/inner.txt")).unwrap(), "deep\n");
    assert_eq!(fs::read_to_string(main.join("top.txt")).unwrap(), "top\n");
    assert_eq!(fresh.read_index().unwrap().entries.len(), 2);

    // Status on a fresh handle: the HEAD tree walk reaches the packed subtree,
    // so nothing shows as staged-new or missing.
    let mut fresh = Repository::open(&main).unwrap();
    let status = fresh.status().unwrap();
    assert!(
        !status.entries.iter().any(|e| matches!(
            e.status,
            FileStatus::StagedNew | FileStatus::StagedDeleted | FileStatus::Untracked | FileStatus::Deleted
        )),
        "status: {:?}",
        status.entries
    );

    // Tree diff on a fresh handle.
    let mut fresh = Repository::open(&main).unwrap();
    let empty = fresh.write_tree(&Tree { entries: vec![] }).unwrap();
    let changes = fresh.diff_trees(&empty, &root).unwrap();
    let mut paths: Vec<String> = changes
        .iter()
        .map(|c| match c {
            TreeChange::Added { path, .. } => path.clone(),
            other => panic!("unexpected change {:?}", other),
        })
        .collect();
    paths.sort();
    assert_eq!(paths, ["dir/inner.txt", "top.txt"]);

    // A packed commit: merge_base and log on a fresh handle.
    let scratch = base.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let mut scratch_repo = init_repo(&scratch);
    let second = scratch_repo
        .write_commit(&Commit {
            tree: root,
            parents: vec![first],
            author: sig(),
            committer: sig(),
            message: "packed commit\n".into(),
        })
        .unwrap();
    let (kind, data) = raw_object(&mut scratch_repo, &second);
    let ids = write_pack(&main.join(".git"), &[(kind, data)]).unwrap();
    assert_eq!(ids[0], second);
    let fresh = Repository::open(&main).unwrap();
    fresh.create_branch("feature", &second).unwrap();
    let mut fresh = Repository::open(&main).unwrap();
    assert_eq!(fresh.merge_base(&second, &first).unwrap(), Some(first));
    let mut fresh = Repository::open(&main).unwrap();
    let log = fresh.log(&second, 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].1.message, "packed commit\n");

    // The same operations from a linked worktree of that repository.
    let wt = base.path().join("wt");
    link_worktree(&main, &wt, "wt", "ref: refs/heads/main\n", false);
    let mut wt_repo = Repository::open(&wt).unwrap();
    wt_repo.checkout_branch("main").unwrap();
    assert_eq!(fs::read_to_string(wt.join("dir/inner.txt")).unwrap(), "deep\n");
    let mut wt_repo = Repository::open(&wt).unwrap();
    assert_eq!(wt_repo.diff_trees(&empty, &root).unwrap().len(), 2);
}

#[test]
fn http_sync_into_a_linked_worktree_routes_shared_state_to_the_common_dir() {
    let (base, main, wt, feature, private) = fixture(false);
    // An existing configuration must survive the sync untouched.
    let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = keep-me\n";
    fs::write(main.join(".git/config"), config).unwrap();

    // Build the incoming objects in a scratch repository and pack them.
    let scratch = base.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();
    let mut scratch_repo = init_repo(&scratch);
    let blob = scratch_repo.write_blob(b"synced\n").unwrap();
    let tree = scratch_repo
        .write_tree(&Tree {
            entries: vec![TreeEntry {
                mode: 0o100644,
                name: "synced.txt".into(),
                oid: blob,
            }],
        })
        .unwrap();
    let commit = scratch_repo
        .write_commit(&Commit {
            tree,
            parents: vec![feature],
            author: sig(),
            committer: sig(),
            message: "synced\n".into(),
        })
        .unwrap();
    let entries: Vec<PackEntry> = [blob, tree, commit]
        .iter()
        .map(|oid| {
            let (kind, data) = raw_object(&mut scratch_repo, oid);
            PackEntry::Full(kind, data)
        })
        .collect();
    let (pack, _) = build_pack_bytes(&entries);

    let report = apply_pack_and_checkout(
        &wt,
        "http://example.invalid/repo.git",
        commit,
        Some("refs/heads/synced"),
        &pack,
        &mut NoopHttpSyncHooks,
    )
    .unwrap();
    assert_eq!(report.imported_objects, 3);
    assert_eq!(report.checked_out_files, 1);

    // Objects landed loose in the common store, not in the private dir.
    let (dir, file) = commit.loose_path_components();
    assert!(main.join(".git/objects").join(dir).join(file).is_file());
    assert!(!private.join("objects").exists());
    // Shared refs in the common dir; HEAD and the index private.
    assert!(main.join(".git/refs/heads/synced").is_file());
    assert!(main.join(".git/refs/remotes/origin/synced").is_file());
    let wt_repo = Repository::open(&wt).unwrap();
    assert!(matches!(wt_repo.head().unwrap(), RefTarget::Symbolic(ref name) if name == "refs/heads/synced"));
    assert_eq!(wt_repo.head_oid().unwrap(), commit);
    assert_eq!(fs::read_to_string(wt.join("synced.txt")).unwrap(), "synced\n");
    assert!(private.join("index").is_file());
    assert!(!main.join(".git/index").exists());
    assert_eq!(Repository::open(&main).unwrap().current_branch().unwrap().as_deref(), Some("main"));
    // `shallow` and `config` are shared; the existing config is preserved.
    assert!(main.join(".git/shallow").is_file());
    assert!(!private.join("shallow").exists());
    assert_eq!(fs::read_to_string(main.join(".git/config")).unwrap(), config);
    // Layout directories were not created in the private dir.
    assert!(!private.join("refs/heads").exists());
}

#[test]
fn list_refs_enumerates_both_stores_by_ownership() {
    let (_base, main, wt, feature, private) = fixture(false);
    let hex = format!("{}\n", feature.to_hex());
    // Main worktree's private namespaces live in the common dir.
    fs::create_dir_all(main.join(".git/refs/bisect")).unwrap();
    fs::write(main.join(".git/refs/bisect/bad"), &hex).unwrap();
    fs::create_dir_all(main.join(".git/refs/remotes/origin")).unwrap();
    fs::write(main.join(".git/refs/remotes/origin/main"), &hex).unwrap();
    // The linked worktree's own private namespaces.
    fs::create_dir_all(private.join("refs/bisect")).unwrap();
    fs::write(private.join("refs/bisect/good"), &hex).unwrap();
    fs::create_dir_all(private.join("refs/worktree")).unwrap();
    fs::write(private.join("refs/worktree/pin"), &hex).unwrap();

    let wt_repo = Repository::open(&wt).unwrap();
    let names = |prefix: &str| -> Vec<String> {
        refs::list_refs_in(&wt_repo.git_dir, &wt_repo.common_dir, prefix)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect()
    };
    assert_eq!(
        names("refs/"),
        [
            "refs/bisect/good",
            "refs/heads/feature",
            "refs/heads/main",
            "refs/remotes/origin/main",
            "refs/worktree/pin",
        ]
    );
    assert_eq!(names("refs/bisect"), ["refs/bisect/good"]);
    assert_eq!(names("refs/bisect/"), ["refs/bisect/good"]);
    assert_eq!(names("refs/heads/"), ["refs/heads/feature", "refs/heads/main"]);
    assert_eq!(wt_repo.resolve_ref("refs/bisect/good").unwrap(), feature);
    assert!(wt_repo.resolve_ref("refs/bisect/bad").is_err());

    // From the main worktree its own bisect refs are visible, the linked
    // worktree's are not.
    let main_repo = Repository::open(&main).unwrap();
    let main_names: Vec<String> = refs::list_refs_in(&main_repo.git_dir, &main_repo.common_dir, "refs/")
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert!(main_names.contains(&"refs/bisect/bad".to_string()));
    assert!(!main_names.contains(&"refs/bisect/good".to_string()));

    assert!(refs::is_private_ref("refs/bisect"));
    assert!(refs::is_private_ref("refs/bisect/x"));
    assert!(refs::is_private_ref("HEAD"));
    assert!(refs::is_private_ref("ORIG_HEAD"));
    assert!(!refs::is_private_ref("refs/bisection"));
    assert!(!refs::is_private_ref("refs/heads/main"));
}

#[test]
fn pointers_with_several_parent_segments_resolve() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("f", "1\n")], vec![], "one\n");
    repo.create_branch("main", &first).unwrap();

    // base/a/b/wt -> ../../../main/.git/worktrees/deep
    let wt = base.path().join("a/b/wt");
    let private = main.join(".git/worktrees/deep");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    fs::create_dir_all(&wt).unwrap();
    fs::write(wt.join(".git"), "gitdir: ../../../main/.git/worktrees/deep\n").unwrap();
    let opened = Repository::open(&wt).unwrap();
    assert_eq!(canon(&opened.git_dir), canon(&private));
    assert_eq!(opened.head_oid().unwrap(), first);
    assert_eq!(canon(&resolve_gitfile(&wt.join(".git")).unwrap()), canon(&private));
}

#[cfg(unix)]
#[test]
fn pointers_through_symlinks_use_filesystem_semantics() {
    use std::os::unix::fs::symlink;
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("f", "1\n")], vec![], "one\n");
    repo.create_branch("main", &first).unwrap();
    let private = main.join(".git/worktrees/wt3");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();

    // `deeplink` points at the private dir; `../deeplink/../wt3` only means
    // the private dir when `..` is applied to the link's target, as the
    // filesystem does. Lexically it would name the worktree itself.
    symlink(&private, base.path().join("deeplink")).unwrap();
    let wt = base.path().join("wt3");
    fs::create_dir_all(&wt).unwrap();
    fs::write(wt.join(".git"), "gitdir: ../deeplink/../wt3\n").unwrap();
    let opened = Repository::open(&wt).unwrap();
    assert_eq!(canon(&opened.git_dir), canon(&private));
    assert_eq!(canon(&opened.common_dir), canon(&main.join(".git")));
    assert_eq!(opened.head_oid().unwrap(), first);

    // A symlink to the repository as a whole works as a pointer base too.
    symlink(&main, base.path().join("link")).unwrap();
    let private2 = main.join(".git/worktrees/wt4");
    fs::create_dir_all(&private2).unwrap();
    fs::write(private2.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(private2.join("commondir"), "../..\n").unwrap();
    let wt4 = base.path().join("wt4");
    fs::create_dir_all(&wt4).unwrap();
    fs::write(wt4.join(".git"), "gitdir: ../link/.git/worktrees/wt4\n").unwrap();
    assert_eq!(canon(&Repository::open(&wt4).unwrap().git_dir), canon(&private2));
}

#[cfg(unix)]
#[test]
fn dangling_dot_git_symlink_is_an_error_not_ancestor_discovery() {
    use std::os::unix::fs::symlink;
    let (_base, main, _wt, _feature, _) = fixture(false);
    let nested = main.join("nested-dangling");
    fs::create_dir_all(&nested).unwrap();
    symlink(main.join("does-not-exist"), nested.join(".git")).unwrap();
    assert!(repository_paths(&nested).is_err());
    assert!(Repository::open(&nested).is_err());
    // A directory without any `.git` entry still discovers the ancestor.
    let plain = main.join("plain");
    fs::create_dir_all(&plain).unwrap();
    assert!(repository_paths(&plain).unwrap().is_none());
    assert!(Repository::open(&plain).is_ok());
}

#[test]
fn commondir_destinations_are_validated() {
    let (base, main, _wt, _feature, private) = fixture(false);
    let make = |name: &str, commondir: &str| -> PathBuf {
        let dir = main.join(".git/worktrees").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.join("commondir"), format!("{}\n", commondir)).unwrap();
        let wt = base.path().join(name);
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", dir.display())).unwrap();
        wt
    };
    // `.` points at the private directory itself.
    assert!(Repository::open(&make("self", ".")).is_err());
    // A plain directory is not a git directory.
    fs::create_dir_all(base.path().join("plain")).unwrap();
    let plain = base.path().join("plain").display().to_string();
    assert!(Repository::open(&make("plain", &plain)).is_err());
    // A directory with objects but no refs is not one either.
    fs::create_dir_all(base.path().join("half/objects")).unwrap();
    let half = base.path().join("half").display().to_string();
    assert!(Repository::open(&make("half", &half)).is_err());
    // A chained pointer.
    let chained = private.display().to_string();
    assert!(Repository::open(&make("chained", &chained)).is_err());
    // A missing destination.
    assert!(Repository::open(&make("missing", "../../nowhere")).is_err());
    // An empty pointer.
    assert!(Repository::open(&make("empty", "")).is_err());
    // The valid shape still opens.
    assert!(Repository::open(&make("valid", "../..")).is_ok());
    assert!(resolve_commondir(&main.join(".git")).is_ok());
}
