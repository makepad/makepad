use makepad_dbus_wire::{Body, Error, MethodError, MethodResult, Result, Value};
#[cfg(unix)]
use makepad_dbus_wire::{Connection, Message, Writer};
#[cfg(unix)]
use std::time::Duration;

pub(crate) const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
pub(crate) fn expect_signature(body: &Body, signature: &str) -> Result<()> {
    if body.signature() == signature {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "expected signature {signature:?}, got {:?}",
            body.signature()
        )))
    }
}
pub(crate) fn unknown_property(name: &str) -> MethodError {
    MethodError::new(
        "org.freedesktop.DBus.Error.UnknownProperty",
        format!("Unknown property {name}"),
    )
}
pub(crate) fn variant_value(body: Body) -> MethodResult<Value> {
    let mut r = body.reader();
    let sig = r.variant_signature()?;
    let value = r.value(sig)?;
    r.finish()?;
    Ok(value)
}
pub(crate) fn property_values(body: Body) -> MethodResult<Vec<(String, Value)>> {
    let mut r = body.reader();
    let mut a = r.array("{sv}")?;
    let mut result = Vec::new();
    while !a.is_empty() {
        a.begin_dict_entry()?;
        let name = a.str()?.to_owned();
        let sig = a.variant_signature()?;
        let value = a.value(sig)?;
        result.push((name, value));
    }
    a.finish()?;
    r.finish()?;
    Ok(result)
}
#[cfg(unix)]
pub(crate) fn call_property_get(
    conn: &mut Connection,
    dest: &str,
    path: &str,
    interface: &str,
    name: &str,
    timeout: Duration,
) -> Result<Message> {
    let mut w = Writer::new();
    w.str(interface)?;
    w.str(name)?;
    let reply = conn.call(
        Message::method_call(
            dest,
            path,
            PROPERTIES_INTERFACE,
            "Get",
            Body::from_writer("ss", w)?,
        ),
        timeout,
    )?;
    expect_signature(&reply.body, "v")?;
    Ok(reply)
}
