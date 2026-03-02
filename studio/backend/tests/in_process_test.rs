use makepad_studio_backend::{BackendConfig, MountConfig, StudioBackend};
use makepad_studio_protocol::backend_protocol::{StudioToUI, UIToStudio};
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

fn drain_messages(
    connection: &makepad_studio_backend::StudioConnection,
    duration: Duration,
) -> Vec<StudioToUI> {
    let deadline = Instant::now() + duration;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        if let Some(msg) = connection.recv_timeout(Duration::from_millis(50)) {
            out.push(msg);
        }
    }
    out
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

    let _run_query_id = connection.send(UIToStudio::Cargo {
        mount: "repo".to_string(),
        args: vec!["--version".to_string()],
        env: None,
        buildbox: None,
    });

    let started = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, StudioToUI::BuildStarted { mount, .. } if mount == "repo"),
    );
    let build_id = match started {
        Some(StudioToUI::BuildStarted { build_id, .. }) => build_id,
        _ => panic!("did not receive BuildStarted"),
    };

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

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_large_paste_keeps_session_alive() {
    const LARGE_PASTE_BYTES: usize = 512 * 1024;
    const MARKER: &[u8] = b"__paste_ok__";
    const MARKER_TAIL_KEEP: usize = 512;

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

    let path = "repo/.makepad/large_paste.term".to_string();
    let _ = connection.send(UIToStudio::TerminalOpen {
        path: path.clone(),
        cols: 120,
        rows: 30,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(4),
        |msg| matches!(msg, StudioToUI::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");

    // Disable echo so the large paste does not flood output, stream a large paste
    // into cat, send Ctrl-D, then run a marker command.
    let mut input = b"stty -echo\ncat > /dev/null\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', LARGE_PASTE_BYTES));
    input.push(b'\n');
    // Interrupt cat so we reliably return to shell even if EOF semantics vary.
    input.push(0x03);
    input.extend_from_slice(b"stty echo\necho __paste_ok__\n");
    let _ = connection.send(UIToStudio::TerminalInput {
        path: path.clone(),
        data: input,
    });

    let mut saw_marker = false;
    let mut tail = Vec::<u8>::new();
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match msg {
            StudioToUI::TerminalExited {
                path: exited_path, ..
            } if exited_path == path => {
                panic!("terminal exited during large paste");
            }
            StudioToUI::Error { message }
                if message.contains("unknown terminal") && message.contains(&path) =>
            {
                panic!("terminal lost after large paste: {}", message);
            }
            StudioToUI::TerminalOutput {
                path: output_path,
                data,
            } if output_path == path => {
                tail.extend_from_slice(&data);
                if tail.windows(MARKER.len()).any(|window| window == MARKER) {
                    saw_marker = true;
                    break;
                }
                if tail.len() > 8192 {
                    let drain_to = tail.len().saturating_sub(MARKER_TAIL_KEEP);
                    if drain_to > 0 {
                        tail.drain(0..drain_to);
                    }
                }
            }
            _ => {}
        }
    }

    let _ = connection.send(UIToStudio::TerminalClose { path });
    assert!(saw_marker, "did not observe terminal response after large paste");
}

#[test]
fn file_tree_keeps_hidden_directories_for_backend() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join(".hidden")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();
    fs::write(dir.path().join(".hidden/secret.txt"), "secret\n").unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(UIToStudio::LoadFileTree {
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
            assert!(
                data.nodes.iter().any(|node| node.path == "repo/.hidden"),
                "hidden directory should be present in backend file tree"
            );
            assert!(
                data.nodes
                    .iter()
                    .any(|node| node.path == "repo/.hidden/secret.txt"),
                "hidden file should be present in backend file tree"
            );
            assert!(
                data.nodes.iter().any(|node| node.path == "repo/src/lib.rs"),
                "expected visible file to remain in file tree"
            );
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
                    makepad_studio_protocol::backend_protocol::FileTreeChange::Added {
                        path,
                        ..
                    }
                    | makepad_studio_protocol::backend_protocol::FileTreeChange::Removed { path }
                    | makepad_studio_protocol::backend_protocol::FileTreeChange::Modified {
                        path,
                        ..
                    } => {
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

#[test]
fn file_watch_emits_single_path_delta_without_full_tree_reload() {
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

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(UIToStudio::SaveTextFile {
        path: "repo/src/lib.rs".to_string(),
        content: "pub fn hi() { let _x = 1; }\n".to_string(),
    });

    let diff_msg = wait_for_message(&connection, Duration::from_secs(5), |msg| {
        matches!(
            msg,
            StudioToUI::FileTreeDiff { mount, changes }
                if mount == "repo"
                    && changes.len() == 1
                    && matches!(
                        &changes[0],
                        makepad_studio_protocol::backend_protocol::FileTreeChange::Added { path, .. }
                            | makepad_studio_protocol::backend_protocol::FileTreeChange::Modified { path, .. }
                            | makepad_studio_protocol::backend_protocol::FileTreeChange::Removed { path }
                            if path.starts_with("repo/")
                    )
        )
    })
    .expect("did not receive path-scoped filetree delta");

    match diff_msg {
        StudioToUI::FileTreeDiff { changes, .. } => {
            assert_eq!(changes.len(), 1);
        }
        _ => unreachable!(),
    }

    let trailing = drain_messages(&connection, Duration::from_millis(500));
    assert!(
        !trailing
            .iter()
            .any(|msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo")),
        "unexpected full file-tree reload after delta update"
    );
}

#[test]
fn save_text_file_does_not_echo_file_changed_to_saving_client() {
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

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(UIToStudio::SaveTextFile {
        path: "repo/src/lib.rs".to_string(),
        content: "pub fn hi() { let _x = 7; }\n".to_string(),
    });

    let messages = drain_messages(&connection, Duration::from_millis(1200));
    assert!(
        messages.iter().any(|msg| {
            matches!(
                msg,
                StudioToUI::TextFileSaved { path, .. } if path == "repo/src/lib.rs"
            )
        }),
        "did not receive TextFileSaved after save request"
    );
    assert!(
        !messages.iter().any(|msg| {
            matches!(
                msg,
                StudioToUI::FileChanged { path } if path == "repo/src/lib.rs"
            )
        }),
        "unexpected FileChanged echo for saving client"
    );
}

#[test]
fn file_watch_ignores_makepad_term_writes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".makepad")).unwrap();
    fs::write(dir.path().join(".makepad/a.term"), "").unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(UIToStudio::SaveTextFile {
        path: "repo/.makepad/a.term".to_string(),
        content: "echo hi\n".to_string(),
    });

    let messages = drain_messages(&connection, Duration::from_millis(900));
    assert!(
        !messages.iter().any(|msg| {
            matches!(msg, StudioToUI::FileTreeDiff { mount, .. } if mount == "repo")
        }),
        "expected .makepad terminal writes to be ignored by fs delta watcher"
    );
}

#[test]
fn file_watch_emits_hidden_directory_writes() {
    let dir = tempfile::tempdir().unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(UIToStudio::SaveTextFile {
        path: "repo/.hidden/a.txt".to_string(),
        content: "hello\n".to_string(),
    });

    let diff = wait_for_message(&connection, Duration::from_secs(5), |msg| {
        matches!(
            msg,
            StudioToUI::FileTreeDiff { mount, changes }
                if mount == "repo"
                    && changes.iter().any(|change| {
                        matches!(
                            change,
                            makepad_studio_protocol::backend_protocol::FileTreeChange::Added { path, .. }
                                | makepad_studio_protocol::backend_protocol::FileTreeChange::Modified { path, .. }
                                | makepad_studio_protocol::backend_protocol::FileTreeChange::Removed { path }
                                if path == "repo/.hidden/a.txt"
                        )
                    })
        )
    });
    assert!(
        diff.is_some(),
        "expected hidden-directory writes to emit deltas"
    );
}

#[test]
fn file_watch_picks_up_external_new_file() {
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

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    fs::write(dir.path().join("src/new_file.rs"), "pub fn new_file() {}\n").unwrap();

    let deadline = Instant::now() + Duration::from_secs(6);
    let mut saw_new_file = false;
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(120)) else {
            continue;
        };
        match msg {
            StudioToUI::FileTreeDiff { mount, changes } if mount == "repo" => {
                if changes.iter().any(|change| {
                    matches!(
                        change,
                        makepad_studio_protocol::backend_protocol::FileTreeChange::Added { path, .. }
                            if path == "repo/src/new_file.rs"
                    )
                }) {
                    saw_new_file = true;
                    break;
                }
            }
            StudioToUI::FileTree { mount, data } if mount == "repo" => {
                if data
                    .nodes
                    .iter()
                    .any(|node| node.path == "repo/src/new_file.rs")
                {
                    saw_new_file = true;
                    break;
                }
            }
            _ => {}
        }
    }

    assert!(
        saw_new_file,
        "did not observe file-tree update for externally created file"
    );
}

#[test]
fn file_watch_emits_file_changed_for_external_write() {
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

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    // External write (not via SaveTextFile) should still notify UI clients.
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn hi() { let _extern = 1; }\n",
    )
    .unwrap();

    let changed = wait_for_message(
        &connection,
        Duration::from_secs(6),
        |msg| {
            matches!(
                msg,
                StudioToUI::FileChanged { path }
                    if path == "repo/src/lib.rs" || path == "repo"
            )
        },
    );
    assert!(
        changed.is_some(),
        "did not observe FileChanged for external write"
    );
}

#[test]
fn read_text_file_returns_fresh_content_after_external_write() {
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

    let _ = connection.send(UIToStudio::OpenTextFile {
        path: "repo/src/lib.rs".to_string(),
    });
    let opened = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            StudioToUI::TextFileOpened { path, content, .. }
                if path == "repo/src/lib.rs" && content == "pub fn hi() {}\n"
        )
    });
    assert!(opened.is_some(), "did not receive initial TextFileOpened");

    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn hi() { let external = 1; }\n",
    )
    .unwrap();

    let _ = connection.send(UIToStudio::ReadTextFile {
        path: "repo/src/lib.rs".to_string(),
    });
    let read = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            StudioToUI::TextFileRead { path, content }
                if path == "repo/src/lib.rs"
                    && content == "pub fn hi() { let external = 1; }\n"
        )
    });
    assert!(
        read.is_some(),
        "did not receive fresh TextFileRead content after external write"
    );
}

#[test]
fn file_watch_picks_up_external_removed_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/nested")).unwrap();
    fs::write(dir.path().join("src/nested/mod.rs"), "pub fn nested() {}\n").unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, StudioToUI::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    fs::remove_dir_all(dir.path().join("src/nested")).unwrap();

    let deadline = Instant::now() + Duration::from_secs(6);
    let mut saw_nested_removed = false;
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(120)) else {
            continue;
        };
        match msg {
            StudioToUI::FileTreeDiff { mount, changes } if mount == "repo" => {
                if changes.iter().any(|change| {
                    matches!(
                        change,
                        makepad_studio_protocol::backend_protocol::FileTreeChange::Removed { path }
                            if path == "repo/src/nested" || path.starts_with("repo/src/nested/")
                    )
                }) {
                    saw_nested_removed = true;
                    break;
                }
            }
            StudioToUI::FileTree { mount, data } if mount == "repo" => {
                if !data.nodes.iter().any(|node| {
                    node.path == "repo/src/nested" || node.path.starts_with("repo/src/nested/")
                }) {
                    saw_nested_removed = true;
                    break;
                }
            }
            _ => {}
        }
    }

    assert!(
        saw_nested_removed,
        "did not observe file-tree update for externally removed directory"
    );
}

#[test]
fn find_in_files_defaults_to_rs_md_toml_and_returns_concise_hits() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn alpha() {\n    let marker = \"needle\";\n}\n",
    )
    .unwrap();
    fs::write(dir.path().join("README.md"), "needle in markdown\n").unwrap();
    fs::write(
        dir.path().join("notes.txt"),
        "needle in txt should be excluded by default\n",
    )
    .unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let query_id = connection.send(UIToStudio::FindInFiles {
        mount: Some("repo".to_string()),
        pattern: "needle".to_string(),
        is_regex: Some(false),
        glob: None,
        max_results: Some(100),
    });

    let msg = wait_for_message(&connection, Duration::from_secs(4), |msg| {
        matches!(
            msg,
            StudioToUI::SearchFileResults { query_id: id, done: true, .. } if *id == query_id
        )
    })
    .expect("did not receive SearchFileResults");

    match msg {
        StudioToUI::SearchFileResults { results, done, .. } => {
            assert!(done);
            assert!(
                results.iter().any(|item| item.path == "repo/src/lib.rs"),
                "expected .rs match"
            );
            assert!(
                results.iter().any(|item| item.path == "repo/README.md"),
                "expected .md match"
            );
            assert!(
                !results.iter().any(|item| item.path == "repo/notes.txt"),
                "unexpected .txt match without glob override"
            );
            assert!(
                results.iter().all(|item| !item.line_text.is_empty()),
                "expected concise line_text in all hits"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn find_in_files_regex_respects_max_results() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn alpha() {\n    let marker = \"needle\";\n    let marker2 = \"needle\";\n}\n",
    )
    .unwrap();
    fs::write(dir.path().join("README.md"), "needle in markdown\n").unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let query_id = connection.send(UIToStudio::FindInFiles {
        mount: Some("repo".to_string()),
        pattern: "needle".to_string(),
        is_regex: Some(true),
        glob: None,
        max_results: Some(1),
    });

    let msg = wait_for_message(&connection, Duration::from_secs(4), |msg| {
        matches!(
            msg,
            StudioToUI::SearchFileResults { query_id: id, done: true, .. } if *id == query_id
        )
    })
    .expect("did not receive SearchFileResults");

    match msg {
        StudioToUI::SearchFileResults { results, done, .. } => {
            assert!(done);
            assert_eq!(results.len(), 1, "regex search should stop at max_results");
        }
        _ => unreachable!(),
    }
}

#[test]
fn read_text_range_returns_line_window_and_total_line_count() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "line-1\nline-2\nline-3\nline-4\n",
    )
    .unwrap();

    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioBackend::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(UIToStudio::ReadTextRange {
        path: "repo/src/lib.rs".to_string(),
        start_line: 2,
        end_line: 3,
    });

    let msg = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            StudioToUI::TextFileRange {
                path,
                start_line,
                end_line,
                total_lines,
                content
            } if path == "repo/src/lib.rs"
                && *start_line == 2
                && *end_line == 3
                && *total_lines == 4
                && content == "line-2\nline-3"
        )
    });

    assert!(msg.is_some(), "did not receive expected TextFileRange");
}
