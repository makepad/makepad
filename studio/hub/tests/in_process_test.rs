use makepad_studio_hub::{HubConfig, MountConfig, StudioHub};
use makepad_studio_protocol::hub_protocol::{ClientToHub, HubToClient, TerminalFramebuffer};
use std::fs;
use std::time::{Duration, Instant};

fn wait_for_message<F>(
    connection: &makepad_studio_hub::HubConnection,
    timeout: Duration,
    mut matcher: F,
) -> Option<HubToClient>
where
    F: FnMut(&HubToClient) -> bool,
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
    connection: &makepad_studio_hub::HubConnection,
    duration: Duration,
) -> Vec<HubToClient> {
    let deadline = Instant::now() + duration;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        if let Some(msg) = connection.recv_timeout(Duration::from_millis(50)) {
            out.push(msg);
        }
    }
    out
}

fn request_terminal_viewport(
    connection: &mut makepad_studio_hub::HubConnection,
    path: &str,
    cols: u16,
    rows: u16,
    top_row: usize,
) {
    let _ = connection.send(ClientToHub::TerminalViewportRequest {
        path: path.to_string(),
        cols,
        rows,
        pty_rows: rows,
        top_row,
    });
}

fn framebuffer_to_text(frame: &TerminalFramebuffer) -> String {
    let cols = frame.cols as usize;
    let rows = frame.rows as usize;
    let mut out = String::with_capacity(cols * rows + rows.saturating_sub(1));
    for row in 0..rows {
        for col in 0..cols {
            let idx = (row * cols + col) * 7;
            if idx + 6 >= frame.cells.len() {
                out.push(' ');
                continue;
            }
            let codepoint = u32::from_le_bytes([
                frame.cells[idx],
                frame.cells[idx + 1],
                frame.cells[idx + 2],
                frame.cells[idx + 3],
            ]);
            out.push(char::from_u32(codepoint).unwrap_or(' '));
        }
        if row + 1 < rows {
            out.push('\n');
        }
    }
    out
}

fn wait_for_terminal_frame_contains(
    connection: &makepad_studio_hub::HubConnection,
    path: &str,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match msg {
            HubToClient::TerminalFramebuffer {
                path: output_path,
                frame,
            } if output_path == path => {
                if framebuffer_to_text(&frame).contains(needle) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn wait_for_terminal_frame_where<F>(
    connection: &makepad_studio_hub::HubConnection,
    path: &str,
    timeout: Duration,
    mut predicate: F,
) -> Option<TerminalFramebuffer>
where
    F: FnMut(&TerminalFramebuffer) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match msg {
            HubToClient::TerminalFramebuffer {
                path: output_path,
                frame,
            } if output_path == path => {
                if predicate(&frame) {
                    return Some(frame);
                }
            }
            _ => {}
        }
    }
    None
}

fn framebuffer_last_row_text(frame: &TerminalFramebuffer) -> String {
    framebuffer_to_text(frame)
        .lines()
        .last()
        .unwrap_or("")
        .trim_end()
        .to_string()
}

fn framebuffer_row_text(frame: &TerminalFramebuffer, row: usize) -> String {
    framebuffer_to_text(frame)
        .lines()
        .nth(row)
        .unwrap_or("")
        .trim_end()
        .to_string()
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[test]
fn in_process_connection_roundtrip_and_cargo_build_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _tree_query = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let tree = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive file tree");
    match tree {
        HubToClient::FileTree { data, .. } => {
            assert!(data.nodes.iter().any(|node| node.path == "repo/src/lib.rs"));
        }
        _ => unreachable!(),
    }

    let _run_query_id = connection.send(ClientToHub::Cargo {
        mount: "repo".to_string(),
        args: vec!["--version".to_string()],
        env: None,
        buildbox: None,
    });

    let started = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, HubToClient::BuildStarted { mount, .. } if mount == "repo"),
    );
    let build_id = match started {
        Some(HubToClient::BuildStarted { build_id, .. }) => build_id,
        _ => panic!("did not receive BuildStarted"),
    };

    let stopped = wait_for_message(
        &connection,
        Duration::from_secs(10),
        |msg| matches!(msg, HubToClient::BuildStopped { build_id: id, .. } if *id == build_id),
    );
    assert!(stopped.is_some(), "did not receive BuildStopped");

    let query_id = connection.send(ClientToHub::QueryLogs {
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
            HubToClient::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive QueryLogResults");

    match log_results {
        HubToClient::QueryLogResults { entries, done, .. } => {
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

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/large_paste.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 120,
        rows: 30,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(4),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");
    request_terminal_viewport(&mut connection, &path, 120, 30, usize::MAX);

    // Disable echo so the large paste does not flood output, stream a large paste
    // into cat, send Ctrl-D, then run a marker command.
    let mut input = b"stty -echo\ncat > /dev/null\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', LARGE_PASTE_BYTES));
    input.push(b'\n');
    // Interrupt cat so we reliably return to shell even if EOF semantics vary.
    input.push(0x03);
    input.extend_from_slice(b"stty echo\necho __paste_ok__\n");
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: input,
    });

    let mut saw_marker = false;
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        match msg {
            HubToClient::TerminalExited {
                path: exited_path, ..
            } if exited_path == path => {
                panic!("terminal exited during large paste");
            }
            HubToClient::Error { message }
                if message.contains("unknown terminal") && message.contains(&path) =>
            {
                panic!("terminal lost after large paste: {}", message);
            }
            HubToClient::TerminalFramebuffer {
                path: output_path,
                frame,
            } if output_path == path => {
                if framebuffer_to_text(&frame).contains("__paste_ok__") {
                    saw_marker = true;
                    break;
                }
            }
            _ => {}
        }
    }

    let _ = connection.send(ClientToHub::TerminalClose { path });
    assert!(
        saw_marker,
        "did not observe terminal response after large paste"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_resize_delivers_sigwinch_with_updated_stty_size() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let script_path = dir.path().join("resize_sigwinch_test.sh");
    fs::write(
        &script_path,
        r#"#!/bin/sh
printf '\033[?1049h\033[2J\033[H'
report_size() {
    size="$(stty size 2>/dev/null || echo 0 0)"
    printf '__SIZE__:%s\n' "$size"
}
trap 'report_size' WINCH
report_size
while :; do
    sleep 0.2
done
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/resize_sigwinch.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 80,
        rows: 10,
        env: std::collections::HashMap::new(),
    });

    let opened = wait_for_message(
        &connection,
        Duration::from_secs(4),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");
    request_terminal_viewport(&mut connection, &path, 80, 10, usize::MAX);

    let run_cmd = format!("sh {}\n", script_path.to_string_lossy());
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: run_cmd.into_bytes(),
    });

    assert!(
        wait_for_terminal_frame_contains(
            &connection,
            &path,
            "__SIZE__:10 80",
            Duration::from_secs(8),
        ),
        "initial stty size marker was not observed"
    );

    request_terminal_viewport(&mut connection, &path, 80, 20, usize::MAX);
    assert!(
        wait_for_terminal_frame_contains(
            &connection,
            &path,
            "__SIZE__:20 80",
            Duration::from_secs(8),
        ),
        "did not observe stty size update after first resize"
    );

    request_terminal_viewport(&mut connection, &path, 120, 20, usize::MAX);
    assert!(
        wait_for_terminal_frame_contains(
            &connection,
            &path,
            "__SIZE__:20 120",
            Duration::from_secs(8),
        ),
        "did not observe stty size update after second resize"
    );

    // Stop the loop before closing the PTY session.
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: vec![0x03],
    });
    let _ = connection.send(ClientToHub::TerminalClose { path });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_bash_prompt_sticks_to_bottom_after_grow_resize() {
    let bash_available = std::process::Command::new("sh")
        .args(["-lc", "command -v bash >/dev/null 2>&1"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !bash_available {
        eprintln!("skipping: bash binary not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/bash_prompt_stick.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 80,
        rows: 10,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(4),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");
    request_terminal_viewport(&mut connection, &path, 80, 10, usize::MAX);

    // Build deep scrollback and leave the interactive prompt at the bottom.
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: b"bash --noprofile --norc -i\nfor i in $(seq 1 180); do echo __SCROLL__$i; done\n"
            .to_vec(),
    });

    let frame_10 =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(20), |frame| {
            frame.rows == 10
                && frame.total_lines > 120
                && framebuffer_to_text(frame).contains("__SCROLL__180")
        })
        .expect("did not observe 10-row bash viewport with deep scrollback");
    assert!(
        frame_10.cursor_visible && frame_10.cursor_row == 9,
        "10-row viewport should have bash cursor on the last row before resize"
    );

    request_terminal_viewport(&mut connection, &path, 80, 15, usize::MAX);

    let frame_15 =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(10), |frame| {
            frame.rows == 15
                && frame.total_lines >= frame_10.total_lines
                && framebuffer_to_text(frame).contains("__SCROLL__180")
        })
        .expect("did not observe 15-row resized viewport");
    assert!(
        frame_15.cursor_visible && frame_15.cursor_row == 14,
        "15-row viewport should keep bash cursor/prompt pinned to bottom"
    );
    assert!(
        frame_15.top_row <= frame_10.top_row,
        "grow resize should keep bottom content anchored (top_row must not increase)"
    );

    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: b"exit\n".to_vec(),
    });
    let _ = connection.send(ClientToHub::TerminalClose { path });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_bash_grow_resize_clamps_to_top_when_history_is_insufficient() {
    let bash_available = std::process::Command::new("sh")
        .args(["-lc", "command -v bash >/dev/null 2>&1"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !bash_available {
        eprintln!("skipping: bash binary not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/bash_resize_clamp_top.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 80,
        rows: 10,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(4),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");
    request_terminal_viewport(&mut connection, &path, 80, 10, usize::MAX);

    // Fresh shell has insufficient history to fill additional rows on grow.
    let frame_10 =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(10), |frame| {
            frame.rows == 10
        })
        .expect("did not observe short-history 10-row frame");

    request_terminal_viewport(&mut connection, &path, 80, 15, usize::MAX);

    let frame_15 =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(10), |frame| {
            frame.rows == 15
        })
        .expect("did not observe short-history 15-row frame");

    assert!(
        frame_10.top_row == 0,
        "short-history baseline should already be clamped to top"
    );
    assert!(
        frame_15.top_row == 0,
        "grow resize with insufficient history must clamp to top (top_row == 0)"
    );
    assert!(
        frame_15.cursor_row < 14,
        "with insufficient history, cursor/prompt should not be forced to bottom row"
    );

    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: b"exit\n".to_vec(),
    });
    let _ = connection.send(ClientToHub::TerminalClose { path });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_codex_prompt_sticks_to_bottom_after_resize() {
    let codex_available = std::process::Command::new("sh")
        .args(["-lc", "command -v codex >/dev/null 2>&1"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !codex_available {
        eprintln!("skipping: codex binary not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/codex_prompt_stick.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 80,
        rows: 10,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(4),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");

    request_terminal_viewport(&mut connection, &path, 80, 10, usize::MAX);

    // Build scrollback first, then start codex.
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: b"for i in $(seq 1 220); do echo __SCROLL__$i; done\ncodex\n".to_vec(),
    });

    let frame_10 =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(30), |frame| {
            frame.rows == 10
                && frame.total_lines > 60
                && !framebuffer_last_row_text(frame).trim().is_empty()
        })
        .expect("did not observe 10-row codex viewport state");

    request_terminal_viewport(&mut connection, &path, 80, 15, usize::MAX);

    let frame_15 =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(20), |frame| {
            frame.rows == 15
                && frame.total_lines >= frame_10.total_lines
                && !framebuffer_last_row_text(frame).trim().is_empty()
        })
        .expect("did not observe 15-row codex viewport state");

    assert!(
        !framebuffer_last_row_text(&frame_15).trim().is_empty(),
        "expected codex prompt/cursor to stay at bottom after resize (10 -> 15 rows)"
    );

    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: vec![0x03],
    });
    let _ = connection.send(ClientToHub::TerminalClose { path });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_codex_fast_resize_roundtrip_preserves_top_and_bottom_rows() {
    let codex_available = std::process::Command::new("sh")
        .args(["-lc", "command -v codex >/dev/null 2>&1"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !codex_available {
        eprintln!("skipping: codex binary not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/codex_fast_resize.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 80,
        rows: 20,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");

    request_terminal_viewport(&mut connection, &path, 80, 20, usize::MAX);
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: b"for i in $(seq 1 260); do echo __SCROLL__$i; done\ncodex\n".to_vec(),
    });

    let baseline =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(35), |frame| {
            frame.rows == 20
                && frame.total_lines > 80
                && !framebuffer_last_row_text(frame).trim().is_empty()
        })
        .expect("did not observe baseline 20-row codex frame");

    let baseline_top = framebuffer_row_text(&baseline, 0);
    let baseline_sep = framebuffer_row_text(&baseline, 1);
    let baseline_bottom = framebuffer_last_row_text(&baseline);
    let baseline_text = framebuffer_to_text(&baseline);

    // Repeat rapid resize bursts to catch fast-drag race behavior.
    for cycle in 0..5 {
        for rows in 21..=40 {
            request_terminal_viewport(&mut connection, &path, 80, rows, usize::MAX);
        }
        for rows in (20..40).rev() {
            request_terminal_viewport(&mut connection, &path, 80, rows, usize::MAX);
        }
        request_terminal_viewport(&mut connection, &path, 80, 20, usize::MAX);

        let final_frame =
            wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(8), |frame| {
                frame.rows == 20 && !framebuffer_last_row_text(frame).trim().is_empty()
            })
            .expect("did not observe final 20-row codex frame after fast resize burst");

        let final_top = framebuffer_row_text(&final_frame, 0);
        let final_sep = framebuffer_row_text(&final_frame, 1);
        let final_bottom = framebuffer_last_row_text(&final_frame);

        assert_eq!(
            final_top.trim(),
            baseline_top.trim(),
            "top row changed after fast resize roundtrip cycle {}",
            cycle
        );
        assert_eq!(
            final_sep.trim(),
            baseline_sep.trim(),
            "separator row changed after fast resize roundtrip cycle {}",
            cycle
        );
        assert_eq!(
            final_bottom.trim(),
            baseline_bottom.trim(),
            "bottom row changed after fast resize roundtrip cycle {}",
            cycle
        );
        assert_eq!(
            framebuffer_to_text(&final_frame),
            baseline_text,
            "full framebuffer changed after fast resize roundtrip cycle {}",
            cycle
        );
    }

    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: vec![0x03],
    });
    let _ = connection.send(ClientToHub::TerminalClose { path });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_codex_fast_vs_slow_wiggle_same_final_frame() {
    let codex_available = std::process::Command::new("sh")
        .args(["-lc", "command -v codex >/dev/null 2>&1"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !codex_available {
        eprintln!("skipping: codex binary not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let wiggle: &[u16] = &[30, 12, 28, 10, 26, 14, 29, 11, 27, 13, 30, 20];

    let mut run_mode = |path: &str, slow: bool| -> String {
        let tui_log_path = format!("/tmp/{}.{}.log", path.replace('/', "_"), std::process::id());
        let mut env = std::collections::HashMap::new();
        env.insert("TUI_LOG".to_string(), tui_log_path.clone());
        let _ = connection.send(ClientToHub::TerminalOpen {
            path: path.to_string(),
            cols: 80,
            rows: 20,
            env,
        });
        let opened = wait_for_message(
            &connection,
            Duration::from_secs(5),
            |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == path),
        );
        assert!(
            opened.is_some(),
            "did not receive TerminalOpened for {}",
            path
        );

        request_terminal_viewport(&mut connection, path, 80, 20, usize::MAX);
        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: b"for i in $(seq 1 260); do echo __SCROLL__$i; done\ncodex\n".to_vec(),
        });
        let _baseline =
            wait_for_terminal_frame_where(&connection, path, Duration::from_secs(35), |frame| {
                frame.rows == 20
                    && frame.total_lines > 80
                    && !framebuffer_last_row_text(frame).trim().is_empty()
            })
            .expect("did not observe baseline codex frame");

        if slow {
            for rows in wiggle {
                request_terminal_viewport(&mut connection, path, 80, *rows, usize::MAX);
                let _ = wait_for_terminal_frame_where(
                    &connection,
                    path,
                    Duration::from_secs(6),
                    |frame| {
                        frame.rows == *rows && !framebuffer_last_row_text(frame).trim().is_empty()
                    },
                )
                .expect("did not observe slow wiggle step frame");
            }
        } else {
            for rows in wiggle {
                request_terminal_viewport(&mut connection, path, 80, *rows, usize::MAX);
            }
        }

        request_terminal_viewport(&mut connection, path, 80, 20, usize::MAX);
        let final_frame =
            wait_for_terminal_frame_where(&connection, path, Duration::from_secs(10), |frame| {
                frame.rows == 20 && !framebuffer_last_row_text(frame).trim().is_empty()
            })
            .expect("did not observe final 20-row frame");

        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: vec![0x03],
        });
        let _ = connection.send(ClientToHub::TerminalClose {
            path: path.to_string(),
        });
        framebuffer_to_text(&final_frame)
    };

    let slow_text = run_mode("repo/.makepad/codex_slow_wiggle.term", true);
    let fast_text = run_mode("repo/.makepad/codex_fast_wiggle.term", false);
    assert_eq!(
        fast_text, slow_text,
        "fast wiggle final frame diverged from slow wiggle final frame"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn terminal_makepad_tui_fast_wiggle_preserves_framebuffer() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let tui_bin = workspace_root.join("target/release/makepad-tui-test");
    if !tui_bin.exists() {
        eprintln!(
            "skipping: makepad-tui-test binary not found at {}",
            tui_bin.display()
        );
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let path = "repo/.makepad/tui_fast_wiggle.term".to_string();
    let _ = connection.send(ClientToHub::TerminalOpen {
        path: path.clone(),
        cols: 80,
        rows: 20,
        env: std::collections::HashMap::new(),
    });
    let opened = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == &path),
    );
    assert!(opened.is_some(), "did not receive TerminalOpened");

    request_terminal_viewport(&mut connection, &path, 80, 20, usize::MAX);

    let launch = format!("{}\n", tui_bin.display());
    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: launch.into_bytes(),
    });

    let baseline =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(20), |frame| {
            frame.rows == 20
                && framebuffer_to_text(frame).contains("TUI Test")
                && framebuffer_to_text(frame).contains("directory:")
        })
        .expect("did not observe baseline makepad-tui frame");
    let baseline_text = framebuffer_to_text(&baseline);

    // Rapid wiggle with jumps between 10..30 rows, repeated to amplify races.
    let jumps: &[u16] = &[30, 12, 28, 10, 26, 14, 29, 11, 27, 13, 30, 20];
    for _ in 0..6 {
        for rows in jumps {
            request_terminal_viewport(&mut connection, &path, 80, *rows, usize::MAX);
        }
    }
    request_terminal_viewport(&mut connection, &path, 80, 20, usize::MAX);

    let final_frame =
        wait_for_terminal_frame_where(&connection, &path, Duration::from_secs(10), |frame| {
            frame.rows == 20 && framebuffer_to_text(frame).contains("TUI Test")
        })
        .expect("did not observe final 20-row makepad-tui frame");
    let final_text = framebuffer_to_text(&final_frame);

    assert_eq!(
        final_text, baseline_text,
        "makepad-tui framebuffer changed after fast wiggle resize burst"
    );

    let _ = connection.send(ClientToHub::TerminalInput {
        path: path.clone(),
        data: vec![0x03],
    });
    let _ = connection.send(ClientToHub::TerminalClose { path });
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
#[ignore = "stress diagnostic: intermittently reproduces fast-resize garbling"]
fn terminal_makepad_tui_fast_resize_during_output_matches_no_resize() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let tui_bin = workspace_root.join("target/release/makepad-tui-test");
    if !tui_bin.exists() {
        eprintln!(
            "skipping: makepad-tui-test binary not found at {}",
            tui_bin.display()
        );
        return;
    }

    let short_mount = std::path::PathBuf::from("/tmp").join(format!(
        "mpt_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let _ = fs::remove_dir_all(&short_mount);
    fs::create_dir_all(short_mount.join("src")).unwrap();
    fs::write(short_mount.join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: short_mount.clone(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let mut run_mode = |path: &str, fast_resize: bool| -> String {
        let tui_log_path = format!("/tmp/{}.{}.log", path.replace('/', "_"), std::process::id());
        let mut env = std::collections::HashMap::new();
        env.insert("TUI_LOG".to_string(), tui_log_path.clone());
        let _ = connection.send(ClientToHub::TerminalOpen {
            path: path.to_string(),
            cols: 80,
            rows: 20,
            env,
        });
        let opened = wait_for_message(
            &connection,
            Duration::from_secs(5),
            |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == path),
        );
        assert!(
            opened.is_some(),
            "did not receive TerminalOpened for {}",
            path
        );

        request_terminal_viewport(&mut connection, path, 80, 20, usize::MAX);
        let launch = format!("{}\n", tui_bin.display());
        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: launch.into_bytes(),
        });

        let _baseline =
            wait_for_terminal_frame_where(&connection, path, Duration::from_secs(20), |frame| {
                frame.rows == 20
                    && framebuffer_to_text(frame).contains("TUI Test")
                    && framebuffer_to_text(frame).contains("directory:")
            })
            .expect("did not observe baseline makepad-tui frame");

        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: b"hi\r".to_vec(),
        });

        // 10 cycles of multi-line jump scales between 10 and 40 rows.
        let jumps: &[u16] = &[40, 10, 38, 12, 36, 14, 34, 16, 32, 18, 30, 20];
        for _ in 0..10 {
            for rows in jumps {
                request_terminal_viewport(&mut connection, path, 80, *rows, usize::MAX);
                if !fast_resize {
                    let _ = wait_for_terminal_frame_where(
                        &connection,
                        path,
                        Duration::from_secs(2),
                        |frame| frame.rows == *rows,
                    )
                    .expect("did not observe slow resize step frame");
                }
            }
        }

        request_terminal_viewport(&mut connection, path, 80, 20, usize::MAX);
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut next_poll = Instant::now();
        let mut final_text = None;
        let mut last_20_text = String::new();
        while Instant::now() < deadline {
            if Instant::now() >= next_poll {
                request_terminal_viewport(&mut connection, path, 80, 20, usize::MAX);
                next_poll = Instant::now() + Duration::from_millis(200);
            }
            let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            let HubToClient::TerminalFramebuffer {
                path: output_path,
                frame,
            } = msg
            else {
                continue;
            };
            if output_path != path {
                continue;
            }
            if frame.rows != 20 {
                continue;
            }
            let text = framebuffer_to_text(&frame);
            last_20_text = text.clone();
            if text.contains("[10/10] hi")
                && text.contains("Enter a prompt...")
                && text.contains("fake-model")
                && !text.contains("Working (")
            {
                final_text = Some(text);
                break;
            }
        }
        let _completed_text = final_text.unwrap_or_else(|| {
            let log_tail = fs::read_to_string(&tui_log_path)
                .map(|s| tail_lines(&s, 40))
                .unwrap_or_else(|_| "<missing TUI_LOG>".to_string());
            panic!(
                "did not observe completed 20-row frame after resize burst; last 20-row frame:\n{}\n\nTUI_LOG tail:\n{}",
                last_20_text, log_tail
            )
        });
        // Force a deterministic post-resize redraw cycle before comparing.
        request_terminal_viewport(&mut connection, path, 80, 21, usize::MAX);
        let _ = wait_for_terminal_frame_where(&connection, path, Duration::from_secs(3), |frame| {
            frame.rows == 21
        })
        .expect("did not observe settle 21-row frame");
        request_terminal_viewport(&mut connection, path, 80, 20, usize::MAX);
        let settled =
            wait_for_terminal_frame_where(&connection, path, Duration::from_secs(3), |frame| {
                frame.rows == 20
            })
            .expect("did not observe settle 20-row frame");
        let final_text = framebuffer_to_text(&settled);

        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: vec![0x03],
        });
        let _ = connection.send(ClientToHub::TerminalClose {
            path: path.to_string(),
        });
        final_text
    };

    let slow_text = run_mode("repo/.makepad/tui_slow_resize_during_output.term", false);
    let fast_text = run_mode("repo/.makepad/tui_fast_resize_during_output.term", true);
    let _ = fs::remove_dir_all(&short_mount);
    assert_eq!(
        fast_text, slow_text,
        "fast resize during output diverged from slow resize reference"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
#[ignore = "stress diagnostic: reproduces persistent corruption after fast resize"]
fn terminal_makepad_tui_fast_then_slow_same_session_matches_slow_only() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let tui_bin = workspace_root.join("target/release/makepad-tui-test");
    if !tui_bin.exists() {
        eprintln!(
            "skipping: makepad-tui-test binary not found at {}",
            tui_bin.display()
        );
        return;
    }

    let mount = std::path::PathBuf::from("/tmp").join(format!(
        "mpp_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(mount.join("src")).unwrap();
    fs::write(mount.join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: mount.clone(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let jumps: &[u16] = &[40, 10, 38, 12, 36, 14, 34, 16, 32, 18, 30, 20];

    let run_session = |connection: &mut makepad_studio_hub::HubConnection,
                       path: &str,
                       do_fast_first: bool|
     -> String {
        let tui_log_path = format!("/tmp/{}.{}.log", path.replace('/', "_"), std::process::id());
        let mut env = std::collections::HashMap::new();
        env.insert("TUI_LOG".to_string(), tui_log_path.clone());
        let _ = connection.send(ClientToHub::TerminalOpen {
            path: path.to_string(),
            cols: 80,
            rows: 20,
            env,
        });
        let opened = wait_for_message(
            connection,
            Duration::from_secs(5),
            |msg| matches!(msg, HubToClient::TerminalOpened { path: p, .. } if p == path),
        );
        assert!(
            opened.is_some(),
            "did not receive TerminalOpened for {}",
            path
        );

        request_terminal_viewport(connection, path, 80, 20, usize::MAX);
        let launch = format!("{}\n", tui_bin.display());
        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: launch.into_bytes(),
        });
        let _baseline =
            wait_for_terminal_frame_where(connection, path, Duration::from_secs(20), |frame| {
                frame.rows == 20
                    && framebuffer_to_text(frame).contains("TUI Test")
                    && framebuffer_to_text(frame).contains("directory:")
            })
            .expect("did not observe baseline makepad-tui frame");

        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: b"hi\r".to_vec(),
        });

        if do_fast_first {
            for _ in 0..10 {
                for rows in jumps {
                    request_terminal_viewport(connection, path, 80, *rows, usize::MAX);
                }
            }
        }

        // Slow pass after potential corruption.
        for _ in 0..2 {
            for rows in jumps {
                request_terminal_viewport(connection, path, 80, *rows, usize::MAX);
                let _ = wait_for_terminal_frame_where(
                    connection,
                    path,
                    Duration::from_secs(2),
                    |frame| frame.rows == *rows,
                )
                .expect("did not observe slow resize step frame");
            }
        }

        request_terminal_viewport(connection, path, 80, 20, usize::MAX);
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut final_text = None;
        let mut last_text = String::new();
        while Instant::now() < deadline {
            request_terminal_viewport(connection, path, 80, 20, usize::MAX);
            let Some(msg) = connection.recv_timeout(Duration::from_millis(120)) else {
                continue;
            };
            let HubToClient::TerminalFramebuffer {
                path: output_path,
                frame,
            } = msg
            else {
                continue;
            };
            if output_path != path || frame.rows != 20 {
                continue;
            }
            let text = framebuffer_to_text(&frame);
            last_text = text.clone();
            if text.contains("[10/10] hi")
                && text.contains("Enter a prompt...")
                && text.contains("fake-model")
                && !text.contains("Working (")
            {
                final_text = Some(text);
                break;
            }
        }
        let _completed_text = final_text.unwrap_or_else(|| {
            let log_tail = fs::read_to_string(&tui_log_path)
                .map(|s| tail_lines(&s, 40))
                .unwrap_or_else(|_| "<missing TUI_LOG>".to_string());
            panic!(
                "did not observe completed frame:\n{}\n\nTUI_LOG tail:\n{}",
                last_text, log_tail
            )
        });
        request_terminal_viewport(connection, path, 80, 21, usize::MAX);
        let _ = wait_for_terminal_frame_where(connection, path, Duration::from_secs(3), |frame| {
            frame.rows == 21
        })
        .expect("did not observe settle 21-row frame");
        request_terminal_viewport(connection, path, 80, 20, usize::MAX);
        let settled =
            wait_for_terminal_frame_where(connection, path, Duration::from_secs(3), |frame| {
                frame.rows == 20
            })
            .expect("did not observe settle 20-row frame");
        let final_text = framebuffer_to_text(&settled);

        let _ = connection.send(ClientToHub::TerminalInput {
            path: path.to_string(),
            data: vec![0x03],
        });
        let _ = connection.send(ClientToHub::TerminalClose {
            path: path.to_string(),
        });
        final_text
    };

    let slow_only = run_session(
        &mut connection,
        "repo/.makepad/tui_slow_only_after_input.term",
        false,
    );
    let fast_then_slow = run_session(
        &mut connection,
        "repo/.makepad/tui_fast_then_slow_after_input.term",
        true,
    );
    let _ = fs::remove_dir_all(&mount);

    assert_eq!(
        fast_then_slow, slow_only,
        "fast-first corruption persisted into subsequent slow resize"
    );
}

#[test]
fn file_tree_keeps_hidden_directories_for_backend() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join(".hidden")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();
    fs::write(dir.path().join(".hidden/secret.txt"), "secret\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let tree = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive file tree");

    match tree {
        HubToClient::FileTree { data, .. } => {
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

    let config = HubConfig {
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

    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");
    let _ = connection.send(ClientToHub::Unmount {
        name: "alpha".to_string(),
    });

    let diff = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTreeDiff { mount, .. } if mount == "alpha"),
    )
    .expect("did not receive alpha FileTreeDiff");

    match diff {
        HubToClient::FileTreeDiff { mount, changes } => {
            assert_eq!(mount, "alpha");
            assert!(!changes.is_empty(), "expected removed paths for alpha");
            for change in changes {
                match change {
                    makepad_studio_protocol::hub_protocol::FileTreeChange::Added {
                        path, ..
                    }
                    | makepad_studio_protocol::hub_protocol::FileTreeChange::Removed { path }
                    | makepad_studio_protocol::hub_protocol::FileTreeChange::Modified {
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
fn run_items_are_pushed_per_mount() {
    let mount_a = tempfile::tempdir().unwrap();
    let mount_b = tempfile::tempdir().unwrap();

    fs::write(
        mount_a.path().join("makepad.splash"),
        "use mod.hub\nhub.set_run_items([{name:\"alpha-app\" in_studio:true on_run:fn(){}}])\n",
    )
    .unwrap();
    fs::write(
        mount_b.path().join("makepad.splash"),
        "use mod.hub\nhub.set_run_items([{name:\"beta-app\" in_studio:true on_run:fn(){}}])\n",
    )
    .unwrap();

    let config = HubConfig {
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
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "alpha".to_string(),
        primary: Some(true),
    });
    let alpha = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, HubToClient::RunItems { mount, items } if mount == "alpha" && items.len() == 1),
    )
    .expect("did not receive alpha run items");
    match alpha {
        HubToClient::RunItems { mount, items } => {
            assert_eq!(mount, "alpha");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].name, "alpha-app");
        }
        _ => unreachable!(),
    }

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "beta".to_string(),
        primary: Some(true),
    });
    let beta = wait_for_message(
        &connection,
        Duration::from_secs(5),
        |msg| matches!(msg, HubToClient::RunItems { mount, items } if mount == "beta" && items.len() == 1),
    )
    .expect("did not receive beta run items");
    match beta {
        HubToClient::RunItems { mount, items } => {
            assert_eq!(mount, "beta");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].name, "beta-app");
        }
        _ => unreachable!(),
    }
}

#[test]
fn run_item_executes_named_on_run_callback() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("makepad.splash"),
        "use mod.std\nuse mod.hub\nhub.set_run_items([{name:\"hello\" in_studio:true on_run:fn(){std.println(\"hello from item\")}}])\n",
    )
    .unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "repo".to_string(),
        primary: Some(true),
    });
    let started = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| {
            matches!(
                msg,
                HubToClient::BuildStarted { mount, package, .. }
                    if mount == "repo" && package == "makepad.splash"
            )
        },
    )
    .expect("did not receive BuildStarted");
    let build_id = match started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let run_items = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::RunItems { mount, items } if mount == "repo" && items.len() == 1),
    )
    .expect("did not receive RunItems");
    match run_items {
        HubToClient::RunItems { items, .. } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].name, "hello");
        }
        _ => unreachable!(),
    }

    let _ = connection.send(ClientToHub::RunItem {
        mount: "repo".to_string(),
        name: "hello".to_string(),
    });
    let _ = drain_messages(&connection, Duration::from_millis(250));

    let query_id = connection.send(ClientToHub::QueryLogs {
        build_id: Some(build_id),
        level: None,
        source: None,
        file: None,
        pattern: Some("hello from item".to_string()),
        is_regex: None,
        since_index: None,
        live: Some(false),
    });
    let log_results = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive QueryLogResults");

    match log_results {
        HubToClient::QueryLogResults { entries, done, .. } => {
            assert!(done);
            let messages: Vec<String> = entries
                .iter()
                .map(|entry| entry.1.message.clone())
                .collect();
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.1.message.contains("hello from item")),
                "expected run item log in splash logs, got {:?}",
                messages
            );
        }
        _ => unreachable!(),
    }

    let _ = connection.send(ClientToHub::StopBuild { build_id });
}

#[test]
fn run_item_spawns_cargo_run_for_clicked_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"makepad-example-splash\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n    println!(\"hello from clicked item\");\n}\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("makepad.splash"),
        "use mod.hub\nhub.set_run_items([{name:\"makepad-example-splash\" in_studio:true on_run:fn(){let name = me.name hub.run({\"STUDIO\":hub.studio_ip}, \"cargo\", [\"run\" \"-p\" name \"--release\" \"--message-format=json\" \"--\" \"--message-format=json\" \"--stdin-loop\"])}}])\n",
    )
    .unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "repo".to_string(),
        primary: Some(true),
    });
    let splash_started = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| {
            matches!(
                msg,
                HubToClient::BuildStarted { mount, package, .. }
                    if mount == "repo" && package == "makepad.splash"
            )
        },
    )
    .expect("did not receive splash BuildStarted");
    let splash_build_id = match splash_started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let run_items = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::RunItems { mount, items } if mount == "repo" && items.len() == 1),
    )
    .expect("did not receive RunItems");
    match run_items {
        HubToClient::RunItems { items, .. } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].name, "makepad-example-splash");
        }
        _ => unreachable!(),
    }

    let _ = connection.send(ClientToHub::RunItem {
        mount: "repo".to_string(),
        name: "makepad-example-splash".to_string(),
    });

    let child_started = wait_for_message(
        &connection,
        Duration::from_secs(10),
        |msg| {
            matches!(
                msg,
                HubToClient::BuildStarted {
                    build_id,
                    mount,
                    package,
                } if *build_id != splash_build_id
                    && mount == "repo"
                    && package == "makepad-example-splash"
            )
        },
    )
    .expect("did not receive child BuildStarted");
    let child_build_id = match child_started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let child_stopped = wait_for_message(
        &connection,
        Duration::from_secs(20),
        |msg| matches!(msg, HubToClient::BuildStopped { build_id, exit_code: Some(0) } if *build_id == child_build_id),
    );
    assert!(
        child_stopped.is_some(),
        "did not receive successful child BuildStopped"
    );

    let query_id = connection.send(ClientToHub::QueryLogs {
        build_id: Some(child_build_id),
        level: None,
        source: None,
        file: None,
        pattern: Some("hello from clicked item".to_string()),
        is_regex: None,
        since_index: None,
        live: Some(false),
    });
    let log_results = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive child QueryLogResults");

    match log_results {
        HubToClient::QueryLogResults { entries, done, .. } => {
            assert!(done);
            let messages: Vec<String> = entries
                .iter()
                .map(|entry| entry.1.message.clone())
                .collect();
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.1.message.contains("hello from clicked item")),
                "expected child build log output, got {:?}",
                messages
            );
        }
        _ => unreachable!(),
    }

    let _ = connection.send(ClientToHub::StopBuild {
        build_id: splash_build_id,
    });
}

#[test]
fn run_item_reports_script_error_in_hub_run_args() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("makepad.splash"),
        "use mod.hub\nhub.set_run_items([{name:\"broken\" in_studio:true on_run:fn(){hub.run(nil, \"cargo\", [\"run\" \"-p\" self.package])}}])\n",
    )
    .unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "repo".to_string(),
        primary: Some(true),
    });
    let started = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| {
            matches!(
                msg,
                HubToClient::BuildStarted { mount, package, .. }
                    if mount == "repo" && package == "makepad.splash"
            )
        },
    )
    .expect("did not receive BuildStarted");
    let build_id = match started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let run_items = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::RunItems { mount, items } if mount == "repo" && items.len() == 1),
    )
    .expect("did not receive RunItems");
    match run_items {
        HubToClient::RunItems { items, .. } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].name, "broken");
        }
        _ => unreachable!(),
    }

    let _ = connection.send(ClientToHub::RunItem {
        mount: "repo".to_string(),
        name: "broken".to_string(),
    });
    let unexpected_build = wait_for_message(&connection, Duration::from_millis(500), |msg| {
        matches!(
            msg,
            HubToClient::BuildStarted {
                build_id: id, ..
            } if *id != build_id
        )
    });
    assert!(
        unexpected_build.is_none(),
        "expected invalid hub.run args to prevent spawning a child build, got {:?}",
        unexpected_build
    );

    let _ = connection.send(ClientToHub::StopBuild { build_id });
}

#[test]
fn splash_runnable_prints_hello() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("makepad.splash"),
        "use mod.std\nstd.println(\"hello\")\n",
    )
    .unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::Run {
        mount: "repo".to_string(),
        process: "makepad.splash".to_string(),
        args: Vec::new(),
        standalone: None,
        env: None,
        buildbox: None,
    });

    let started = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStarted { mount, package, .. } if mount == "repo" && package == "makepad.splash"),
    )
    .expect("did not receive BuildStarted");
    let build_id = match started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let stopped = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStopped { build_id: id, exit_code: Some(0) } if *id == build_id),
    );
    assert!(stopped.is_some(), "did not receive successful BuildStopped");

    let query_id = connection.send(ClientToHub::QueryLogs {
        build_id: Some(build_id),
        level: None,
        source: None,
        file: None,
        pattern: Some("hello".to_string()),
        is_regex: None,
        since_index: None,
        live: Some(false),
    });
    let log_results = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive QueryLogResults");

    match log_results {
        HubToClient::QueryLogResults { entries, done, .. } => {
            assert!(done);
            let messages: Vec<String> = entries
                .iter()
                .map(|entry| entry.1.message.clone())
                .collect();
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.1.message.contains("hello")),
                "expected hello in splash logs, got {:?}",
                messages
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn observe_mount_auto_starts_splash() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("makepad.splash"),
        "use mod.std\nstd.println(\"hello\")\n",
    )
    .unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "repo".to_string(),
        primary: Some(true),
    });

    let started = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStarted { mount, package, .. } if mount == "repo" && package == "makepad.splash"),
    )
    .expect("did not receive BuildStarted");
    let build_id = match started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let stopped = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStopped { build_id: id, exit_code: Some(0) } if *id == build_id),
    );
    assert!(stopped.is_some(), "did not receive successful BuildStopped");

    let query_id = connection.send(ClientToHub::QueryLogs {
        build_id: Some(build_id),
        level: None,
        source: None,
        file: None,
        pattern: Some("hello".to_string()),
        is_regex: None,
        since_index: None,
        live: Some(false),
    });
    let log_results = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive QueryLogResults");

    match log_results {
        HubToClient::QueryLogResults { entries, done, .. } => {
            assert!(done);
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.1.message.contains("hello")),
                "expected hello in splash logs"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn observe_mount_reload_splash_after_save() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("makepad.splash"),
        "use mod.std\nstd.println(\"one\")\n",
    )
    .unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ObserveMount {
        mount: "repo".to_string(),
        primary: Some(true),
    });

    let first_started = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStarted { mount, package, .. } if mount == "repo" && package == "makepad.splash"),
    )
    .expect("did not receive initial BuildStarted");
    let first_build_id = match first_started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let first_stopped = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStopped { build_id: id, exit_code: Some(0) } if *id == first_build_id),
    );
    assert!(
        first_stopped.is_some(),
        "did not receive successful initial BuildStopped"
    );

    let new_content = "use mod.std\nstd.println(\"two\")\n".to_string();
    let _ = connection.send(ClientToHub::SaveTextFile {
        path: "repo/makepad.splash".to_string(),
        content: new_content,
    });

    let saved = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::TextFileSaved { path, .. } if path == "repo/makepad.splash"
        )
    });
    assert!(saved.is_some(), "did not receive TextFileSaved");

    let second_started = wait_for_message(&connection, Duration::from_secs(6), |msg| {
        matches!(
            msg,
            HubToClient::BuildStarted {
                build_id,
                mount,
                package,
            } if mount == "repo" && package == "makepad.splash" && *build_id != first_build_id
        )
    })
    .expect("did not receive reloaded BuildStarted");
    let second_build_id = match second_started {
        HubToClient::BuildStarted { build_id, .. } => build_id,
        _ => unreachable!(),
    };

    let second_stopped = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::BuildStopped { build_id: id, exit_code: Some(0) } if *id == second_build_id),
    );
    assert!(
        second_stopped.is_some(),
        "did not receive successful reloaded BuildStopped"
    );

    let query_id = connection.send(ClientToHub::QueryLogs {
        build_id: Some(second_build_id),
        level: None,
        source: None,
        file: None,
        pattern: Some("two".to_string()),
        is_regex: None,
        since_index: None,
        live: Some(false),
    });
    let log_results = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::QueryLogResults {
                query_id: id, ..
            } if *id == query_id
        )
    })
    .expect("did not receive QueryLogResults for reloaded splash");

    match log_results {
        HubToClient::QueryLogResults { entries, done, .. } => {
            assert!(done);
            assert!(
                entries.iter().any(|entry| entry.1.message.contains("two")),
                "expected updated splash logs"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn file_watch_emits_single_path_delta_without_full_tree_reload() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(ClientToHub::SaveTextFile {
        path: "repo/src/lib.rs".to_string(),
        content: "pub fn hi() { let _x = 1; }\n".to_string(),
    });

    let diff_msg = wait_for_message(&connection, Duration::from_secs(5), |msg| {
        matches!(
            msg,
            HubToClient::FileTreeDiff { mount, changes }
                if mount == "repo"
                    && changes.len() == 1
                    && matches!(
                        &changes[0],
                        makepad_studio_protocol::hub_protocol::FileTreeChange::Added { path, .. }
                            | makepad_studio_protocol::hub_protocol::FileTreeChange::Modified { path, .. }
                            | makepad_studio_protocol::hub_protocol::FileTreeChange::Removed { path }
                            if path.starts_with("repo/")
                    )
        )
    })
    .expect("did not receive path-scoped filetree delta");

    match diff_msg {
        HubToClient::FileTreeDiff { changes, .. } => {
            assert_eq!(changes.len(), 1);
        }
        _ => unreachable!(),
    }

    let trailing = drain_messages(&connection, Duration::from_millis(500));
    assert!(
        !trailing
            .iter()
            .any(|msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo")),
        "unexpected full file-tree reload after delta update"
    );
}

#[test]
fn save_text_file_does_not_echo_file_changed_to_saving_client() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn hi() {}\n").unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(ClientToHub::SaveTextFile {
        path: "repo/src/lib.rs".to_string(),
        content: "pub fn hi() { let _x = 7; }\n".to_string(),
    });

    let messages = drain_messages(&connection, Duration::from_millis(1200));
    assert!(
        messages.iter().any(|msg| {
            matches!(
                msg,
                HubToClient::TextFileSaved { path, .. } if path == "repo/src/lib.rs"
            )
        }),
        "did not receive TextFileSaved after save request"
    );
    assert!(
        !messages.iter().any(|msg| {
            matches!(
                msg,
                HubToClient::FileChanged { path } if path == "repo/src/lib.rs"
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(ClientToHub::SaveTextFile {
        path: "repo/.makepad/a.term".to_string(),
        content: "echo hi\n".to_string(),
    });

    let messages = drain_messages(&connection, Duration::from_millis(900));
    assert!(
        !messages.iter().any(|msg| {
            matches!(msg, HubToClient::FileTreeDiff { mount, .. } if mount == "repo")
        }),
        "expected .makepad terminal writes to be ignored by fs delta watcher"
    );
}

#[test]
fn file_watch_emits_hidden_directory_writes() {
    let dir = tempfile::tempdir().unwrap();

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    let _ = connection.send(ClientToHub::SaveTextFile {
        path: "repo/.hidden/a.txt".to_string(),
        content: "hello\n".to_string(),
    });

    let diff = wait_for_message(&connection, Duration::from_secs(5), |msg| {
        matches!(
            msg,
            HubToClient::FileTreeDiff { mount, changes }
                if mount == "repo"
                    && changes.iter().any(|change| {
                        matches!(
                            change,
                            makepad_studio_protocol::hub_protocol::FileTreeChange::Added { path, .. }
                                | makepad_studio_protocol::hub_protocol::FileTreeChange::Modified { path, .. }
                                | makepad_studio_protocol::hub_protocol::FileTreeChange::Removed { path }
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
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
            HubToClient::FileTreeDiff { mount, changes } if mount == "repo" => {
                if changes.iter().any(|change| {
                    matches!(
                        change,
                        makepad_studio_protocol::hub_protocol::FileTreeChange::Added { path, .. }
                            if path == "repo/src/new_file.rs"
                    )
                }) {
                    saw_new_file = true;
                    break;
                }
            }
            HubToClient::FileTree { mount, data } if mount == "repo" => {
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
    )
    .expect("did not receive initial file tree");

    // External write (not via SaveTextFile) should still notify UI clients.
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn hi() { let _extern = 1; }\n",
    )
    .unwrap();

    let changed = wait_for_message(&connection, Duration::from_secs(6), |msg| {
        matches!(
            msg,
            HubToClient::FileChanged { path }
                if path == "repo/src/lib.rs" || path == "repo"
        )
    });
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::OpenTextFile {
        path: "repo/src/lib.rs".to_string(),
    });
    let opened = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::TextFileOpened { path, content, .. }
                if path == "repo/src/lib.rs" && content == "pub fn hi() {}\n"
        )
    });
    assert!(opened.is_some(), "did not receive initial TextFileOpened");

    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn hi() { let external = 1; }\n",
    )
    .unwrap();

    let _ = connection.send(ClientToHub::ReadTextFile {
        path: "repo/src/lib.rs".to_string(),
    });
    let read = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::TextFileRead { path, content }
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::LoadFileTree {
        mount: "repo".to_string(),
    });
    let _ = wait_for_message(
        &connection,
        Duration::from_secs(3),
        |msg| matches!(msg, HubToClient::FileTree { mount, .. } if mount == "repo"),
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
            HubToClient::FileTreeDiff { mount, changes } if mount == "repo" => {
                if changes.iter().any(|change| {
                    matches!(
                        change,
                        makepad_studio_protocol::hub_protocol::FileTreeChange::Removed { path }
                            if path == "repo/src/nested" || path.starts_with("repo/src/nested/")
                    )
                }) {
                    saw_nested_removed = true;
                    break;
                }
            }
            HubToClient::FileTree { mount, data } if mount == "repo" => {
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let query_id = connection.send(ClientToHub::FindInFiles {
        mount: Some("repo".to_string()),
        pattern: "needle".to_string(),
        is_regex: Some(false),
        glob: None,
        max_results: Some(100),
    });

    let msg = wait_for_message(&connection, Duration::from_secs(4), |msg| {
        matches!(
            msg,
            HubToClient::SearchFileResults { query_id: id, done: true, .. } if *id == query_id
        )
    })
    .expect("did not receive SearchFileResults");

    match msg {
        HubToClient::SearchFileResults { results, done, .. } => {
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let query_id = connection.send(ClientToHub::FindInFiles {
        mount: Some("repo".to_string()),
        pattern: "needle".to_string(),
        is_regex: Some(true),
        glob: None,
        max_results: Some(1),
    });

    let msg = wait_for_message(&connection, Duration::from_secs(4), |msg| {
        matches!(
            msg,
            HubToClient::SearchFileResults { query_id: id, done: true, .. } if *id == query_id
        )
    })
    .expect("did not receive SearchFileResults");

    match msg {
        HubToClient::SearchFileResults { results, done, .. } => {
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

    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: dir.path().to_path_buf(),
        }],
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start in-process backend");

    let _ = connection.send(ClientToHub::ReadTextRange {
        path: "repo/src/lib.rs".to_string(),
        start_line: 2,
        end_line: 3,
    });

    let msg = wait_for_message(&connection, Duration::from_secs(3), |msg| {
        matches!(
            msg,
            HubToClient::TextFileRange {
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
