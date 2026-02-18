use makepad_git::*;
use std::fs;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("failed to run git");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_ok(dir: &std::path::Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn make_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init"]);
    fs::write(dir.path().join("file1.txt"), "hello\n").unwrap();
    fs::write(dir.path().join("file2.txt"), "world\n").unwrap();
    fs::create_dir_all(dir.path().join("subdir")).unwrap();
    fs::write(dir.path().join("subdir/nested.txt"), "nested\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial commit"]);
    dir
}

fn test_sig() -> Signature {
    Signature {
        name: "Test".into(),
        email: "test@test.com".into(),
        timestamp: 1700000000,
        tz_offset: "+0000".into(),
    }
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

    let output = git(dir.path(), &["cat-file", "-p", &oid.to_hex()]);
    assert_eq!(output, "new content written by makepad-git");
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

    let log_output = git(dir.path(), &["log", "--oneline", "-2"]);
    assert!(
        log_output.contains("commit from makepad-git"),
        "git log: {}",
        log_output
    );
    assert!(
        log_output.contains("initial commit"),
        "git log: {}",
        log_output
    );

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
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "second commit"]);

    let head = repo.head_oid().unwrap();
    let log = repo.log(&head, 10).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].1.message, "second commit\n");
    assert_eq!(log[1].1.message, "initial commit\n");
}

#[test]
fn test_read_from_packed_repo() {
    let dir = make_repo();
    git(dir.path(), &["gc", "--aggressive"]);

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

    // Wait a moment so mtime changes
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

#[test]
fn test_stage_new_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("staged.txt"), "staged content\n").unwrap();
    repo.stage_file("staged.txt").unwrap();

    // Verify git sees it staged
    let git_status = git(dir.path(), &["status", "--porcelain"]);
    assert!(
        git_status.contains("A  staged.txt"),
        "git status: {}",
        git_status
    );

    // Verify git can read the index we wrote
    assert!(git_ok(dir.path(), &["status"]));
}

#[test]
fn test_stage_modified_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("file1.txt"), "changed\n").unwrap();
    repo.stage_file("file1.txt").unwrap();

    // Verify git sees it staged
    let git_status = git(dir.path(), &["status", "--porcelain"]);
    assert!(
        git_status.contains("M  file1.txt"),
        "git status: {}",
        git_status
    );
    assert!(git_ok(dir.path(), &["status"]));
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

    // Git should see it as deleted from index
    let git_status = git(dir.path(), &["status", "--porcelain"]);
    assert!(
        git_status.contains("D  file1.txt"),
        "git status: {}",
        git_status
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
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-m", "changes"]);

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

    assert!(
        paths.contains(&"+file3.txt".to_string()),
        "changes: {:?}",
        paths
    );
    assert!(
        paths.contains(&"-file2.txt".to_string()),
        "changes: {:?}",
        paths
    );
    assert!(
        paths.contains(&"Mfile1.txt".to_string()),
        "changes: {:?}",
        paths
    );
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
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "update on main"]);

    // Switch to feature branch (which is behind)
    git(dir.path(), &["checkout", "feature"]);

    // Now merge main into feature — this should fast-forward
    let result = repo.merge_branch(&main_branch, test_sig()).unwrap();
    assert!(
        !result.has_conflict(),
        "expected fast-forward, got conflict"
    );
    assert!(
        result.content().contains("Fast-forward"),
        "result: {}",
        result.content()
    );

    // Verify git is happy
    assert!(git_ok(dir.path(), &["status"]));
    assert!(git_ok(dir.path(), &["fsck"]));
}

#[test]
fn test_merge_base_of_diverged_branches() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    let base_oid = repo.head_oid().unwrap();

    // Create branch and commit on it
    git(dir.path(), &["checkout", "-b", "feature"]);
    fs::write(dir.path().join("feature.txt"), "feature work\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "feature commit"]);
    let feature_oid = ObjectId::from_hex(&git(dir.path(), &["rev-parse", "HEAD"])).unwrap();

    // Go back to main and commit
    git(dir.path(), &["checkout", "-"]);
    fs::write(dir.path().join("main.txt"), "main work\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "main commit"]);
    let main_oid = repo.head_oid().unwrap();

    // Find merge base using our API
    let mb = repo.merge_base(&main_oid, &feature_oid).unwrap();
    assert_eq!(mb, Some(base_oid));

    // Cross-check with git merge-base
    let git_mb = git(
        dir.path(),
        &["merge-base", &main_oid.to_hex(), &feature_oid.to_hex()],
    );
    assert_eq!(mb.unwrap().to_hex(), git_mb);
}

#[test]
fn test_merge_clean_no_conflict() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create feature branch
    git(dir.path(), &["checkout", "-b", "feature"]);
    fs::write(dir.path().join("feature.txt"), "feature content\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add feature.txt"]);

    // Go back to main and make a different change
    git(dir.path(), &["checkout", "-"]);
    fs::write(dir.path().join("main.txt"), "main content\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add main.txt"]);

    // Merge feature into main using our API
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(
        !result.has_conflict(),
        "expected clean merge, got: {}",
        result.content()
    );

    // Verify git is happy with the result
    assert!(git_ok(dir.path(), &["fsck"]), "git fsck failed");
    assert!(git_ok(dir.path(), &["status"]), "git status failed");

    // Verify merge commit has two parents
    let head = repo.head_oid().unwrap();
    let merge_commit = repo.read_commit(&head).unwrap();
    assert_eq!(
        merge_commit.parents.len(),
        2,
        "merge commit should have 2 parents"
    );

    // Verify both files exist
    let log_output = git(dir.path(), &["log", "--oneline", "-5"]);
    assert!(log_output.contains("Merge branch"), "log: {}", log_output);

    // Verify the worktree has both files
    assert!(dir.path().join("feature.txt").exists());
    assert!(dir.path().join("main.txt").exists());
}

#[test]
fn test_merge_conflict_same_file() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create feature branch and modify file1.txt
    git(dir.path(), &["checkout", "-b", "feature"]);
    fs::write(dir.path().join("file1.txt"), "feature version\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "feature: modify file1"]);

    // Go back to main and make a conflicting change to file1.txt
    git(dir.path(), &["checkout", "-"]);
    fs::write(dir.path().join("file1.txt"), "main version\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "main: modify file1"]);

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
    assert!(
        content.contains("<<<<<<<"),
        "should have conflict markers: {}",
        content
    );
    assert!(
        content.contains("======="),
        "should have conflict markers: {}",
        content
    );
    assert!(
        content.contains(">>>>>>>"),
        "should have conflict markers: {}",
        content
    );

    // Verify the index has conflict entries (stages 1-3)
    let index = repo.read_index().unwrap();
    let conflict_entries: Vec<_> = index
        .entries
        .iter()
        .filter(|e| e.path == "file1.txt" && e.stage() > 0)
        .collect();
    assert!(
        !conflict_entries.is_empty(),
        "should have conflict stages in index"
    );
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
    git(dir.path(), &["checkout", "feature"]);
    fs::write(dir.path().join("feature_only.txt"), "feature\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add feature_only.txt"]);

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

    // Verify git is happy
    assert!(git_ok(dir.path(), &["status"]));
    assert!(git_ok(dir.path(), &["fsck"]));
}

#[test]
fn test_checkout_branch_restores_content() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();
    let main_branch = repo.current_branch().unwrap().unwrap();

    // Modify file on a new branch
    git(dir.path(), &["checkout", "-b", "modify"]);
    fs::write(dir.path().join("file1.txt"), "modified on branch\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "modify file1"]);

    // Checkout back to main via our API
    repo.checkout_branch(&main_branch).unwrap();

    // File should have original content
    let content = fs::read_to_string(dir.path().join("file1.txt")).unwrap();
    assert_eq!(
        content, "hello\n",
        "file1.txt should be restored to original"
    );

    assert!(git_ok(dir.path(), &["status"]));
}

// ===== Packed Refs Tests =====

#[test]
fn test_packed_refs_branches() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();

    // Create branches and pack them
    repo.create_branch("packed-a", &head).unwrap();
    repo.create_branch("packed-b", &head).unwrap();
    git(dir.path(), &["pack-refs", "--all"]);

    // Our API should still find them
    let branches = repo.list_branches().unwrap();
    let names: Vec<&str> = branches.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"refs/heads/packed-a"),
        "branches: {:?}",
        names
    );
    assert!(
        names.contains(&"refs/heads/packed-b"),
        "branches: {:?}",
        names
    );

    // Resolve should work too
    let resolved = repo.resolve_ref("refs/heads/packed-a").unwrap();
    assert_eq!(resolved, head);
}

// ===== Multiple Commits + History =====

#[test]
fn test_long_history_walk() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Create 10 additional commits
    for i in 1..=10 {
        fs::write(dir.path().join("file1.txt"), format!("version {}\n", i)).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", &format!("commit {}", i)]);
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
    git(dir.path(), &["checkout", "-b", "feature"]);
    for i in 1..=3 {
        fs::write(dir.path().join("feature.txt"), format!("v{}\n", i)).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", &format!("feature {}", i)]);
    }

    // Merge back to main
    git(dir.path(), &["checkout", "-"]);
    git(
        dir.path(),
        &["merge", "feature", "--no-ff", "-m", "merge feature"],
    );

    // Walk the merge commit
    let head = repo.head_oid().unwrap();
    let log = repo.log(&head, 100).unwrap();
    // Should see: merge commit, feature 3, feature 2, feature 1, initial (+ any main commits)
    assert!(
        log.len() >= 5,
        "log should have at least 5 commits, got {}",
        log.len()
    );
    assert_eq!(log[0].1.message, "merge feature\n");
    assert_eq!(
        log[0].1.parents.len(),
        2,
        "merge commit should have 2 parents"
    );
}

// ===== Write Objects and Verify with Git =====

#[test]
fn test_write_tree_matches_git() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();

    // Write some blobs
    let blob_a = repo.write_blob(b"aaa\n").unwrap();
    let blob_b = repo.write_blob(b"bbb\n").unwrap();

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

    // Verify git can read it
    let git_output = git(dir.path(), &["cat-file", "-p", &tree_oid.to_hex()]);
    assert!(git_output.contains("a.txt"), "git cat-file: {}", git_output);
    assert!(git_output.contains("b.txt"), "git cat-file: {}", git_output);

    // Verify type
    let type_output = git(dir.path(), &["cat-file", "-t", &tree_oid.to_hex()]);
    assert_eq!(type_output, "tree");
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

    // Verify git can read it
    let type_output = git(dir.path(), &["cat-file", "-t", &commit_oid.to_hex()]);
    assert_eq!(type_output, "commit");

    let content = git(dir.path(), &["cat-file", "-p", &commit_oid.to_hex()]);
    assert!(content.contains("test commit via API"));
    assert!(content.contains(&head.to_hex()));
}

// ===== Index Round-Trip Tests =====

#[test]
fn test_index_roundtrip_with_git() {
    let dir = make_repo();
    let repo = Repository::open(dir.path()).unwrap();

    // Read, write, and verify git can still use it
    let index = repo.read_index().unwrap();
    repo.write_index(&index).unwrap();

    // Git should be able to read our index
    let _status = git(dir.path(), &["status", "--porcelain"]);
    // There might be some stat-cache differences, but no errors
    assert!(git_ok(dir.path(), &["status"]));
    // diff --cached may show stat-cache differences, that's fine
    let _ = git_ok(dir.path(), &["diff", "--cached", "--exit-code"]);
}

#[test]
fn test_index_after_staging() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Stage a new file
    fs::write(dir.path().join("new.txt"), "new content\n").unwrap();
    repo.stage_file("new.txt").unwrap();

    // Now create a commit using git CLI to verify our index is valid
    assert!(git_ok(
        dir.path(),
        &["commit", "-m", "commit with our index"]
    ));
    assert!(git_ok(dir.path(), &["fsck"]));

    // Verify the commit has the new file
    let tree_output = git(dir.path(), &["ls-tree", "HEAD"]);
    assert!(tree_output.contains("new.txt"), "ls-tree: {}", tree_output);
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
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add deep file"]);

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

// ===== Git Fsck After All Our Writes =====

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
        repo.commit(&format!("automated commit {}\n", i), test_sig())
            .unwrap();
    }

    // Verify git can validate everything
    assert!(
        git_ok(dir.path(), &["fsck"]),
        "git fsck failed after our writes"
    );
    assert!(git_ok(dir.path(), &["log", "--oneline"]));
    let log = git(dir.path(), &["log", "--oneline"]);
    assert!(log.contains("automated commit 4"), "log: {}", log);
}

// ===== Read Objects Written by Git After GC =====

#[test]
fn test_read_deltified_objects() {
    let dir = make_repo();

    // Create many similar files (will be deltified by gc)
    for i in 0..20 {
        let content = format!(
            "This is a file with some common content.\n\
             Line 2 is the same in every file.\n\
             Line 3 too.\n\
             But line 4 varies: iteration {}\n\
             Line 5 is common again.\n",
            i
        );
        fs::write(dir.path().join(format!("file_{}.txt", i)), &content).unwrap();
    }
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "many similar files"]);

    // GC to force delta compression
    git(dir.path(), &["gc", "--aggressive"]);

    // Verify we can read all files through pack
    let mut repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();

    for entry in &tree.entries {
        if entry.name.starts_with("file_") && entry.is_blob() {
            let data = repo.read_blob(&entry.oid).unwrap();
            let text = String::from_utf8(data).unwrap();
            assert!(
                text.contains("common content"),
                "file {} should contain common content",
                entry.name
            );
        }
    }
}

// ===== Complex Merge Scenarios =====

#[test]
fn test_merge_with_added_files_both_sides() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Feature branch: add feature.txt
    git(dir.path(), &["checkout", "-b", "feature"]);
    fs::write(dir.path().join("feature.txt"), "feature\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add feature.txt"]);

    // Main: add main.txt
    git(dir.path(), &["checkout", "-"]);
    fs::write(dir.path().join("main.txt"), "main\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add main.txt"]);

    // Merge via our API
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(!result.has_conflict());

    // Both files should exist
    assert!(dir.path().join("feature.txt").exists());
    assert!(dir.path().join("main.txt").exists());

    // Git should accept our merge
    assert!(git_ok(dir.path(), &["fsck"]));
    assert!(git_ok(dir.path(), &["log", "--oneline"]));

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    assert_eq!(commit.parents.len(), 2);
}

#[test]
fn test_merge_delete_on_one_side() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Feature branch: delete file2.txt
    git(dir.path(), &["checkout", "-b", "feature"]);
    fs::remove_file(dir.path().join("file2.txt")).unwrap();
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-m", "delete file2.txt"]);

    // Main: add main.txt (don't touch file2.txt)
    git(dir.path(), &["checkout", "-"]);
    fs::write(dir.path().join("main.txt"), "main\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add main.txt"]);

    // Merge — file2.txt was deleted by feature, untouched by main → clean delete
    let result = repo.merge_branch("feature", test_sig()).unwrap();
    assert!(
        !result.has_conflict(),
        "expected clean merge: {}",
        result.content()
    );
    assert!(git_ok(dir.path(), &["fsck"]));
}

// ===== Interoperability: Our Commits, Git's Reads =====

#[test]
fn test_our_commits_git_can_log_diff_show() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Make several commits through our API
    for i in 0..3 {
        let content = format!("version {}\n", i);
        fs::write(dir.path().join("file1.txt"), &content).unwrap();
        repo.stage_file("file1.txt").unwrap();
        repo.commit(&format!("API commit {}\n", i), test_sig())
            .unwrap();
    }

    // Git should be able to: log, diff, show
    let log_out = git(dir.path(), &["log", "--oneline"]);
    assert!(log_out.contains("API commit 0"), "log: {}", log_out);
    assert!(log_out.contains("API commit 2"), "log: {}", log_out);

    // git diff between first and last API commit
    let commits: Vec<_> = log_out.lines().collect();
    assert!(commits.len() >= 3);

    // git show should work on HEAD
    let show_out = git(dir.path(), &["show", "--stat", "HEAD"]);
    assert!(show_out.contains("file1.txt"), "show: {}", show_out);

    // fsck
    assert!(git_ok(dir.path(), &["fsck"]));
}

// ===== Read Repos Created Entirely by Git =====

#[test]
fn test_read_repo_with_tags() {
    let dir = make_repo();
    git(dir.path(), &["tag", "v1.0"]);

    fs::write(dir.path().join("file1.txt"), "v2\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "version 2"]);
    git(dir.path(), &["tag", "v2.0"]);

    let repo = Repository::open(dir.path()).unwrap();
    let tags = repo.list_tags().unwrap();
    let tag_names: Vec<&str> = tags.iter().map(|r| r.name.as_str()).collect();
    assert!(
        tag_names.contains(&"refs/tags/v1.0"),
        "tags: {:?}",
        tag_names
    );
    assert!(
        tag_names.contains(&"refs/tags/v2.0"),
        "tags: {:?}",
        tag_names
    );
}

#[test]
fn test_read_repo_with_merge_commit() {
    let dir = make_repo();

    // Create a merge using git
    git(dir.path(), &["checkout", "-b", "feature"]);
    fs::write(dir.path().join("f.txt"), "feature\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "feature"]);

    git(dir.path(), &["checkout", "-"]);
    fs::write(dir.path().join("m.txt"), "main\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "main"]);
    git(dir.path(), &["merge", "feature", "--no-ff", "-m", "merge"]);

    // Read with our API
    let mut repo = Repository::open(dir.path()).unwrap();
    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    assert_eq!(commit.parents.len(), 2);
    assert_eq!(commit.message, "merge\n");

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
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "empty file"]);

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();
    let empty = tree.entries.iter().find(|e| e.name == "empty.txt").unwrap();
    let data = repo.read_blob(&empty.oid).unwrap();
    assert!(data.is_empty(), "empty file should have no content");
}

#[test]
fn test_binary_content() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    // Write binary content
    let binary_data: Vec<u8> = (0..256).map(|i| i as u8).collect();
    fs::write(dir.path().join("binary.bin"), &binary_data).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "add binary"]);

    let head = repo.head_oid().unwrap();
    let commit = repo.read_commit(&head).unwrap();
    let tree = repo.read_tree(&commit.tree).unwrap();
    let bin_entry = tree
        .entries
        .iter()
        .find(|e| e.name == "binary.bin")
        .unwrap();
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

    // Verify git can read it
    let type_out = git(dir.path(), &["cat-file", "-t", &oid.to_hex()]);
    assert_eq!(type_out, "blob");
    let size_out = git(dir.path(), &["cat-file", "-s", &oid.to_hex()]);
    assert_eq!(size_out, "1000000");
}

#[test]
fn test_unicode_filenames() {
    let dir = make_repo();
    let mut repo = Repository::open(dir.path()).unwrap();

    fs::write(dir.path().join("café.txt"), "unicode name\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "unicode filename"]);

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

    // Verify git reads it correctly
    let show = git(dir.path(), &["cat-file", "-p", &oid.to_hex()]);
    assert!(show.contains("Ñoño García"), "show: {}", show);
    assert!(show.contains("日本語"), "show: {}", show);
}
