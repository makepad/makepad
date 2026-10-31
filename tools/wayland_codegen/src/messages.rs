use crate::model::{arg_variant, camel, doc_attr, enum_type, ident, object_type, Arg, Interface, Message};

pub fn field_type(a: &Arg, event: bool) -> Option<String> {
    let ty = if !a.enum_name.is_empty() { format!("WEnum<{}>",enum_type(&a.enum_name)) }
    else { match a.kind.as_str() {
        "int"=>"i32".into(),"uint"=>"u32".into(),"fixed"=>"f64".into(),"string"=>"String".into(),"array"=>"Vec<u8>".into(),
        "fd"=>if event {"OwnedFd".into()} else {"std::os::fd::BorrowedFd<'a>".into()},
        "new_id" if !event => { if !a.interface.is_empty() { return None; } "(&'static Interface, u32)".into() },
        "object"|"new_id"=>if a.interface.is_empty() {"ObjectId".into()} else {object_type(&a.interface)},
        _=>unreachable!(),
    }};
    Some(if a.nullable {format!("Option<{ty}>")} else {ty})
}
pub fn enums(out: &mut String, i: &Interface) {
    for (event,name,messages) in [(false,"Request",&i.requests),(true,"Event",&i.events)] {
        let generic = if event {""} else {"<'a>"};
        out.push_str(&format!("#[derive(Debug)]\n#[non_exhaustive]\npub enum {name}{generic} {{\n"));
        for m in messages {
            doc_attr(out,&m.doc);
            if m.since>1 { doc_attr(out,&format!("Available since interface version {}.",m.since)); }
            if m.destructor { doc_attr(out,"This message destroys the protocol object."); }
            out.push_str(&format!("    {}",camel(&m.name)));
            if !m.args.is_empty() {
                out.push_str(" {\n");
                for a in &m.args {
                    if let Some(ty)=field_type(a,event) { doc_attr(out,&a.doc);out.push_str(&format!("        {}: {ty},\n",ident(&a.name))); }
                }
                out.push_str("    }");
            }
            out.push_str(",\n");
        }
        if !event { out.push_str("    #[doc(hidden)]\n    __PhantomLifetime { phantom: std::marker::PhantomData<&'a ()>, never: std::convert::Infallible },\n"); }
        out.push_str(&format!("}}\nimpl{generic} {name}{generic} {{ pub fn opcode(&self) -> u16 {{ match *self {{\n"));
        for (opcode,m) in messages.iter().enumerate() {
            out.push_str(&format!("    Self::{}{} => {opcode},\n",camel(&m.name),if m.args.is_empty(){""}else{" { .. }"}));
        }
        if !event {out.push_str("    Self::__PhantomLifetime { never, .. } => match never {},\n");}
        out.push_str("} } }\n");
        for (opcode,m) in messages.iter().enumerate() {
            let prefix=if event {"EVT"}else{"REQ"};
            out.push_str(&format!("pub const {prefix}_{}_SINCE: u32 = {};\npub const {prefix}_{}_OPCODE: u16 = {opcode};\n",m.name.to_ascii_uppercase(),m.since,m.name.to_ascii_uppercase()));
        }
    }
}
/// Arguments wayland-backend stores inline in a Message (`protocol::INLINE_ARGS`).
const INLINE_ARGS: usize = 4;
const BAD: &str = "DispatchError::BadMessage { sender_id: msg.sender_id.clone(), interface: Self::interface().name, opcode: msg.opcode }";
/// Error for a malformed event; the `msg` fields are still owned here (args were moved out).
const BAD_OWNED: &str = "DispatchError::BadMessage { sender_id: msg.sender_id, interface: Self::interface().name, opcode: msg.opcode }";
/// Decodes events inline in one match, the shape of the original scanner output: all arguments
/// are taken first and matched as a tuple, so a well-formed event has one success path.
pub fn parser(out: &mut String, i: &Interface) {
    if i.events.is_empty() {
        out.push_str(&format!("fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {{\n    let _me = Self::from_id(conn, msg.sender_id.clone()).unwrap();\n    Err({BAD_OWNED})\n}}\n"));
        return;
    }
    let any_args = i.events.iter().any(|m| !m.args.is_empty());
    out.push_str("fn parse_event(conn: &Connection, msg: Message<ObjectId, OwnedFd>) -> Result<(Self, Self::Event), DispatchError> {\n    let me = Self::from_id(conn, msg.sender_id.clone()).unwrap();\n");
    if any_args { out.push_str("    let mut arg_iter = msg.args.into_iter();\n"); }
    out.push_str("    match msg.opcode {\n");
    for (opcode,m) in i.events.iter().enumerate() {
        if m.args.is_empty() {
            out.push_str(&format!("        {opcode} => Ok((me, Event::{})),\n",camel(&m.name)));
            continue;
        }
        let pats: Vec<String>=m.args.iter().map(|a|format!("Some(Argument::{}({}))",arg_variant(&a.kind),ident(&a.name))).collect();
        let nexts: Vec<&str>=m.args.iter().map(|_|"arg_iter.next()").collect();
        let fields: Vec<String>=m.args.iter().map(|a|format!("{}: {}",ident(&a.name),read_arg(a))).collect();
        out.push_str(&format!("        {opcode} => {{\n            if let ({},) = ({},) {{\n                Ok((me, Event::{} {{ {} }}))\n            }} else {{\n                Err({BAD_OWNED})\n            }}\n        }}\n",pats.join(", "),nexts.join(", "),camel(&m.name),fields.join(", ")));
    }
    out.push_str(&format!("        _ => Err({BAD_OWNED}),\n    }}\n}}\n"));
}
fn read_arg(a: &Arg) -> String {
    let n=ident(&a.name);
    if !a.enum_name.is_empty() { return format!("WEnum::from({n} as u32)"); }
    match a.kind.as_str() {
        "int"|"uint"|"fd"=>n,
        "fixed"=>format!("({n} as f64) / 256.0"),
        "string" if a.nullable=>format!("{n}.as_ref().map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned())"),
        "string"=>format!("String::from_utf8_lossy({n}.as_ref().unwrap().as_bytes()).into_owned()"),
        "array" if a.nullable=>format!("if {n}.is_empty() {{ None }} else {{ Some(*{n}) }}"),
        "array"=>format!("*{n}"),
        "object"|"new_id"=>{
            let value=if a.interface.is_empty(){n.clone()}else{format!("{}::from_id(conn, {n}.clone()).map_err(|_| {BAD})?",object_type(&a.interface))};
            if a.nullable {format!("if {n}.is_null() {{ None }} else {{ Some({value}) }}")}else{value}
        },_=>unreachable!(),
    }
}
pub fn writer(out: &mut String, i: &Interface) {
    let conn = if i.requests.iter().any(|m|m.args.iter().any(|a|a.kind=="new_id"&&!a.interface.is_empty())) {"conn"} else {"_conn"};
    out.push_str(&format!("fn write_request<'a>(&self, {conn}: &Connection, request: Self::Request<'a>) -> Result<(Message<ObjectId, std::os::fd::BorrowedFd<'a>>, Option<(&'static Interface, u32)>), InvalidId> {{\n    match request {{\n"));
    for (opcode,m) in i.requests.iter().enumerate() {
        let fields: Vec<String>=m.args.iter().filter(|a|field_type(a,false).is_some()).map(|a|ident(&a.name)).collect();
        let pattern=if m.args.is_empty(){camel(&m.name)}else{format!("{} {{ {} }}",camel(&m.name),fields.join(", "))};
        out.push_str(&format!("        Request::{pattern} => {{\n            let child_spec = {};\n",child_spec(m)));
        let values: Vec<String>=m.args.iter().flat_map(write_arg).collect();
        if m.args.len()>INLINE_ARGS {
            // More arguments than Message keeps inline: build one exactly sized
            // heap buffer and hand it over, as the original scanner's from_vec did.
            out.push_str(&format!("            let mut args = Vec::with_capacity({});\n",values.len()));
            for expr in &values {out.push_str(&format!("            args.push({expr});\n"));}
            out.push_str(&format!("            let msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message {{ sender_id: self.id.clone(), opcode: {opcode}, args: args.into() }};\n"));
        } else {
            let mutable=if values.is_empty(){""}else{"mut "};
            out.push_str(&format!("            let {mutable}msg: Message<ObjectId, std::os::fd::BorrowedFd<'a>> = Message {{ sender_id: self.id.clone(), opcode: {opcode}, args: Default::default() }};\n"));
            for expr in &values {out.push_str(&format!("            msg.args.push({expr});\n"));}
        }
        out.push_str("            Ok((msg, child_spec))\n        },\n");
    }
    out.push_str("        Request::__PhantomLifetime { never, .. } => match never {},\n    }\n}\n");
}
fn child_spec(m: &Message) -> String {
    if let Some(a)=m.args.iter().find(|a|a.kind=="new_id") {
        if a.interface.is_empty(){format!("Some({})",ident(&a.name))}
        else {format!("Some(({}::interface(), conn.object_info(self.id())?.version))",object_type(&a.interface))}
    } else {"None".into()}
}
fn write_arg(a: &Arg) -> Vec<String> {
    let n=ident(&a.name);
    let value=match a.kind.as_str() {
        "int" if !a.enum_name.is_empty()=>format!("Argument::Int(u32::from({n}) as i32)"),
        "uint" if !a.enum_name.is_empty()=>format!("Argument::Uint(u32::from({n}))"),
        "int"|"uint"|"fd"=>format!("Argument::{}({n})",arg_variant(&a.kind)),
        "fixed"=>format!("Argument::Fixed(({n} * 256.0) as i32)"),
        "object" if a.nullable=>format!("Argument::Object({n}.as_ref().map_or_else(ObjectId::null, Proxy::id))"),
        "object"=>format!("Argument::Object({n}.id())"),
        "array" if a.nullable=>format!("Argument::Array(Box::new({n}.unwrap_or_default()))"),
        "array"=>format!("Argument::Array(Box::new({n}))"),
        "string" if a.nullable=>format!("Argument::Str({n}.map(|s| Box::new(std::ffi::CString::new(s).unwrap())))"),
        "string"=>format!("Argument::Str(Some(Box::new(std::ffi::CString::new({n}).unwrap())))"),
        "new_id"=>{
            let mut values: Vec<String>=Vec::new();
            if a.interface.is_empty(){values.push(format!("Argument::Str(Some(Box::new(std::ffi::CString::new({n}.0.name).unwrap())))"));values.push(format!("Argument::Uint({n}.1)"));}
            values.push("Argument::NewId(ObjectId::null())".into());return values;
        },_=>unreachable!(),
    };
    vec![value]
}
