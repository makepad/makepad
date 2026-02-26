use makepad_studio_backend::{
    BackendConfig, MountConfig, StudioBackend, StudioToUI, UIToStudio,
};
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
        root: "repo".to_string(),
    });
    let tree = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(msg, StudioToUI::FileTree { root, .. } if root == "repo")
    })
    .expect("did not receive file tree");
    match tree {
        StudioToUI::FileTree { data, .. } => {
            assert!(data.nodes.iter().any(|node| node.path == "repo/src/lib.rs"));
        }
        _ => unreachable!(),
    }

    let build_id = connection.send(UIToStudio::CargoRun {
        root: "repo".to_string(),
        args: vec!["--version".to_string()],
        startup_query: None,
        env: None,
        buildbox: None,
    });

    let started = wait_for_message(&connection, Duration::from_secs(5), |msg| {
        matches!(msg, StudioToUI::BuildStarted { build_id: id, .. } if *id == build_id)
    });
    assert!(started.is_some(), "did not receive BuildStarted");

    let stopped = wait_for_message(&connection, Duration::from_secs(10), |msg| {
        matches!(msg, StudioToUI::BuildStopped { build_id: id, .. } if *id == build_id)
    });
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
