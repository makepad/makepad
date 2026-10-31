use crate::{Body, Error, Message, MessageType, Result, Value, NO_REPLY_EXPECTED};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodError {
    pub name: String,
    pub detail: String,
}
impl MethodError {
    pub fn new(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: detail.into(),
        }
    }
    pub fn unknown_method() -> Self {
        Self::new("org.freedesktop.DBus.Error.UnknownMethod", "Unknown method")
    }
    pub fn invalid_args(detail: impl Into<String>) -> Self {
        Self::new("org.freedesktop.DBus.Error.InvalidArgs", detail)
    }
}
impl From<Error> for MethodError {
    fn from(error: Error) -> Self {
        Self::invalid_args(error.to_string())
    }
}
pub type MethodResult<T> = std::result::Result<T, MethodError>;

#[derive(Clone, Copy)]
pub struct MethodCall<'a> {
    pub path: &'a str,
    /// Empty when the caller omitted the optional interface header.
    pub interface: &'a str,
    pub member: &'a str,
    pub args: &'a Body,
    pub sender: Option<&'a str>,
}

/// One object, or a family of paths. Generated dispatchers can use args.reader()
/// directly. Property callbacks receive unwrapped values; Get/GetAll wrap them.
pub trait ObjectHandler: Send {
    fn call(&mut self, _call: MethodCall<'_>) -> MethodResult<Body> {
        Err(MethodError::unknown_method())
    }
    /// Complete <node> XML for this path, including any advertised built-ins.
    fn introspect(&self, _path: &str) -> String {
        "<node/>".into()
    }
    fn get_property(&mut self, _path: &str, _interface: &str, _name: &str) -> MethodResult<Value> {
        Err(MethodError::new(
            "org.freedesktop.DBus.Error.UnknownProperty",
            "Unknown property",
        ))
    }
    fn get_all_properties(
        &mut self,
        _path: &str,
        _interface: &str,
    ) -> MethodResult<Vec<(String, Value)>> {
        Err(MethodError::new(
            "org.freedesktop.DBus.Error.UnknownInterface",
            "Unknown property interface",
        ))
    }
    fn set_property(
        &mut self,
        _path: &str,
        _interface: &str,
        _name: &str,
        _value: Value,
    ) -> MethodResult<()> {
        Err(MethodError::new(
            "org.freedesktop.DBus.Error.PropertyReadOnly",
            "Property is read-only",
        ))
    }
}

/// Exact paths win; otherwise the longest component-boundary prefix wins.
/// A single prefix registration can serve any number of object paths.
#[derive(Default)]
pub struct ObjectServer {
    paths: BTreeMap<String, Box<dyn ObjectHandler>>,
    prefixes: BTreeMap<String, Box<dyn ObjectHandler>>,
}
impl ObjectServer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, path: &str, handler: impl ObjectHandler + 'static) -> Result<()> {
        crate::validate_object_path(path)?;
        self.paths.insert(path.into(), Box::new(handler));
        Ok(())
    }
    /// A trailing slash is accepted. `/a` covers `/a` and `/a/b`, never `/ab`.
    pub fn register_prefix(
        &mut self,
        path: &str,
        handler: impl ObjectHandler + 'static,
    ) -> Result<()> {
        let path = prefix_path(path);
        crate::validate_object_path(path)?;
        self.prefixes.insert(path.into(), Box::new(handler));
        Ok(())
    }
    pub fn unregister(&mut self, path: &str) -> Option<Box<dyn ObjectHandler>> {
        self.paths.remove(path)
    }
    pub fn unregister_prefix(&mut self, path: &str) -> Option<Box<dyn ObjectHandler>> {
        self.prefixes.remove(prefix_path(path))
    }
    fn handler(&mut self, path: &str) -> Option<&mut Box<dyn ObjectHandler>> {
        if self.paths.contains_key(path) {
            return self.paths.get_mut(path);
        }
        let mut prefix = path;
        loop {
            if self.prefixes.contains_key(prefix) {
                return self.prefixes.get_mut(prefix);
            }
            if prefix == "/" {
                return None;
            }
            let end = prefix.rfind('/')?;
            prefix = if end == 0 { "/" } else { &prefix[..end] };
        }
    }
    /// Returns None for non-calls or NO_REPLY_EXPECTED. No-reply calls still run
    /// the handler (including mutations). Enqueue the returned message with send.
    pub fn dispatch(&mut self, message: &Message) -> Result<Option<Message>> {
        if message.header.kind != MessageType::MethodCall {
            return Ok(None);
        }
        let path = message
            .header
            .path
            .as_deref()
            .ok_or_else(|| Error::invalid("call has no path"))?;
        let member = message
            .header
            .member
            .as_deref()
            .ok_or_else(|| Error::invalid("call has no member"))?;
        let call = MethodCall {
            path,
            member,
            interface: message.header.interface.as_deref().unwrap_or(""),
            args: &message.body,
            sender: message.header.sender.as_deref(),
        };
        let result = match self.handler(path) {
            Some(handler) => dispatch(handler.as_mut(), call),
            None => Err(MethodError::new(
                "org.freedesktop.DBus.Error.UnknownObject",
                "Unknown object path",
            )),
        };
        if message.header.flags & NO_REPLY_EXPECTED != 0 {
            return Ok(None);
        }
        Ok(Some(match result {
            Ok(body) => Message::method_return(message, body),
            Err(error) => Message::error(message, &error.name, &error.detail)?,
        }))
    }
}
fn prefix_path(path: &str) -> &str {
    if path == "/" {
        path
    } else {
        path.strip_suffix('/').unwrap_or(path)
    }
}
fn expect(call: MethodCall<'_>, signature: &str) -> MethodResult<()> {
    if call.args.signature() == signature {
        Ok(())
    } else {
        Err(MethodError::invalid_args(format!(
            "Expected signature {signature}"
        )))
    }
}
fn dispatch(handler: &mut dyn ObjectHandler, call: MethodCall<'_>) -> MethodResult<Body> {
    match (call.interface, call.member) {
        ("org.freedesktop.DBus.Peer", "Ping") => {
            expect(call, "")?;
            Ok(Body::default())
        }
        ("org.freedesktop.DBus.Introspectable", "Introspect") => {
            expect(call, "")?;
            Ok(Body::from_values(&[Value::Str(
                handler.introspect(call.path),
            )])?)
        }
        ("org.freedesktop.DBus.Properties", member) => {
            let mut args = call.args.reader();
            match member {
                "Get" => {
                    expect(call, "ss")?;
                    let interface = args.str()?;
                    let name = args.str()?;
                    let value = handler.get_property(call.path, interface, name)?;
                    Ok(Body::from_values(&[Value::Variant(Box::new(value))])?)
                }
                "GetAll" => {
                    expect(call, "s")?;
                    let interface = args.str()?;
                    let values = handler
                        .get_all_properties(call.path, interface)?
                        .into_iter()
                        .map(|(k, v)| {
                            Value::DictEntry(Box::new((Value::Str(k), Value::Variant(Box::new(v)))))
                        })
                        .collect();
                    Ok(Body::from_values(&[Value::Array {
                        element: "{sv}".into(),
                        values,
                    }])?)
                }
                "Set" => {
                    expect(call, "ssv")?;
                    let interface = args.str()?;
                    let name = args.str()?;
                    let sig = args.variant_signature()?;
                    let value = args.value(sig)?;
                    handler.set_property(call.path, interface, name, value)?;
                    Ok(Body::default())
                }
                _ => Err(MethodError::unknown_method()),
            }
        }
        _ => handler.call(call),
    }
}
