#![cfg(unix)]
use dbus_wire::*;
use makepad_atspi::*;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(5);
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const CHILD: &str = "/org/a11y/atspi/accessible/child";
struct Tree {
    bus: String,
    address: String,
}
impl AccessibleServer for Tree {
    fn get_child_at_index(&mut self, path: &str, index: i32) -> MethodResult<ObjRef> {
        if path != ROOT || index != 0 {
            return Err(MethodError::invalid_args("child index"));
        }
        Ok(ObjRef {
            bus: self.bus.clone(),
            path: CHILD.into(),
        })
    }
    fn get_children(&mut self, path: &str) -> MethodResult<Vec<ObjRef>> {
        if path == ROOT {
            Ok(vec![self.get_child_at_index(path, 0)?])
        } else {
            Ok(vec![])
        }
    }
    fn get_role(&mut self, path: &str) -> MethodResult<u32> {
        Ok(if path == ROOT {
            AtspiRole::Application
        } else {
            AtspiRole::Button
        } as u32)
    }
    fn prop_name(&mut self, path: &str) -> MethodResult<String> {
        Ok(if path == ROOT { "Tree" } else { "Press" }.into())
    }
    fn prop_child_count(&mut self, path: &str) -> MethodResult<i32> {
        Ok(i32::from(path == ROOT))
    }
}
impl BusServer for Tree {
    fn get_address(&mut self, _: &str) -> MethodResult<String> {
        Ok(self.address.clone())
    }
}
impl StatusServer for Tree {
    fn prop_is_enabled(&mut self, _: &str) -> MethodResult<bool> {
        Ok(true)
    }
    fn prop_screen_reader_enabled(&mut self, _: &str) -> MethodResult<bool> {
        Ok(false)
    }
}
impl SocketServer for Tree {
    fn embed(&mut self, _: &str, plug: &ObjRef) -> MethodResult<ObjRef> {
        Ok(plug.clone())
    }
}
impl ObjectHandler for Tree {
    fn call(&mut self, call: MethodCall<'_>) -> MethodResult<Body> {
        dispatch_accessible(self, &call)
            .or_else(|| dispatch_socket(self, &call))
            .or_else(|| dispatch_bus(self, &call))
            .unwrap_or_else(|| Err(MethodError::unknown_method()))
    }
    fn introspect(&self, _: &str) -> String {
        format!("<node>{ACCESSIBLE_INTROSPECTION_XML}{SOCKET_INTROSPECTION_XML}{BUS_INTROSPECTION_XML}{STATUS_INTROSPECTION_XML}</node>")
    }
    fn get_property(&mut self, path: &str, interface: &str, name: &str) -> MethodResult<Value> {
        match interface {
            ACCESSIBLE_INTERFACE => get_accessible_property(self, path, name),
            STATUS_INTERFACE => get_status_property(self, path, name),
            _ => Err(MethodError::unknown_method()),
        }
    }
    fn get_all_properties(
        &mut self,
        path: &str,
        interface: &str,
    ) -> MethodResult<Vec<(String, Value)>> {
        match interface {
            ACCESSIBLE_INTERFACE => get_accessible_all_properties(self, path),
            STATUS_INTERFACE => get_status_all_properties(self, path),
            _ => Err(MethodError::unknown_method()),
        }
    }
}
struct Stop<'a>(&'a AtomicBool);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
#[test]
fn live_tree_proxies_and_state_changed() {
    if std::env::var("DBUS_WIRE_LIVE").as_deref() != Ok("1") {
        eprintln!("live bus test skipped; run tests/live.sh");
        return;
    }
    let mut client = Connection::session(TIMEOUT).unwrap();
    let mut server = Connection::session(TIMEOUT).unwrap();
    let destination = server.unique_name().unwrap().to_owned();
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
    let mut objects = ObjectServer::new();
    objects
        .register_prefix(
            "/org/a11y/atspi/accessible",
            Tree {
                bus: destination.clone(),
                address: address.clone(),
            },
        )
        .unwrap();
    let rule=format!("type='signal',sender='{destination}',interface='{EVENT_OBJECT_INTERFACE}',member='StateChanged'");
    client.add_match(&rule, TIMEOUT).unwrap();
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            while !stop.load(Ordering::Acquire) {
                server.read_ready().unwrap();
                while let Some(message) = server.pop_message() {
                    if let Some(reply) = objects.dispatch(&message).unwrap() {
                        server.send(reply).unwrap();
                    }
                    if message.header.path.as_deref() == Some(CHILD)
                        && message.header.member.as_deref() == Some("GetRole")
                    {
                        emit_object_state_changed(
                            &mut server,
                            CHILD,
                            "focused",
                            1,
                            0,
                            &Value::Str("".into()),
                            &[
                                ("Name".into(), Value::Str("Press".into())),
                                ("ChildCount".into(), Value::I32(0)),
                            ],
                        )
                        .unwrap();
                    }
                }
                server.flush().unwrap();
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let guard = Stop(&stop);
        assert_eq!(
            call_accessible_get_child_count(&mut client, &destination, ROOT, TIMEOUT).unwrap(),
            1
        );
        assert_eq!(
            call_accessible_get_name(&mut client, &destination, ROOT, TIMEOUT).unwrap(),
            "Tree"
        );
        assert_eq!(
            call_accessible_get_role(&mut client, &destination, ROOT, TIMEOUT).unwrap(),
            AtspiRole::Application as u32
        );
        let child = call_accessible_get_child_at_index(&mut client, &destination, ROOT, 0, TIMEOUT)
            .unwrap();
        assert_eq!(
            child,
            ObjRef {
                bus: destination.clone(),
                path: CHILD.into()
            }
        );
        assert_eq!(
            call_accessible_get_children(&mut client, &destination, ROOT, TIMEOUT).unwrap(),
            vec![child.clone()]
        );
        assert_eq!(
            call_accessible_get_child_count(&mut client, &child.bus, &child.path, TIMEOUT).unwrap(),
            0
        );
        assert!(
            call_accessible_get_children(&mut client, &child.bus, &child.path, TIMEOUT)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            call_accessible_get_name(&mut client, &child.bus, &child.path, TIMEOUT).unwrap(),
            "Press"
        );
        assert_eq!(
            call_accessible_get_role(&mut client, &child.bus, &child.path, TIMEOUT).unwrap(),
            AtspiRole::Button as u32
        );
        assert_eq!(
            call_socket_embed(&mut client, &destination, ROOT, &child, TIMEOUT).unwrap(),
            child
        );
        assert_eq!(
            call_bus_get_address(&mut client, &destination, ROOT, TIMEOUT).unwrap(),
            address
        );
        assert!(call_status_get_is_enabled(&mut client, &destination, ROOT, TIMEOUT).unwrap());
        assert!(
            !call_status_get_screen_reader_enabled(&mut client, &destination, ROOT, TIMEOUT)
                .unwrap()
        );
        let error = call_accessible_get_child_at_index(&mut client, &destination, ROOT, 1, TIMEOUT)
            .unwrap_err();
        assert!(matches!(error,Error::Remote {name,..} if name.ends_with("InvalidArgs")));
        let deadline = Instant::now() + TIMEOUT;
        loop {
            client.read_ready().unwrap();
            let mut found = false;
            while let Some(message) = client.pop_message() {
                if message.header.kind == MessageType::Signal
                    && message.header.interface.as_deref() == Some(EVENT_OBJECT_INTERFACE)
                    && message.header.member.as_deref() == Some("StateChanged")
                {
                    assert_eq!(message.header.path.as_deref(), Some(CHILD));
                    assert_eq!(message.body.signature(), "siiva{sv}");
                    let mut r = message.body.reader();
                    assert_eq!(r.str().unwrap(), "focused");
                    assert_eq!(r.i32().unwrap(), 1);
                    assert_eq!(r.i32().unwrap(), 0);
                    assert_eq!(r.variant_signature().unwrap(), "s");
                    assert_eq!(r.str().unwrap(), "");
                    let mut props = r.array("{sv}").unwrap();
                    props.begin_dict_entry().unwrap();
                    assert_eq!(props.str().unwrap(), "Name");
                    assert_eq!(props.variant_signature().unwrap(), "s");
                    assert_eq!(props.str().unwrap(), "Press");
                    props.begin_dict_entry().unwrap();
                    assert_eq!(props.str().unwrap(), "ChildCount");
                    assert_eq!(props.variant_signature().unwrap(), "i");
                    assert_eq!(props.i32().unwrap(), 0);
                    props.finish().unwrap();
                    r.finish().unwrap();
                    found = true;
                }
            }
            if found {
                break;
            }
            assert!(Instant::now() < deadline, "StateChanged not delivered");
            std::thread::sleep(Duration::from_millis(1));
        }
        client.remove_match(&rule, TIMEOUT).unwrap();
        drop(guard);
        worker.join().unwrap();
    });
    eprintln!("two-node tree, generated method/property proxies, Socket.Embed, Bus.GetAddress, Status, remote error and StateChanged passed");
}
