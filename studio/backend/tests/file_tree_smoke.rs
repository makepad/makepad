use makepad_studio_backend::{BackendConfig, MountConfig, StudioBackend};
use makepad_studio_protocol::backend_protocol::{StudioToUI, UIToStudio};
use std::time::Duration;

#[test]
fn load_file_tree_smoke() {
    let root = std::env::current_dir().expect("current_dir");
    let config = BackendConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: root,
        }],
        enable_in_process_gateway: false,
        ..Default::default()
    };
    let mut conn = StudioBackend::start_in_process(config).expect("start backend");
    let _query_id = conn.send(UIToStudio::LoadFileTree {
        mount: "repo".to_string(),
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let now = std::time::Instant::now();
        assert!(now < deadline, "timed out waiting for FileTree response");
        if let Some(msg) = conn.recv_timeout(Duration::from_millis(150)) {
            match msg {
                StudioToUI::FileTree { mount, data } => {
                    assert_eq!(mount, "repo");
                    assert!(!data.nodes.is_empty(), "expected non-empty file tree");
                    break;
                }
                StudioToUI::Error { message } => {
                    panic!("backend returned error: {}", message);
                }
                _ => {}
            }
        }
    }
}
