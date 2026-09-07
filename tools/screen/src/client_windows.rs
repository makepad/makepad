//! Windows attachment shares wire framing, bounded queues and the literal
//! paste/detach decoder with Unix. Only console and transport I/O differ.
use super::{
    clean_message,
    windows_console::{Console, ConsoleIo},
    DisplayColors, InputDecoder, Queue, StartOptions, INPUT_CHUNK, INPUT_LIMIT, OUTPUT_LIMIT,
    RESTORE_VT,
};
use crate::{
    protocol::{self, SessionLocation},
    windows::{self, Stream},
};
use makepad_strict_json::{self as json, Value};
use std::{
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

pub fn attach(location: &SessionLocation, read_only: bool) -> Result<(), String> {
    attach_inner(location, read_only, None)
}

pub fn start_attached(options: StartOptions, restart: bool) -> Result<(), String> {
    let location = SessionLocation::open(&options.state_dir, &options.session_id, true)?;
    attach_inner(&location, false, Some((options, restart)))
}

fn attach_inner(
    location: &SessionLocation,
    read_only: bool,
    start: Option<(StartOptions, bool)>,
) -> Result<(), String> {
    let mut console =
        Console::new().map_err(|error| format!("Prepare Windows console: {error}"))?;
    let mut io = console
        .io()
        .map_err(|error| format!("Start console I/O: {error}"))?;
    let mut colors = DisplayColors::default();
    let result = (|| {
        if console.is_terminal() {
            let probe_io = std::cell::RefCell::new(&mut io);
            colors.capture(
                || probe_io.borrow_mut().read(),
                |bytes| probe_io.borrow_mut().write(bytes),
                || console.interrupted().is_some(),
            )?;
        }
        if let Some((mut options, restart)) = start {
            options.theme.merge_missing(&colors.theme);
            (options.cols, options.rows) = console.size().map_err(|e| e.to_string())?;
            crate::server::start(options, restart)?;
        }
        if !location.validate_socket()? {
            return Err(
                "Screen session has no live endpoint; attach never starts a replacement".into(),
            );
        }
        let stream = windows::connect(location, Duration::from_secs(3))
            .map_err(|error| format!("Connect screen: {error}"))?;
        attach_loop(stream, location, read_only, &console, &mut io, &mut colors)
    })();
    let presentation = if console.is_terminal() {
        let mut restore = RESTORE_VT.to_vec();
        restore.extend(colors.restore());
        io.restore_vt(&restore)
    } else {
        Ok(())
    };
    let workers = io.shutdown();
    let modes = console.restore();
    for (name, restored) in [
        ("terminal presentation", presentation),
        ("console workers", workers),
        ("console mode", modes),
    ] {
        if let Err(error) = restored {
            return Err(format!(
                "Could not restore {name}: {error}; attach outcome: {}",
                result
                    .as_ref()
                    .err()
                    .map(String::as_str)
                    .unwrap_or("detached")
            ));
        }
    }
    match result {
        Ok(reason) => {
            eprintln!("makepad-screen: {} · {reason}", location.session_id);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn attach_loop(
    mut stream: Stream,
    location: &SessionLocation,
    read_only: bool,
    console: &Console,
    io: &mut ConsoleIo,
    colors: &mut DisplayColors,
) -> Result<&'static str, String> {
    let mut size = console.size().map_err(|error| error.to_string())?;
    let hello = json::obj(vec![
        ("version", Value::Int(protocol::VERSION as i64)),
        ("session_id", json::s(&location.session_id)),
        ("read_only", Value::Bool(read_only)),
        ("cols", Value::Int(size.0 as i64)),
        ("rows", Value::Int(size.1 as i64)),
    ])
    .to_json();
    let mut outgoing = Queue::new(INPUT_LIMIT);
    outgoing.push(protocol::encode_frame(protocol::HELLO, hello.as_bytes())?)?;
    let mut output = Queue::new(OUTPUT_LIMIT);
    let mut input = InputDecoder::default();
    let (initial_input, detached) = input.push(&std::mem::take(&mut colors.initial_input));
    if detached {
        return Ok("detached; session kept running");
    }
    if !read_only && !initial_input.is_empty() {
        outgoing.push(protocol::encode_frame(protocol::INPUT, &initial_input)?)?;
    }
    let mut received = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    let started = Instant::now();
    let mut last_size = started;
    let mut ready = false;
    let mut closing: Option<(Result<&'static str, String>, Instant)> = None;
    loop {
        if let Some(event) = console.interrupted() {
            return Err(format!(
                "Console control event {event}; detached session was not stopped"
            ));
        }
        io.error()
            .map_err(|error| format!("Console output: {error}"))?;
        if closing.is_none() && !ready && started.elapsed() > Duration::from_secs(5) {
            return Err("Screen did not send an initial snapshot within five seconds".into());
        }
        if closing.is_none() && last_size.elapsed() >= Duration::from_millis(100) {
            last_size = Instant::now();
            let next = console.size().map_err(|error| error.to_string())?;
            if next != size && outgoing.remaining() >= 256 {
                let dimensions = json::obj(vec![
                    ("cols", Value::Int(next.0 as i64)),
                    ("rows", Value::Int(next.1 as i64)),
                ])
                .to_json();
                outgoing.push(protocol::encode_frame(
                    protocol::RESIZE,
                    dimensions.as_bytes(),
                )?)?;
                size = next;
            }
        }
        if closing.is_none() && outgoing.remaining() >= 1024 {
            let (bytes, detach) = input.push(&colors.expired_input());
            if !read_only && !bytes.is_empty() {
                outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
            }
            if detach {
                return Ok("detached; session kept running");
            }
            if let Some(bytes) = input.expired_sequence() {
                if !read_only {
                    outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
                }
            }
        }
        while closing.is_none() && output.remaining() >= protocol::MAX_FRAME {
            let Some(frame) = protocol::decode_frame(&mut received)? else {
                break;
            };
            match frame.kind {
                protocol::SNAPSHOT | protocol::OUTPUT => {
                    ready = true;
                    colors.observe_output(&frame.payload);
                    output.push(frame.payload)?;
                }
                protocol::EXIT => {
                    let value = protocol::parse_object(&frame.payload)?;
                    let reason = value
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(clean_message)
                        .unwrap_or_else(|| "session ended".into());
                    let result = match value.get("code") {
                        Some(Value::Int(0)) => Ok("session exited (0)"),
                        Some(Value::Int(code)) => {
                            Err(format!("Session exited with status {code}: {reason}"))
                        }
                        _ => Err(format!(
                            "Session ended without an observed exit status: {reason}"
                        )),
                    };
                    outgoing.clear();
                    closing = Some((result, Instant::now()));
                }
                protocol::ERROR => {
                    outgoing.clear();
                    closing = Some((
                        Err(format!(
                            "Screen: {}",
                            clean_message(&String::from_utf8_lossy(&frame.payload))
                        )),
                        Instant::now(),
                    ));
                }
                kind => return Err(format!("Unexpected frame {kind} on screen attachment")),
            }
        }
        output
            .flush(|bytes| io.write(bytes))
            .map_err(|error| format!("Write console output: {error}"))?;
        outgoing
            .flush(|bytes| stream.write(bytes))
            .map_err(|error| format!("Send screen input: {error}"))?;
        if let Some((_, since)) = &closing {
            if output.is_empty() && outgoing.is_empty() && io.drained() {
                return closing.take().unwrap().0;
            }
            if since.elapsed() > Duration::from_secs(2) {
                return Err(
                    "Attach ended before buffered terminal data could drain within two seconds"
                        .into(),
                );
            }
        }
        if closing.is_none()
            && output.remaining() >= protocol::MAX_FRAME
            && received.len() < protocol::MAX_FRAME + 5
        {
            let room = (protocol::MAX_FRAME + 5)
                .saturating_sub(received.len())
                .min(buffer.len());
            match stream.read(&mut buffer[..room]) {
                Ok(0) => {
                    outgoing.clear();
                    closing = Some((Err("Screen connection closed without an exit report; session state is unknown".into()), Instant::now()));
                }
                Ok(count) => received.extend_from_slice(&buffer[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(format!("Read screen output: {error}")),
            }
        }
        if closing.is_none() && outgoing.remaining() >= INPUT_CHUNK + 1024 {
            if let Some(bytes) = io
                .read()
                .map_err(|error| format!("Read console input: {error}"))?
            {
                if bytes.is_empty() {
                    let bytes = input.flush_sequence();
                    if !read_only && !bytes.is_empty() {
                        outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
                    }
                    closing = Some((Ok("detached (input closed)"), Instant::now()));
                } else {
                    let (bytes, detach) = input.push(&colors.filter_input(&bytes));
                    if !read_only && !bytes.is_empty() {
                        outgoing.push(protocol::encode_frame(protocol::INPUT, &bytes)?)?;
                    }
                    if detach {
                        closing = Some((Ok("detached; session kept running"), Instant::now()));
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}
