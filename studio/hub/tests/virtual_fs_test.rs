use makepad_studio_hub::VirtualFs;
use makepad_studio_protocol::hub_protocol::GitStatus;
use std::fs;
use std::path::Path;
use std::process::Command;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("git command failed");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn resolves_mount_and_branch_paths() {
    let dir = makepad_studio_hub::test_support::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();
    fs::create_dir_all(dir.path().join("branch/feature-ui/src")).unwrap();
    fs::write(
        dir.path().join("branch/feature-ui/src/lib.rs"),
        "fn branch() {}\n",
    )
    .unwrap();

    let mut vfs = VirtualFs::new();
    vfs.mount("makepad", dir.path()).unwrap();

    let main_root = vfs.resolve_mount("makepad").unwrap();
    assert_eq!(main_root, dir.path().canonicalize().unwrap());

    let branch_root = vfs.resolve_mount("makepad/@feature-ui").unwrap();
    assert_eq!(
        branch_root,
        dir.path().canonicalize().unwrap().join("branch/feature-ui")
    );

    let branch_file = vfs.resolve_path("makepad/@feature-ui/src/lib.rs").unwrap();
    assert!(branch_file.ends_with("branch/feature-ui/src/lib.rs"));

    let tree = vfs.load_file_tree("makepad").unwrap();
    assert!(tree
        .nodes
        .iter()
        .any(|n| n.path == "makepad" && n.name == "makepad"));
    assert!(tree
        .nodes
        .iter()
        .any(|n| n.path == "makepad/@feature-ui" && n.name == "@feature-ui"));
    assert!(tree.nodes.iter().any(|n| n.path == "makepad/src/lib.rs"));
    assert!(tree
        .nodes
        .iter()
        .any(|n| n.path == "makepad/@feature-ui/src/lib.rs"));
}

#[test]
fn git_statuses_are_mapped_for_tree_nodes() {
    if !git_available() {
        return;
    }

    let dir = makepad_studio_hub::test_support::tempdir().unwrap();
    fs::write(dir.path().join("tracked.txt"), "hello\n").unwrap();
    git(dir.path(), &["init"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "initial"]);

    fs::write(dir.path().join("tracked.txt"), "hello modified\n").unwrap();
    fs::write(dir.path().join("new_untracked.txt"), "new\n").unwrap();

    let mut vfs = VirtualFs::new();
    vfs.mount("repo", dir.path()).unwrap();
    let tree = vfs.load_file_tree("repo").unwrap();
    let root = tree
        .nodes
        .iter()
        .find(|n| n.path == "repo")
        .expect("repo root node missing");
    assert_eq!(root.git_status, GitStatus::Modified);

    let tracked = tree
        .nodes
        .iter()
        .find(|n| n.path == "repo/tracked.txt")
        .expect("tracked node missing");
    assert_eq!(tracked.git_status, GitStatus::Modified);

    let untracked = tree
        .nodes
        .iter()
        .find(|n| n.path == "repo/new_untracked.txt")
        .expect("untracked node missing");
    assert_eq!(untracked.git_status, GitStatus::Untracked);
}

#[test]
fn load_file_tree_is_scoped_to_requested_mount() {
    let mount_a = makepad_studio_hub::test_support::tempdir().unwrap();
    let mount_b = makepad_studio_hub::test_support::tempdir().unwrap();
    fs::create_dir_all(mount_a.path().join("src")).unwrap();
    fs::create_dir_all(mount_b.path().join("src")).unwrap();
    fs::write(mount_a.path().join("src/a.rs"), "pub fn a() {}\n").unwrap();
    fs::write(mount_b.path().join("src/b.rs"), "pub fn b() {}\n").unwrap();

    let mut vfs = VirtualFs::new();
    vfs.mount("alpha", mount_a.path()).unwrap();
    vfs.mount("beta", mount_b.path()).unwrap();

    let alpha_tree = vfs.load_file_tree("alpha").unwrap();
    assert!(!alpha_tree.nodes.is_empty());
    assert!(alpha_tree
        .nodes
        .iter()
        .all(|node| node.path.starts_with("alpha")));
    assert!(!alpha_tree
        .nodes
        .iter()
        .any(|node| node.path.starts_with("beta")));
    assert!(alpha_tree
        .nodes
        .iter()
        .any(|node| node.path == "alpha/src/a.rs"));

    let beta_tree = vfs.load_file_tree("beta").unwrap();
    assert!(!beta_tree.nodes.is_empty());
    assert!(beta_tree
        .nodes
        .iter()
        .all(|node| node.path.starts_with("beta")));
    assert!(!beta_tree
        .nodes
        .iter()
        .any(|node| node.path.starts_with("alpha")));
    assert!(beta_tree
        .nodes
        .iter()
        .any(|node| node.path == "beta/src/b.rs"));
}
