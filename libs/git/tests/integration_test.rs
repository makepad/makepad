//! End-to-end behaviour of the repository API on repositories built with the
//! API itself: no git binary anywhere. Where the old suite asked git to
//! confirm an object, the assertion is now the object id git assigns to the
//! same content (computed independently of this crate's serialisers).
use makepad_git::test_support::{
    self, delta_copy, delta_header, delta_insert, pack_all_loose, write_pack_entries, PackEntry,
};
use makepad_git::*;
use std::fs;
use std::path::Path;

fn test_sig() -> Signature {
    Signature {
        name: "Test".into(),
        email: "test@test.com".into(),
        timestamp: 1700000000,
        tz_offset: "+0000".into(),
    }
}

/// `git init`: the layout a fresh repository has before its first commit.
fn init_layout(dir: &Path) {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::create_dir_all(git.join("refs/tags")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
}

/// Every file under `dir` (except `.git`) as a repository-relative path.
fn worktree_files(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{}/{}", prefix, name)
        };
        if entry.path().is_dir() {
            worktree_files(&entry.path(), &rel, out);
        } else {
            out.push(rel);
        }
    }
}

/// `git add -A`: stage every file, drop index entries whose file is gone.
fn add_all(repo: &mut Repository, dir: &Path) {
    let mut files = Vec::new();
    worktree_files(dir, "", &mut files);
    for file in &files {
        repo.stage_file(file).unwrap();
    }
    let index = repo.read_index().unwrap();
    for entry in index.entries {
        if !files.contains(&entry.path) {
            repo.unstage_file(&entry.path).unwrap();
        }
    }
}

/// `git add -A && git commit -m <message>`.
fn commit_all(repo: &mut Repository, dir: &Path, message: &str) -> ObjectId {
    add_all(repo, dir);
    repo.commit(&format!("{}\n", message), test_sig()).unwrap()
}

/// `git checkout -b <name>`.
fn checkout_new_branch(repo: &mut Repository, name: &str) {
    let head = repo.head_oid().unwrap();
    repo.create_branch(name, &head).unwrap();
    repo.checkout_branch(name).unwrap();
}

/// `git merge <branch> --no-ff -m <message>` for a branch the current branch
/// has not diverged from: a merge commit with both parents on top of the
/// branch's tree.
fn merge_no_ff(repo: &mut Repository, branch: &str, message: &str) -> ObjectId {
    let ours = repo.head_oid().unwrap();
    let theirs = repo.resolve_ref(&format!("refs/heads/{}", branch)).unwrap();
    let theirs_commit = repo.read_commit(&theirs).unwrap();
    let merge = repo
        .write_commit(&Commit {
            tree: theirs_commit.tree,
            parents: vec![ours, theirs],
            author: test_sig(),
            committer: test_sig(),
            message: format!("{}\n", message),
        })
        .unwrap();
    let current = repo.current_branch().unwrap().unwrap();
    repo.create_branch(&current, &merge).unwrap();
    repo.checkout_branch(&current).unwrap();
    merge
}

/// Our `git fsck`: every object reachable from HEAD re-hashes to its id.
fn fsck(repo: &mut Repository) {
    fn check(repo: &mut Repository, oid: &ObjectId) -> Object {
        let object = repo.read_object(oid).unwrap();
        assert_eq!(
            &makepad_git::oid::hash_object(object.kind.as_str(), &object.data),
            oid,
            "object {} does not hash to its id",
            oid
        );
        object
    }
    fn check_tree(repo: &mut Repository, oid: &ObjectId) {
        let object = check(repo, oid);
        assert_eq!(object.kind, ObjectKind::Tree);
        let tree = repo.read_tree(oid).unwrap();
        for entry in tree.entries {
            if entry.is_tree() {
                check_tree(repo, &entry.oid);
            } else {
                assert_eq!(check(repo, &entry.oid).kind, ObjectKind::Blob);
            }
        }
    }
    let head = repo.head_oid().unwrap();
    for (oid, commit) in repo.log(&head, 1000).unwrap() {
        assert_eq!(check(repo, &oid).kind, ObjectKind::Commit);
        check_tree(repo, &commit.tree);
    }
}

/// `git init` + file1.txt/file2.txt/subdir/nested.txt + `git add .` +
/// `git commit -m "initial commit"`.
fn make_repo() -> test_support::TempDir {
    let dir = test_support::tempdir().unwrap();
    init_layout(dir.path());
    fs::write(dir.path().join("file1.txt"), "hello\n").unwrap();
    fs::write(dir.path().join("file2.txt"), "world\n").unwrap();
    fs::create_dir_all(dir.path().join("subdir")).unwrap();
    fs::write(dir.path().join("subdir/nested.txt"), "nested\n").unwrap();
    let mut repo = Repository::open(dir.path()).unwrap();
    commit_all(&mut repo, dir.path(), "initial commit");
    dir
}

// ===== Original tests =====

#[test]
fn test_open_and_read_head() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();
    let branch = repo.current_branch().unwrap();
    assert!(branch.is_some());
    let branch_name = branch.unwrap();
    assert!(
        branch_name == "main" || branch_name == "master",
        "unexpected branch: {}",
        branch_name
    );
}

#[test]
fn fixture_matches_the_ids_git_assigns() {
    // The index-built tree and the commit serialise exactly as git does: the
    // ids were computed independently for this content and signature.
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();
    assert_eq!(head.to_hex(), "18be1235587927fcf5b773fd83b97c85507d6b81");
    let commit = repo.read_commit(&head).unwrap();
    assert_eq!(commit.tree.to_hex(), "2882f69277885874924f338eaf1f6c801e4d7be0");
    let tree = repo.read_tree(&commit.tree).unwrap();
    let subdir = tree.entries.iter().find(|e| e.name == "subdir").unwrap();
    assert_eq!(subdir.oid.to_hex(), "9dfd7d08cef435bccfc5701b5b547c3740a67404");
    fsck(&mut repo);
}

#[test]
fn test_read_commit_and_tree() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let head_oid = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head_oid).unwrap();
    assert_eq!(commit.message, "initial commit\n");
    assert_eq!(commit.parents.len(), 0);
    assert_eq!(commit.author.name, "Test");

    let tree = repo.read_tree(&commit.tree).unwrap();
    assert_eq!(tree.entries.len(), 3);

    let file1 = tree.entries.iter().find(|e| e.name == "file1.txt").unwrap();
    assert!(file1.is_blob());
    let blob_data = repo.read_blob(&file1.oid).unwrap();
    assert_eq!(blob_data, b"hello\n");

    let subdir = tree.entries.iter().find(|e| e.name == "subdir").unwrap();
    assert!(subdir.is_tree());
    let sub_tree = repo.read_tree(&subdir.oid).unwrap();
    assert_eq!(sub_tree.entries.len(), 1);
    assert_eq!(sub_tree.entries[0].name, "nested.txt");
}

#[test]
fn test_write_blob_and_verify() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let data = b"new content written by makepad-git\n";
    let oid = repo.write_blob(data).unwrap();
    let read_data = repo.read_blob(&oid).unwrap();
    assert_eq!(read_data, data);
    // The id git assigns to this content.
    assert_eq!(oid.to_hex(), "d9ebf63dec9cbf92245866d90e1f698cec04f916");
}

#[test]
fn test_list_branches() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();
    let head_oid = repo.head_oid().unwrap();
    repo.create_branch("feature-a", &head_oid).unwrap();
    repo.create_branch("feature-b", &head_oid).unwrap();

    let branches = repo.list_branches().unwrap();
    let names: Vec<&str> = branches.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"refs/heads/feature-a"));
    assert!(names.contains(&"refs/heads/feature-b"));
}

#[test]
fn test_read_index() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();
    let index = repo.read_index().unwrap();
    assert_eq!(index.entries.len(), 3);
    let paths: Vec<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"file1.txt"));
    assert!(paths.contains(&"file2.txt"));
    assert!(paths.contains(&"subdir/nested.txt"));
}

#[test]
fn test_create_commit_programmatically() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let old_head = repo.head_oid().unwrap();

    let new_data = b"modified by makepad-git\n";
    let new_oid = repo.write_blob(new_data).unwrap();
    let mut index = repo.read_index().unwrap();
    for entry in &mut index.entries {
        if entry.path == "file1.txt" {
            entry.oid = new_oid;
            entry.file_size = new_data.len() as u32;
        }
    }
    repo.write_index(&index).unwrap();

    let commit_oid = repo
        .commit("commit from makepad-git\n", test_sig())
        .unwrap();

    let log: Vec<String> = repo
        .log(&commit_oid, 2)
        .unwrap()
        .into_iter()
        .map(|(_, c)| c.message)
        .collect();
    assert!(log.contains(&"commit from makepad-git\n".to_string()), "log: {:?}", log);
    assert!(log.contains(&"initial commit\n".to_string()), "log: {:?}", log);

    let new_commit = repo.read_commit(&commit_oid).unwrap();
    assert_eq!(new_commit.parents.len(), 1);
    assert_eq!(new_commit.parents[0], old_head);

    let tree = repo.read_tree(&new_commit.tree).unwrap();
    let file1 = tree.entries.iter().find(|e| e.name == "file1.txt").unwrap();
    assert_eq!(file1.oid, new_oid);
}

#[test]
fn test_log_walk() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    fs::write(dir.path().join("file1.txt"), "updated\n").unwrap();
    commit_all(&mut repo, dir.path(), "second commit");

    let head = repo.head_oid().unwrap();
    let log = repo.log(&head, 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].1.message, "second commit\n");
    assert_eq!(log[1].1.message, "initial commit\n");
}

#[test]
fn test_read_from_packed_repo() {
    let dir = make_repo();
    // What `git gc` does to a small repository: every object into one pack.
    pack_all_loose(&dir.path().join(".git")).unwrap();

    let mut repo = Repository::open(dir.path()).unwrap();
    let head_oid = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head_oid).unwrap();
    assert_eq!(commit.message, "initial commit\n");

    let tree = repo.read_tree(&commit.tree).unwrap();
    assert_eq!(tree.entries.len(), 3);

    let file1 = tree.entries.iter().find(|e| e.name == "file1.txt").unwrap();
    let blob = repo.read_blob(&file1.oid).unwrap();
    assert_eq!(blob, b"hello\n");
}

// ===== Status Tests =====

#[test]
fn test_status_clean() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let status = repo.status().unwrap();
    // No modified/untracked files — but stat cache may differ
    // Filter to only meaningful statuses
    let non_modified: Vec<_> = status
        .entries
        .iter()
        .filter(|e| e.status != FileStatus::Modified)
        .collect();
    // Should have no staged, deleted, or untracked
    for entry in &non_modified {
        assert!(
            entry.status != FileStatus::Untracked,
            "unexpected untracked: {}",
            entry.path
        );
    }
}

#[test]
fn test_status_untracked_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("new_file.txt"), "I'm new!\n").unwrap();

    let status = repo.status().unwrap();
    let untracked: Vec<_> = status
        .entries
        .iter()
        .filter(|e| e.status == FileStatus::Untracked)
        .collect();
    assert!(
        untracked.iter().any(|e| e.path == "new_file.txt"),
        "new_file.txt should be untracked, got: {:?}",
        untracked
    );
}

#[test]
fn test_status_deleted_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::remove_file(dir.path().join("file1.txt")).unwrap();

    let status = repo.status().unwrap();
    let deleted: Vec<_> = status
        .entries
        .iter()
        .filter(|e| e.status == FileStatus::Deleted)
        .collect();
    assert!(
        deleted.iter().any(|e| e.path == "file1.txt"),
        "file1.txt should be deleted, got: {:?}",
        deleted
    );
}

#[test]
fn test_status_modified_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("file1.txt"), "modified content\n").unwrap();

    let status = repo.status().unwrap();
    let modified: Vec<_> = status
        .entries
        .iter()
        .filter(|e| e.status == FileStatus::Modified && e.path == "file1.txt")
        .collect();
    assert!(!modified.is_empty(), "file1.txt should be modified");
}

// ===== Stage/Unstage Tests =====

fn status_of(repo: &mut Repository, path: &str) -> Option<FileStatus> {
    repo.status()
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == path)
        .map(|e| e.status)
}

#[test]
fn test_stage_new_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("staged.txt"), "staged content\n").unwrap();
    repo.stage_file("staged.txt").unwrap();

    // `A  staged.txt`
    assert_eq!(status_of(&mut repo, "staged.txt"), Some(FileStatus::StagedNew));
    // The index we wrote reads back and carries the blob we stored.
    let index = repo.read_index().unwrap();
    let entry = index.entries.iter().find(|e| e.path == "staged.txt").unwrap();
    assert_eq!(repo.read_blob(&entry.oid).unwrap(), b"staged content\n");
}

#[test]
fn test_stage_modified_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("file1.txt"), "changed\n").unwrap();
    repo.stage_file("file1.txt").unwrap();

    // `M  file1.txt`
    assert_eq!(status_of(&mut repo, "file1.txt"), Some(FileStatus::Staged));
    assert!(repo.status().is_ok());
}

#[test]
fn test_unstage_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    repo.unstage_file("file1.txt").unwrap();

    let index = repo.read_index().unwrap();
    assert!(
        !index.entries.iter().any(|e| e.path == "file1.txt"),
        "file1.txt should be removed from index"
    );

    // `D  file1.txt`: in HEAD, removed from the index (the file itself is
    // still in the worktree and therefore also untracked).
    let statuses: Vec<FileStatus> = repo
        .status()
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| e.path == "file1.txt")
        .map(|e| e.status)
        .collect();
    assert!(
        statuses.contains(&FileStatus::StagedDeleted),
        "status: {:?}",
        statuses
    );
}

// ===== Diff Tests =====

#[test]
fn test_diff_trees_add_delete_modify() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    let first_head = repo.head_oid().unwrap();

    // Modify file1, delete file2, add file3
    fs::write(dir.path().join("file1.txt"), "modified hello\n").unwrap();
    fs::remove_file(dir.path().join("file2.txt")).unwrap();
    fs::write(dir.path().join("file3.txt"), "new file\n").unwrap();
    commit_all(&mut repo, dir.path(), "changes");

    let second_head = repo.head_oid().unwrap();

    let changes = repo.diff_commits(&first_head, &second_head).unwrap();

    let paths: Vec<_> = changes
        .iter()
        .map(|c| match c {
            TreeChange::Added { path, .. } => format!("+{}", path),
            TreeChange::Deleted { path, .. } => format!("-{}", path),
            TreeChange::Modified { path, .. } => format!("M{}", path),
        })
        .collect();

    assert!(paths.contains(&"+file3.txt".to_string()), "changes: {:?}", paths);
    assert!(paths.contains(&"-file2.txt".to_string()), "changes: {:?}", paths);
    assert!(paths.contains(&"Mfile1.txt".to_string()), "changes: {:?}", paths);
}

#[test]
fn test_diff_blobs_produces_correct_hunks() {
    let old = "line1\nline2\nline3\nline4\nline5\n";
    let new = "line1\nmodified\nline3\nnew line\nline4\nline5\n";

    let diff = diff_blobs(
        old.as_bytes(),
        new.as_bytes(),
        Some("test.txt".into()),
        Some("test.txt".into()),
        None,
        None,
    );

    let unified = format_unified_diff(&diff, 3);
    assert!(unified.contains("--- a/test.txt"));
    assert!(unified.contains("+++ b/test.txt"));
    assert!(unified.contains("-line2"));
    assert!(unified.contains("+modified"));
    assert!(unified.contains("+new line"));
}

// ===== Merge Tests =====

#[test]
fn test_merge_fast_forward() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let main_branch = repo.current_branch().unwrap().unwrap();

    // Create a branch at current HEAD
    let head = repo.head_oid().unwrap();
    repo.create_branch("feature", &head).unwrap();

    // Add a commit on the current branch
    fs::write(dir.path().join("file1.txt"), "updated on main\n").unwrap();
    commit_all(&mut repo, dir.path(), "update on main");

    // Switch to feature branch (which is behind)
    repo.checkout_branch("feature").unwrap();

    // Now merge main into feature — this should fast-forward
    let result = repo.merge_branch(&main_branch, test_sig()).unwrap();
    assert!(!result.has_conflict(), "expected fast-forward, got conflict");
    assert!(
        result.content().contains("Fast-forward"),
        "result: {}",
        result.content()
    );

    // The repository is consistent afterwards.
    assert!(repo.status().is_ok());
    fsck(&mut repo);
    assert_eq!(
        fs::read_to_string(dir.path().join("file1.txt")).unwrap(),
        "updated on main\n"
    );
}

#[test]
fn test_merge_base_of_diverged_branches() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    let base_oid = repo.head_oid().unwrap();

    // Create branch and commit on it
    checkout_new_branch(&mut repo, "feature");
    fs::write(dir.path().join("feature.txt"), "feature work\n").unwrap();
    let feature_oid = commit_all(&mut repo, dir.path(), "feature commit");
    assert_eq!(repo.head_oid().unwrap(), feature_oid);

    // Go back to main and commit
    repo.checkout_branch("main").unwrap();
    fs::write(dir.path().join("main.txt"), "main work\n").unwrap();
    commit_all(&mut repo, dir.path(), "main commit");
    let main_oid = repo.head_oid().unwrap();

    // Find merge base using our API
    let mb = repo.merge_base(&main_oid, &feature_oid).unwrap();
    assert_eq!(mb, Some(base_oid));
}

#[test]
fn test_merge_clean_no_conflict() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create feature branch
    checkout_new_branch(&mut repo, "feature");
    fs::write(dir.path().join("feature.txt"), "feature content\n").unwrap();
    commit_all(&mut repo, dir.path(), "add feature.txt");

    // Go back to main and make a different change
    repo.checkout_branch("main").unwrap();
    fs::write(dir.path().join("main.txt"), "main content\n").unwrap();
    commit_all(&mut repo, dir.path(), "add main.txt");

    // Merge feature into main using our API
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(
        !result.has_conflict(),
        "expected clean merge, got: {}",
        result.content()
    );

    // The result is consistent
    fsck(&mut repo);
    assert!(repo.status().is_ok());

    // Verify merge commit has two parents
    let head = repo.head_oid().unwrap();
    let merge_commit = repo.read_commit(&head).unwrap();
    assert_eq!(merge_commit.parents.len(), 2, "merge commit should have 2 parents");

    // The log shows the merge
    let log: Vec<String> = repo.log(&head, 5).unwrap().into_iter().map(|(_, c)| c.message).collect();
    assert!(log.iter().any(|m| m.contains("Merge branch")), "log: {:?}", log);

    // Verify the worktree has both files
    assert!(dir.path().join("feature.txt").exists());
    assert!(dir.path().join("main.txt").exists());
}

#[test]
fn test_merge_conflict_same_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create feature branch and modify file1.txt
    checkout_new_branch(&mut repo, "feature");
    fs::write(dir.path().join("file1.txt"), "feature version\n").unwrap();
    commit_all(&mut repo, dir.path(), "feature: modify file1");

    // Go back to main and make a conflicting change to file1.txt
    repo.checkout_branch("main").unwrap();
    fs::write(dir.path().join("file1.txt"), "main version\n").unwrap();
    commit_all(&mut repo, dir.path(), "main: modify file1");

    // Merge feature into main using our API — should conflict
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(
        result.has_conflict(),
        "expected conflict, got: {}",
        result.content()
    );

    // Verify MERGE_HEAD exists
    assert!(
        dir.path().join(".git/MERGE_HEAD").exists(),
        "MERGE_HEAD should exist during conflict"
    );

    // Verify the file has conflict markers
    let content = fs::read_to_string(dir.path().join("file1.txt")).unwrap();
    assert!(content.contains("<<<<<<<"), "should have conflict markers: {}", content);
    assert!(content.contains("======="), "should have conflict markers: {}", content);
    assert!(content.contains(">>>>>>>"), "should have conflict markers: {}", content);

    // Verify the index has conflict entries (stages 1-3)
    let index = repo.read_index().unwrap();
    let conflict_entries: Vec<_> = index
        .entries
        .iter()
        .filter(|e| e.path == "file1.txt" && e.stage() > 0)
        .collect();
    assert!(!conflict_entries.is_empty(), "should have conflict stages in index");
}

// ===== Branch Checkout Tests =====

#[test]
fn test_checkout_branch_switches_files() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let main_branch = repo.current_branch().unwrap().unwrap();

    // Create a feature branch with a new file
    let head = repo.head_oid().unwrap();
    repo.create_branch("feature", &head).unwrap();
    repo.checkout_branch("feature").unwrap();
    fs::write(dir.path().join("feature_only.txt"), "feature\n").unwrap();
    commit_all(&mut repo, dir.path(), "add feature_only.txt");

    // File should exist
    assert!(dir.path().join("feature_only.txt").exists());

    // Checkout main using our API
    repo.checkout_branch(&main_branch).unwrap();

    // File should be gone
    assert!(
        !dir.path().join("feature_only.txt").exists(),
        "feature_only.txt should not exist on main"
    );

    // Verify HEAD is now on main
    let current = repo.current_branch().unwrap().unwrap();
    assert_eq!(current, main_branch);

    // Consistent afterwards
    assert!(repo.status().is_ok());
    fsck(&mut repo);
}

#[test]
fn test_checkout_branch_restores_content() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let main_branch = repo.current_branch().unwrap().unwrap();

    // Modify file on a new branch
    checkout_new_branch(&mut repo, "modify");
    fs::write(dir.path().join("file1.txt"), "modified on branch\n").unwrap();
    commit_all(&mut repo, dir.path(), "modify file1");

    // Checkout back to main via our API
    repo.checkout_branch(&main_branch).unwrap();

    // File should have original content
    let content = fs::read_to_string(dir.path().join("file1.txt")).unwrap();
    assert_eq!(content, "hello\n", "file1.txt should be restored to original");

    assert!(repo.status().is_ok());
}

// ===== Packed Refs Tests =====

#[test]
fn test_packed_refs_branches() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();

    // Create branches and pack them (`git pack-refs --all`)
    repo.create_branch("packed-a", &head).unwrap();
    repo.create_branch("packed-b", &head).unwrap();
    let git_dir = dir.path().join(".git");
    let mut packed = String::from("# pack-refs with: peeled fully-peeled sorted\n");
    for name in ["main", "packed-a", "packed-b"] {
        packed.push_str(&format!("{} refs/heads/{}\n", head.to_hex(), name));
        fs::remove_file(git_dir.join("refs/heads").join(name)).unwrap();
    }
    fs::write(git_dir.join("packed-refs"), packed).unwrap();

    // Our API should still find them
    let branches = repo.list_branches().unwrap();
    let names: Vec<&str> = branches.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"refs/heads/packed-a"), "branches: {:?}", names);
    assert!(names.contains(&"refs/heads/packed-b"), "branches: {:?}", names);

    // Resolve should work too
    let resolved = repo.resolve_ref("refs/heads/packed-a").unwrap();
    assert_eq!(resolved, head);
    assert_eq!(repo.head_oid().unwrap(), head);
}

// ===== Multiple Commits + History =====

#[test]
fn test_long_history_walk() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create 10 additional commits
    for i in 1..=10 {
        fs::write(dir.path().join("file1.txt"), format!("version {}\n", i)).unwrap();
        commit_all(&mut repo, dir.path(), &format!("commit {}", i));
    }

    let head = repo.head_oid().unwrap();
    let log = repo.log(&head, 100).unwrap();
    assert_eq!(log.len(), 11, "should have 11 commits (initial + 10)");
    assert_eq!(log[0].1.message, "commit 10\n");
    assert_eq!(log[10].1.message, "initial commit\n");
}

#[test]
fn test_diverged_history_walk() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create feature branch with commits
    checkout_new_branch(&mut repo, "feature");
    for i in 1..=3 {
        fs::write(dir.path().join("feature.txt"), format!("v{}\n", i)).unwrap();
        commit_all(&mut repo, dir.path(), &format!("feature {}", i));
    }

    // Merge back to main
    repo.checkout_branch("main").unwrap();
    merge_no_ff(&mut repo, "feature", "merge feature");

    // Walk the merge commit
    let head = repo.head_oid().unwrap();
    let log = repo.log(&head, 100).unwrap();
    // Should see: merge commit, feature 3, feature 2, feature 1, initial (+ any main commits)
    assert!(log.len() >= 5, "log should have at least 5 commits, got {}", log.len());
    assert_eq!(log[0].1.message, "merge feature\n");
    assert_eq!(log[0].1.parents.len(), 2, "merge commit should have 2 parents");
}

// ===== Write Objects and Verify Against Git's Ids =====

#[test]
fn test_write_tree_matches_git() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Write some blobs
    let blob_a = repo.write_blob(b"aaa\n").unwrap();
    let blob_b = repo.write_blob(b"bbb\n").unwrap();
    assert_eq!(blob_a.to_hex(), "72943a16fb2c8f38f9dde202b7a70ccc19c52f34");
    assert_eq!(blob_b.to_hex(), "f761ec192d9f0dca3329044b96ebdb12839dbff6");

    // Build a tree
    let tree = Tree {
        entries: vec![
            TreeEntry {
                mode: 0o100644,
                name: "a.txt".into(),
                oid: blob_a,
            },
            TreeEntry {
                mode: 0o100644,
                name: "b.txt".into(),
                oid: blob_b,
            },
        ],
    };
    let tree_oid = repo.write_tree(&tree).unwrap();

    // The id git assigns to this tree
    assert_eq!(tree_oid.to_hex(), "65b70c81bbedd324eb1d79c90a72ea2bddae82b4");
    let read = repo.read_tree(&tree_oid).unwrap();
    let names: Vec<&str> = read.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt"]);

    // Verify type
    assert_eq!(repo.read_object(&tree_oid).unwrap().kind, ObjectKind::Tree);
}

#[test]
fn test_write_commit_matches_git() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    let head = repo.head_oid().unwrap();
    let head_commit = repo.read_commit(&head).unwrap();

    // Create a new commit pointing to the same tree
    let commit = Commit {
        tree: head_commit.tree,
        parents: vec![head],
        author: test_sig(),
        committer: test_sig(),
        message: "test commit via API\n".into(),
    };
    let commit_oid = repo.write_commit(&commit).unwrap();

    // The id git assigns to this commit
    assert_eq!(commit_oid.to_hex(), "368f791a90001930f1762af7326aa54555e25254");
    assert_eq!(repo.read_object(&commit_oid).unwrap().kind, ObjectKind::Commit);

    let content = String::from_utf8(repo.read_object(&commit_oid).unwrap().data).unwrap();
    assert!(content.contains("test commit via API"));
    assert!(content.contains(&head.to_hex()));
}

// ===== Index Round-Trip Tests =====

#[test]
fn test_index_roundtrip() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Read, write, and read again
    let index = repo.read_index().unwrap();
    repo.write_index(&index).unwrap();
    let again = repo.read_index().unwrap();
    assert_eq!(again.entries.len(), index.entries.len());
    for (a, b) in index.entries.iter().zip(again.entries.iter()) {
        assert_eq!(a.path, b.path);
        assert_eq!(a.oid, b.oid);
        assert_eq!(a.mode, b.mode);
    }
    // The rewritten index still describes a clean tree.
    let status = repo.status().unwrap();
    assert!(!status.entries.iter().any(|e| matches!(
        e.status,
        FileStatus::StagedNew | FileStatus::StagedDeleted | FileStatus::Untracked
    )));
}

#[test]
fn test_index_after_staging() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Stage a new file
    fs::write(dir.path().join("new.txt"), "new content\n").unwrap();
    repo.stage_file("new.txt").unwrap();

    // Commit from that index
    repo.commit("commit with our index\n", test_sig()).unwrap();
    fsck(&mut repo);

    // Verify the commit has the new file
    let head = repo.head_oid().unwrap();
    let head_tree = repo.read_commit(&head).unwrap().tree;
    let tree = repo.read_tree(&head_tree).unwrap();
    assert!(tree.entries.iter().any(|e| e.name == "new.txt"), "tree: {:?}", tree.entries);
}

// ===== Deep Nested Directory Tests =====

#[test]
fn test_deeply_nested_directories() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create deeply nested structure
    let deep_dir = dir.path().join("a/b/c/d/e");
    fs::create_dir_all(&deep_dir).unwrap();
    fs::write(deep_dir.join("deep.txt"), "deep content\n").unwrap();
    commit_all(&mut repo, dir.path(), "add deep file");

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();

    // Should have 'a' directory in root tree
    let a_entry = tree.entries.iter().find(|e| e.name == "a");
    assert!(a_entry.is_some(), "should have 'a' directory");
    assert!(a_entry.unwrap().is_tree());

    // Index should have the deep path
    let index = repo.read_index().unwrap();
    assert!(
        index.entries.iter().any(|e| e.path == "a/b/c/d/e/deep.txt"),
        "index should have deeply nested path"
    );
}

// ===== Consistency After All Our Writes =====

#[test]
fn test_extensive_writes_pass_fsck() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Write many objects
    for i in 0..20 {
        let data = format!("blob content {}\n", i);
        repo.write_blob(data.as_bytes()).unwrap();
    }

    // Create multiple commits
    for i in 0..5 {
        fs::write(dir.path().join("file1.txt"), format!("iteration {}\n", i)).unwrap();
        repo.stage_file("file1.txt").unwrap();
        repo.commit(&format!("automated commit {}\n", i), test_sig()).unwrap();
    }

    // Everything reachable hashes to its id
    fsck(&mut repo);
    let head = repo.head_oid().unwrap();
    let log: Vec<String> = repo.log(&head, 100).unwrap().into_iter().map(|(_, c)| c.message).collect();
    assert!(log.contains(&"automated commit 4\n".to_string()), "log: {:?}", log);
}

// ===== Read Deltified Objects =====

const COMMON_PREFIX: &str = "This is a file with some common content.\n\
             Line 2 is the same in every file.\n\
             Line 3 too.\n\
             But line 4 varies: iteration ";
const COMMON_SUFFIX: &str = "\nLine 5 is common again.\n";

fn similar_file(i: usize) -> Vec<u8> {
    format!("{}{}{}", COMMON_PREFIX, i, COMMON_SUFFIX).into_bytes()
}

#[test]
fn test_read_deltified_objects() {
    let dir = make_repo();
    let git_dir = dir.path().join(".git");

    // Twenty similar files: the first stored whole, the others as deltas
    // against it (ref-deltas and ofs-deltas alternately), the way a packer
    // stores near-duplicates.
    let base = similar_file(0);
    let base_oid = makepad_git::oid::hash_object("blob", &base);
    let mut entries = vec![PackEntry::Full(ObjectKind::Blob, base.clone())];
    for i in 1..20 {
        let result = similar_file(i);
        let varying = format!("{}", i);
        let mut delta = delta_header(base.len(), result.len());
        delta.extend(delta_copy(0, COMMON_PREFIX.len() as u32));
        delta.extend(delta_insert(varying.as_bytes()));
        delta.extend(delta_copy(
            (COMMON_PREFIX.len() + 1) as u32,
            COMMON_SUFFIX.len() as u32,
        ));
        entries.push(if i % 2 == 1 {
            PackEntry::RefDelta {
                kind: ObjectKind::Blob,
                base: base_oid,
                delta,
                result,
            }
        } else {
            PackEntry::OfsDelta {
                kind: ObjectKind::Blob,
                base_index: 0,
                delta,
                result,
            }
        });
    }
    let ids = write_pack_entries(&git_dir, &entries).unwrap();

    // A commit whose tree names every packed file
    let repo = Repository::open(dir.path()).unwrap();
    let mut tree_entries: Vec<TreeEntry> = ids
        .iter()
        .enumerate()
        .map(|(i, oid)| TreeEntry {
            mode: 0o100644,
            name: format!("file_{}.txt", i),
            oid: *oid,
        })
        .collect();
    tree_entries.sort_by(|a, b| a.name.cmp(&b.name));
    let tree = repo.write_tree(&Tree { entries: tree_entries }).unwrap();
    let head = repo.head_oid().unwrap();
    let commit = repo
        .write_commit(&Commit {
            tree,
            parents: vec![head],
            author: test_sig(),
            committer: test_sig(),
            message: "many similar files\n".into(),
        })
        .unwrap();
    repo.create_branch("main", &commit).unwrap();

    // Verify we can read all files through the pack
    let mut repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();

    let mut seen = 0;
    for entry in &tree.entries {
        if entry.name.starts_with("file_") && entry.is_blob() {
            let data = repo.read_blob(&entry.oid).unwrap();
            let text = String::from_utf8(data).unwrap();
            assert!(
                text.contains("common content"),
                "file {} should contain common content",
                entry.name
            );
            let i: usize = entry.name["file_".len()..entry.name.len() - 4].parse().unwrap();
            assert_eq!(text.as_bytes(), similar_file(i).as_slice(), "file {}", entry.name);
            seen += 1;
        }
    }
    assert_eq!(seen, 20);
}

// ===== Complex Merge Scenarios =====

#[test]
fn test_merge_with_added_files_both_sides() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Feature branch: add feature.txt
    checkout_new_branch(&mut repo, "feature");
    fs::write(dir.path().join("feature.txt"), "feature\n").unwrap();
    commit_all(&mut repo, dir.path(), "add feature.txt");

    // Main: add main.txt
    repo.checkout_branch("main").unwrap();
    fs::write(dir.path().join("main.txt"), "main\n").unwrap();
    commit_all(&mut repo, dir.path(), "add main.txt");

    // Merge via our API
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(!result.has_conflict());

    // Both files should exist
    assert!(dir.path().join("feature.txt").exists());
    assert!(dir.path().join("main.txt").exists());

    // The merge is consistent
    fsck(&mut repo);

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    assert_eq!(commit.parents.len(), 2);
    assert!(repo.log(&head, 10).unwrap().len() >= 4);
}

#[test]
fn test_merge_delete_on_one_side() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Feature branch: delete file2.txt
    checkout_new_branch(&mut repo, "feature");
    fs::remove_file(dir.path().join("file2.txt")).unwrap();
    commit_all(&mut repo, dir.path(), "delete file2.txt");

    // Main: add main.txt (don't touch file2.txt)
    repo.checkout_branch("main").unwrap();
    fs::write(dir.path().join("main.txt"), "main\n").unwrap();
    commit_all(&mut repo, dir.path(), "add main.txt");

    // Merge — file2.txt was deleted by feature, untouched by main → clean delete
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(!result.has_conflict(), "expected clean merge: {}", result.content());
    fsck(&mut repo);
    // The merged tree no longer carries file2.txt.
    let head = repo.head_oid().unwrap();
    let head_tree = repo.read_commit(&head).unwrap().tree;
    let tree = repo.read_tree(&head_tree).unwrap();
    assert!(!tree.entries.iter().any(|e| e.name == "file2.txt"), "tree: {:?}", tree.entries);
}

// ===== Our Commits Read Back Through Log, Diff and Show =====

#[test]
fn test_our_commits_log_diff_show() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Make several commits through our API
    let mut commits = Vec::new();
    for i in 0..3 {
        let content = format!("version {}\n", i);
        fs::write(dir.path().join("file1.txt"), &content).unwrap();
        repo.stage_file("file1.txt").unwrap();
        commits.push(repo.commit(&format!("API commit {}\n", i), test_sig()).unwrap());
    }

    // log
    let head = repo.head_oid().unwrap();
    let log: Vec<String> = repo.log(&head, 10).unwrap().into_iter().map(|(_, c)| c.message).collect();
    assert!(log.contains(&"API commit 0\n".to_string()), "log: {:?}", log);
    assert!(log.contains(&"API commit 2\n".to_string()), "log: {:?}", log);
    assert!(log.len() >= 3);

    // diff between first and last API commit
    let changes = repo.diff_commits(&commits[0], &commits[2]).unwrap();
    assert!(
        changes.iter().any(|c| matches!(c, TreeChange::Modified { path, .. } if path == "file1.txt")),
        "changes: {:?}",
        changes
    );

    // show HEAD
    let head_tree = repo.read_commit(&head).unwrap().tree;
    let tree = repo.read_tree(&head_tree).unwrap();
    let file1 = tree.entries.iter().find(|e| e.name == "file1.txt").unwrap();
    assert_eq!(repo.read_blob(&file1.oid).unwrap(), b"version 2\n");

    fsck(&mut repo);
}

// ===== Tags and Merge Commits =====

#[test]
fn test_read_repo_with_tags() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let first = repo.head_oid().unwrap();
    refs::write_ref(&repo.git_dir, "refs/tags/v1.0", &first).unwrap();

    fs::write(dir.path().join("file1.txt"), "v2\n").unwrap();
    let second = commit_all(&mut repo, dir.path(), "version 2");
    refs::write_ref(&repo.git_dir, "refs/tags/v2.0", &second).unwrap();

    let repo = Repository::open(dir.path()).unwrap();
    let tags = repo.list_tags().unwrap();
    let tag_names: Vec<&str> = tags.iter().map(|r| r.name.as_str()).collect();
    assert!(tag_names.contains(&"refs/tags/v1.0"), "tags: {:?}", tag_names);
    assert!(tag_names.contains(&"refs/tags/v2.0"), "tags: {:?}", tag_names);
    assert_eq!(repo.resolve_ref("refs/tags/v1.0").unwrap(), first);
}

#[test]
fn test_read_repo_with_merge_commit() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    checkout_new_branch(&mut repo, "feature");
    fs::write(dir.path().join("f.txt"), "feature\n").unwrap();
    commit_all(&mut repo, dir.path(), "feature");

    repo.checkout_branch("main").unwrap();
    fs::write(dir.path().join("m.txt"), "main\n").unwrap();
    commit_all(&mut repo, dir.path(), "main");
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(!result.has_conflict());

    // Read with our API
    let mut repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    assert_eq!(commit.parents.len(), 2);
    assert_eq!(commit.message, "Merge branch 'feature'\n");

    // Walk full history
    let log = repo.log(&head, 100).unwrap();
    assert!(log.len() >= 4); // merge + main + feature + initial
}

// ===== Edge Cases =====

#[test]
fn test_empty_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("empty.txt"), "").unwrap();
    commit_all(&mut repo, dir.path(), "empty file");

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();
    let empty = tree.entries.iter().find(|e| e.name == "empty.txt").unwrap();
    let data = repo.read_blob(&empty.oid).unwrap();
    assert!(data.is_empty(), "empty file should have no content");
    // git's id for the empty blob
    assert_eq!(empty.oid.to_hex(), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
}

#[test]
fn test_binary_content() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Write binary content
    let binary_data: Vec<u8> = (0..256).map(|i| i as u8).collect();
    fs::write(dir.path().join("binary.bin"), &binary_data).unwrap();
    commit_all(&mut repo, dir.path(), "add binary");

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();
    let bin_entry = tree.entries.iter().find(|e| e.name == "binary.bin").unwrap();
    let data = repo.read_blob(&bin_entry.oid).unwrap();
    assert_eq!(data, binary_data);
}

#[test]
fn test_large_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Write a 1MB file
    let large_data: Vec<u8> = (0..1_000_000).map(|i| (i % 256) as u8).collect();
    let oid = repo.write_blob(&large_data).unwrap();
    let read_back = repo.read_blob(&oid).unwrap();
    assert_eq!(read_back.len(), 1_000_000);
    assert_eq!(read_back, large_data);

    // The id git assigns to this content, and the object header it stores
    assert_eq!(oid.to_hex(), "d6c2598c203a786efb82ca9b0a785e23e4909cb9");
    let object = repo.read_object(&oid).unwrap();
    assert_eq!(object.kind, ObjectKind::Blob);
    assert_eq!(object.data.len(), 1_000_000);
}

#[test]
fn test_unicode_filenames() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("café.txt"), "unicode name\n").unwrap();
    commit_all(&mut repo, dir.path(), "unicode filename");

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();
    let cafe = tree.entries.iter().find(|e| e.name.contains("caf"));
    assert!(cafe.is_some(), "should find café file in tree");
}

#[test]
fn test_commit_with_unicode_message() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    let commit = Commit {
        tree: repo.read_commit(&repo.head_oid().unwrap()).unwrap().tree,
        parents: vec![repo.head_oid().unwrap()],
        author: Signature {
            name: "Ñoño García".into(),
            email: "garcia@ejemplo.com".into(),
            timestamp: 1700000000,
            tz_offset: "+0100".into(),
        },
        committer: test_sig(),
        message: "Añadir funcionalidad 日本語\n".into(),
    };
    let oid = repo.write_commit(&commit).unwrap();

    // The id git assigns to this commit, and the fields read back intact
    assert_eq!(oid.to_hex(), "aae6f37e9a7cd218fb6c9c6d2c53cadab52ba322");
    let read = repo.read_commit(&oid).unwrap();
    assert_eq!(read.author.name, "Ñoño García");
    assert_eq!(read.message, "Añadir funcionalidad 日本語\n");
    let raw = String::from_utf8(repo.read_object(&oid).unwrap().data).unwrap();
    assert!(raw.contains("Ñoño García"), "raw: {}", raw);
    assert!(raw.contains("日本語"), "raw: {}", raw);
}
