use makepad_studio_backend::{BackendConfig, MountConfig, StudioBackend, StudioToUI, UIToStudio};
use std::fs;
use std::time::{Duration, Instant};

fn wait_for_message<F>(
    connection: &makepad_studio_backend::StudioConnection,
    timeout: Duration,
    mut matcher: F,
) -> Option<StudioToUI>
where
    F: FnMut(&StudioToUI) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) {
            if matcher(&msg) {
                return Some(msg);
            }
        }
    }
    None
}

#[test]
fn in_process_connection_roundtrip_and_cargo_build_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _tree_query = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let tree = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive file tree");
    match tree {
        StudioToUI::FileTree { data, .. } => {
            assert!(data.nodes.iter().any(|node| node.path == "repo/src/lib.rs"));
        }
        _ => unreachable!(),
    }

    let build_id = connection.send(UIToStudio::CargoRun {
        mount: "repo".to_string(),
        args: vec!["--version".to_string()],
        startup_query: None,
        env: None,
        buildbox: None,
    });

    let started = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, StudioToUI::BuildStarted { build_id: id, .. } if *id == build_id),
    );
    assert!(started.is_some(), "did not receive BuildStarted");

    let stopped = wait_for_message(
        &connection,
        Duration::from_secs(10),
        |msg| matches!(msg, StudioToUI::BuildStopped { build_id: id, .. } if *id == build_id),
    );
    assert!(stopped.is_some(), "did not receive BuildStopped");

    let query_id = connection.send(UIToStudio::QueryLogs {
        build_id: Some(build_id),
        level: None,
        source: None,
        file: None,
        pattern: None,
        is_regex: None,
        since_index: None,
        live: Some(false),
    });
    let log_results = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            StudioToUI::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive QueryLogResults");

    match log_results {
        StudioToUI::QueryLogResults { entries, done, .. } => {
            assert!(done);
            assert!(!entries.is_empty(), "expected cargo output entries");
        }
        _ => unreachable!(),
    }
}

#[test]
fn unmount_emits_file_tree_diff_scoped_to_mount() {
    let mount_a = tempfile::tempdir().unwrap();
    let mount_b = tempfile::tempdir().unwrap();
    fs::create_dir_all(mount_a.path().join("src")).unwrap();
    fs::create_dir_all(mount_b.path().join("src")).unwrap();
    fs::write(mount_a.path().join("src/a.rs"), "pub fn a() {}\n").unwrap();
    fs::write(mount_b.path().join("src/b.rs"), "pub fn b() {}\n").unwrap();

    let config = BackendConfig {
        mounts: vec![
            MountConfig {
                name: "alpha".to_string(),
                path: mount_a.path().to_path_buf(),
            },
            MountConfig {
                name: "beta".to_string(),
                path: mount_b.path().to_path_buf(),
            },
        ],
        ..Default::default()
    };

    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");
    let _ = connection.send(UIToStudio::Unmount {
        name: "alpha".to_string(),
    });

    let diff = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTreeDiff { mount, .. } if mount == "alpha"),
    )
    .expect("did not receive alpha FileTreeDiff");

    match diff {
        StudioToUI::FileTreeDiff { mount, changes } => {
            assert_eq!(mount, "alpha");
            assert!(!changes.is_empty(), "expected removed paths for alpha");
            for change in changes {
                match change {
                    makepad_studio_backend::FileTreeChange::Added { path, .. }
                    | makepad_studio_backend::FileTreeChange::Removed { path }
                    | makepad_studio_backend::FileTreeChange::Modified { path, .. } => {
                        assert!(path == "alpha" || path.starts_with("alpha/"));
                        assert!(!path.starts_with("beta/"));
                    }
                }
            }
        }
        _ => unreachable!(),
    }
}

#[test]
fn runnable_builds_are_scoped_per_mount() {
    let mount_a = tempfile::tempdir().unwrap();
    let mount_b = tempfile::tempdir().unwrap();

    fs::create_dir_all(mount_a.path().join("src")).unwrap();
    fs::write(
        mount_a.path().join("Cargo.toml"),
        "[package]\nname = \"alpha-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(mount_a.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    fs::create_dir_all(mount_b.path().join("src")).unwrap();
    fs::write(
        mount_b.path().join("Cargo.toml"),
        "[package]\nname = \"beta-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(mount_b.path().join("src/main.rs"), "fn main() {}\n").unwrap();

    let config = BackendConfig {
        mounts: vec![
            MountConfig {
                name: "alpha".to_string(),
                path: mount_a.path().to_path_buf(),
            },
            MountConfig {
                name: "beta".to_string(),
                path: mount_b.path().to_path_buf(),
            },
        ],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(UIToStudio::LoadRunnableBuilds {
        mount: "alpha".to_string(),
    });
    let alpha = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, StudioToUI::RunnableBuilds { mount, .. } if mount == "alpha"),
    )
    .expect("did not receive alpha runnable builds");
    match alpha {
        StudioToUI::RunnableBuilds { mount, builds } => {
            assert_eq!(mount, "alpha");
            assert_eq!(builds.len(), 1);
            assert_eq!(builds[0].package, "alpha-app");
        }
        _ => unreachable!(),
    }

    let _ = connection.send(UIToStudio::LoadRunnableBuilds {
        mount: "beta".to_string(),
    });
    let beta = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, StudioToUI::RunnableBuilds { mount, .. } if mount == "beta"),
    )
    .expect("did not receive beta runnable builds");
    match beta {
        StudioToUI::RunnableBuilds { mount, builds } => {
            assert_eq!(mount, "beta");
            assert_eq!(builds.len(), 1);
            assert_eq!(builds[0].package, "beta-app");
        }
        _ => unreachable!(),
    }
}
