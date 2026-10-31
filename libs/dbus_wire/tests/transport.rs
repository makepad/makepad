#![cfg(unix)]
use makepad_dbus_wire::*;
use std::{
    io::Write,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

fn pair() -> (Connection, Connection) {
    let (a, b) = UnixStream::pair().unwrap();
    (
        Connection::from_authenticated_stream(a).unwrap(),
        Connection::from_authenticated_stream(b).unwrap(),
    )
}
fn call() -> Message {
    Message::method_call("", "/object", "org.example.Test", "Echo", Body::default())
}

#[test]
fn addresses_and_auth_lines() {
    assert_eq!(
        parse_address("unix:path=/tmp/a%20b,guid=ignored").unwrap(),
        Address::Path("/tmp/a b".into())
    );
    assert_eq!(
        parse_address("unix:abstract=bus%2fname").unwrap(),
        Address::Abstract(b"bus/name".to_vec())
    );
    for bad in [
        "tcp:host=example",
        "unix:path=%",
        "unix:path=%xz",
        "unix:path=%00",
        "unix:path=",
        "unix:guid=hi",
        "unix:path=a,abstract=b",
    ] {
        assert!(parse_address(bad).is_err(), "{bad}");
    }
    assert_eq!(
        parse_auth_line(b"OK 0123456789abcdef0123456789ABCDEF\r\n").unwrap(),
        AuthReply::Ok("0123456789abcdef0123456789ABCDEF".into())
    );
    assert_eq!(
        parse_auth_line(b"DATA\r\n").unwrap(),
        AuthReply::Data("".into())
    );
    assert_eq!(
        parse_auth_line(b"REJECTED EXTERNAL ANONYMOUS\r\n").unwrap(),
        AuthReply::Rejected(vec!["EXTERNAL".into(), "ANONYMOUS".into()])
    );
    assert_eq!(
        parse_auth_line(b"ERROR failed\r\n").unwrap(),
        AuthReply::Error("failed".into())
    );
    for bad in [
        b"OK x\r\n".as_slice(),
        b"OK\n",
        b"DATA 1\r\n",
        b"DATA zz\r\n",
        b"ERROR x\ny\r\n",
        b"BEGIN\r\n",
    ] {
        assert!(parse_auth_line(bad).is_err());
    }
}

#[test]
fn fragmented_frames_and_eof_preserve_complete_messages() {
    let (mut socket, client) = UnixStream::pair().unwrap();
    let mut client = Connection::from_authenticated_stream(client).unwrap();
    let mut message = call();
    message.header.serial = 20;
    let bytes = message.encode().unwrap();
    assert_eq!(client.read_ready().unwrap(), 0);
    for byte in &bytes[..bytes.len() - 1] {
        socket.write_all(&[*byte]).unwrap();
        assert_eq!(client.read_ready().unwrap(), 0);
    }
    socket.write_all(&bytes[bytes.len() - 1..]).unwrap();
    socket.write_all(&bytes).unwrap();
    drop(socket);
    assert_eq!(client.read_ready().unwrap(), 2);
    assert!(client.is_closed());
    assert_eq!(client.pop_message().unwrap(), message);
    assert_eq!(client.pop_message().unwrap(), message);
    assert!(client.read_ready().is_err());
}

#[test]
fn partial_writes_retain_data_and_reply_matching_retains_other_messages() {
    let (mut a, mut b) = pair();
    let payload = "x".repeat(2 * 1024 * 1024);
    let mut request = call();
    request.body = Body::from_values(&[Value::Str(payload.clone())]).unwrap();
    let serial = a.send(request).unwrap();
    a.flush().unwrap();
    assert!(a.wants_write(), "payload must exercise socket backpressure");
    let limit = Instant::now() + Duration::from_secs(5);
    let request = loop {
        assert!(Instant::now() < limit);
        b.read_ready().unwrap();
        a.flush().unwrap();
        if let Some(message) = b.pop_message() {
            break message;
        }
    };
    assert_eq!(request.body.reader().str().unwrap(), payload);
    assert!(!a.wants_write());
    b.emit_signal("/object", "org.example.Test", "Changed", Body::default())
        .unwrap();
    let mut unrelated = Message::method_return(&request, Body::default());
    unrelated.header.reply_serial = Some(serial + 1);
    b.send(unrelated).unwrap();
    b.send(Message::method_return(&request, Body::default()))
        .unwrap();
    b.flush().unwrap();
    assert_eq!(a.read_ready().unwrap(), 3);
    assert_eq!(
        a.take_reply(serial).unwrap().header.reply_serial,
        Some(serial)
    );
    assert_eq!(a.pop_message().unwrap().header.kind, MessageType::Signal);
    assert_eq!(
        a.take_reply(serial + 1).unwrap().header.reply_serial,
        Some(serial + 1)
    );
}

#[test]
fn blocking_timeout_is_bounded_and_does_not_discard_late_reply() {
    let (mut a, mut b) = pair();
    let start = Instant::now();
    assert!(matches!(
        a.call(call(), Duration::from_millis(20)),
        Err(Error::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    b.read_ready().unwrap();
    let request = b.pop_message().unwrap();
    b.send(Message::method_return(&request, Body::default()))
        .unwrap();
    b.flush().unwrap();
    a.read_ready().unwrap();
    assert!(a.take_reply(request.header.serial).is_some());
    assert!(matches!(
        a.call(call(), Duration::ZERO),
        Err(Error::Timeout)
    ));
}

#[test]
fn blocking_call_keeps_signals_and_matches_prequeued_reply() {
    let (mut a, mut b) = pair();
    let mut anticipated = call();
    anticipated.header.serial = 1;
    b.emit_signal("/object", "org.example.Test", "Changed", Body::default())
        .unwrap();
    b.send(Message::method_return(
        &anticipated,
        Body::from_values(&[Value::U32(8)]).unwrap(),
    ))
    .unwrap();
    b.flush().unwrap();
    let reply = a.call(call(), Duration::from_secs(1)).unwrap();
    assert_eq!(reply.body.reader().u32().unwrap(), 8);
    assert_eq!(a.pop_message().unwrap().header.kind, MessageType::Signal);
}

#[test]
fn future_message_types_are_skipped_and_protocol_errors_poison_connection() {
    let (mut socket, stream) = UnixStream::pair().unwrap();
    let mut client = Connection::from_authenticated_stream(stream).unwrap();
    let mut message = call();
    message.header.serial = 1;
    let bytes = message.encode().unwrap();
    let mut future = bytes.clone();
    future[1] = 5;
    socket.write_all(&future).unwrap();
    socket.write_all(&bytes).unwrap();
    assert_eq!(client.read_ready().unwrap(), 1);
    assert_eq!(client.pop_message().unwrap(), message);
    let mut bad = bytes.clone();
    bad[16] = 0;
    socket.write_all(&bytes).unwrap();
    socket.write_all(&bad).unwrap();
    assert!(client.read_ready().is_err());
    assert!(client.is_closed());
    assert!(client.pop_message().is_some());
    assert!(client.read_ready().is_err());
    assert!(client.pop_message().is_none());
    assert!(client.send(call()).is_err());
}
