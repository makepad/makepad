use crate::xml::Document;

pub struct Protocol { pub interfaces: Vec<Interface>, pub copyright: String }
pub struct Interface {
    pub name: String,
    pub version: u32,
    pub doc: String,
    pub requests: Vec<Message>,
    pub events: Vec<Message>,
    pub enums: Vec<Enum>,
}
pub struct Message {
    pub name: String, pub doc: String, pub since: u32,
    pub destructor: bool, pub args: Vec<Arg>,
}
pub struct Arg {
    pub name: String, pub kind: String, pub interface: String,
    pub enum_name: String, pub nullable: bool, pub doc: String,
}
pub struct Enum {
    pub name: String, pub doc: String, pub bitfield: bool,
    pub entries: Vec<Entry>,
}
pub struct Entry { pub name: String, pub value: u32, pub doc: String }

fn documentation(doc: &Document, id: usize) -> String {
    let e = &doc.nodes[id];
    if let Some(child) = doc.child(id,"description") {
        let desc = &doc.nodes[child];
        let lines: Vec<&str> = desc.text.lines().map(str::trim).collect();
        format!("{}\n\n{}",desc.attr("summary"),lines.join("\n").trim())
    } else { e.attr("summary").to_owned() }
}
fn since(doc: &Document, id: usize) -> u32 {
    let value = doc.nodes[id].attr("since");
    if value.is_empty() { 1 } else { value.parse::<u32>().expect("message version") }
}
fn messages(doc: &Document, id: usize, kind: &str) -> Vec<Message> {
    doc.children(id, kind).into_iter().map(|id| {
        let e = &doc.nodes[id];
        let args: Vec<Arg> = doc.children(id,"arg").into_iter().map(|id| {
            let e = &doc.nodes[id];
            assert!(matches!(e.attr("type"),"int"|"uint"|"fixed"|"string"|"array"|"fd"|"object"|"new_id"), "unknown argument type");
            Arg { name: e.attr("name").into(),kind:e.attr("type").into(),
                interface:e.attr("interface").into(),enum_name:e.attr("enum").into(),
                nullable:e.attr("allow-null")=="true",doc:documentation(doc,id) }
        }).collect();
        assert!(args.iter().filter(|a|a.kind=="new_id").count()<=1,"multiple constructors");
        Message { name:e.attr("name").into(),doc:documentation(doc,id),since:since(doc,id),
            destructor:e.attr("type")=="destructor",args }
    }).collect()
}
pub fn parse(source: &str) -> Result<Protocol, String> {
    let doc = Document::parse(source)?;
    let root = doc.child(0,"protocol").ok_or("missing protocol root")?;
    let mut interfaces: Vec<Interface> = Vec::new();
    for id in doc.children(root,"interface") {
        let e = &doc.nodes[id];
        let mut enums: Vec<Enum> = Vec::new();
        for eid in doc.children(id,"enum") {
            let en = &doc.nodes[eid];
            let entries: Vec<Entry> = doc.children(eid,"entry").into_iter().map(|id| {
                let e = &doc.nodes[id];
                let literal = e.attr("value");
                let value = if let Some(hex) = literal.strip_prefix("0x") {
                    u32::from_str_radix(hex,16).expect("enum hex value")
                } else { literal.parse::<i64>().expect("enum value") as u32 };
                Entry { name:e.attr("name").into(),value,doc:documentation(&doc,id) }
            }).collect();
            enums.push(Enum { name:en.attr("name").into(),doc:documentation(&doc,eid),bitfield:en.attr("bitfield")=="true",entries });
        }
        interfaces.push(Interface { name:e.attr("name").into(),version:e.attr("version").parse::<u32>().map_err(|e|e.to_string())?,doc:documentation(&doc,id),requests:messages(&doc,id,"request"),events:messages(&doc,id,"event"),enums });
    }
    Ok(Protocol { interfaces, copyright:doc.child_text(root,"copyright").to_owned() })
}
pub fn camel(name: &str) -> String {
    let mut out = String::new();
    for word in name.split('_') {
        let mut chars = word.chars();
        if let Some(first) = chars.next() { out.push(first.to_ascii_uppercase()); out.extend(chars); }
    }
    if out=="Self" || out.as_bytes().first().is_some_and(u8::is_ascii_digit) { out.insert(0,'_'); }
    out
}
pub fn ident(name: &str) -> String {
    if matches!(name,"type"|"move"|"ref"|"match"|"enum"|"struct"|"mod"|"fn"|"self"|"in"|"loop"|"use"|"async"|"await") { format!("_{name}") } else { name.into() }
}
pub fn object_type(name: &str) -> String { format!("super::{name}::{}",camel(name)) }
pub fn enum_type(name: &str) -> String {
    if let Some((module, ty)) = name.split_once('.') { format!("super::{module}::{}",camel(ty)) } else { camel(name) }
}
pub fn doc_attr(out: &mut String, text: &str) {
    if !text.is_empty() { out.push_str(&format!("#[doc = {text:?}]\n")); }
}
pub fn copyright(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("//");
        if !line.trim().is_empty() { out.push(' '); out.push_str(line.trim()); }
        out.push('\n');
    }
}
pub fn arg_variant(kind: &str) -> &str {
    match kind { "int"=>"Int","uint"=>"Uint","fixed"=>"Fixed","string"=>"Str","array"=>"Array","fd"=>"Fd","object"=>"Object","new_id"=>"NewId",_=>panic!("unknown argument") }
}
