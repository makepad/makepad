use makepad_studio_hub::{HubConfig, MountConfig, StudioHub};
use makepad_studio_protocol::hub_protocol::{AiMessageRole, AiMountState, ClientToHub, HubToClient};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

fn ai_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: String) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    let mut request = Vec::new();
    let mut buf = [0u8; 4096];
    let mut header_end = None;
    let mut content_length = 0usize;

    loop {
        let read = stream.read(&mut buf).expect("read request bytes");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buf[..read]);
        if header_end.is_none() {
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let end = pos + 4;
                header_end = Some(end);
                let headers = String::from_utf8_lossy(&request[..end]);
                for line in headers.lines() {
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
        }
        if let Some(end) = header_end {
            if request.len() >= end + content_length {
                break;
            }
        }
    }

    String::from_utf8_lossy(&request).to_string()
}

fn write_chunked_sse(stream: &mut std::net::TcpStream, chunks: &[&str]) {
    let headers = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/event-stream\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Connection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes()).expect("write sse headers");
    stream.flush().expect("flush sse headers");
    for chunk in chunks {
        let bytes = chunk.as_bytes();
        let prefix = format!("{:X}\r\n", bytes.len());
        stream
            .write_all(prefix.as_bytes())
            .expect("write sse chunk prefix");
        stream.write_all(bytes).expect("write sse chunk body");
        stream.write_all(b"\r\n").expect("write sse chunk trailer");
        stream.flush().expect("flush sse chunk");
        thread::sleep(Duration::from_millis(60));
    }
    stream.write_all(b"0\r\n\r\n").expect("write sse terminator");
    stream.flush().expect("flush sse terminator");
}

fn write_chunked_sse_and_hold_open(
    mut stream: std::net::TcpStream,
    chunks: &[&str],
    hold_open_for: Duration,
) {
    write_chunked_sse(&mut stream, chunks);
    thread::sleep(hold_open_for);
}

fn wait_for_ai_state(
    connection: &makepad_studio_hub::HubConnection,
    mount: &str,
    timeout: Duration,
    predicate: impl Fn(&AiMountState) -> bool,
) -> AiMountState {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let Some(msg) = connection.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        if let HubToClient::AiMountState { mount: got, state } = msg {
            if got == mount && predicate(&state) {
                return state;
            }
        }
    }
    panic!("timed out waiting for AiMountState for mount {}", mount);
}

#[test]
fn ai_manager_round_trips_prompt_through_local_backend() {
    let _env_lock = ai_env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local ai server");
    let addr = listener.local_addr().expect("local addr");
    let _base_url = EnvGuard::set(
        "MAKEPAD_STUDIO_AI_BASE_URL",
        format!("http://{}/v1/chat/completions", addr),
    );

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept ai request");
        let request_text = read_http_request(&mut stream);
        assert!(request_text.contains("POST /v1/chat/completions"));
        assert!(request_text.contains("\"stream\":true"));
        write_chunked_sse(
            &mut stream,
            &[
                "data: {\"choices\":[{\"delta\":{\"content\":\"assistant reply\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
    });

    let root = std::env::current_dir().expect("current_dir");
    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: root,
        }],
        enable_in_process_gateway: false,
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start backend");

    let _ = connection.send(ClientToHub::AiGetState {
        mount: "repo".to_string(),
    });
    let initial = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state.active_agent.is_some()
    });
    let agent_id = initial.active_agent_id.expect("default ai agent");

    let _ = connection.send(ClientToHub::AiSetBackend {
        mount: "repo".to_string(),
        backend_id: "openai_localhost".to_string(),
    });
    let _ = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state.active_backend_id.as_deref() == Some("openai_localhost")
    });

    let _ = connection.send(ClientToHub::AiSendPrompt {
        mount: "repo".to_string(),
        agent_id,
        text: "hello from test".to_string(),
    });

    let _pending = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| agent.pending)
            .unwrap_or(false)
    });

    let done = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                !agent.pending
                    && agent.messages.iter().any(|message| message.text == "assistant reply")
            })
            .unwrap_or(false)
    });

    let messages = &done.active_agent.expect("active agent").messages;
    assert!(messages.iter().any(|message| message.text == "hello from test"));
    assert!(messages.iter().any(|message| message.text == "assistant reply"));

    server.join().expect("join ai server");
}

#[test]
fn ai_manager_executes_tool_calls_inside_the_hub() {
    let _env_lock = ai_env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local ai server");
    let addr = listener.local_addr().expect("local addr");
    let _base_url = EnvGuard::set(
        "MAKEPAD_STUDIO_AI_BASE_URL",
        format!("http://{}/v1/chat/completions", addr),
    );

    let server = thread::spawn(move || {
        let (mut stream1, _) = listener.accept().expect("accept first ai request");
        let request1 = read_http_request(&mut stream1);
        assert!(request1.contains("\"tools\""));
        assert!(request1.contains("\"read_file\""));
        assert!(request1.contains("\"stream\":true"));
        write_chunked_sse(
            &mut stream1,
            &[
                concat!(
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,",
                    "\"id\":\"call_1\",\"type\":\"function\",\"function\":{",
                    "\"name\":\"read_file\",",
                    "\"arguments\":\"{\\\"path\\\":\\\"Cargo.toml\\\",\\\"offset\\\":1,\\\"limit\\\":2}\"",
                    "}}]},\"finish_reason\":\"tool_calls\"}]}\n\n"
                ),
                "data: [DONE]\n\n",
            ],
        );

        let (mut stream2, _) = listener.accept().expect("accept second ai request");
        let request2 = read_http_request(&mut stream2);
        assert!(request2.contains("\"role\":\"tool\""));
        assert!(request2.contains("\"tool_call_id\":\"call_1\""));
        assert!(request2.contains("Cargo.toml"));
        write_chunked_sse(
            &mut stream2,
            &[
                "data: {\"choices\":[{\"delta\":{\"content\":\"finished after tool call\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
    });

    let root = std::env::current_dir().expect("current_dir");
    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: root,
        }],
        enable_in_process_gateway: false,
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start backend");

    let _ = connection.send(ClientToHub::AiGetState {
        mount: "repo".to_string(),
    });
    let initial = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state.active_agent.is_some()
    });
    let agent_id = initial.active_agent_id.expect("default ai agent");

    let _ = connection.send(ClientToHub::AiSendPrompt {
        mount: "repo".to_string(),
        agent_id,
        text: "inspect Cargo.toml".to_string(),
    });

    let done = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                !agent.pending
                    && agent
                        .messages
                        .iter()
                        .any(|message| message.text.contains("finished after tool call"))
            })
            .unwrap_or(false)
    });

    let messages = &done.active_agent.expect("active agent").messages;
    assert!(messages.iter().any(|message| message.text.contains("read_file")));
    assert!(messages
        .iter()
        .any(|message| message.text.contains("finished after tool call")));

    server.join().expect("join ai server");
}

#[test]
fn ai_manager_streams_thinking_before_completion() {
    let _env_lock = ai_env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local ai server");
    let addr = listener.local_addr().expect("local addr");
    let _base_url = EnvGuard::set(
        "MAKEPAD_STUDIO_AI_BASE_URL",
        format!("http://{}/v1/chat/completions", addr),
    );

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept ai request");
        let request_text = read_http_request(&mut stream);
        assert!(request_text.contains("\"stream\":true"));
        write_chunked_sse(
            &mut stream,
            &[
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking now\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
    });

    let root = std::env::current_dir().expect("current_dir");
    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: root,
        }],
        enable_in_process_gateway: false,
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start backend");

    let _ = connection.send(ClientToHub::AiGetState {
        mount: "repo".to_string(),
    });
    let initial = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state.active_agent.is_some()
    });
    let agent_id = initial.active_agent_id.expect("default ai agent");

    let _ = connection.send(ClientToHub::AiSendPrompt {
        mount: "repo".to_string(),
        agent_id,
        text: "stream it".to_string(),
    });

    let thinking = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                agent.pending
                    && agent.messages.iter().any(|message| {
                        matches!(message.role, AiMessageRole::Thinking)
                            && message.text.contains("thinking now")
                    })
            })
            .unwrap_or(false)
    });
    assert!(thinking
        .active_agent
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .any(|message| matches!(message.role, AiMessageRole::Thinking)));

    let done = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                !agent.pending
                    && agent.messages.iter().any(|message| {
                        matches!(message.role, AiMessageRole::Assistant)
                            && message.text.contains("hello")
                    })
            })
            .unwrap_or(false)
    });
    assert!(done
        .active_agent
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .any(|message| matches!(message.role, AiMessageRole::Assistant)));

    server.join().expect("join ai server");
}

#[test]
fn ai_manager_preserves_streamed_thinking_whitespace() {
    let _env_lock = ai_env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local ai server");
    let addr = listener.local_addr().expect("local addr");
    let _base_url = EnvGuard::set(
        "MAKEPAD_STUDIO_AI_BASE_URL",
        format!("http://{}/v1/chat/completions", addr),
    );

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept ai request");
        let request_text = read_http_request(&mut stream);
        assert!(request_text.contains("\"stream\":true"));
        write_chunked_sse(
            &mut stream,
            &[
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"The\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\" user\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\" says hi\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );
    });

    let root = std::env::current_dir().expect("current_dir");
    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: root,
        }],
        enable_in_process_gateway: false,
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start backend");

    let _ = connection.send(ClientToHub::AiGetState {
        mount: "repo".to_string(),
    });
    let initial = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state.active_agent.is_some()
    });
    let agent_id = initial.active_agent_id.expect("default ai agent");

    let _ = connection.send(ClientToHub::AiSendPrompt {
        mount: "repo".to_string(),
        agent_id,
        text: "say hi".to_string(),
    });

    let done = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                !agent.pending
                    && agent.messages.iter().any(|message| {
                        matches!(message.role, AiMessageRole::Thinking)
                            && message.text.contains("The user says hi")
                    })
            })
            .unwrap_or(false)
    });

    assert!(done
        .active_agent
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .any(|message| {
            matches!(message.role, AiMessageRole::Thinking)
                && message.text.contains("The user says hi")
        }));

    server.join().expect("join ai server");
}

#[test]
fn ai_manager_accepts_second_prompt_after_done_before_socket_close() {
    let _env_lock = ai_env_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local ai server");
    let addr = listener.local_addr().expect("local addr");
    let _base_url = EnvGuard::set(
        "MAKEPAD_STUDIO_AI_BASE_URL",
        format!("http://{}/v1/chat/completions", addr),
    );

    let server = thread::spawn(move || {
        let (stream1, _) = listener.accept().expect("accept first ai request");
        let mut stream1_reader = stream1.try_clone().expect("clone first stream");
        let request1 = read_http_request(&mut stream1_reader);
        assert!(request1.contains("\"stream\":true"));
        let first_handler = thread::spawn(move || {
            write_chunked_sse_and_hold_open(
                stream1,
                &[
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hi reasoning\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hi!\"}}]}\n\n",
                    "data: [DONE]\n\n",
                ],
                Duration::from_secs(2),
            );
        });

        let (mut stream2, _) = listener.accept().expect("accept second ai request");
        let request2 = read_http_request(&mut stream2);
        assert!(request2.contains("write a poem"));
        write_chunked_sse(
            &mut stream2,
            &[
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"poem reasoning\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Roses are red\"}}]}\n\n",
                "data: [DONE]\n\n",
            ],
        );

        first_handler.join().expect("join first handler");
    });

    let root = std::env::current_dir().expect("current_dir");
    let config = HubConfig {
        mounts: vec![MountConfig {
            name: "repo".to_string(),
            path: root,
        }],
        enable_in_process_gateway: false,
        ..Default::default()
    };
    let mut connection = StudioHub::start_in_process(config).expect("start backend");

    let _ = connection.send(ClientToHub::AiGetState {
        mount: "repo".to_string(),
    });
    let initial = wait_for_ai_state(&connection, "repo", Duration::from_secs(2), |state| {
        state.active_agent.is_some()
    });
    let agent_id = initial.active_agent_id.expect("default ai agent");

    let _ = connection.send(ClientToHub::AiSendPrompt {
        mount: "repo".to_string(),
        agent_id,
        text: "say hi".to_string(),
    });

    let first_done = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                !agent.pending
                    && agent
                        .messages
                        .iter()
                        .any(|message| matches!(message.role, AiMessageRole::Assistant) && message.text.contains("Hi!"))
            })
            .unwrap_or(false)
    });
    assert!(!first_done.active_agent.as_ref().unwrap().pending);

    let _ = connection.send(ClientToHub::AiSendPrompt {
        mount: "repo".to_string(),
        agent_id,
        text: "write a poem".to_string(),
    });

    let second_done = wait_for_ai_state(&connection, "repo", Duration::from_secs(5), |state| {
        state
            .active_agent
            .as_ref()
            .map(|agent| {
                !agent.pending
                    && agent.messages.iter().any(|message| {
                        matches!(message.role, AiMessageRole::Assistant)
                            && message.text.contains("Roses are red")
                    })
            })
            .unwrap_or(false)
    });

    assert!(second_done
        .active_agent
        .as_ref()
        .unwrap()
        .messages
        .iter()
        .any(|message| {
            matches!(message.role, AiMessageRole::Assistant)
                && message.text.contains("Roses are red")
        }));

    server.join().expect("join ai server");
}
