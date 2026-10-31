//! Server mode: compact protocol tables for `libs/wayland_server`. Per interface one static
//! `Interface` (message signatures drive a generic decoder) plus opcode and enum constants.
//! Dispatch stays hand-written over the decoded messages, so the output stays small.
use crate::model::{Interface, Message, Protocol};

fn arg_type(kind: &str, interface: &str, nullable: bool) -> String {
    match kind {
        "int" => "ArgType::Int".into(),
        "uint" => "ArgType::Uint".into(),
        "fixed" => "ArgType::Fixed".into(),
        "string" => if nullable { "ArgType::StrOpt".into() } else { "ArgType::Str".into() },
        "array" => "ArgType::Array".into(),
        "fd" => "ArgType::Fd".into(),
        "object" => if nullable { format!("ArgType::ObjectOpt({interface:?})") } else { format!("ArgType::Object({interface:?})") },
        "new_id" => if interface.is_empty() { "ArgType::NewIdDyn".into() } else { format!("ArgType::NewId({interface:?})") },
        _ => unreachable!(),
    }
}
fn messages(out: &mut String, list: &[Message]) {
    out.push_str("&[");
    for m in list {
        let args: Vec<String> = m.args.iter().map(|a| arg_type(&a.kind, &a.interface, a.nullable)).collect();
        out.push_str(&format!("\n        Msg {{ name: {:?}, since: {}, destructor: {}, args: &[{}] }},", m.name, m.since, m.destructor, args.join(", ")));
    }
    out.push_str("\n    ]");
}
fn interface(out: &mut String, i: &Interface) {
    out.push_str(&format!("pub mod {} {{\n    use crate::schema::*;\n    pub static INTERFACE: Interface = Interface {{\n    name: {:?}, version: {},\n    requests: ", i.name, i.name, i.version));
    messages(out, &i.requests);
    out.push_str(",\n    events: ");
    messages(out, &i.events);
    out.push_str(",\n    };\n");
    for (prefix, list) in [("REQ", &i.requests), ("EVT", &i.events)] {
        for (opcode, m) in list.iter().enumerate() {
            out.push_str(&format!("    pub const {prefix}_{}: u16 = {opcode};\n", m.name.to_ascii_uppercase()));
        }
    }
    for e in &i.enums {
        for entry in &e.entries {
            out.push_str(&format!("    pub const {}_{}: u32 = {};\n", e.name.to_ascii_uppercase(), entry.name.to_ascii_uppercase(), entry.value));
        }
    }
    out.push_str("}\n");
}
pub fn emit(protocols: &[Protocol]) -> String {
    let mut out = String::new();
    out.push_str("//! Generated from protocol XML by `tools/wayland_codegen --server`.\n//! Do not edit manually; regenerate instead.\n#![allow(dead_code)]\n\n");
    let mut names: Vec<&str> = Vec::new();
    for p in protocols {
        for i in &p.interfaces {
            interface(&mut out, i);
            names.push(&i.name);
        }
    }
    out.push_str("\npub static ALL: &[&crate::schema::Interface] = &[\n");
    for n in names { out.push_str(&format!("    &{n}::INTERFACE,\n")); }
    out.push_str("];\n");
    out
}
