//! `worktree_add/list/remove/prune` without a git binary: the created layout
//! is git's own, opens as a linked worktree, shares objects, and removal and
//! pruning follow `git worktree` rules.
use makepad_git::test_support::{tempdir, TempDir};
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

/// `main` with one commit on `main`, plus the path where `wt` will go.
fn fixture() -> (TempDir, PathBuf, PathBuf, ObjectId) {
    let base = tempdir().unwrap();
    let main = base.path().join("main");
    fs::create_dir_all(&main).unwrap();
    let mut repo = init_repo(&main);
    let (first, _) = commit(
        &mut repo,
        &[("file.txt", "hello\n"), ("dir/inner.txt", "inner\n")],
        vec![],
        "initial\n",
    );
    repo.create_branch("main", &first).unwrap();
    let wt = base.path().join("wt");
    (base, main, wt, first)
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[test]
fn add_new_branch_opens_as_linked_worktree_sharing_objects() {
    let (_base, main, wt, first) = fixture();
    let mut repo = Repository::open(&main).unwrap();
    let added = repo
        .worktree_add(&wt, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap();
    assert_eq!(added.name, "wt");
    assert_eq!(added.branch.as_deref(), Some("feature"));
    assert_eq!(added.head, Some(first));
    assert!(!added.main);

    // Layout is git's.
    let private = main.join(".git/worktrees/wt");
    assert_eq!(canon(&added.git_dir), canon(&private));
    assert_eq!(fs::read_to_string(private.join("HEAD")).unwrap(), "ref: refs/heads/feature\n");
    assert_eq!(fs::read_to_string(private.join("commondir")).unwrap(), "../..\n");
    assert_eq!(
        fs::read_to_string(private.join("gitdir")).unwrap().trim_end(),
        format!("{}", canon(&wt).join(".git").display())
    );
    assert!(private.join("index").is_file());
    assert!(fs::read_to_string(wt.join(".git")).unwrap().starts_with("gitdir: "));
    assert_eq!(fs::read_to_string(wt.join("file.txt")).unwrap(), "hello\n");
    assert_eq!(fs::read_to_string(wt.join("dir/inner.txt")).unwrap(), "inner\n");

    // Path resolution helpers accept it.
    let paths = repository_paths(&wt).unwrap().unwrap();
    assert_eq!(canon(&paths.git_dir), canon(&private));
    assert_eq!(canon(&paths.common_dir), canon(&main.join(".git")));
    assert_eq!(canon(&resolve_gitfile(&wt.join(".git")).unwrap()), canon(&private));
    assert_eq!(canon(&resolve_commondir(&private).unwrap()), canon(&main.join(".git")));

    // Opens on its own path with its own HEAD and the shared object store.
    let mut linked = Repository::open(&wt).unwrap();
    assert_eq!(linked.current_branch().unwrap().as_deref(), Some("feature"));
    assert_eq!(linked.head_oid().unwrap(), first);
    assert_eq!(linked.read_commit(&first).unwrap().message, "initial\n");
    assert!(linked.status().unwrap().entries.is_empty(), "fresh checkout is clean");
    let (second, _) = commit(&mut linked, &[("file.txt", "changed\n")], vec![first], "second\n");
    assert_eq!(repo.read_commit(&second).unwrap().message, "second\n");
    // The main worktree's HEAD is untouched and the new branch exists there.
    assert_eq!(repo.current_branch().unwrap().as_deref(), Some("main"));
    assert_eq!(repo.resolve_ref("refs/heads/feature").unwrap(), first);
}

#[test]
fn list_shows_main_and_linked_with_flags() {
    let (_base, main, wt, first) = fixture();
    let mut repo = Repository::open(&main).unwrap();
    repo.worktree_add(&wt, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap();
    let list = repo.worktree_list().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].main);
    assert_eq!(list[0].name, "");
    assert_eq!(canon(&list[0].path), canon(&main));
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    assert_eq!(list[0].head, Some(first));
    assert_eq!(list[1].name, "wt");
    assert_eq!(canon(&list[1].path), canon(&wt));
    assert_eq!(list[1].branch.as_deref(), Some("feature"));
    assert_eq!(list[1].head, Some(first));
    assert_eq!(list[1].locked, None);
    assert_eq!(list[1].prunable, None);

    // Locking is reported and suppresses prunable; a detached HEAD has no branch.
    fs::write(main.join(".git/worktrees/wt/locked"), "for tests\n").unwrap();
    fs::write(main.join(".git/worktrees/wt/HEAD"), format!("{}\n", first.to_hex())).unwrap();
    let list = repo.worktree_list().unwrap();
    assert_eq!(list[1].locked.as_deref(), Some("for tests"));
    assert_eq!(list[1].branch, None);
    assert_eq!(list[1].head, Some(first));

    // The same list is visible from the linked worktree.
    let linked = Repository::open(&wt).unwrap();
    assert_eq!(linked.worktree_list().unwrap().len(), 2);
}

#[test]
fn checked_out_branch_is_refused_and_names_deduplicate() {
    let (base, main, wt, first) = fixture();
    let mut repo = Repository::open(&main).unwrap();
    repo.worktree_add(&wt, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap();
    let other = base.path().join("other");
    let error = repo
        .worktree_add(&other, WorktreeBranch::Existing("feature"), None)
        .unwrap_err();
    assert!(error.to_string().contains("already checked out"), "{error}");
    assert!(!other.exists(), "nothing is created for a refused add");
    assert!(!main.join(".git/worktrees/other").exists());
    let error = repo
        .worktree_add(&other, WorktreeBranch::Existing("main"), None)
        .unwrap_err();
    assert!(error.to_string().contains("already checked out"), "{error}");
    let error = repo
        .worktree_add(&other, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");

    // A second registration for the same file name gets a numeric suffix.
    let nested = base.path().join("elsewhere").join("wt");
    let added = repo
        .worktree_add(&nested, WorktreeBranch::NewFrom { name: "second", start: first }, None)
        .unwrap();
    assert_eq!(added.name, "wt1");
    assert!(main.join(".git/worktrees/wt1/HEAD").is_file());

    // Explicit names and a non-empty destination.
    let named = base.path().join("named");
    fs::create_dir_all(&named).unwrap();
    fs::write(named.join("stale"), "x").unwrap();
    let error = repo
        .worktree_add(&named, WorktreeBranch::NewFrom { name: "third", start: first }, Some("third"))
        .unwrap_err();
    assert!(error.to_string().contains("not empty"), "{error}");
    fs::remove_file(named.join("stale")).unwrap();
    let added = repo
        .worktree_add(&named, WorktreeBranch::NewFrom { name: "third", start: first }, Some("third"))
        .unwrap();
    assert_eq!(added.name, "third");
    assert_eq!(repo.worktree_list().unwrap().len(), 4);
}

#[test]
fn remove_refuses_dirty_without_force_and_keeps_the_branch() {
    let (_base, main, wt, first) = fixture();
    let mut repo = Repository::open(&main).unwrap();
    repo.worktree_add(&wt, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap();
    fs::write(wt.join("file.txt"), "edited\n").unwrap();
    let error = repo.worktree_remove("wt", false).unwrap_err();
    assert!(error.to_string().contains("modified or untracked"), "{error}");
    assert!(wt.join("file.txt").is_file(), "dirty work is preserved");
    assert!(main.join(".git/worktrees/wt").is_dir());

    repo.worktree_remove(wt.to_str().unwrap(), true).unwrap();
    assert!(!wt.exists());
    assert!(!main.join(".git/worktrees/wt").exists());
    assert_eq!(repo.resolve_ref("refs/heads/feature").unwrap(), first, "branch is never touched");
    assert_eq!(repo.worktree_list().unwrap().len(), 1);
    assert!(repo.worktree_remove("wt", true).is_err(), "unknown worktree");
    assert!(repo.worktree_remove("", true).is_err(), "main is never removed");

    // A clean worktree removes without force; an untracked file counts as dirty.
    let wt2 = _base.path().join("wt2");
    repo.worktree_add(&wt2, WorktreeBranch::Existing("feature"), None).unwrap();
    fs::write(wt2.join("scratch.txt"), "x\n").unwrap();
    assert!(repo.worktree_remove("wt2", false).is_err());
    fs::remove_file(wt2.join("scratch.txt")).unwrap();
    repo.worktree_remove("wt2", false).unwrap();
    assert!(!wt2.exists());
}

#[test]
fn prune_drops_registrations_whose_checkout_vanished() {
    let (base, main, wt, first) = fixture();
    let mut repo = Repository::open(&main).unwrap();
    repo.worktree_add(&wt, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap();
    let keep = base.path().join("keep");
    repo.worktree_add(&keep, WorktreeBranch::NewFrom { name: "keep", start: first }, None)
        .unwrap();
    assert!(repo.worktree_prune().unwrap().is_empty());

    fs::remove_dir_all(&wt).unwrap();
    let list = repo.worktree_list().unwrap();
    let gone = list.iter().find(|e| e.name == "wt").unwrap();
    assert!(gone.prunable.as_deref().unwrap().contains("non-existent"), "{:?}", gone.prunable);
    assert_eq!(list.iter().find(|e| e.name == "keep").unwrap().prunable, None);
    assert_eq!(repo.worktree_prune().unwrap(), vec!["wt".to_string()]);
    assert!(!main.join(".git/worktrees/wt").exists());
    assert!(main.join(".git/worktrees/keep").is_dir());
    assert_eq!(repo.resolve_ref("refs/heads/feature").unwrap(), first);

    // A locked registration with a missing checkout is not pruned.
    fs::remove_dir_all(&keep).unwrap();
    fs::write(main.join(".git/worktrees/keep/locked"), "").unwrap();
    assert!(repo.worktree_prune().unwrap().is_empty());
    assert!(main.join(".git/worktrees/keep").is_dir());
}

#[test]
fn update_worktree_rewrites_only_changed_paths_and_refuses_local_edits() {
    let (_base, main, wt, first) = fixture();
    let mut repo = Repository::open(&main).unwrap();
    repo.worktree_add(&wt, WorktreeBranch::NewFrom { name: "feature", start: first }, None)
        .unwrap();
    let old_tree = repo.read_commit(&first).unwrap().tree;
    let (second, new_tree) = commit(
        &mut repo,
        &[("file.txt", "hello\n"), ("new.txt", "new\n")],
        vec![first],
        "second\n",
    );
    let mut linked = Repository::open(&wt).unwrap();
    let before = fs::metadata(wt.join("file.txt")).unwrap().modified().unwrap();
    linked.update_worktree(&old_tree, &new_tree).unwrap();
    assert_eq!(fs::read_to_string(wt.join("new.txt")).unwrap(), "new\n");
    assert!(!wt.join("dir").exists(), "removed file and its empty directory are gone");
    assert_eq!(fs::metadata(wt.join("file.txt")).unwrap().modified().unwrap(), before);
    linked.create_branch("feature", &second).unwrap();
    assert!(linked.status().unwrap().entries.is_empty(), "index follows the update");

    // A locally edited file is never overwritten.
    let (_, third_tree) = commit(&mut repo, &[("file.txt", "v3\n"), ("new.txt", "new\n")], vec![second], "third\n");
    fs::write(wt.join("file.txt"), "my edit\n").unwrap();
    let error = linked.update_worktree(&new_tree, &third_tree).unwrap_err();
    assert!(error.to_string().contains("modified locally"), "{error}");
    assert_eq!(fs::read_to_string(wt.join("file.txt")).unwrap(), "my edit\n");
}
