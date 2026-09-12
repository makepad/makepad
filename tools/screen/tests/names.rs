#![cfg(unix)]
use makepad_screen::{protocol::SessionLocation, server, terminal::HostedTerminal};
use makepad_strict_json::{self as json, Value};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

struct Session {
    root: PathBuf,
    state: PathBuf,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = server::stop(&self.state, "name-test");
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn send(stream: &mut UnixStream, kind: u8, payload: &[u8]) {
    let mut frame = vec![kind];
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    stream.write_all(&frame).unwrap();
}
fn observe_title(stream: &mut UnixStream, terminal: &mut HostedTerminal, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while terminal.title() != expected {
        assert!(
            Instant::now() < deadline,
            "attached client did not receive {expected}"
        );
        let mut header = [0; 5];
        stream.read_exact(&mut header).unwrap();
        let length = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
        assert!(length <= 1024 * 1024);
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).unwrap();
        assert_ne!(header[0], 5, "{}", String::from_utf8_lossy(&bytes));
        if matches!(header[0], 2 | 3) {
            terminal.process(&bytes);
        }
    }
}
#[test]
fn osc_and_current_session_cli_names_reach_list_and_attached_client() {
    let root = std::env::temp_dir().join(format!(
        "screen-names-{}-{}",
        std::process::id(),
        makepad_screen::protocol::random_token().unwrap()
    ));
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let session = Session {
        state: root.join("sessions"),
        root,
    };
    let binary = env!("CARGO_BIN_EXE_makepad-screen");
    let output = Command::new(binary)
        .args(["start", "--state-dir"])
        .arg(&session.state)
        .args(["--session", "name-test", "--cwd"])
        .arg(&session.root)
        .args(["--", "/bin/sh"])
        .env("SCREEN_TEST_BINARY", binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}; host log: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(session.state.join("name-test.log")).unwrap_or_default()
    );
    let location = SessionLocation::open(&session.state, "name-test", false).unwrap();
    let mut stream = UnixStream::connect(&location.socket_path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    send(
        &mut stream,
        1,
        br#"{"version":1,"session_id":"name-test","cols":80,"rows":24,"read_only":false}"#,
    );
    let mut attached = HostedTerminal::new(80, 24, 100);
    for (command, expected) in [
        ("printf '\\033]0;osc-zero\\007'\n", "osc-zero"),
        ("printf '\\033]2;osc-two\\033\\\\'\n", "osc-two"),
        ("\"$SCREEN_TEST_BINARY\" name cli-name\n", "cli-name"),
        (
            "\"$SCREEN_TEST_BINARY\" agents name wrapper-name\n",
            "wrapper-name",
        ),
    ] {
        send(&mut stream, 6, command.as_bytes());
        observe_title(&mut stream, &mut attached, expected);
        let output = Command::new(binary)
            .args(["list", "--state-dir"])
            .arg(&session.state)
            .output()
            .unwrap();
        assert!(output.status.success());
        let inventory = json::parse(&output.stdout).unwrap();
        let row = &inventory.as_arr().unwrap()[0];
        assert_eq!(row.get("title").and_then(Value::as_str), Some(expected));
        assert_eq!(
            row.get("session_id").and_then(Value::as_str),
            Some("name-test")
        );
        assert_eq!(
            row.get("cwd").and_then(Value::as_str),
            session.root.to_str()
        );
        assert!(row.get("started_at_ms").and_then(Value::as_u64).unwrap() > 0);
        let stored = json::parse(&fs::read(&location.metadata_path).unwrap()).unwrap();
        assert_eq!(stored.get("title").and_then(Value::as_str), Some(expected));
    }
    drop(stream);
    let stopped = server::stop(&session.state, "name-test").unwrap();
    assert_eq!(
        stopped.get("title").and_then(Value::as_str),
        Some("wrapper-name")
    );
    assert_eq!(stopped.get("running").and_then(Value::as_bool), Some(false));
}

#[test]
fn shell_started_session_is_listed_with_osc_title_in_workspace_scope() {
    let root = std::env::temp_dir().join(format!(
        "screen-inventory-{}-{}",
        std::process::id(),
        makepad_screen::protocol::random_token().unwrap()
    ));
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let session = Session {
        state: root.join("local/agent_state/studio/iteration-verification/state/agent_sessions"),
        root,
    };
    makepad_screen::protocol::private_directory(&session.state, true).unwrap();
    let binary = env!("CARGO_BIN_EXE_makepad-screen");
    // A shell starts the session before any Studio/client attaches. Omit cwd
    // and state flags, as in tools/agents start --session NAME -- COMMAND.
    let output = Command::new(binary)
        .current_dir(&session.root)
        .env_remove("MAKEPAD_SCREEN_STATE_DIR")
        .env_remove("MAKEPAD_SCREEN_SESSION")
        .args([
            "start",
            "--session",
            "name-test",
            "--",
            "/bin/sh",
            "-c",
            "printf '\\033]0;hello-agent\\007'; exec sleep 600",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}; host log: {}",
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(session.state.join("name-test.log")).unwrap_or_default()
    );
    for scope in ["workspace", "explicit", "inherited"] {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut list = Command::new(binary);
            list.arg("list")
                .current_dir(&session.root)
                .env_remove("MAKEPAD_SCREEN_STATE_DIR");
            match scope {
                "explicit" => {
                    // Explicit scope wins over an unrelated inherited scope.
                    list.arg("--state-dir")
                        .arg(&session.state)
                        .env("MAKEPAD_SCREEN_STATE_DIR", session.root.join("empty"));
                }
                "inherited" => {
                    // Outside the workspace, the inherited scope still wins.
                    list.current_dir(&session.state)
                        .env("MAKEPAD_SCREEN_STATE_DIR", &session.state);
                }
                _ => {}
            }
            let output = list.output().unwrap();
            assert!(
                output.status.success(),
                "{scope}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let inventory = json::parse(&output.stdout).unwrap();
            let rows = inventory.as_arr().unwrap();
            assert_eq!(rows.len(), 1, "{scope}: {inventory:?}");
            let row = &rows[0];
            assert_eq!(
                row.get("session_id").and_then(Value::as_str),
                Some("name-test")
            );
            assert_eq!(
                row.get("state_dir").and_then(Value::as_str),
                session.state.to_str()
            );
            assert_eq!(
                row.get("cwd").and_then(Value::as_str),
                session.root.to_str()
            );
            assert_eq!(row.get("running").and_then(Value::as_bool), Some(true));
            assert_eq!(row.get("clients").and_then(Value::as_u64), Some(0));
            if row.get("title").and_then(Value::as_str) == Some("hello-agent") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{scope}: OSC title missing: {row:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let stopped = server::stop(&session.state, "name-test").unwrap();
    assert_eq!(stopped.get("running").and_then(Value::as_bool), Some(false));
    assert!(server::list(&session.state)
        .unwrap()
        .as_arr()
        .unwrap()
        .is_empty());
}

#[test]
fn title_changes_project_to_existing_and_new_clients() {
    use makepad_screen::snapshot::Projection;
    let mut source = HostedTerminal::new(80, 24, 100);
    let mut client = HostedTerminal::new(80, 24, 100);
    let mut projection = Projection::default();
    for (sequence, title) in [
        (b"\x1b]0;zero\x07".as_slice(), "zero"),
        (b"\x1b]2;two\x1b\\".as_slice(), "two"),
    ] {
        source.process(sequence);
        client.process(&source.render(&mut projection, 80, 24));
        assert_eq!(client.title(), title);
    }
    source.set_title("same name");
    client.process(&source.render(&mut projection, 80, 24));
    assert_eq!(client.title(), "same name");
    let mut new_client = HostedTerminal::new(100, 30, 100);
    new_client.process(&source.render(&mut Projection::default(), 100, 30));
    assert_eq!(new_client.title(), "same name");
}
