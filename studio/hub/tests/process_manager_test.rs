use makepad_studio_hub::process_manager::ProcessManager;
use makepad_studio_hub::HubEvent;
use makepad_studio_protocol::hub_protocol::{ClientId, QueryId};
use std::collections::HashMap;
use std::fs;
use std::sync::mpsc;
use std::thread;
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

#[cfg(unix)]
#[test]
fn process_manager_stop_build_kills_process_group() {
    let (tx, rx) = mpsc::channel::<HubEvent>();
    let mut manager = ProcessManager::default();
    let tmp = tempfile::tempdir().unwrap();

    let build_id = QueryId::new(ClientId(7), 2);
    manager
        .start_command_run(
            build_id,
            "repo".to_string(),
            "group-test".to_string(),
            tmp.path(),
            "/bin/sh".to_string(),
            vec![
                "-c".to_string(),
                "sleep 30 & echo $! > child.pid; wait".to_string(),
            ],
            HashMap::new(),
            false,
            None,
            tx,
        )
        .expect("start shell");

    let pid_path = tmp.path().join("child.pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    let child_pid = loop {
        if let Ok(pid) = fs::read_to_string(&pid_path) {
            break pid.trim().parse::<i32>().expect("parse child pid");
        }
        assert!(Instant::now() < deadline, "child pid file was not created");
        thread::sleep(Duration::from_millis(20));
    };

    assert!(pid_exists(child_pid), "background child did not start");

    manager.stop_build(build_id).expect("stop build");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_exit = false;
    while Instant::now() < deadline {
        let Ok(event) = rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        if let HubEvent::ProcessExited {
            build_id: id,
            exit_code,
        } = event
        {
            if id == build_id {
                manager.mark_exited(id, exit_code);
                saw_exit = true;
                break;
            }
        }
    }

    assert!(saw_exit, "did not observe process exit after stop");

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !pid_exists(child_pid) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    if pid_exists(child_pid) {
        kill_pid(child_pid, 9);
    }
    assert!(
        !pid_exists(child_pid),
        "background child survived stop_build"
    );
}

#[cfg(unix)]
fn pid_exists(pid: i32) -> bool {
    unsafe {
        if kill(pid, 0) == 0 {
            return true;
        }
    }
    std::io::Error::last_os_error().raw_os_error() != Some(3)
}

#[cfg(unix)]
fn kill_pid(pid: i32, sig: i32) {
    unsafe {
        let _ = kill(pid, sig);
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
