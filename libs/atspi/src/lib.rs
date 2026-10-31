//! Typed AT-SPI2 glue. Implement the generated server traits and forward calls
//! through their dispatchers. `get_*_property`, `get_*_all_properties` and
//! `set_*_property` bridge the separate `dbus_wire::ObjectHandler` callbacks.
//!
//! Client calls block and belong on the bus worker. Signal emitters only queue;
//! drive `Connection::flush` and readiness on that same worker. A variant argument
//! is its inner `Value` (the generated encoder supplies the variant wrapper).
//! Introspection constants contain one `<interface>` each; wrap the implemented
//! interfaces in `<node>` when answering `ObjectHandler::introspect`.
mod generated;
mod wire;
pub use generated::*;
pub use makepad_dbus_wire as dbus_wire;
use makepad_dbus_wire::{Body, MethodCall, MethodError, MethodResult, Value, Writer};
#[cfg(unix)]
use makepad_dbus_wire::{Connection, Message, Result};
#[cfg(unix)]
use std::time::Duration;

/// AT-SPI's `(so)` object reference. Paths are validated when encoded/decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjRef {
    pub bus: String,
    pub path: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropertyInfo {
    pub name: &'static str,
    pub signature: &'static str,
    pub readable: bool,
    pub writable: bool,
}

pub const BUS_INTERFACE: &str = "org.a11y.Bus";
pub const STATUS_INTERFACE: &str = "org.a11y.Status";
pub const BUS_INTROSPECTION_XML: &str = "<interface name=\"org.a11y.Bus\"><method name=\"GetAddress\"><arg type=\"s\" direction=\"out\"/></method></interface>";
pub const STATUS_INTROSPECTION_XML: &str = "<interface name=\"org.a11y.Status\"><property name=\"IsEnabled\" type=\"b\" access=\"readwrite\"/><property name=\"ScreenReaderEnabled\" type=\"b\" access=\"readwrite\"/></interface>";
pub trait BusServer {
    fn get_address(&mut self, _path: &str) -> MethodResult<String> {
        Err(MethodError::unknown_method())
    }
}
pub fn dispatch_bus(
    server: &mut (impl BusServer + ?Sized),
    call: &MethodCall<'_>,
) -> Option<MethodResult<Body>> {
    if (!call.interface.is_empty() && call.interface != BUS_INTERFACE)
        || call.member != "GetAddress"
    {
        return None;
    }
    Some((|| {
        wire::expect_signature(call.args, "")?;
        call.args.reader().finish()?;
        let mut w = Writer::new();
        w.str(&server.get_address(call.path)?)?;
        Ok(Body::from_writer("s", w)?)
    })())
}
#[cfg(unix)]
pub fn call_bus_get_address(
    conn: &mut Connection,
    dest: &str,
    path: &str,
    timeout: Duration,
) -> Result<String> {
    let reply = conn.call(
        Message::method_call(dest, path, BUS_INTERFACE, "GetAddress", Body::default()),
        timeout,
    )?;
    wire::expect_signature(&reply.body, "s")?;
    let mut r = reply.body.reader();
    let value = r.str()?.to_owned();
    r.finish()?;
    Ok(value)
}
pub trait StatusServer {
    fn prop_is_enabled(&mut self, _path: &str) -> MethodResult<bool> {
        Err(MethodError::unknown_method())
    }
    fn prop_screen_reader_enabled(&mut self, _path: &str) -> MethodResult<bool> {
        Err(MethodError::unknown_method())
    }
    fn set_prop_is_enabled(&mut self, _path: &str, _value: bool) -> MethodResult<()> {
        Err(MethodError::unknown_method())
    }
    fn set_prop_screen_reader_enabled(&mut self, _path: &str, _value: bool) -> MethodResult<()> {
        Err(MethodError::unknown_method())
    }
}
fn status_value(
    server: &mut (impl StatusServer + ?Sized),
    path: &str,
    name: &str,
) -> MethodResult<bool> {
    match name {
        "IsEnabled" => server.prop_is_enabled(path),
        "ScreenReaderEnabled" => server.prop_screen_reader_enabled(path),
        _ => Err(wire::unknown_property(name)),
    }
}
pub fn get_status_property(
    server: &mut (impl StatusServer + ?Sized),
    path: &str,
    name: &str,
) -> MethodResult<Value> {
    Ok(Value::Bool(status_value(server, path, name)?))
}
pub fn get_status_all_properties(
    server: &mut (impl StatusServer + ?Sized),
    path: &str,
) -> MethodResult<Vec<(String, Value)>> {
    let mut values = Vec::new();
    for name in ["IsEnabled", "ScreenReaderEnabled"] {
        match get_status_property(server, path, name) {
            Ok(value) => values.push((name.into(), value)),
            Err(e) if e.name == "org.freedesktop.DBus.Error.UnknownMethod" => (),
            Err(e) => return Err(e),
        }
    }
    Ok(values)
}
pub fn set_status_property(
    server: &mut (impl StatusServer + ?Sized),
    path: &str,
    name: &str,
    value: Value,
) -> MethodResult<()> {
    let Value::Bool(value) = value else {
        return Err(MethodError::invalid_args("expected boolean"));
    };
    match name {
        "IsEnabled" => server.set_prop_is_enabled(path, value),
        "ScreenReaderEnabled" => server.set_prop_screen_reader_enabled(path, value),
        _ => Err(wire::unknown_property(name)),
    }
}
pub fn dispatch_status_properties(
    server: &mut (impl StatusServer + ?Sized),
    call: &MethodCall<'_>,
) -> Option<MethodResult<Body>> {
    if call.interface != wire::PROPERTIES_INTERFACE {
        return None;
    }
    let sig = match call.member {
        "Get" => "ss",
        "GetAll" => "s",
        "Set" => "ssv",
        _ => return None,
    };
    if let Err(e) = wire::expect_signature(call.args, sig) {
        return Some(Err(e.into()));
    }
    let mut r = call.args.reader();
    match r.str() {
        Ok(STATUS_INTERFACE) => (),
        Ok(_) => return None,
        Err(e) => return Some(Err(e.into())),
    }
    Some((|| {
        let mut w = Writer::new();
        let sig = match call.member {
            "Get" => {
                let name = r.str()?;
                r.finish()?;
                let value = status_value(server, call.path, name)?;
                w.variant("b", |w| {
                    w.bool(value);
                    Ok(())
                })?;
                "v"
            }
            "GetAll" => {
                r.finish()?;
                let array = w.begin_array("{sv}")?;
                for name in ["IsEnabled", "ScreenReaderEnabled"] {
                    match status_value(server, call.path, name) {
                        Ok(value) => {
                            w.begin_dict_entry();
                            w.str(name)?;
                            w.variant("b", |w| {
                                w.bool(value);
                                Ok(())
                            })?;
                        }
                        Err(e) if e.name == "org.freedesktop.DBus.Error.UnknownMethod" => (),
                        Err(e) => return Err(e),
                    }
                }
                w.end_array(array)?;
                "a{sv}"
            }
            "Set" => {
                let name = r.str()?;
                if r.variant_signature()? != "b" {
                    return Err(MethodError::invalid_args("expected boolean"));
                }
                let value = r.bool()?;
                r.finish()?;
                set_status_property(server, call.path, name, Value::Bool(value))?;
                ""
            }
            _ => unreachable!(),
        };
        Ok(Body::from_writer(sig, w)?)
    })())
}
#[cfg(unix)]
fn call_status_get(
    conn: &mut Connection,
    dest: &str,
    path: &str,
    name: &str,
    timeout: Duration,
) -> Result<bool> {
    let reply = wire::call_property_get(conn, dest, path, STATUS_INTERFACE, name, timeout)?;
    let mut r = reply.body.reader();
    if r.variant_signature()? != "b" {
        return Err(dbus_wire::Error::Invalid(
            "expected boolean property reply".into(),
        ));
    }
    let value = r.bool()?;
    r.finish()?;
    Ok(value)
}
#[cfg(unix)]
pub fn call_status_get_is_enabled(
    conn: &mut Connection,
    dest: &str,
    path: &str,
    timeout: Duration,
) -> Result<bool> {
    call_status_get(conn, dest, path, "IsEnabled", timeout)
}
#[cfg(unix)]
pub fn call_status_get_screen_reader_enabled(
    conn: &mut Connection,
    dest: &str,
    path: &str,
    timeout: Duration,
) -> Result<bool> {
    call_status_get(conn, dest, path, "ScreenReaderEnabled", timeout)
}

#[cfg(test)]
mod tests;
