use crate::model::{arg_variant, Interface, Message, Protocol};

fn signature(message: &Message) -> String {
    let mut signature = if message.since>1 { message.since.to_string() } else { String::new() };
    for arg in &message.args {
        if arg.nullable { signature.push('?'); }
        signature.push_str(match arg.kind.as_str() {
            "int"=>"i","uint"=>"u","fixed"=>"f","string"=>"s","array"=>"a","fd"=>"h","object"=>"o",
            "new_id" if arg.interface.is_empty()=>"sun", "new_id"=>"n", _=>unreachable!(),
        });
    }
    signature
}
pub fn emit(protocol: &Protocol) -> String {
    let mut out = String::from("// Immutable libwayland metadata, shared by every connection.\nstruct InterfacePointers(&'static [*const wayland_backend::protocol::wl_interface]);\nunsafe impl Sync for InterfacePointers {}\nstruct Messages(&'static [wayland_backend::protocol::wl_message]);\nunsafe impl Sync for Messages {}\n");
    for iface in &protocol.interfaces { interface(&mut out,iface); }
    out
}
fn interface(out: &mut String, i: &Interface) {
    let upper = i.name.to_ascii_uppercase();
    out.push_str(&format!("pub static {upper}_INTERFACE: wayland_backend::protocol::Interface = wayland_backend::protocol::Interface {{\n    name: {:?}, version: {},\n",i.name,i.version));
    for (side,msgs) in [("requests",&i.requests),("events",&i.events)] {
        out.push_str(&format!("    {side}: &[\n"));
        for m in msgs {
            out.push_str(&format!("        wayland_backend::protocol::MessageDesc {{ name: {:?}, since: {}, is_destructor: {},\n            signature: &[",m.name,m.since,m.destructor));
            for arg in &m.args {
                let prefix = "wayland_backend::protocol::ArgumentType::";
                if arg.kind=="new_id" && arg.interface.is_empty() {
                    out.push_str(&format!("{prefix}Str(wayland_backend::protocol::AllowNull::No), {prefix}Uint, {prefix}NewId,"));
                } else {
                    out.push_str(prefix);out.push_str(arg_variant(&arg.kind));
                    if arg.kind=="string" || arg.kind=="object" { out.push_str(&format!("(wayland_backend::protocol::AllowNull::{})",if arg.nullable {"Yes"} else {"No"})); }
                    out.push(',');
                }
            }
            let child = m.args.iter().find(|a|a.kind=="new_id"&&!a.interface.is_empty());
            let child = child.map(|a|format!("Some(&{}_INTERFACE)",a.interface.to_ascii_uppercase())).unwrap_or_else(||"None".into());
            out.push_str(&format!("],\n            child_interface: {child}, arg_interfaces: &["));
            for arg in m.args.iter().filter(|a|a.kind=="object") {
                if arg.interface.is_empty() { out.push_str("&wayland_backend::protocol::ANONYMOUS_INTERFACE,"); }
                else { out.push_str(&format!("&{}_INTERFACE,",arg.interface.to_ascii_uppercase())); }
            }
            out.push_str("],\n        },\n");
        }
        out.push_str("    ],\n");
    }
    out.push_str(&format!("    c_ptr: Some(&{}_interface),\n}};\n",i.name));
    for (side,msgs) in [("requests",&i.requests),("events",&i.events)] { c_messages(out,&i.name,side,msgs); }
    let requests = if i.requests.is_empty() { "core::ptr::null()".into() } else { format!("{}_requests.0.as_ptr()",i.name) };
    let events = if i.events.is_empty() { "core::ptr::null()".into() } else { format!("{}_events.0.as_ptr()",i.name) };
    out.push_str(&format!("pub static {}_interface: wayland_backend::protocol::wl_interface = wayland_backend::protocol::wl_interface {{\n    name: c{:?}.as_ptr(), version: {}, request_count: {}, requests: {requests}, event_count: {}, events: {events},\n}};\n",i.name,i.name,i.version,i.requests.len(),i.events.len()));
}
fn c_messages(out: &mut String, interface: &str, side: &str, messages: &[Message]) {
    if messages.is_empty() { return; }
    for m in messages {
        let base = format!("{interface}_{side}_{}",m.name);
        out.push_str(&format!("static {base}_types: InterfacePointers = InterfacePointers(&["));
        for a in &m.args {
            if a.kind=="new_id" && a.interface.is_empty() { out.push_str("core::ptr::null(),core::ptr::null(),"); }
            if matches!(a.kind.as_str(),"object"|"new_id") && !a.interface.is_empty() { out.push_str(&format!("&{}_interface,",a.interface)); }
            else { out.push_str("core::ptr::null(),"); }
        }
        out.push_str("]);\n");
    }
    out.push_str(&format!("static {interface}_{side}: Messages = Messages(&[\n"));
    for m in messages {
        out.push_str(&format!("    wayland_backend::protocol::wl_message {{ name: c{:?}.as_ptr(), signature: c{:?}.as_ptr(), types: {interface}_{side}_{}_types.0.as_ptr() }},\n",m.name,signature(m),m.name));
    }
    out.push_str("]);\n");
}
