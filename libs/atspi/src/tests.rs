use super::*;
use dbus_wire::{Endian, ObjectHandler, ObjectServer};

#[derive(Default)]
struct Accessible {
    current: f64,
    enabled: bool,
}
impl AccessibleServer for Accessible {
    fn get_child_at_index(&mut self, path: &str, index: i32) -> MethodResult<ObjRef> {
        assert_eq!(path, "/root");
        if index != 0 {
            return Err(MethodError::invalid_args("child index"));
        }
        Ok(ObjRef {
            bus: ":1.42".into(),
            path: "/root/child".into(),
        })
    }
    fn get_role(&mut self, _: &str) -> MethodResult<u32> {
        Ok(AtspiRole::Button as u32)
    }
    fn prop_name(&mut self, _: &str) -> MethodResult<String> {
        Ok("A button".into())
    }
    fn prop_child_count(&mut self, _: &str) -> MethodResult<i32> {
        Ok(1)
    }
    fn get_relation_set(&mut self, _: &str) -> MethodResult<Vec<(u32, Vec<ObjRef>)>> {
        Ok(vec![(
            AtspiRelationType::LabelledBy as u32,
            vec![ObjRef {
                bus: ":1.42".into(),
                path: "/label".into(),
            }],
        )])
    }
}
impl ValueServer for Accessible {
    fn prop_current_value(&mut self, _: &str) -> MethodResult<f64> {
        Ok(self.current)
    }
    fn set_prop_current_value(&mut self, _: &str, value: f64) -> MethodResult<()> {
        self.current = value;
        Ok(())
    }
}
impl BusServer for Accessible {
    fn get_address(&mut self, _: &str) -> MethodResult<String> {
        Ok("unix:path=/bus".into())
    }
}
impl StatusServer for Accessible {
    fn prop_is_enabled(&mut self, _: &str) -> MethodResult<bool> {
        Ok(self.enabled)
    }
    fn set_prop_is_enabled(&mut self, _: &str, value: bool) -> MethodResult<()> {
        self.enabled = value;
        Ok(())
    }
}
fn call<'a>(interface: &'a str, member: &'a str, args: &'a Body) -> MethodCall<'a> {
    MethodCall {
        path: "/root",
        interface,
        member,
        args,
        sender: Some(":1.8"),
    }
}
fn body(values: &[Value]) -> Body {
    Body::from_values(values).unwrap()
}
fn properties(
    member: &str,
    interface: &str,
    name: Option<&str>,
    value: Option<Value>,
) -> (String, Body) {
    let mut args = vec![Value::Str(interface.into())];
    if let Some(name) = name {
        args.push(Value::Str(name.into()));
    }
    if let Some(value) = value {
        args.push(Value::Variant(Box::new(value)));
    }
    (member.into(), body(&args))
}
#[test]
fn generated_dispatch_and_properties() {
    let mut server = Accessible::default();
    let index = body(&[Value::I32(0)]);
    let child = dispatch_accessible(
        &mut server,
        &call(ACCESSIBLE_INTERFACE, "GetChildAtIndex", &index),
    )
    .unwrap()
    .unwrap();
    assert_eq!(child.signature(), "(so)");
    let mut r = child.reader();
    r.begin_struct().unwrap();
    assert_eq!(r.str().unwrap(), ":1.42");
    assert_eq!(r.object_path().unwrap(), "/root/child");
    r.finish().unwrap();
    // Same dispatch for the peer's byte order, without a Value decode tree.
    let big = Body::from_bytes("i", 0i32.to_be_bytes().to_vec(), Endian::Big).unwrap();
    assert_eq!(
        dispatch_accessible(
            &mut server,
            &call(ACCESSIBLE_INTERFACE, "GetChildAtIndex", &big)
        )
        .unwrap()
        .unwrap(),
        child
    );
    let empty = Body::default();
    let role = dispatch_accessible(&mut server, &call(ACCESSIBLE_INTERFACE, "GetRole", &empty))
        .unwrap()
        .unwrap();
    assert_eq!(role.signature(), "u");
    assert_eq!(role.reader().u32().unwrap(), AtspiRole::Button as u32);
    assert!(
        dispatch_accessible(&mut server, &call("org.example.Other", "GetRole", &empty)).is_none()
    );
    assert!(
        dispatch_accessible(&mut server, &call(ACCESSIBLE_INTERFACE, "Missing", &empty)).is_none()
    );
    assert_eq!(
        dispatch_accessible(
            &mut server,
            &call(ACCESSIBLE_INTERFACE, "GetRoleName", &empty)
        )
        .unwrap()
        .unwrap_err(),
        MethodError::unknown_method()
    );
    let wrong = body(&[Value::U32(0)]);
    assert!(dispatch_accessible(
        &mut server,
        &call(ACCESSIBLE_INTERFACE, "GetChildAtIndex", &wrong)
    )
    .unwrap()
    .unwrap_err()
    .name
    .ends_with("InvalidArgs"));
    let (member, args) = properties("Get", ACCESSIBLE_INTERFACE, Some("Name"), None);
    let name = dispatch_accessible_properties(
        &mut server,
        &call(wire::PROPERTIES_INTERFACE, &member, &args),
    )
    .unwrap()
    .unwrap();
    assert_eq!(name.signature(), "v");
    let mut r = name.reader();
    assert_eq!(r.variant_signature().unwrap(), "s");
    assert_eq!(r.str().unwrap(), "A button");
    r.finish().unwrap();
    let (_, args) = properties("GetAll", ACCESSIBLE_INTERFACE, None, None);
    let all = dispatch_accessible_properties(
        &mut server,
        &call(wire::PROPERTIES_INTERFACE, "GetAll", &args),
    )
    .unwrap()
    .unwrap();
    assert_eq!(all.signature(), "a{sv}");
    assert_eq!(
        wire::property_values(all).unwrap(),
        vec![
            ("Name".into(), Value::Str("A button".into())),
            ("ChildCount".into(), Value::I32(1))
        ]
    );
    let (_, args) = properties("Get", VALUE_INTERFACE, Some("Name"), None);
    assert!(dispatch_accessible_properties(
        &mut server,
        &call(wire::PROPERTIES_INTERFACE, "Get", &args)
    )
    .is_none());
    let (_, args) = properties("Get", ACCESSIBLE_INTERFACE, Some("Missing"), None);
    assert!(dispatch_accessible_properties(
        &mut server,
        &call(wire::PROPERTIES_INTERFACE, "Get", &args)
    )
    .unwrap()
    .unwrap_err()
    .name
    .ends_with("UnknownProperty"));
    let relations = dispatch_accessible(
        &mut server,
        &call(ACCESSIBLE_INTERFACE, "GetRelationSet", &empty),
    )
    .unwrap()
    .unwrap();
    assert_eq!(relations.signature(), "a(ua(so))");
    let mut r = relations.reader();
    let mut a = r.array("(ua(so))").unwrap();
    a.begin_struct().unwrap();
    assert_eq!(a.u32().unwrap(), AtspiRelationType::LabelledBy as u32);
    let mut refs = a.array("(so)").unwrap();
    refs.begin_struct().unwrap();
    assert_eq!(refs.str().unwrap(), ":1.42");
    assert_eq!(refs.object_path().unwrap(), "/label");
    refs.finish().unwrap();
    a.finish().unwrap();
    r.finish().unwrap();
    let (_, args) = properties(
        "Set",
        VALUE_INTERFACE,
        Some("CurrentValue"),
        Some(Value::F64(3.5)),
    );
    assert_eq!(
        dispatch_value_properties(&mut server, &call(wire::PROPERTIES_INTERFACE, "Set", &args))
            .unwrap()
            .unwrap(),
        Body::default()
    );
    assert_eq!(
        get_value_property(&mut server, "/root", "CurrentValue").unwrap(),
        Value::F64(3.5)
    );
    let (_, args) = properties(
        "Set",
        VALUE_INTERFACE,
        Some("CurrentValue"),
        Some(Value::Bool(true)),
    );
    assert!(dispatch_value_properties(
        &mut server,
        &call(wire::PROPERTIES_INTERFACE, "Set", &args)
    )
    .unwrap()
    .unwrap_err()
    .name
    .ends_with("InvalidArgs"));
    assert_eq!(server.current, 3.5);
    assert!(
        set_accessible_property(&mut server, "/root", "Name", Value::Str("x".into()))
            .unwrap_err()
            .name
            .ends_with("PropertyReadOnly")
    );
    assert_eq!(
        dispatch_bus(&mut server, &call(BUS_INTERFACE, "GetAddress", &empty))
            .unwrap()
            .unwrap()
            .reader()
            .str()
            .unwrap(),
        "unix:path=/bus"
    );
    let (_, args) = properties(
        "Set",
        STATUS_INTERFACE,
        Some("IsEnabled"),
        Some(Value::Bool(true)),
    );
    dispatch_status_properties(&mut server, &call(wire::PROPERTIES_INTERFACE, "Set", &args))
        .unwrap()
        .unwrap();
    assert_eq!(
        get_status_all_properties(&mut server, "/root").unwrap(),
        vec![("IsEnabled".into(), Value::Bool(true))]
    );
    assert_eq!(INTERFACES.len(), 19);
    assert_eq!(AtspiRole::Button as u32, 43);
    assert_eq!(AtspiCoordType::Window as u32, 1);
    assert!(ACCESSIBLE_INTROSPECTION_XML.contains("<method name=\"GetRole\">"));
    assert!(VALUE_PROPERTIES
        .iter()
        .any(|p| p.name == "CurrentValue" && p.writable && p.signature == "d"));
}
// The library's object table intercepts Properties before ObjectHandler::call.
// Exercise the generated callback bridge, including partial GetAll.
impl ObjectHandler for Accessible {
    fn call(&mut self, call: MethodCall<'_>) -> MethodResult<Body> {
        dispatch_accessible(self, &call).unwrap_or_else(|| Err(MethodError::unknown_method()))
    }
    fn get_property(&mut self, path: &str, interface: &str, name: &str) -> MethodResult<Value> {
        assert_eq!(interface, ACCESSIBLE_INTERFACE);
        get_accessible_property(self, path, name)
    }
    fn get_all_properties(
        &mut self,
        path: &str,
        interface: &str,
    ) -> MethodResult<Vec<(String, Value)>> {
        assert_eq!(interface, ACCESSIBLE_INTERFACE);
        get_accessible_all_properties(self, path)
    }
}
#[test]
fn generated_object_handler_bridge() {
    let mut objects = ObjectServer::new();
    objects.register("/root", Accessible::default()).unwrap();
    for member in ["Get", "GetAll"] {
        let (_, args) = properties(
            member,
            ACCESSIBLE_INTERFACE,
            if member == "Get" { Some("Name") } else { None },
            None,
        );
        let mut message = dbus_wire::Message::method_call(
            "org.example.Tree",
            "/root",
            wire::PROPERTIES_INTERFACE,
            member,
            args,
        );
        message.header.serial = 1;
        let reply = objects
            .dispatch(&message)
            .unwrap()
            .unwrap()
            .into_result()
            .unwrap();
        if member == "Get" {
            assert_eq!(
                wire::variant_value(reply.body).unwrap(),
                Value::Str("A button".into())
            );
        } else {
            assert_eq!(wire::property_values(reply.body).unwrap().len(), 2);
        }
    }
}
