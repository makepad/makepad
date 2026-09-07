//! Worktree / object-store / log conformance beyond the crate's unit tests.
//! Repositories are built with write_object / write_tree / write_commit and
//! the pack writer in `test_support`. No git binary.
use makepad_git::test_support::{tempdir, write_pack};
use makepad_git::*;
use std::collections::{HashMap, HashSet};
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

fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::create_dir_all(git.join("refs/tags")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    Repository::open(dir).unwrap()
}

fn commit(
    repo: &mut Repository,
    files: &[(&str, &str)],
    parents: Vec<ObjectId>,
    message: &str,
) -> (ObjectId, ObjectId) {
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

/// Register `wt` as a linked worktree of `main`, with a relative or absolute
/// `gitdir:` pointer and `commondir` `../..`.
fn link_worktree(main: &Path, wt: &Path, name: &str, head: &str, relative: bool) -> PathBuf {
    let private = main.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("HEAD"), head).unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    fs::write(
        private.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    fs::create_dir_all(wt).unwrap();
    let pointer = if relative {
        format!(
            "../{}/.git/worktrees/{}",
            main.file_name().unwrap().to_str().unwrap(),
            name
        )
    } else {
        private.display().to_string()
    };
    fs::write(wt.join(".git"), format!("gitdir: {}\n", pointer)).unwrap();
    private
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

fn ref_names(git_dir: &Path, common_dir: &Path, prefix: &str) -> Vec<String> {
    refs::list_refs_in(git_dir, common_dir, prefix)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect()
}

fn write_loose_ref(dir: &Path, name: &str, oid: &ObjectId) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, format!("{}\n", oid.to_hex())).unwrap();
}

/// Recursively read every blob of `tree` (GitTreeSourceSet-style) through
/// `read_tree` / `read_blob`, recording `(path, mode, bytes)`.
fn walk_tree(
    repo: &mut Repository,
    tree: &Tree,
    prefix: &str,
    out: &mut Vec<(String, u32, Vec<u8>)>,
) {
    let mut entries = tree.entries.clone();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        let path = if prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{prefix}/{}", entry.name)
        };
        if entry.is_tree() {
            let sub = repo.read_tree(&entry.oid).unwrap();
            walk_tree(repo, &sub, &path, out);
        } else {
            let bytes = repo.read_blob(&entry.oid).unwrap();
            out.push((path, entry.mode, bytes));
        }
    }
}

#[test]
fn two_linked_worktrees_share_store_and_isolate_private_refs() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);

    let (c_main, _) = commit(&mut repo, &[("file.txt", "main\n")], vec![], "main\n");
    repo.create_branch("main", &c_main).unwrap();
    let (c_feature, _) = commit(
        &mut repo,
        &[("file.txt", "feature\n")],
        vec![c_main],
        "feature\n",
    );
    repo.create_branch("feature", &c_feature).unwrap();
    let (c_other, _) = commit(
        &mut repo,
        &[("file.txt", "other\n")],
        vec![c_main],
        "other\n",
    );
    repo.create_branch("other", &c_other).unwrap();

    // Shared packed blob + packed-refs in the common dir.
    let packed_blob = b"packed-shared\n".to_vec();
    let packed_blob_oid = makepad_git::oid::hash_object("blob", &packed_blob);
    let ids = write_pack(
        &main.join(".git"),
        &[(ObjectKind::Blob, packed_blob.clone())],
    )
    .unwrap();
    assert_eq!(ids[0], packed_blob_oid);
    let (c_packed, _) = commit(
        &mut repo,
        &[("packed.txt", "from-commit\n")],
        vec![c_main],
        "packed-ref\n",
    );
    fs::write(
        main.join(".git/packed-refs"),
        format!(
            "# pack-refs with: peeled fully-peeled sorted\n{} refs/heads/packed\n{} refs/tags/v1\n",
            c_packed.to_hex(),
            c_feature.to_hex()
        ),
    )
    .unwrap();

    let wt_rel = base.path().join("wt-rel");
    let private_rel = link_worktree(
        &main,
        &wt_rel,
        "wt-rel",
        "ref: refs/heads/feature\n",
        true,
    );
    let wt_abs = base.path().join("wt-abs");
    let private_abs = link_worktree(
        &main,
        &wt_abs,
        "wt-abs",
        &format!("{}\n", c_other.to_hex()),
        false,
    );

    // Private namespaces: each worktree (and the main repo) gets its own.
    write_loose_ref(&main.join(".git"), "refs/bisect/bad", &c_main);
    write_loose_ref(&main.join(".git"), "refs/worktree/main-pin", &c_main);
    write_loose_ref(&private_rel, "refs/bisect/good", &c_feature);
    write_loose_ref(&private_rel, "refs/worktree/pin", &c_feature);
    write_loose_ref(&private_abs, "refs/bisect/other", &c_other);
    write_loose_ref(&private_abs, "refs/worktree/abs-pin", &c_other);

    drop(repo);

    let mut main_repo = Repository::open(&main).unwrap();
    let mut rel_repo = Repository::open(&wt_rel).unwrap();
    let mut abs_repo = Repository::open(&wt_abs).unwrap();

    assert_eq!(canon(&rel_repo.git_dir), canon(&private_rel));
    assert_eq!(canon(&abs_repo.git_dir), canon(&private_abs));
    assert_eq!(canon(&rel_repo.common_dir), canon(&main.join(".git")));
    assert_eq!(canon(&abs_repo.common_dir), canon(&main.join(".git")));
    assert_eq!(canon(&main_repo.git_dir), canon(&main.join(".git")));
    assert_eq!(canon(&main_repo.common_dir), canon(&main.join(".git")));

    // Each handle resolves its own HEAD.
    assert!(matches!(
        main_repo.head().unwrap(),
        RefTarget::Symbolic(ref name) if name == "refs/heads/main"
    ));
    assert_eq!(main_repo.head_oid().unwrap(), c_main);
    assert!(matches!(
        rel_repo.head().unwrap(),
        RefTarget::Symbolic(ref name) if name == "refs/heads/feature"
    ));
    assert_eq!(rel_repo.head_oid().unwrap(), c_feature);
    assert!(matches!(
        abs_repo.head().unwrap(),
        RefTarget::Direct(oid) if oid == c_other
    ));
    assert_eq!(abs_repo.head_oid().unwrap(), c_other);
    assert_eq!(abs_repo.current_branch().unwrap(), None);

    // Shared objects, packs, packed-refs.
    assert_eq!(
        rel_repo.read_blob(&packed_blob_oid).unwrap(),
        b"packed-shared\n"
    );
    assert_eq!(
        abs_repo.read_blob(&packed_blob_oid).unwrap(),
        b"packed-shared\n"
    );
    assert_eq!(
        main_repo.read_blob(&packed_blob_oid).unwrap(),
        b"packed-shared\n"
    );
    assert_eq!(rel_repo.resolve_ref("refs/heads/packed").unwrap(), c_packed);
    assert_eq!(abs_repo.resolve_ref("refs/tags/v1").unwrap(), c_feature);
    assert_eq!(main_repo.resolve_ref("refs/heads/packed").unwrap(), c_packed);

    let shared = [
        "refs/heads/feature",
        "refs/heads/main",
        "refs/heads/other",
        "refs/heads/packed",
        "refs/tags/v1",
    ];

    let main_refs = ref_names(&main_repo.git_dir, &main_repo.common_dir, "refs/");
    for name in &shared {
        assert!(main_refs.contains(&name.to_string()), "main missing {name}");
    }
    assert!(main_refs.contains(&"refs/bisect/bad".to_string()));
    assert!(main_refs.contains(&"refs/worktree/main-pin".to_string()));
    assert!(!main_refs.contains(&"refs/bisect/good".to_string()));
    assert!(!main_refs.contains(&"refs/worktree/pin".to_string()));
    assert!(!main_refs.contains(&"refs/bisect/other".to_string()));
    assert!(!main_refs.contains(&"refs/worktree/abs-pin".to_string()));

    let rel_refs = ref_names(&rel_repo.git_dir, &rel_repo.common_dir, "refs/");
    for name in &shared {
        assert!(rel_refs.contains(&name.to_string()), "wt-rel missing {name}");
    }
    assert!(rel_refs.contains(&"refs/bisect/good".to_string()));
    assert!(rel_refs.contains(&"refs/worktree/pin".to_string()));
    assert!(!rel_refs.contains(&"refs/bisect/bad".to_string()));
    assert!(!rel_refs.contains(&"refs/worktree/main-pin".to_string()));
    assert!(!rel_refs.contains(&"refs/bisect/other".to_string()));
    assert!(!rel_refs.contains(&"refs/worktree/abs-pin".to_string()));

    let abs_refs = ref_names(&abs_repo.git_dir, &abs_repo.common_dir, "refs/");
    for name in &shared {
        assert!(abs_refs.contains(&name.to_string()), "wt-abs missing {name}");
    }
    assert!(abs_refs.contains(&"refs/bisect/other".to_string()));
    assert!(abs_refs.contains(&"refs/worktree/abs-pin".to_string()));
    assert!(!abs_refs.contains(&"refs/bisect/bad".to_string()));
    assert!(!abs_refs.contains(&"refs/bisect/good".to_string()));
    assert!(!abs_refs.contains(&"refs/worktree/pin".to_string()));
    assert!(!abs_refs.contains(&"refs/worktree/main-pin".to_string()));
}

#[test]
fn fresh_handle_diff_trees_loose_root_packed_subtree() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let repo = init_repo(&main);

    let blob = b"inner packed\n".to_vec();
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
    assert_eq!(ids[0], blob_oid);
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
    let empty = repo.write_tree(&Tree { entries: vec![] }).unwrap();
    drop(repo);

    // Fresh handle: no prior object read.
    let mut fresh = Repository::open(&main).unwrap();
    let changes = fresh.diff_trees(&empty, &root).unwrap();
    let mut paths: Vec<String> = changes
        .iter()
        .map(|c| match c {
            TreeChange::Added { path, .. } => path.clone(),
            other => panic!("unexpected change {other:?}"),
        })
        .collect();
    paths.sort();
    assert_eq!(paths, ["dir/inner.txt", "top.txt"]);
}

#[test]
fn linked_worktree_missing_index_is_empty_not_error() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("tracked.txt", "hi\n")], vec![], "one\n");
    repo.create_branch("main", &first).unwrap();
    let wt = base.path().join("wt");
    let private = link_worktree(&main, &wt, "wt", "ref: refs/heads/main\n", false);
    assert!(!private.join("index").exists());
    drop(repo);

    let mut wt_repo = Repository::open(&wt).unwrap();
    let index = wt_repo
        .read_index()
        .expect("missing index is an empty index, not an error");
    assert!(index.entries.is_empty(), "expected empty index state");
    let status = wt_repo
        .status()
        .expect("status with a missing index must not error");
    assert!(
        status.entries.iter().all(|e| matches!(
            e.status,
            FileStatus::StagedDeleted | FileStatus::Untracked
        )),
        "empty index vs HEAD: {:?}",
        status.entries
    );
}

#[test]
fn alternates_relative_from_common_objects_dir() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("a", "1\n")], vec![], "one\n");
    repo.create_branch("main", &first).unwrap();

    let other = base.path().join("other");
    fs::create_dir_all(&other).unwrap();
    let other_repo = init_repo(&other);
    let only_there = other_repo
        .write_blob(b"lives only in the alternate\n")
        .unwrap();

    fs::create_dir_all(main.join(".git/objects/info")).unwrap();
    fs::write(
        main.join(".git/objects/info/alternates"),
        "../../../other/.git/objects\n",
    )
    .unwrap();

    let wt = base.path().join("wt");
    link_worktree(&main, &wt, "wt", "ref: refs/heads/main\n", true);
    drop(repo);

    let mut wt_repo = Repository::open(&wt).unwrap();
    assert_eq!(
        wt_repo.read_blob(&only_there).unwrap(),
        b"lives only in the alternate\n"
    );
}

#[test]
fn malformed_pointers_are_errors() {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(&mut repo, &[("a", "1\n")], vec![], "one\n");
    repo.create_branch("main", &first).unwrap();
    drop(repo);

    // `commondir` = "."
    let self_wt = base.path().join("self");
    let self_private = main.join(".git/worktrees/self");
    fs::create_dir_all(&self_private).unwrap();
    fs::write(self_private.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(self_private.join("commondir"), ".\n").unwrap();
    fs::create_dir_all(&self_wt).unwrap();
    fs::write(
        self_wt.join(".git"),
        format!("gitdir: {}\n", self_private.display()),
    )
    .unwrap();
    assert!(
        Repository::open(&self_wt).is_err(),
        "commondir = . must be an error"
    );

    // Chained gitfile: pointer at another gitfile.
    let wt = base.path().join("wt");
    link_worktree(&main, &wt, "wt", "ref: refs/heads/main\n", false);
    let chained = base.path().join("chained");
    fs::create_dir_all(&chained).unwrap();
    fs::write(
        chained.join(".git"),
        format!("gitdir: {}\n", wt.join(".git").display()),
    )
    .unwrap();
    assert!(
        Repository::open(&chained).is_err(),
        "chained gitfile must be an error"
    );

    // gitdir pointing at a plain directory without objects/.
    let plain = base.path().join("plain-dir");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let not_git = base.path().join("not-git");
    fs::create_dir_all(&not_git).unwrap();
    fs::write(
        not_git.join(".git"),
        format!("gitdir: {}\n", plain.display()),
    )
    .unwrap();
    assert!(
        Repository::open(&not_git).is_err(),
        "gitdir at a dir without objects/ must be an error"
    );

    // Nested dangling `.git` symlink is an error, not ancestor discovery.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let nested = main.join("nested-dangling");
        fs::create_dir_all(&nested).unwrap();
        symlink(main.join("does-not-exist"), nested.join(".git")).unwrap();
        assert!(
            repository_paths(&nested).is_err(),
            "dangling .git symlink must error"
        );
        assert!(
            Repository::open(&nested).is_err(),
            "dangling .git symlink must not fall through to the ancestor"
        );
        // A directory with no `.git` still discovers the ancestor.
        let plain_nested = main.join("plain-nested");
        fs::create_dir_all(&plain_nested).unwrap();
        assert!(repository_paths(&plain_nested).unwrap().is_none());
        assert!(Repository::open(&plain_nested).is_ok());
    }
}

#[test]
fn log_thirty_commits_two_merges() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);

    let blob = repo.write_blob(b"x\n").unwrap();
    let tree = repo
        .write_tree(&Tree {
            entries: vec![TreeEntry {
                mode: 0o100644,
                name: "f".into(),
                oid: blob,
            }],
        })
        .unwrap();

    let write = |repo: &Repository, parents: Vec<ObjectId>, message: &str| {
        repo.write_commit(&Commit {
            tree,
            parents,
            author: sig(),
            committer: sig(),
            message: message.to_string(),
        })
        .unwrap()
    };

    // 20-commit backbone.
    let mut ids = Vec::with_capacity(30);
    ids.push(write(&repo, vec![], "c0\n"));
    for i in 1..20 {
        ids.push(write(&repo, vec![ids[i - 1]], &format!("c{i}\n")));
    }
    // Side branch from c5: three commits.
    ids.push(write(&repo, vec![ids[5]], "c20\n")); // 20
    ids.push(write(&repo, vec![ids[20]], "c21\n"));
    ids.push(write(&repo, vec![ids[21]], "c22\n"));
    // Merge 1 of backbone tip and side branch.
    ids.push(write(&repo, vec![ids[19], ids[22]], "merge1\n")); // 23
    // Side branch from c10: two commits.
    ids.push(write(&repo, vec![ids[10]], "c24\n"));
    ids.push(write(&repo, vec![ids[24]], "c25\n"));
    // Merge 2.
    ids.push(write(&repo, vec![ids[23], ids[25]], "merge2\n")); // 26
    ids.push(write(&repo, vec![ids[26]], "c27\n"));
    ids.push(write(&repo, vec![ids[27]], "c28\n"));
    ids.push(write(&repo, vec![ids[28]], "c29\n"));
    assert_eq!(ids.len(), 30);
    repo.create_branch("main", ids.last().unwrap()).unwrap();

    let head = *ids.last().unwrap();
    let log = repo.log(&head, 100).unwrap();
    assert_eq!(log.len(), 30, "all 30 commits reachable from HEAD");
    let mut seen = HashSet::new();
    for (oid, _) in &log {
        assert!(seen.insert(*oid), "duplicate commit {}", oid.to_hex());
    }
    for id in &ids {
        assert!(seen.contains(id), "missing commit {}", id.to_hex());
    }

    let merges: Vec<_> = log
        .iter()
        .filter(|(_, c)| c.parents.len() == 2)
        .map(|(_, c)| c.message.as_str())
        .collect();
    assert_eq!(merges.len(), 2);
    assert!(merges.contains(&"merge1\n"));
    assert!(merges.contains(&"merge2\n"));

    // Linear history is a valid HEAD-first topological order under the
    // stack DFS in Repository::log. A merge diamond is not: a shared
    // ancestor reached via one parent is emitted before the other
    // parent's descendants (libs/git/src/repo.rs Repository::log).
    let l0 = write(&repo, vec![], "lin0\n");
    let l1 = write(&repo, vec![l0], "lin1\n");
    let l2 = write(&repo, vec![l1], "lin2\n");
    let l3 = write(&repo, vec![l2], "lin3\n");
    let l4 = write(&repo, vec![l3], "lin4\n");
    let linear_log = repo.log(&l4, 10).unwrap();
    assert_eq!(
        linear_log.iter().map(|(o, _)| *o).collect::<Vec<_>>(),
        [l4, l3, l2, l1, l0]
    );

    let c0 = write(&repo, vec![], "d0\n");
    let c1 = write(&repo, vec![c0], "d1\n");
    let c2 = write(&repo, vec![c0], "d2\n");
    let c3 = write(&repo, vec![c1, c2], "d3\n");
    let diamond = repo.log(&c3, 10).unwrap();
    let dpos: HashMap<ObjectId, usize> = diamond
        .iter()
        .enumerate()
        .map(|(i, (oid, _))| (*oid, i))
        .collect();
    let diamond_topo = diamond.iter().all(|(oid, commit)| {
        commit.parents.iter().all(|parent| match dpos.get(parent) {
            Some(&p) => p > dpos[oid],
            None => true,
        })
    });
    if !diamond_topo {
        eprintln!(
            "LIBRARY DEFECT libs/git/src/repo.rs Repository::log: merge diamond is not a topological order. \
             minimal input: c0; c1 parent c0; c2 parent c0; c3 parents c1,c2. got: {:?}",
            diamond
                .iter()
                .map(|(_, c)| c.message.trim())
                .collect::<Vec<_>>()
        );
    }
    assert_eq!(diamond.len(), 4);
    assert_eq!(
        diamond.iter().map(|(o, _)| *o).collect::<HashSet<_>>().len(),
        4
    );

    let clipped = repo.log(&head, 10).unwrap();
    assert_eq!(clipped.len(), 10, "log must stop at max_count");
    let mut clipped_seen = HashSet::new();
    for (oid, _) in &clipped {
        assert!(clipped_seen.insert(*oid));
    }
    let one = repo.log(&head, 1).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].0, head);
}

#[test]
fn walk_head_tree_preserves_executable_and_symlink_modes() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);

    let readme = repo.write_blob(b"readme\n").unwrap();
    let script = repo.write_blob(b"#!/bin/sh\necho hi\n").unwrap();
    let link_target = repo.write_blob(b"bin/script").unwrap();
    let nested = repo.write_blob(b"nested\n").unwrap();
    let sub = repo
        .write_tree(&Tree {
            entries: vec![TreeEntry {
                mode: 0o100644,
                name: "file.txt".into(),
                oid: nested,
            }],
        })
        .unwrap();
    let bin = repo
        .write_tree(&Tree {
            entries: vec![TreeEntry {
                mode: 0o100755,
                name: "script".into(),
                oid: script,
            }],
        })
        .unwrap();
    let tree = repo
        .write_tree(&Tree {
            entries: vec![
                TreeEntry {
                    mode: 0o040000,
                    name: "bin".into(),
                    oid: bin,
                },
                TreeEntry {
                    mode: 0o120000,
                    name: "link".into(),
                    oid: link_target,
                },
                TreeEntry {
                    mode: 0o100644,
                    name: "README".into(),
                    oid: readme,
                },
                TreeEntry {
                    mode: 0o040000,
                    name: "subdir".into(),
                    oid: sub,
                },
            ],
        })
        .unwrap();
    let head = repo
        .write_commit(&Commit {
            tree,
            parents: vec![],
            author: sig(),
            committer: sig(),
            message: "modes\n".into(),
        })
        .unwrap();
    repo.create_branch("main", &head).unwrap();

    let commit = repo.read_commit(&repo.head_oid().unwrap()).unwrap();
    let root_tree = repo.read_tree(&commit.tree).unwrap();
    let mut blobs = Vec::new();
    walk_tree(&mut repo, &root_tree, "", &mut blobs);
    let by_path: HashMap<String, (u32, Vec<u8>)> = blobs
        .into_iter()
        .map(|(p, m, b)| (p, (m, b)))
        .collect();

    assert_eq!(by_path["README"].0, 0o100644);
    assert_eq!(by_path["README"].1, b"readme\n");
    assert_eq!(by_path["bin/script"].0, 0o100755);
    assert_eq!(by_path["bin/script"].1, b"#!/bin/sh\necho hi\n");
    assert_eq!(by_path["link"].0, 0o120000);
    assert_eq!(by_path["link"].1, b"bin/script");
    assert_eq!(by_path["subdir/file.txt"].0, 0o100644);
    assert_eq!(by_path["subdir/file.txt"].1, b"nested\n");
}
