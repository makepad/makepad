use makepad_dbus_wire::*;
use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};
struct Object {
    value: Arc<AtomicU32>,
}
impl ObjectHandler for Object {
    fn call(&mut self, call: MethodCall<'_>) -> MethodResult<Body> {
        match (call.interface, call.member) {
            ("org.example.Test", "Echo") => Ok(call.args.clone()),
            ("org.example.Test", "Path") => {
                Ok(Body::from_values(&[Value::Path(call.path.into())])?)
            }
            _ => Err(MethodError::unknown_method()),
        }
    }
    fn introspect(&self, _path: &str) -> String {
        "<node><interface name=\"org.example.Test\"/></node>".into()
    }
    fn get_property(&mut self, _: &str, interface: &str, name: &str) -> MethodResult<Value> {
        if interface != "org.example.Test" || name != "Count" {
            return Err(MethodError::new(
                "org.freedesktop.DBus.Error.UnknownProperty",
                "Unknown",
            ));
        }
        Ok(Value::U32(self.value.load(Ordering::Relaxed)))
    }
    fn get_all_properties(
        &mut self,
        path: &str,
        interface: &str,
    ) -> MethodResult<Vec<(String, Value)>> {
        Ok(vec![(
            "Count".into(),
            self.get_property(path, interface, "Count")?,
        )])
    }
    fn set_property(
        &mut self,
        _: &str,
        interface: &str,
        name: &str,
        value: Value,
    ) -> MethodResult<()> {
        if let ("org.example.Test", "Count", Value::U32(value)) = (interface, name, value) {
            self.value.store(value, Ordering::Relaxed);
            Ok(())
        } else {
            Err(MethodError::invalid_args("Expected Count u32"))
        }
    }
}
fn call(path: &str, interface: &str, member: &str, values: &[Value]) -> Message {
    let mut call = Message::method_call(
        "org.example.Service",
        path,
        interface,
        member,
        Body::from_values(values).unwrap(),
    );
    call.header.serial = 42;
    call.header.sender = Some(":1.23".into());
    call
}
fn dispatch(server: &mut ObjectServer, call: &Message) -> Message {
    server.dispatch(call).unwrap().unwrap()
}

#[test]
fn exact_paths_and_longest_prefix_have_component_boundaries() {
    let mut server = ObjectServer::new();
    for (path, value) in [("/objects", 1), ("/objects/nested/", 2)] {
        server
            .register_prefix(
                path,
                Object {
                    value: Arc::new(AtomicU32::new(value)),
                },
            )
            .unwrap();
    }
    server
        .register(
            "/objects/nested/exact",
            Object {
                value: Arc::new(AtomicU32::new(3)),
            },
        )
        .unwrap();
    for (path, value) in [
        ("/objects/a", 1),
        ("/objects/nested/a", 2),
        ("/objects/nested/exact", 3),
    ] {
        let reply = dispatch(
            &mut server,
            &call(
                path,
                "org.freedesktop.DBus.Properties",
                "Get",
                &[
                    Value::Str("org.example.Test".into()),
                    Value::Str("Count".into()),
                ],
            ),
        );
        assert_eq!(reply.header.reply_serial, Some(42));
        assert_eq!(reply.header.destination.as_deref(), Some(":1.23"));
        assert_eq!(
            reply.body.values().unwrap(),
            [Value::Variant(Box::new(Value::U32(value)))]
        );
    }
    assert_eq!(
        dispatch(
            &mut server,
            &call("/objectstuff", "org.example.Test", "Echo", &[])
        )
        .header
        .error_name
        .as_deref(),
        Some("org.freedesktop.DBus.Error.UnknownObject")
    );
    let reply = dispatch(
        &mut server,
        &call("/objects/a", "org.example.Test", "Path", &[]),
    );
    assert_eq!(reply.body.reader().object_path().unwrap(), "/objects/a");
    assert!(server.unregister("/objects/nested/exact").is_some());
    assert!(server.unregister_prefix("/objects/nested/").is_some());
}

#[test]
fn builtins_properties_errors_and_no_reply_mutations() {
    let mut server = ObjectServer::new();
    let value = Arc::new(AtomicU32::new(0));
    server
        .register(
            "/object",
            Object {
                value: value.clone(),
            },
        )
        .unwrap();
    let call = |interface, member, values: &[Value]| call("/object", interface, member, values);
    let reply = dispatch(&mut server, &call("org.freedesktop.DBus.Peer", "Ping", &[]));
    assert_eq!(reply.body.signature(), "");
    let reply = dispatch(
        &mut server,
        &call("org.freedesktop.DBus.Introspectable", "Introspect", &[]),
    );
    assert!(reply
        .body
        .reader()
        .str()
        .unwrap()
        .contains("org.example.Test"));
    let mut set = call(
        "org.freedesktop.DBus.Properties",
        "Set",
        &[
            Value::Str("org.example.Test".into()),
            Value::Str("Count".into()),
            Value::Variant(Box::new(Value::U32(7))),
        ],
    );
    set.header.flags = NO_REPLY_EXPECTED;
    assert!(server.dispatch(&set).unwrap().is_none());
    assert_eq!(value.load(Ordering::Relaxed), 7);
    let all = dispatch(
        &mut server,
        &call(
            "org.freedesktop.DBus.Properties",
            "GetAll",
            &[Value::Str("org.example.Test".into())],
        ),
    );
    assert_eq!(all.body.signature(), "a{sv}");
    let mut reader = all.body.reader();
    let mut dict = reader.array("{sv}").unwrap();
    dict.begin_dict_entry().unwrap();
    assert_eq!(dict.str().unwrap(), "Count");
    assert_eq!(dict.variant_signature().unwrap(), "u");
    assert_eq!(dict.u32().unwrap(), 7);
    dict.finish().unwrap();
    reader.finish().unwrap();
    for (interface, member, expected) in [
        ("org.example.Test", "Missing", "UnknownMethod"),
        ("org.freedesktop.DBus.Properties", "Get", "InvalidArgs"),
    ] {
        let reply = dispatch(&mut server, &call(interface, member, &[]));
        assert_eq!(
            reply.header.error_name,
            Some(format!("org.freedesktop.DBus.Error.{expected}"))
        );
    }
    assert!(server
        .dispatch(&Message::signal(
            "/object",
            "org.example.Test",
            "Changed",
            Body::default()
        ))
        .unwrap()
        .is_none());
}
