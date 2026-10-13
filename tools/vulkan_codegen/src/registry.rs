use crate::xml::{Document, Element};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub base: String,
    pub ty: String,
    pub pointer: usize,
    pub constant: bool,
    pub array: String,
    pub len: String,
    pub value: String,
    pub optional: bool,
}
#[derive(Clone)]
pub struct Type {
    pub name: String,
    pub category: String,
    pub alias: String,
    pub base: String,
    pub fields: Vec<Field>,
    pub ret: String,
    pub extends: Vec<String>,
    pub dispatchable: bool,
}
#[derive(Clone)]
pub struct Command {
    pub name: String,
    pub alias: String,
    pub fields: Vec<Field>,
    pub ret: String,
}
#[derive(Clone)]
pub struct Value {
    pub group: String,
    pub value: String,
    pub alias: String,
}
pub struct Registry {
    pub types: BTreeMap<String, Type>,
    pub commands: BTreeMap<String, Command>,
    pub values: BTreeMap<String, Value>,
    pub revision: String,
}
pub fn snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 && chars[i-1] != '_'
            && (chars[i-1].is_ascii_lowercase() || chars[i-1].is_ascii_digit()
                || chars.get(i+1).is_some_and(char::is_ascii_lowercase)) { out.push('_'); }
        out.push(c.to_ascii_lowercase());
    }
    // Vulkan/ash convention treats dimensions as a single word.
    out.replace("_2_d", "_2d").replace("_3_d", "_3d")
}
pub fn ident(name: &str) -> String {
    if name == "type" { return "ty".into(); }
    if matches!(name, "match" | "ref" | "self" | "in" | "loop" | "move") {
        format!("{name}_")
    } else { name.into() }
}
pub fn rust_type(name: &str) -> String {
    match name {
        "void" => "core::ffi::c_void".into(), "char" => "core::ffi::c_char".into(),
        "int" | "int32_t" => "i32".into(), "uint32_t" => "u32".into(),
        "int64_t" => "i64".into(), "uint64_t" => "u64".into(),
        "uint8_t" => "u8".into(), "int8_t" => "i8".into(),
        "uint16_t" => "u16".into(), "int16_t" => "i16".into(),
        "size_t" => "usize".into(), "float" => "f32".into(), "double" => "f64".into(),
        "Display" => "core::ffi::c_void".into(), "Window" => "core::ffi::c_ulong".into(),
        "wl_display" => "WlDisplay".into(), "wl_surface" => "WlSurface".into(),
        _ if name.starts_with("PFN_vk") => format!("Pfn{}", &name[6..]),
        _ => name.strip_prefix("Vk").unwrap_or(name).replace("FlagBits", "Flags"),
    }
}
pub fn supported(e: &Element) -> bool {
    e.attr("api").is_empty() || e.attr("api").split(',').any(|s| s == "vulkan")
}
fn field(doc: &Document, id: usize) -> Field {
    let e = &doc.nodes[id];
    let base = doc.child_text(id, "type").to_owned();
    let name = doc.child_text(id, "name").to_owned();
    let comment = doc.child_text(id, "comment");
    let declaration = if comment.is_empty() { e.text.as_str() } else { e.text.split(comment).next().unwrap() };
    let name_at = declaration.rfind(&name).expect("C declaration name");
    let before = &declaration[..name_at];
    let pointer = before.bytes().filter(|b| *b == b'*').count();
    let constant = before.trim_start().starts_with("const ");
    let mut ty = rust_type(&base);
    for depth in 0..pointer {
        let is_const = if depth == 0 { constant } else { before.contains("* const") || before.contains("*const") };
        ty = format!("*{} {ty}", if is_const { "const" } else { "mut" });
    }
    let after = &declaration[name_at + name.len()..];
    let mut array = String::new();
    let mut rest = after;
    let mut dimensions: Vec<String> = Vec::new();
    while let Some(start) = rest.find('[') {
        let end = rest[start..].find(']').expect("array dimension") + start;
        let dim = rest[start+1..end].trim().to_string();
        dimensions.push(dim.clone());
        if array.is_empty() { array = dim; }
        rest = &rest[end+1..];
    }
    for dim in dimensions.iter().rev() {
        let dim = dim.strip_prefix("VK_").unwrap_or(dim);
        ty = format!("[{ty}; {dim} as usize]");
    }
    if before.contains(':') || after.trim_start().starts_with(':') {
        ty = format!("UNSUPPORTED_BITFIELD_{ty}");
    }
    Field { name: ident(&snake(&name)), base, ty, pointer, constant, array,
        len: e.attr("len").into(), value: e.attr("values").into(), optional: e.attr("optional").starts_with("true") }
}
impl Registry {
    pub fn parse(source: &str) -> Self {
        let doc = Document::parse(source).expect("parse Vulkan registry XML");
        let root = doc.child(0, "registry").expect("registry root");
        let mut r = Self { types: BTreeMap::new(), commands: BTreeMap::new(), values: BTreeMap::new(), revision: String::new() };
        let types = doc.child(root, "types").expect("registry types");
        for id in doc.children(types, "type") {
            let e = &doc.nodes[id];
            if !supported(e) { continue; }
            let proto = doc.child(id, "proto");
            let name = if !e.attr("name").is_empty() { e.attr("name") }
                else { doc.child_text(proto.unwrap_or(id), "name") }.to_owned();
            if name.is_empty() { continue; }
            if name == "VK_HEADER_VERSION" {
                r.revision = e.text.split_whitespace().last().unwrap().into();
            }
            let base = if !e.attr("bitvalues").is_empty() { e.attr("bitvalues").into() }
                else if e.attr("category") == "bitmask" && !e.attr("requires").is_empty() { e.attr("requires").into() }
                else { doc.child_text(id, "type").into() };
            let ret = proto.map(|p| field(&doc, p).ty).unwrap_or_default();
            let field_kind = if proto.is_some() { "param" } else { "member" };
            let fields: Vec<Field> = doc.children(id, field_kind).iter().copied()
                .filter(|&i| supported(&doc.nodes[i])).map(|i| field(&doc, i)).collect();
            r.types.insert(name.clone(), Type { name, category: e.attr("category").into(),
                alias: e.attr("alias").into(), base, fields, ret,
                extends: e.attr("structextends").split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
                dispatchable: e.text.contains("VK_DEFINE_HANDLE") });
        }
        for group in doc.children(root, "enums") {
            let group_name = doc.nodes[group].attr("name");
            for id in doc.children(group, "enum") { r.read_value(&doc.nodes[id], group_name, 0); }
        }
        // Extension values live outside their defining enum group.
        for (id, e) in doc.nodes.iter().enumerate() {
            if e.name != "extension" && e.name != "feature" { continue; }
            if !supported(e) || (!e.attr("supported").is_empty() && !e.attr("supported").split(',').any(|s| s == "vulkan")) { continue; }
            let extension: i64 = e.attr("number").parse().unwrap_or(0);
            for req in doc.children(id, "require") {
                if !supported(&doc.nodes[req]) { continue; }
                for value in doc.children(req, "enum") {
                    let v = &doc.nodes[value];
                    r.read_value(v, v.attr("extends"), extension);
                }
            }
        }
        let commands = doc.child(root, "commands").expect("registry commands");
        for id in doc.children(commands, "command") {
            let e = &doc.nodes[id];
            if !supported(e) { continue; }
            let proto = doc.child(id, "proto");
            let name = proto.map(|p| doc.child_text(p,"name")).unwrap_or(e.attr("name")).to_owned();
            let ret = proto.map(|p| field(&doc,p).ty).unwrap_or_default();
            let fields: Vec<Field> = doc.children(id,"param").iter().copied()
                .filter(|&i| supported(&doc.nodes[i])).map(|i| field(&doc,i)).collect();
            r.commands.insert(name.clone(), Command { name, alias: e.attr("alias").into(), ret, fields });
        }
        r
    }
    fn read_value(&mut self, e: &Element, group: &str, extension: i64) {
        if !supported(e) || e.attr("name").is_empty() { return; }
        let value = if !e.attr("value").is_empty() { e.attr("value").into() }
            else if !e.attr("bitpos").is_empty() { format!("{}", 1u64 << e.attr("bitpos").parse::<u32>().unwrap()) }
            else if !e.attr("offset").is_empty() {
                let number: i64 = e.attr("extnumber").parse().unwrap_or(extension);
                assert!(number > 0);
                let v = 1_000_000_000 + (number-1)*1000 + e.attr("offset").parse::<i64>().unwrap();
                (if e.attr("dir") == "-" { -v } else { v }).to_string()
            } else { String::new() };
        if value.is_empty() && e.attr("alias").is_empty() { return; }
        let v = Value { group: group.into(), value, alias: e.attr("alias").into() };
        self.values.entry(e.attr("name").into()).or_insert(v);
    }
    pub fn command(&self, name: &str) -> Command {
        let c = self.commands.get(name).unwrap_or_else(|| panic!("unknown command {name}"));
        if c.alias.is_empty() { c.clone() } else { let mut v = self.command(&c.alias); v.name = name.into(); v }
    }
    pub fn value(&self, name: &str) -> String {
        let v = self.values.get(name).unwrap_or_else(|| panic!("unknown constant {name}"));
        if v.alias.is_empty() { v.value.clone() } else { self.value(&v.alias) }
    }
    pub fn require_type(&self, name: &str, selected: &mut BTreeSet<String>) {
        if !self.types.contains_key(name) || selected.contains(name) { return; }
        selected.insert(name.into());
        let t = &self.types[name];
        if name.contains("FlagBits") { self.require_type(&name.replace("FlagBits", "Flags"), selected); }
        if !t.alias.is_empty() { self.require_type(&t.alias, selected); }
        if !t.base.is_empty() && t.category != "handle" { self.require_type(&t.base, selected); }
        for f in &t.fields { self.require_type(&f.base, selected); }
        if t.category == "funcpointer" {
            let ret = t.ret.trim_start_matches("*mut ").trim_start_matches("*const ");
            self.require_type(&format!("Vk{ret}"), selected);
        }
    }
}
pub fn method_name(command: &str) -> String {
    let name = command.strip_prefix("vk").unwrap();
    let plain = name.strip_suffix("KHR").or_else(|| name.strip_suffix("EXT"))
        .or_else(|| name.strip_suffix("ANDROID")).unwrap_or(name);
    snake(plain)
}
