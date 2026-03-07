use makepad_studio_hub::process_manager::ProcessManager;
use makepad_studio_hub::HubEvent;
use makepad_studio_protocol::hub_protocol::{ClientId, QueryId};
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn process_manager_emits_output_and_exit_for_cargo() {
    let (tx, rx) = mpsc::channel::<HubEvent>();
    let mut manager = ProcessManager::default();
    let tmp = tempfile::tempdir().unwrap();

    let build_id = QueryId::new(ClientId(7), 1);
    manager
        .start_cargo_run(
            build_id,
            "repo".to_string(),
            tmp.path(),
            vec!["--version".to_string()],
            HashMap::new(),
            None,
            tx,
        )
        .expect("start cargo");

    assert_eq!(manager.list_builds().len(), 1);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_output = false;
    let mut saw_exit = false;
    while Instant::now() < deadline {
        let Ok(event) = rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match event {
            HubEvent::ProcessOutput {
                build_id: id, line, ..
            } if id == build_id => {
                if line.contains("cargo") {
                    saw_output = true;
                }
            }
            HubEvent::ProcessExited {
                build_id: id,
                exit_code,
            } if id == build_id => {
                assert_eq!(exit_code, Some(0));
                manager.mark_exited(id, exit_code);
                saw_exit = true;
                break;
            }
            _ => {}
        }
    }

    assert!(saw_output, "did not observe cargo output");
    assert!(saw_exit, "did not observe process exit");
    assert!(manager.list_builds().is_empty());
}
