use crate::model::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Codecs {
    read: BTreeSet<Type>,
    write: BTreeSet<Type>,
    ids: BTreeMap<Type, usize>,
}
impl Codecs {
    fn add(&mut self, ty: &Type, read: bool, write: bool) {
        if read {
            self.read.insert(ty.clone());
        }
        if write {
            self.write.insert(ty.clone());
        }
        match ty {
            Type::Array(t) => self.add(t, read, write),
            // (so) is read and written inline as an ObjRef: its members need no helpers
            Type::Struct(_) if ty.sig() == "(so)" => (),
            Type::Struct(ts) => {
                for t in ts {
                    self.add(t, read, write);
                }
            }
            Type::Dict(k, v) => {
                self.add(k, read, write);
                self.add(v, read, write);
            }
            _ => (),
        }
    }
    fn id(&self, t: &Type) -> usize {
        self.ids[t]
    }
    fn read(&self, t: &Type, r: &str) -> String {
        format!("read_{}({r})?", self.id(t))
    }
    fn write(&self, t: &Type, w: &str, v: &str) -> String {
        format!("write_{}({w}, {v})?;\n", self.id(t))
    }
    fn emit(&self, out: &mut String) {
        for t in &self.read {
            out.push_str(&format!(
                "fn read_{}(r: &mut Reader<'_>) -> Result<{}> {{\n",
                self.id(t),
                t.owned()
            ));
            match t {
                Type::Basic('v') => out.push_str("let sig = r.variant_signature()?; r.value(sig)\n"),
                Type::Basic('s'|'o'|'g') => out.push_str(&format!("Ok(r.{}()?.to_owned())\n",t.codec())),
                Type::Basic(_) => out.push_str(&format!("r.{}()\n",t.codec())),
                Type::Array(inner) => out.push_str(&format!("let mut a=r.array({:?})?; let mut result=Vec::new(); while !a.is_empty() {{ result.push({}); }} a.finish()?; Ok(result)\n",inner.sig(),self.read(inner,"&mut a"))),
                Type::Struct(ts) => {
                    out.push_str("r.begin_struct()?;\n");
                    if t.sig()=="(so)" {
                        out.push_str("Ok(ObjRef { bus: r.str()?.to_owned(), path: r.object_path()?.to_owned() })\n");
                    } else {
                        out.push_str(&format!("Ok({})\n",tuple(&ts.iter().map(|t|self.read(t,"r")).collect::<Vec<_>>())));
                    }
                }
                Type::Dict(k,v) => out.push_str(&format!("r.begin_dict_entry()?; Ok(({}, {}))\n",self.read(k,"r"),self.read(v,"r"))),
            }
            out.push_str("}\n");
        }
        for t in &self.write {
            out.push_str(&format!(
                "fn write_{}(w: &mut Writer, value: {}) -> Result<()> {{\n",
                self.id(t),
                t.input()
            ));
            match t {
                Type::Basic('v') => {
                    out.push_str("w.variant(&value.signature()?, |w| w.value(value))?;\n")
                }
                Type::Basic('s' | 'o' | 'g') => {
                    out.push_str(&format!("w.{}(value)?;\n", t.codec()))
                }
                Type::Basic(_) => out.push_str(&format!("w.{}(value);\n", t.codec())),
                Type::Array(inner) => {
                    out.push_str(&format!(
                        "let array=w.begin_array({:?})?; for item in value {{\n",
                        inner.sig()
                    ));
                    out.push_str(&self.write(inner, "w", &inner.borrow("*item")));
                    out.push_str("} w.end_array(array)?;\n");
                }
                Type::Struct(ts) => {
                    out.push_str("w.begin_struct();\n");
                    if t.sig() == "(so)" {
                        out.push_str("w.str(&value.bus)?; w.object_path(&value.path)?;\n");
                    } else {
                        for (i, t) in ts.iter().enumerate() {
                            out.push_str(&self.write(t, "w", &t.borrow(&format!("value.{i}"))));
                        }
                    }
                }
                Type::Dict(k, v) => {
                    out.push_str("w.begin_dict_entry();\n");
                    out.push_str(&self.write(k, "w", &k.borrow("value.0")));
                    out.push_str(&self.write(v, "w", &v.borrow("value.1")));
                }
            }
            out.push_str("Ok(())\n}\n");
        }
    }
}
fn name(a: &Arg, idx: usize) -> String {
    if a.name.is_empty() {
        format!("arg{idx}")
    } else {
        format!("arg_{}", snake(&a.name))
    }
}
fn args(m: &Member, output: bool) -> Vec<&Arg> {
    m.args.iter().filter(|a| a.output == output).collect()
}
fn signature(args: &[&Arg]) -> String {
    args.iter().map(|a| a.ty.sig()).collect()
}
fn result_type(args: &[&Arg]) -> String {
    if args.len() == 1 {
        args[0].ty.owned()
    } else {
        tuple(&args.iter().map(|a| a.ty.owned()).collect::<Vec<_>>())
    }
}
fn params(args: &[&Arg], unused: bool) -> String {
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            format!(
                ", {}{}: {}",
                if unused { "_" } else { "" },
                name(a, i),
                a.ty.input()
            )
        })
        .collect()
}
fn encode_args(out: &mut String, c: &Codecs, args: &[&Arg]) {
    out.push_str(if args.is_empty() {
        "let w=Writer::new();\n"
    } else {
        "let mut w=Writer::new();\n"
    });
    for (idx, a) in args.iter().enumerate() {
        out.push_str(&c.write(&a.ty, "&mut w", &name(a, idx)));
    }
}
fn decode_result(out: &mut String, c: &Codecs, args: &[&Arg]) {
    if args.is_empty() {
        out.push_str("reply.body.reader().finish()?; Ok(())\n");
        return;
    }
    out.push_str("let mut r=reply.body.reader();\n");
    let expressions: Vec<_> = args.iter().map(|a| c.read(&a.ty, "&mut r")).collect();
    let expression = if expressions.len() == 1 {
        expressions[0].clone()
    } else {
        tuple(&expressions)
    };
    out.push_str(&format!(
        "let result={expression}; r.finish()?; Ok(result)\n"
    ));
}
fn emit_interface(out: &mut String, c: &Codecs, i: &Interface) {
    let id = i.id();
    let server = i.server();
    let upper = id.to_ascii_uppercase();
    out.push_str(&format!("pub const {upper}_INTERFACE: &str = {:?};\npub const {upper}_INTROSPECTION_XML: &str = {:?};\n",i.name,introspection(i)));
    out.push_str(&format!(
        "pub const {upper}_PROPERTIES: &[PropertyInfo] = &[\n"
    ));
    for p in &i.properties {
        out.push_str(&format!(
            "PropertyInfo {{ name: {:?}, signature: {:?}, readable: {}, writable: {} }},\n",
            p.name,
            p.ty.sig(),
            p.access.contains("read"),
            p.access.contains("write")
        ));
    }
    out.push_str("];\n");
    out.push_str(&format!("pub trait {server} {{\n"));
    for m in &i.methods {
        out.push_str(&format!("fn {}(&mut self, _path: &str{}) -> MethodResult<{}> {{ Err(MethodError::unknown_method()) }}\n",snake(&m.name),params(&args(m,false),true),result_type(&args(m,true))));
    }
    for p in &i.properties {
        if p.access.contains("read") {
            out.push_str(&format!("fn prop_{}(&mut self, _path: &str) -> MethodResult<{}> {{ Err(MethodError::unknown_method()) }}\n",snake(&p.name),p.ty.owned()));
        }
        if p.access.contains("write") {
            out.push_str(&format!("fn set_prop_{}(&mut self, _path: &str, _value: {}) -> MethodResult<()> {{ Err(MethodError::unknown_method()) }}\n",snake(&p.name),p.ty.input()));
        }
    }
    out.push_str("}\n");
    out.push_str(&format!("pub fn dispatch_{id}(server: &mut (impl {server} + ?Sized), call: &MethodCall<'_>) -> Option<MethodResult<Body>> {{\n"));
    if i.methods.is_empty() {
        out.push_str("let _=(server,call); None\n}\n");
    } else {
        out.push_str(&format!("if !call.interface.is_empty() && call.interface != {upper}_INTERFACE {{ return None; }}\nmatch call.member {{\n"));
        for m in &i.methods {
            let input = args(m, false);
            let output = args(m, true);
            out.push_str(&format!(
                "{:?} => Some((|| {{ expect_signature(call.args, {:?})?;\n",
                m.name,
                signature(&input)
            ));
            if input.is_empty() {
                out.push_str("call.args.reader().finish()?;\n");
            } else {
                out.push_str("let mut r=call.args.reader();\n");
            }
            let mut passed = String::new();
            for (idx, a) in input.iter().enumerate() {
                let n = name(a, idx);
                let borrow_string = matches!(a.ty, Type::Basic('s' | 'o' | 'g'));
                let read = if borrow_string {
                    format!("r.{}()?", a.ty.codec())
                } else {
                    c.read(&a.ty, "&mut r")
                };
                out.push_str(&format!("let {n}={read};\n"));
                passed.push_str(&format!(
                    ", {}",
                    if borrow_string { n } else { a.ty.borrow(&n) }
                ));
            }
            if !input.is_empty() {
                out.push_str("r.finish()?;\n");
            }
            out.push_str(&format!(
                "let {}=server.{}(call.path{passed})?;\n",
                if output.is_empty() { "()" } else { "result" },
                snake(&m.name)
            ));
            out.push_str(if output.is_empty() {
                "let w=Writer::new();\n"
            } else {
                "let mut w=Writer::new();\n"
            });
            for (idx, a) in output.iter().enumerate() {
                let value = if output.len() == 1 {
                    "result".into()
                } else {
                    format!("result.{idx}")
                };
                out.push_str(&c.write(&a.ty, "&mut w", &a.ty.borrow(&value)));
            }
            out.push_str(&format!(
                "Ok(Body::from_writer({:?}, w)?)\n}})()),\n",
                signature(&output)
            ));
        }
        out.push_str("_=>None,\n}\n}\n");
    }
    for m in &i.methods {
        let input = args(m, false);
        let output = args(m, true);
        out.push_str(&format!("#[cfg(unix)]\npub fn call_{id}_{}(conn: &mut Connection, dest: &str, path: &str{}, timeout: Duration) -> Result<{}> {{\n",snake(&m.name),params(&input,false),result_type(&output)));
        encode_args(out, c, &input);
        out.push_str(&format!("let reply=conn.call(Message::method_call(dest,path,{upper}_INTERFACE,{:?},Body::from_writer({:?},w)?),timeout)?;\nexpect_signature(&reply.body,{:?})?;\n",m.name,signature(&input),signature(&output)));
        decode_result(out, c, &output);
        out.push_str("}\n");
    }
    for m in &i.signals {
        let input: Vec<_> = m.args.iter().collect();
        let signal_id = id.strip_prefix("event_").unwrap_or(&id);
        out.push_str(&format!("#[cfg(unix)]\npub fn emit_{signal_id}_{}(conn: &mut Connection,path: &str{}) -> Result<()> {{\n",snake(&m.name),params(&input,false)));
        encode_args(out, c, &input);
        out.push_str(&format!("conn.emit_signal(path,{upper}_INTERFACE,{:?},Body::from_writer({:?},w)?)?; Ok(())\n}}\n",m.name,signature(&input)));
    }
    emit_properties(out, c, i);
}
fn emit_properties(out: &mut String, c: &Codecs, i: &Interface) {
    let id = i.id();
    let server = i.server();
    let upper = id.to_ascii_uppercase();
    // All supplied interfaces have properties, but empty interfaces work too.
    out.push_str(&format!("fn write_{id}_property(server: &mut (impl {server} + ?Sized),path: &str,name: &str,w: &mut Writer) -> MethodResult<()> {{\n"));
    if !i.properties.iter().any(|p| p.access.contains("read")) {
        out.push_str("let _=(&server,path,&w);\n");
    }
    out.push_str("match name {\n");
    for p in &i.properties {
        if p.access.contains("read") {
            out.push_str(&format!("{:?}=>{{ let value=server.prop_{}(path)?; w.variant({:?}, |w| write_{}(w,{}))?; Ok(()) }},\n",p.name,snake(&p.name),p.ty.sig(),c.id(&p.ty),p.ty.borrow("value")));
        }
    }
    out.push_str("_=>Err(unknown_property(name)),\n}\n}\n");
    out.push_str(&format!("fn write_{id}_all_properties(server: &mut (impl {server} + ?Sized),path: &str,w: &mut Writer) -> MethodResult<()> {{\n"));
    if !i.properties.iter().any(|p| p.access.contains("read")) {
        out.push_str("let _=(&server,path);\n");
    }
    out.push_str("let array=w.begin_array(\"{sv}\")?;\n");
    for p in &i.properties {
        if p.access.contains("read") {
            out.push_str(&format!("match server.prop_{}(path) {{ Ok(value)=>{{w.begin_dict_entry(); w.str({:?})?; w.variant({:?},|w|write_{}(w,{}))?;}}, Err(e) if e.name==\"org.freedesktop.DBus.Error.UnknownMethod\"=>(), Err(e)=>return Err(e) }}\n",snake(&p.name),p.name,p.ty.sig(),c.id(&p.ty),p.ty.borrow("value")));
        }
    }
    out.push_str("w.end_array(array)?; Ok(())\n}\n");
    out.push_str(&format!("fn set_{id}_property_reader(server: &mut (impl {server} + ?Sized),path: &str,name: &str,r: &mut Reader<'_>) -> MethodResult<()> {{\n"));
    if !i.properties.iter().any(|p| p.access.contains("write")) {
        out.push_str("let _=(&server,path,&r);\n");
    }
    out.push_str("match name {\n");
    for p in &i.properties {
        if p.access.contains("write") {
            out.push_str(&format!("{:?}=>{{ if r.variant_signature()? != {:?} {{ return Err(MethodError::invalid_args(\"property type mismatch\")); }} let value={}; r.clone().finish()?; server.set_prop_{}(path,{}) }},\n",p.name,p.ty.sig(),c.read(&p.ty,"r"),snake(&p.name),p.ty.borrow("value")));
        } else {
            out.push_str(&format!("{:?}=>Err(MethodError::new(\"org.freedesktop.DBus.Error.PropertyReadOnly\",\"Property is read-only\")),\n",p.name));
        }
    }
    out.push_str("_=>Err(unknown_property(name)),\n}\n}\n");
    out.push_str(&format!("pub fn dispatch_{id}_properties(server: &mut (impl {server} + ?Sized),call: &MethodCall<'_>) -> Option<MethodResult<Body>> {{\nif call.interface != PROPERTIES_INTERFACE {{return None;}}\nlet sig=match call.member {{\"Get\"=>\"ss\",\"GetAll\"=>\"s\",\"Set\"=>\"ssv\",_=>return None}};\nif let Err(e)=expect_signature(call.args,sig) {{return Some(Err(e.into()));}}\nlet mut r=call.args.reader();\nlet interface=match r.str() {{Ok(s)=>s,Err(e)=>return Some(Err(e.into()))}};\nif interface!={upper}_INTERFACE {{return None;}}\nSome((||{{let mut w=Writer::new(); let sig=match call.member {{\n\"Get\"=>{{let name=r.str()?;r.finish()?;write_{id}_property(server,call.path,name,&mut w)?;\"v\"}},\n\"GetAll\"=>{{r.finish()?;write_{id}_all_properties(server,call.path,&mut w)?;\"a{{sv}}\"}},\n\"Set\"=>{{let name=r.str()?;set_{id}_property_reader(server,call.path,name,&mut r)?;r.finish()?;\"\"}},\n_=>unreachable!(),\n}}; Ok(Body::from_writer(sig,w)?)}})())\n}}\n"));
    // These wrappers are for dbus_wire's ObjectHandler callbacks; typed dispatch stays tree-free.
    out.push_str(&format!("pub fn get_{id}_property(server: &mut (impl {server} + ?Sized),path: &str,name: &str) -> MethodResult<Value> {{let mut w=Writer::new();write_{id}_property(server,path,name,&mut w)?;variant_value(Body::from_writer(\"v\",w)?)}}\n"));
    out.push_str(&format!("pub fn get_{id}_all_properties(server: &mut (impl {server} + ?Sized),path: &str) -> MethodResult<Vec<(String,Value)>> {{let mut w=Writer::new();write_{id}_all_properties(server,path,&mut w)?;property_values(Body::from_writer(\"a{{sv}}\",w)?)}}\n"));
    out.push_str(&format!("pub fn set_{id}_property(server: &mut (impl {server} + ?Sized),path: &str,name: &str,value: Value) -> MethodResult<()> {{let mut w=Writer::new();w.variant(&value.signature()?,|w|w.value(&value))?;let body=Body::from_writer(\"v\",w)?;set_{id}_property_reader(server,path,name,&mut body.reader())}}\n"));
    for p in &i.properties {
        let n = snake(&p.name);
        if p.access.contains("read") {
            out.push_str(&format!("#[cfg(unix)]\npub fn call_{id}_get_{n}(conn: &mut Connection,dest: &str,path: &str,timeout: Duration) -> Result<{}> {{let reply=call_property_get(conn,dest,path,{upper}_INTERFACE,{:?},timeout)?;let mut r=reply.body.reader(); if r.variant_signature()? != {:?} {{return Err(Error::Invalid(\"property reply type mismatch\".into()));}}let value={};r.finish()?;Ok(value)}}\n",p.ty.owned(),p.name,p.ty.sig(),c.read(&p.ty,"&mut r")));
        }
        if p.access.contains("write") {
            out.push_str(&format!("#[cfg(unix)]\npub fn call_{id}_set_{n}(conn: &mut Connection,dest: &str,path: &str,value: {},timeout: Duration) -> Result<()> {{let mut w=Writer::new();w.str({upper}_INTERFACE)?;w.str({:?})?;w.variant({:?},|w|write_{}(w,value))?;let reply=conn.call(Message::method_call(dest,path,PROPERTIES_INTERFACE,\"Set\",Body::from_writer(\"ssv\",w)?),timeout)?;expect_signature(&reply.body,\"\")?;reply.body.reader().finish()}}\n",p.ty.input(),p.name,p.ty.sig(),c.id(&p.ty)));
        }
    }
}
pub fn generate(interfaces: &[Interface]) -> String {
    let mut c = Codecs::default();
    for i in interfaces {
        for m in &i.methods {
            for a in &m.args {
                c.add(
                    &a.ty,
                    !matches!(a.ty, Type::Basic('s' | 'o' | 'g')) || a.output,
                    true,
                );
            }
        }
        for m in &i.signals {
            for a in &m.args {
                c.add(&a.ty, false, true);
            }
        }
        for p in &i.properties {
            c.add(&p.ty, true, true);
        }
    }
    for (idx, t) in c.read.union(&c.write).enumerate() {
        c.ids.insert(t.clone(), idx);
    }
    let mut out=String::from("// generated by tools/dbus_codegen, do not edit\n// Regenerate (from the workspace root, on the build node):\n// cargo build --offline --release -j8 -p makepad-dbus-codegen\n// target/release/makepad-dbus-codegen tools/dbus_codegen/atspi tools/dbus_codegen/atspi/atspi-constants.h libs/atspi/src/generated.rs\n\nuse crate::{wire::*, ObjRef, PropertyInfo};\nuse makepad_dbus_wire::{Body, Error, MethodCall, MethodError, MethodResult, Reader, Result, Value, Writer};\n#[cfg(unix)]\nuse makepad_dbus_wire::{Connection, Message};\n#[cfg(unix)]\nuse std::time::Duration;\n");
    c.emit(&mut out);
    for i in interfaces {
        emit_interface(&mut out, &c, i);
    }
    out.push_str("pub const INTERFACES: &[(&str,&str)] = &[\n");
    for i in interfaces {
        let n = i.id().to_ascii_uppercase();
        out.push_str(&format!("({n}_INTERFACE,{n}_INTROSPECTION_XML),\n"));
    }
    out.push_str("];\n");
    out
}
pub fn constants(header: &str) -> Result<String, String> {
    // Remove comments before finding declarations; documentation also contains enum names.
    let mut clean = String::new();
    let mut rest = header;
    while let Some(pos) = rest.find("/*") {
        clean.push_str(&rest[..pos]);
        let end = rest[pos + 2..].find("*/").ok_or("unterminated C comment")?;
        rest = &rest[pos + 2 + end + 2..];
    }
    clean.push_str(rest);
    let wanted = [
        ("AtspiRole", "ATSPI_ROLE_"),
        ("AtspiStateType", "ATSPI_STATE_"),
        ("AtspiRelationType", "ATSPI_RELATION_"),
        ("AtspiCoordType", "ATSPI_COORD_TYPE_"),
        ("AtspiTextBoundaryType", "ATSPI_TEXT_BOUNDARY_"),
        ("AtspiTextGranularity", "ATSPI_TEXT_GRANULARITY_"),
        ("AtspiScrollType", "ATSPI_SCROLL_"),
        ("AtspiCollectionMatchType", "ATSPI_Collection_MATCH_"),
        ("AtspiLive", "ATSPI_LIVE_"),
    ];
    let mut enums = BTreeMap::new();
    for declaration in clean.split("typedef enum").skip(1) {
        let (_, after) = declaration
            .split_once('{')
            .ok_or("enum missing opening brace")?;
        let (body, tail) = after.split_once('}').ok_or("enum missing closing brace")?;
        let name = tail.split(';').next().unwrap().trim();
        if let Some((_, prefix)) = wanted.iter().find(|(n, _)| *n == name) {
            let mut entries = String::new();
            let mut next = 0u32;
            let mut symbols = BTreeMap::new();
            for entry in body.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let (key, value) = entry
                    .split_once('=')
                    .map(|(k, v)| (k.trim(), Some(v.trim())))
                    .unwrap_or((entry, None));
                let mut alias = false;
                let number = if let Some(v) = value {
                    if let Some(value) = symbols.get(v) {
                        alias = true;
                        *value
                    } else if let Some(hex) = v.strip_prefix("0x") {
                        u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?
                    } else {
                        v.parse::<u32>()
                            .map_err(|_| format!("unsupported {name} enum value {v}"))?
                    }
                } else {
                    next
                };
                symbols.insert(key.to_owned(), number);
                next = number.checked_add(1).ok_or("enum overflow")?;
                if !alias {
                    let variant = camel(
                        key.strip_prefix(prefix)
                            .ok_or_else(|| format!("unexpected enum prefix {key}"))?,
                    );
                    entries.push_str(&format!("{variant} = {number},\n"));
                }
            }
            enums.insert(name.to_owned(),format!("#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n#[repr(u32)]\npub enum {name} {{\n{entries}}}\n"));
        }
    }
    let mut out = String::new();
    for (name, _) in wanted {
        if let Some(e) = enums.get(name) {
            out.push_str(e);
        } else if name != "AtspiLive" {
            return Err(format!("missing enum {name}"));
        }
    }
    Ok(out)
}
