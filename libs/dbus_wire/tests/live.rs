#![cfg(unix)]
use makepad_dbus_wire::*;
use std::time::{Duration, Instant};
const TIMEOUT: Duration = Duration::from_secs(5);
const INTERFACE: &str = "org.example.WireTest";

struct Exported {
    count: u32,
}
impl ObjectHandler for Exported {
    fn call(&mut self, call: MethodCall<'_>) -> MethodResult<Body> {
        match (call.interface, call.member) {
            (INTERFACE, "Echo") => Ok(call.args.clone()),
            (INTERFACE, "Path") => Ok(Body::from_values(&[Value::Path(call.path.into())])?),
            _ => Err(MethodError::unknown_method()),
        }
    }
    fn introspect(&self, _: &str) -> String {
        "<node><interface name=\"org.example.WireTest\"><method name=\"Echo\"/></interface></node>"
            .into()
    }
    fn get_property(&mut self, _: &str, interface: &str, name: &str) -> MethodResult<Value> {
        if (interface, name) == (INTERFACE, "Count") {
            Ok(Value::U32(self.count))
        } else {
            Err(MethodError::invalid_args("Unknown property"))
        }
    }
    fn get_all_properties(
        &mut self,
        _: &str,
        interface: &str,
    ) -> MethodResult<Vec<(String, Value)>> {
        if interface != INTERFACE {
            return Err(MethodError::invalid_args("Unknown interface"));
        }
        Ok(vec![("Count".into(), Value::U32(self.count))])
    }
    fn set_property(
        &mut self,
        _: &str,
        interface: &str,
        name: &str,
        value: Value,
    ) -> MethodResult<()> {
        if let (INTERFACE, "Count", Value::U32(v)) = (interface, name, value) {
            self.count = v;
            Ok(())
        } else {
            Err(MethodError::invalid_args("Expected Count u32"))
        }
    }
}

fn pump(server: &mut Connection, objects: &mut ObjectServer, client: &mut Connection) {
    client.flush().unwrap();
    server.read_ready().unwrap();
    while let Some(message) = server.pop_message() {
        if let Some(reply) = objects.dispatch(&message).unwrap() {
            server.send(reply).unwrap();
        }
    }
    server.flush().unwrap();
    client.read_ready().unwrap();
}
fn remote_call(
    server: &mut Connection,
    objects: &mut ObjectServer,
    client: &mut Connection,
    path: &str,
    interface: &str,
    member: &str,
    args: &[Value],
) -> Result<Message> {
    let destination = server.unique_name().unwrap().to_owned();
    let serial = client.send(Message::method_call(
        &destination,
        path,
        interface,
        member,
        Body::from_values(args)?,
    ))?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        pump(server, objects, client);
        if let Some(reply) = client.take_reply(serial) {
            return reply.into_result();
        }
        assert!(Instant::now() < deadline, "live call timed out: {member}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn live_session_bus_and_exported_object() {
    if std::env::var("DBUS_WIRE_LIVE").as_deref() != Ok("1") {
        eprintln!("live bus test skipped; set DBUS_WIRE_LIVE=1 or use tests/live.sh");
        return;
    }
    let mut client = Connection::session(TIMEOUT).unwrap();
    let names = client
        .call(
            Message::method_call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ListNames",
                Body::default(),
            ),
            TIMEOUT,
        )
        .unwrap();
    let names = names.body.values().unwrap();
    assert!(
        matches!(&names[0], Value::Array { values, .. } if values.iter().any(|v| v.as_str() == Some("org.freedesktop.DBus")))
    );
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    // Exercise fallback from unsupported transport, and arbitrary-address connect.
    let mut server = Connection::connect(&format!("unsupported:;{address}"), TIMEOUT).unwrap();
    let name = format!("org.example.WireTest.p{}", std::process::id());
    assert_eq!(server.request_name(&name, 4, TIMEOUT).unwrap(), 1);
    let mut objects = ObjectServer::new();
    objects
        .register_prefix("/objects", Exported { count: 1 })
        .unwrap();
    objects.register("/exact", Exported { count: 8 }).unwrap();
    let args = [
        Value::Struct(vec![Value::Str("round trip".into()), Value::I64(-12)]),
        Value::Array {
            element: "{sv}".into(),
            values: vec![Value::DictEntry(Box::new((
                Value::Str("value".into()),
                Value::Variant(Box::new(Value::U32(42))),
            )))],
        },
    ];
    let reply = remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/objects/17",
        INTERFACE,
        "Echo",
        &args,
    )
    .unwrap();
    assert_eq!(reply.body.values().unwrap(), args);
    let reply = remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/objects/17",
        INTERFACE,
        "Path",
        &[],
    )
    .unwrap();
    assert_eq!(reply.body.reader().object_path().unwrap(), "/objects/17");
    for path in ["/exact", "/objects/17"] {
        remote_call(
            &mut server,
            &mut objects,
            &mut client,
            path,
            "org.freedesktop.DBus.Peer",
            "Ping",
            &[],
        )
        .unwrap();
    }
    let reply = remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/exact",
        "org.freedesktop.DBus.Introspectable",
        "Introspect",
        &[],
    )
    .unwrap();
    assert!(reply.body.reader().str().unwrap().contains(INTERFACE));
    let property = [Value::Str(INTERFACE.into()), Value::Str("Count".into())];
    let mut set = property.to_vec();
    set.push(Value::Variant(Box::new(Value::U32(25))));
    remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/exact",
        "org.freedesktop.DBus.Properties",
        "Set",
        &set,
    )
    .unwrap();
    let reply = remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/exact",
        "org.freedesktop.DBus.Properties",
        "Get",
        &property,
    )
    .unwrap();
    assert_eq!(
        reply.body.values().unwrap(),
        [Value::Variant(Box::new(Value::U32(25)))]
    );
    let reply = remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/exact",
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &property[..1],
    )
    .unwrap();
    assert_eq!(reply.body.signature(), "a{sv}");
    let error = remote_call(
        &mut server,
        &mut objects,
        &mut client,
        "/exact",
        INTERFACE,
        "Missing",
        &[],
    )
    .unwrap_err();
    assert!(matches!(error, Error::Remote { name, .. } if name.ends_with("UnknownMethod")));
    let rule = format!("type='signal',sender='{name}',interface='{INTERFACE}',member='Changed'");
    client.add_match(&rule, TIMEOUT).unwrap();
    server
        .emit_signal(
            "/exact",
            INTERFACE,
            "Changed",
            Body::from_values(&[Value::U32(25)]).unwrap(),
        )
        .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        pump(&mut server, &mut objects, &mut client);
        let mut found = false;
        while let Some(message) = client.pop_message() {
            if message.header.kind == MessageType::Signal
                && message.header.member.as_deref() == Some("Changed")
            {
                assert_eq!(message.body.reader().u32().unwrap(), 25);
                found = true;
            }
        }
        if found {
            break;
        }
        assert!(Instant::now() < deadline, "signal not delivered");
        std::thread::sleep(Duration::from_millis(1));
    }
    client.remove_match(&rule, TIMEOUT).unwrap();
    // The daemon must answer both connections even after all exported interactions.
    client
        .call(
            Message::method_call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "ListNames",
                Body::default(),
            ),
            TIMEOUT,
        )
        .unwrap();
    eprintln!("live ListNames, RequestName, two connections, Echo, paths, Properties, Introspect, Ping, errors and match/signal round trip passed");
}
