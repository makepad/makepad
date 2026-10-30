mod winrt;
mod additive;
use crate::metadata::{self, fail, File, Method, Param, Sig};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

#[derive(Clone, Copy)]
struct Item {
    file: usize,
    table: usize,
    id: u32,
    owner: u32,
}
struct Generator {
    files: Vec<File>,
    index: BTreeMap<String, Item>,
    selected: BTreeSet<String>,
    output: BTreeMap<String, String>,
    callbacks: BTreeSet<String>,
    thread_safe_callbacks: BTreeSet<String>,
    nullable: BTreeSet<String>,
    /// functions resolved at run time (an export newer than the oldest supported system)
    dynamic: BTreeSet<String>,
    module: Option<String>,
    existing: BTreeSet<String>,
}
fn short(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}
fn namespace(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |p| p.0)
}
fn ident(name: &str) -> String {
    let mut s = name.replace('`', "_");
    s = s
        .replace("_Anonymous_e__Union", "_0")
        .replace("_Anonymous_e__Struct", "_0");
    if s == "type"
        || s == "match"
        || s == "self"
        || s == "Self"
        || s == "ref"
        || s == "fn"
        || s == "loop"
        || s == "in"
        || s == "use"
        || s == "move"
        || s == "where"
        || s == "async"
        || s == "box"
    {
        s.insert_str(0, "r#");
    }
    s
}
fn path(name: &str) -> String {
    match name {
        "System.Guid" => String::from("crate::core::GUID"),
        "Windows.Win32.Foundation.HRESULT"
        | "Windows.Win32.Foundation.BOOL"
        | "Windows.Win32.Foundation.PWSTR"
        | "Windows.Win32.Foundation.PSTR"
        | "Windows.Win32.Foundation.BSTR" => String::from("crate::core::") + short(name),
        _ => {
            String::from("crate::")
                + &ident(name.strip_prefix("Windows.").unwrap_or(name)).replace('.', "::")
        }
    }
}
fn write(text: &str) {
    std::io::stdout()
        .write_all(text.as_bytes())
        .expect("write stdout");
}
impl Generator {
    fn path(&self, name: &str) -> String {
        let original = path(name);
        if let Some(module) = &self.module {
            if !original.starts_with("crate::core::") && !self.existing.contains(name) {
                return original.replacen("crate::", &format!("crate::{module}::"), 1);
            }
        }
        original
    }
    fn load(dir: &str) -> Self {
        let mut paths = Vec::new();
        for e in std::fs::read_dir(dir).expect("read metadata dir") {
            let p = e.expect("metadata entry").path();
            if matches!(p.extension().and_then(|v| v.to_str()), Some("winmd" | "tlb")) {
                paths.push(p);
            }
        }
        paths.sort();
        let mut g = Self {
            files: Vec::new(),
            index: BTreeMap::new(),
            selected: BTreeSet::new(),
            output: BTreeMap::new(),
            callbacks: BTreeSet::new(),
            thread_safe_callbacks: BTreeSet::new(),
            nullable: BTreeSet::new(),
            dynamic: BTreeSet::new(),
            module: None,
            existing: BTreeSet::new(),
        };
        for p in paths {
            g.files.push(File::read(p.to_str().expect("metadata path")));
        }
        for fi in 0..g.files.len() {
            let f = &g.files[fi];
            for id in 1..=f.tables[2].len() as u32 {
                let name = f.type_name(id << 2);
                let item = Item {
                    file: fi,
                    table: 2,
                    id,
                    owner: id,
                };
                if let Some(a) = f.attribute(2, id, "SupportedArchitectureAttribute") {
                    if metadata::u32_at(a, 2) & 2 == 0 {
                        continue;
                    }
                }
                if short(&name) == "Apis" {
                    for m in f.range(2, id, 5, 6) {
                        g.index.insert(
                            String::from(namespace(&name)) + "." + f.string(f.row(6, m)[3]),
                            Item {
                                table: 6,
                                id: m,
                                ..item
                            },
                        );
                    }
                    for field in f.range(2, id, 4, 4) {
                        g.index.insert(
                            String::from(namespace(&name)) + "." + f.string(f.row(4, field)[1]),
                            Item {
                                table: 4,
                                id: field,
                                ..item
                            },
                        );
                    }
                } else if name.starts_with("Windows.") {
                    if (name.starts_with("Windows.Win32.") || f.path.ends_with(".tlb"))
                        && f.type_name(f.row(2, id)[3]) == "System.Enum"
                    {
                        for field in f.range(2, id, 4, 4) {
                            if f.string(f.row(4, field)[1]) != "value__" {
                                g.index.insert(
                                    String::from(namespace(&name))
                                        + "."
                                        + f.string(f.row(4, field)[1]),
                                    Item {
                                        table: 4,
                                        id: field,
                                        ..item
                                    },
                                );
                            }
                        }
                    }
                    g.index.entry(name).or_insert(item);
                }
            }
        }
        g
    }
    fn iface(&self, name: &str) -> bool {
        if let Some(item) = self.index.get(name) {
            self.files[item.file].row(2, item.id)[0] & 0x20 != 0
        } else {
            false
        }
    }
    fn delegate(&self, name: &str) -> bool {
        if let Some(item) = self.index.get(name) {
            self.files[item.file].type_name(self.files[item.file].row(2, item.id)[3])
                == "System.MulticastDelegate"
        } else {
            false
        }
    }
    fn require_callback(&mut self, name: &str, thread_safe: bool) {
        self.require(name);
        self.callbacks.insert(name.to_string());
        if thread_safe {
            self.thread_safe_callbacks.insert(name.to_string());
        }
        let item = self.index[name];
        let base = self.files[item.file].base(item.id);
        if !base.is_empty() && short(&base) != "IUnknown" {
            self.require_callback(&base, thread_safe);
        }
    }
    fn require_constants(&mut self, pattern: &str) {
        let (prefix, suffix) = pattern.split_once('*').expect("constant pattern needs *");
        assert!(!suffix.contains('*'), "constant pattern supports one *");
        let names: Vec<_> = self.index.iter().filter_map(|(name, item)| {
            (item.table == 4 && name.starts_with(prefix) && name.ends_with(suffix))
                .then(|| name.clone())
        }).collect();
        assert!(!names.is_empty(), "constant pattern matched nothing: {pattern}");
        for name in names {
            self.require(&name);
        }
    }
    fn nullable_param(&self, item: Item, m: &Method, p: &Param) -> bool {
        p.flags & 16 != 0 || self.nullable.contains(
            &(self.files[item.file].type_name(item.owner << 2) + "." + &m.name + "." + &p.name)
        )
    }
    fn require(&mut self, name: &str) {
        if name.is_empty() || name.starts_with("System.") || self.selected.contains(name)
            || self.existing.contains(name) {
            return;
        }
        let item = *self
            .index
            .get(name)
            .unwrap_or_else(|| fail(&(String::from("metadata item not found: ") + name)));
        let f = &self.files[item.file];
        if item.table == 4 && f.type_name(f.row(2, item.owner)[3]) == "System.Enum" {
            let owner = f.type_name(item.owner << 2);
            self.selected.insert(name.to_string());
            self.require(&owner);
            return;
        }
        self.selected.insert(name.to_string());
        let f = &self.files[item.file];
        let mut deps = Vec::new();
        if item.table == 2 {
            let base = f.base(item.id);
            if !base.is_empty() {
                deps.push(base);
            }
            for id in f.range(2, item.id, 4, 4) {
                sig_deps(&f.field_sig(id), &mut deps);
                if (f.path.ends_with(".tlb") || self.module.is_some()) && f.type_name(f.row(2, item.id)[3]) == "System.Enum"
                    && f.string(f.row(4, id)[1]) != "value__" {
                    deps.push(format!("{}.{}", namespace(name), f.string(f.row(4, id)[1])));
                }
            }
            if self.iface(name) || self.delegate(name) {
                for id in f.range(2, item.id, 5, 6) {
                    let m = f.method(id);
                    sig_deps(&m.ret, &mut deps);
                    for p in m.params {
                        sig_deps(&p.sig, &mut deps);
                        if let Some(name) = self.associated_enum(f, &p) {
                            deps.push(name);
                        }
                    }
                }
            }
        } else if item.table == 6 {
            let m = f.method(item.id);
            sig_deps(&m.ret, &mut deps);
            for p in m.params {
                sig_deps(&p.sig, &mut deps);
                if let Some(name) = self.associated_enum(f, &p) {
                    deps.push(name);
                }
            }
        } else {
            sig_deps(&f.field_sig(item.id), &mut deps);
        }
        for dep in deps {
            self.require(&dep);
        }
    }
    fn abi(&self, s: &Sig, is_const: bool) -> String {
        match s.kind {
            1 => String::from("::core::ffi::c_void"),
            2 => String::from("bool"),
            3 | 7 => String::from("u16"),
            4 => String::from("i8"),
            5 => String::from("u8"),
            6 => String::from("i16"),
            8 => String::from("i32"),
            9 => String::from("u32"),
            10 => String::from("i64"),
            11 => String::from("u64"),
            12 => String::from("f32"),
            13 => String::from("f64"),
            24 => String::from("isize"),
            25 => String::from("usize"),
            14 | 28 => String::from("*mut ::core::ffi::c_void"),
            15 | 16 => {
                String::from(if is_const { "*const " } else { "*mut " })
                    + &self.abi(&s.args[0], false)
            }
            17 | 18 => {
                if s.name == "Windows.Win32.Foundation.BSTR" {
                    return String::from("*mut u16");
                }
                if self.iface(&s.name) {
                    String::from("*mut ::core::ffi::c_void")
                } else if is_const && s.name == "Windows.Win32.Foundation.PWSTR" {
                    String::from("crate::core::PCWSTR")
                } else if is_const && s.name == "Windows.Win32.Foundation.PSTR" {
                    String::from("crate::core::PCSTR")
                } else {
                    self.path(&s.name)
                }
            }
            20 => {
                String::from("[") + &self.abi(&s.args[0], false) + "; " + &s.len.to_string() + "]"
            }
            _ => fail(&(String::from("unsupported ABI signature: ") + &s.describe())),
        }
    }
    fn param_abi(&self, f: &File, p: &Param) -> String {
        self.abi(
            &p.sig,
            f.attribute(8, p.id, "ConstAttribute").is_some()
                || (p.sig.kind == 15 && p.flags & 3 == 1),
        )
    }
    fn return_abi(&self, s: &Sig) -> String {
        if s.kind == 1 {
            String::from("()")
        } else {
            self.abi(s, false)
        }
    }
    fn emit(&mut self, name: &str) {
        let item = self.index[name];
        let f = &self.files[item.file];
        let n = ident(short(name));
        let mut out = String::new();
        if item.table == 6 && self.dynamic.contains(name) {
            self.emit_dynamic(item, &n, &mut out);
        } else if item.table == 6 {
            self.emit_function(item, &n, &mut out);
        } else if item.table == 4 {
            self.emit_constant(item, &n, &mut out);
        } else if self.iface(name) {
            self.emit_interface(item, &n, &mut out);
            let methods = f.range(2, item.id, 5, 6);
            let library_callback = f.path.ends_with(".tlb") && methods.len() == 1
                && f.method(methods.start).name == "Invoke";
            if self.callbacks.contains(name) || library_callback {
                self.emit_callback(item, &n, &mut out);
            }
        } else if self.delegate(name) {
            for id in f.range(2, item.id, 5, 6) {
                let m = f.method(id);
                if m.name != "Invoke" {
                    continue;
                }
                out.push_str("pub type ");
                out.push_str(&n);
                out.push_str(" = Option<unsafe extern \"system\" fn(");
                for i in 0..m.params.len() {
                    let p = &m.params[i];
                    if i != 0 {
                        out.push(',');
                    }
                    out.push_str(&self.param_abi(f, p));
                }
                out.push_str(") -> ");
                out.push_str(&self.return_abi(&m.ret));
                out.push_str(">;\n");
            }
        } else {
            let row = f.row(2, item.id);
            let base = f.type_name(row[3]);
            if let Some(bytes) = f.attribute(2, item.id, "GuidAttribute") {
                out.push_str("pub const ");
                out.push_str(&n);
                out.push_str(": crate::core::GUID = ");
                out.push_str(&guid(bytes));
                out.push_str(";\n");
            } else if base == "System.Enum" {
                let mut repr = String::from("i32");
                for fid in f.range(2, item.id, 4, 4) {
                    if f.string(f.row(4, fid)[1]) == "value__" {
                        repr = self.abi(&f.field_sig(fid), false);
                    }
                }
                self.emit_scalar(&n, &repr, true, &mut out);
            } else if f.attribute(2, item.id, "AliasAttribute").is_some() {
                out.push_str(&format!("pub type {n} = {};\n", self.abi(&f.field_sig(row[4]), false)));
            } else if f.attribute(2, item.id, "NativeTypedefAttribute").is_some()
                || f.attribute(2, item.id, "InvalidHandleValueAttribute")
                    .is_some()
            {
                if ["HRESULT", "BOOL", "PWSTR", "PSTR", "BSTR"].contains(&n.as_str()) {
                    out.push_str("pub use crate::core::");
                    out.push_str(&n);
                    out.push_str(";\n");
                } else {
                    let sig = f.field_sig(f.row(2, item.id)[4]);
                    self.emit_scalar(&n, &self.abi(&sig, false), false, &mut out);
                }
            } else if base == "System.ValueType" {
                let union = row[0] & 0x18 == 0x10;
                let mut packing = 0;
                for layout in &f.tables[15] {
                    if layout[2] == item.id {
                        packing = layout[0];
                    }
                }
                out.push_str("#[repr(C");
                if packing != 0 {
                    out.push_str(", packed(");
                    out.push_str(&packing.to_string());
                    out.push(')');
                }
                out.push_str(")]\n#[derive(Clone, Copy)]\npub ");
                out.push_str(if union { "union " } else { "struct " });
                out.push_str(&n);
                out.push_str(" {\n");
                for fid in f.range(2, item.id, 4, 4) {
                    let field = f.row(4, fid);
                    if field[0] & 0x10 != 0 {
                        continue;
                    }
                    out.push_str("pub ");
                    out.push_str(&ident(f.string(field[1])));
                    out.push_str(": ");
                    out.push_str(&self.abi(
                        &f.field_sig(fid),
                        f.attribute(4, fid, "ConstAttribute").is_some(),
                    ));
                    out.push_str(",\n");
                }
                out.push_str("}\nimpl ");
                out.push_str(&n);
                out.push_str(" { pub fn default() -> Self { unsafe { ::core::mem::zeroed() } } }\nimpl ::core::default::Default for ");
                out.push_str(&n);
                out.push_str(" { fn default() -> Self {Self::default()} }\n");
            } else {
                fail(&(String::from("unsupported type ") + name + " base " + &base));
            }
        }
        self.output
            .entry(namespace(name).to_string())
            .or_default()
            .push_str(&out);
    }
    fn emit_scalar(&self, n: &str, repr: &str, flags: bool, out: &mut String) {
        out.push_str("#[repr(transparent)]\n#[derive(Clone, Copy)]\npub struct ");
        out.push_str(n);
        out.push_str("(pub ");
        out.push_str(repr);
        out.push_str(");\nimpl ");
        out.push_str(n);
        out.push_str(" { pub fn default() -> Self { unsafe { ::core::mem::zeroed() } } pub fn is_invalid(self) -> bool { self.0 as isize == 0 || self.0 as isize == -1 } }\n");
        out.push_str("impl ::core::default::Default for ");
        out.push_str(n);
        out.push_str(" { fn default() -> Self {Self::default()} }\n");
        out.push_str("impl ::core::cmp::PartialEq for ");
        out.push_str(n);
        out.push_str(" { fn eq(&self, other: &Self) -> bool { self.0 == other.0 } }\nimpl ::core::cmp::Eq for ");
        out.push_str(n);
        out.push_str(" {}\n");
        out.push_str("impl ::core::fmt::Debug for ");
        out.push_str(n);
        out.push_str(" { fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result { ::core::fmt::Debug::fmt(&self.0, f) } }\n");
        if flags {
            for (tr, method, op) in [
                ("BitOr", "bitor", "|"),
                ("BitAnd", "bitand", "&"),
                ("BitXor", "bitxor", "^"),
            ] {
                out.push_str("impl ::core::ops::");
                out.push_str(tr);
                out.push_str(" for ");
                out.push_str(n);
                out.push_str(" { type Output = Self; fn ");
                out.push_str(method);
                out.push_str("(self, rhs: Self) -> Self { Self(self.0 ");
                out.push_str(op);
                out.push_str(" rhs.0) } }\n");
            }
            out.push_str("impl ::core::ops::Not for ");
            out.push_str(n);
            out.push_str(" { type Output = Self; fn not(self) -> Self { Self(!self.0) } }\n");
        }
    }
    fn emit_constant(&self, item: Item, n: &str, out: &mut String) {
        let f = &self.files[item.file];
        let sig = f.field_sig(item.id);
        if let Some(bytes) = f.attribute(4, item.id, "GuidAttribute") {
            out.push_str("pub const ");
            out.push_str(n);
            out.push_str(": crate::core::GUID = ");
            out.push_str(&guid(bytes));
            out.push_str(";\n");
            return;
        }
        if let Some(bytes) = f.attribute(4, item.id, "ConstantAttribute") {
            let value = ser_string(bytes, 2);
            let mut parts = Vec::new();
            for p in value.split(|c: char| c == ',' || c == '{' || c == '}') {
                if !p.trim().is_empty() {
                    parts.push(p.trim().to_string());
                }
            }
            if short(&sig.name) == "PROPERTYKEY" && parts.len() == 12 {
                out.push_str("pub const ");
                out.push_str(n);
                out.push_str(": ");
                out.push_str(&self.path(&sig.name));
                out.push_str(" = ");
                out.push_str(&self.path(&sig.name));
                out.push_str(" { fmtid: crate::core::GUID { data1: ");
                out.push_str(&parts[0]);
                out.push_str(", data2: ");
                out.push_str(&parts[1]);
                out.push_str(", data3: ");
                out.push_str(&parts[2]);
                out.push_str(", data4: [");
                for v in &parts[3..11] {
                    out.push_str(v);
                    out.push(',');
                }
                out.push_str("] }, pid: ");
                out.push_str(&parts[11]);
                out.push_str(" };\n");
                return;
            }
            fail(&(String::from("unsupported structured constant ") + n + " " + &value));
        }
        let mut value = None;
        for r in &f.tables[11] {
            if metadata::decode(1, r[1]) == (4, item.id) {
                value = Some((r[0], f.blob(r[2])));
                break;
            }
        }
        let Some((kind, bytes)) = value else {
            if let Some(attrs) = f.attrs.get(&(4, item.id)) {
                for a in attrs {
                    write(&(f.attr_name(*a) + " "));
                    for b in f.blob(f.row(12, *a)[2]) {
                        write(&(b.to_string() + ","));
                    }
                    write("\n");
                }
            }
            fail(&(String::from("constant lacks metadata value: ") + n));
        };
        let mut literal = match kind {
            2 | 5 => bytes[0].to_string(),
            4 => (bytes[0] as i8).to_string(),
            6 => (metadata::u16_at(bytes, 0) as i16).to_string(),
            7 => metadata::u16_at(bytes, 0).to_string(),
            8 => (metadata::u32_at(bytes, 0) as i32).to_string(),
            9 => metadata::u32_at(bytes, 0).to_string(),
            10 => i64::from_le_bytes(bytes.try_into().expect("i64 constant")).to_string(),
            11 => u64::from_le_bytes(bytes.try_into().expect("u64 constant")).to_string(),
            12 => f32::from_le_bytes(bytes.try_into().expect("f32 constant")).to_string() + "f32",
            13 => f64::from_le_bytes(bytes.try_into().expect("f64 constant")).to_string() + "f64",
            14 => {
                let mut units = Vec::new();
                let mut at = 0;
                while at < bytes.len() {
                    units.push(metadata::u16_at(bytes, at));
                    at += 2;
                }
                rust_string(&String::from_utf16(&units).expect("constant string"))
            }
            _ => fail(&(String::from("unsupported constant kind: ") + &kind.to_string() + " " + n)),
        };
        let ty = if kind == 14 {
            String::from("&str")
        } else {
            self.abi(
                &sig,
                f.attribute(4, item.id, "ConstAttribute").is_some()
                    || short(&sig.name) == "PWSTR"
                    || short(&sig.name) == "PSTR",
            )
        };
        if sig.kind == 17 || sig.kind == 18 {
            if (2..=11).contains(&kind) {
                literal.push_str(if kind == 4 || kind == 6 || kind == 8 || kind == 10 {
                    "i64"
                } else {
                    "u64"
                });
            }
            literal = ty.clone() + "(" + &literal + " as _)";
        }
        out.push_str("pub const ");
        out.push_str(n);
        out.push_str(": ");
        out.push_str(&ty);
        out.push_str(" = ");
        out.push_str(&literal);
        out.push_str(";\n");
    }
    /// A function the caller loads itself: its pointer type and where it lives, no import.
    fn emit_dynamic(&self, item: Item, n: &str, out: &mut String) {
        let f = &self.files[item.file];
        let m = f.method(item.id);
        let mut module = String::new();
        let mut entry = String::new();
        for r in &f.tables[28] {
            if metadata::decode(8, r[1]) == (6, item.id) {
                module = f.string(f.row(26, r[3])[0]).to_string();
                entry = f.string(r[2]).to_string();
            }
        }
        if module.is_empty() {
            fail(&(String::from("missing import library: ") + n));
        }
        out.push_str("pub type ");
        out.push_str(n);
        out.push_str(" = unsafe extern \"system\" fn(");
        self.params_abi(f, &m, out, false);
        out.push_str(") -> ");
        out.push_str(&self.return_abi(&m.ret));
        out.push_str(";\n/// The library and NUL-terminated export name of [`");
        out.push_str(n);
        out.push_str("`].\npub const ");
        out.push_str(n);
        out.push_str("_EXPORT: (&str, &str) = (");
        out.push_str(&rust_string(&module));
        out.push_str(", ");
        out.push_str(&rust_string(&(entry + "\0")));
        out.push_str(");\n");
    }
    fn emit_function(&self, item: Item, n: &str, out: &mut String) {
        let f = &self.files[item.file];
        let m = f.method(item.id);
        let mut module = String::new();
        let mut entry = String::new();
        for r in &f.tables[28] {
            if metadata::decode(8, r[1]) == (6, item.id) {
                module = f.string(f.row(26, r[3])[0]).to_string();
                entry = f.string(r[2]).to_string();
            }
        }
        if module.is_empty() {
            fail(&(String::from("missing import library: ") + n));
        }
        let dll = module.strip_suffix(".dll").unwrap_or(&module);
        out.push_str("mod import_");
        out.push_str(n);
        out.push_str(" { #[link(name = ");
        out.push_str(&rust_string(dll));
        out.push_str(", kind = \"raw-dylib\")] extern \"system\" { #[link_name = ");
        out.push_str(&rust_string(&entry));
        out.push_str("] pub fn call(");
        self.params_abi(f, &m, out, false);
        out.push_str(") -> ");
        out.push_str(&self.return_abi(&m.ret));
        out.push_str("; } }\n");
        out.push_str("pub use import_");
        out.push_str(n);
        out.push_str("::call as ");
        out.push_str(n);
        out.push_str("_raw;\n");
        self.wrapper(
            item,
            &m,
            n,
            &(String::from("import_") + n + "::call"),
            false,
            out,
        );
    }
    fn params_abi(&self, f: &File, m: &Method, out: &mut String, with_this: bool) {
        if with_this {
            out.push_str("this: *mut ::core::ffi::c_void");
        }
        for i in 0..m.params.len() {
            let p = &m.params[i];
            if with_this || i != 0 {
                out.push(',');
            }
            out.push_str(&ident(&p.name));
            out.push_str(": ");
            out.push_str(&self.param_abi(f, p));
        }
    }
    fn wrapper(
        &self,
        item: Item,
        m: &Method,
        n: &str,
        call: &str,
        with_this: bool,
        out: &mut String,
    ) {
        let f = &self.files[item.file];
        let hresult = short(&m.ret.name) == "HRESULT";
        let preserve = f
            .attribute(6, m.id, "CanReturnMultipleSuccessValuesAttribute")
            .is_some()
            || f.attribute(6, m.id, "CanReturnErrorsAsSuccessAttribute")
                .is_some();
        let mut retval = None;
        let mut output_count = 0;
        for p in &m.params {
            if p.flags & 2 != 0 {
                output_count += 1;
            }
        }
        if !preserve {
            if let Some(p) = m.params.last() {
                if p.flags & 2 != 0
                    && p.flags & 17 == 0
                    && p.sig.kind == 15
                    && f.attribute(8, p.id, "NativeArrayInfoAttribute").is_none()
                    && (p.flags & 8 != 0 || output_count == 1)
                    && (hresult || self.iface(&p.sig.args[0].name))
                {
                    retval = Some(m.params.len() - 1);
                }
            }
        }
        let mut set_last_error = false;
        for row in &f.tables[28] {
            if metadata::decode(8, row[1]) == (6, m.id) {
                set_last_error = row[0] & 0x40 != 0;
            }
        }
        let bool_error = short(&m.ret.name) == "BOOL" && set_last_error && !preserve;
        let handle_error = set_last_error
            && self.index.get(&m.ret.name).is_some_and(|i| {
                self.files[i.file]
                    .attribute(2, i.id, "InvalidHandleValueAttribute")
                    .is_some()
            });
        let result = (hresult && !preserve) || bool_error || handle_error || retval.is_some();
        let mut array_counts = Vec::new();
        let mut count_uses = BTreeMap::new();
        for p in &m.params {
            let mut index = None;
            if p.sig.kind == 15 {
                if let Some(bytes) = f.attribute(8, p.id, "NativeArrayInfoAttribute") {
                    if let Some(i) = named_integer(bytes, "CountParamIndex") {
                        if (i as usize) < m.params.len() && m.params[i as usize].sig.kind != 15 {
                            index = Some(i as usize);
                            *count_uses.entry(i as usize).or_insert(0usize) += 1;
                        }
                    }
                }
            }
            array_counts.push(index);
        }
        let mut count_arrays = BTreeMap::new();
        for i in 0..array_counts.len() {
            let count = &mut array_counts[i];
            if let Some(c) = *count {
                if count_uses[&c] == 1 {
                    count_arrays.insert(c, i);
                } else {
                    *count = None;
                }
            }
        }
        let mut sig = String::new();
        let mut args = String::new();
        if with_this {
            sig.push_str("&self");
            args.push_str("self.as_raw()");
        }
        for i in 0..m.params.len() {
            let p = &m.params[i];
            if !args.is_empty() {
                args.push(',');
            }
            if Some(i) == retval {
                args.push_str("&mut result__");
                continue;
            }
            if let Some(array) = count_arrays.get(&i) {
                let ap = &m.params[*array];
                let an = ident(&ap.name);
                if ap.flags & 16 != 0 {
                    args.push_str("match &");
                    args.push_str(&an);
                    args.push_str(" { Some(v) => v.len() as _, None => 0 }");
                } else {
                    args.push_str(&an);
                    args.push_str(".len() as _");
                }
                continue;
            }
            if !sig.is_empty() {
                sig.push(',');
            }
            let pn = ident(&p.name);
            sig.push_str(&pn);
            sig.push_str(": ");
            if short(&p.sig.name) == "BOOL" {
                sig.push_str("bool");
                args.push_str("crate::core::BOOL(");
                args.push_str(&pn);
                args.push_str(" as i32)");
            } else if let Some(enumeration) = self.associated_enum(f, p) {
                sig.push_str(&self.path(&enumeration));
                args.push_str(&pn);
                args.push_str(".0 as _");
            } else if p.sig.kind == 15
                && f.attribute(8, p.id, "NativeArrayInfoAttribute")
                    .and_then(|b| named_integer(b, "CountConst"))
                    .is_some()
            {
                let count = named_integer(
                    f.attribute(8, p.id, "NativeArrayInfoAttribute")
                        .expect("array"),
                    "CountConst",
                )
                .expect("count");
                let input = p.flags & 2 == 0;
                let optional = p.flags & 16 != 0;
                if optional {
                    sig.push_str("Option<");
                }
                sig.push_str(if input { "&[" } else { "&mut [" });
                sig.push_str(&self.abi(&p.sig.args[0], false));
                sig.push_str("; ");
                sig.push_str(&count.to_string());
                sig.push(']');
                if optional {
                    sig.push('>');
                    args.push_str("match ");
                    args.push_str(&pn);
                    args.push_str(" { Some(v) => v.");
                    args.push_str(if input { "as_ptr()" } else { "as_mut_ptr()" });
                    args.push_str(" as _, None => ::core::ptr::null_mut() }");
                } else {
                    args.push_str(&pn);
                    args.push_str(if input {
                        ".as_ptr() as _"
                    } else {
                        ".as_mut_ptr() as _"
                    });
                }
            } else if array_counts[i].is_some() {
                let input = p.flags & 2 == 0;
                let optional = p.flags & 16 != 0;
                if optional {
                    sig.push_str("Option<");
                }
                sig.push_str(if input { "&[" } else { "&mut [" });
                let element = &p.sig.args[0];
                if self.iface(&element.name) {
                    sig.push_str("Option<");
                    sig.push_str(&self.path(&element.name));
                    sig.push('>');
                } else if element.kind == 1 {
                    sig.push_str("u8");
                } else {
                    sig.push_str(&self.abi(element, false));
                }
                sig.push(']');
                if optional {
                    sig.push('>');
                    args.push_str("match ");
                    args.push_str(&pn);
                    args.push_str(" { Some(v) => v.");
                    args.push_str(if input { "as_ptr()" } else { "as_mut_ptr()" });
                    args.push_str(" as _, None => ::core::ptr::null_mut() }");
                } else {
                    args.push_str(&pn);
                    args.push_str(if input {
                        ".as_ptr() as _"
                    } else {
                        ".as_mut_ptr() as _"
                    });
                }
            } else if self.iface(&p.sig.name) {
                if p.flags & 16 != 0
                    || self
                        .nullable
                        .contains(&(f.type_name(item.owner << 2) + "." + &m.name + "." + &p.name))
                {
                    sig.push_str("Option<&");
                    sig.push_str(&self.path(&p.sig.name));
                    sig.push('>');
                    args.push_str("match ");
                    args.push_str(&pn);
                    args.push_str(" { Some(v) => v.as_raw(), None => ::core::ptr::null_mut() }");
                } else {
                    sig.push('&');
                    sig.push_str(&self.path(&p.sig.name));
                    args.push_str(&pn);
                    args.push_str(".as_raw()");
                }
            } else if p.sig.kind == 15 && self.iface(&p.sig.args[0].name) {
                let input = p.flags & 2 == 0;
                let pt = String::from(if input {
                    "*const Option<"
                } else {
                    "*mut Option<"
                }) + &self.path(&p.sig.args[0].name)
                    + ">";
                if p.flags & 16 != 0 {
                    sig.push_str("Option<");
                    sig.push_str(&pt);
                    sig.push('>');
                    args.push_str(&pn);
                    args.push_str(if input {
                        ".unwrap_or(::core::ptr::null()) as _"
                    } else {
                        ".unwrap_or(::core::ptr::null_mut()) as _"
                    });
                } else {
                    sig.push_str(&pt);
                    args.push_str(&pn);
                    args.push_str(" as _");
                }
            } else if p.flags & 16 != 0
                && (p.sig.kind == 17 || p.sig.kind == 18)
                && self.index.get(&p.sig.name).is_some_and(|item| {
                    self.files[item.file]
                        .attribute(2, item.id, "NativeTypedefAttribute")
                        .is_some()
                })
            {
                let pt = self.param_abi(f, p);
                sig.push_str("Option<");
                sig.push_str(&pt);
                sig.push('>');
                args.push_str(&pn);
                args.push_str(".unwrap_or(unsafe { ::core::mem::zeroed() })");
            } else {
                let pt = self.param_abi(f, p);
                if p.flags & 16 != 0 && p.sig.kind == 15 {
                    sig.push_str("Option<");
                    sig.push_str(&pt);
                    sig.push('>');
                    args.push_str(&pn);
                    args.push_str(if pt.starts_with("*const ") {
                        ".unwrap_or(::core::ptr::null())"
                    } else {
                        ".unwrap_or(::core::ptr::null_mut())"
                    });
                } else {
                    sig.push_str(&pt);
                    args.push_str(&pn);
                }
            }
        }
        let ret = if let Some(i) = retval {
            let s = &m.params[i].sig.args[0];
            if self.iface(&s.name) {
                if self.nullable_param(item, m, &m.params[i]) {
                    String::from("Option<") + &self.path(&s.name) + ">"
                } else {
                    self.path(&s.name)
                }
            } else {
                self.abi(s, false)
            }
        } else if handle_error {
            self.abi(&m.ret, false)
        } else {
            String::from("()")
        };
        out.push_str("#[inline]\npub unsafe fn ");
        out.push_str(n);
        out.push('(');
        out.push_str(&sig);
        out.push_str(") -> ");
        if result {
            out.push_str("Result<");
            out.push_str(&ret);
            out.push_str(", crate::core::HRESULT>");
        } else {
            out.push_str(&self.return_abi(&m.ret));
        }
        out.push_str(" {\n");
        if retval.is_some() {
            out.push_str("let mut result__ = ::core::mem::zeroed();\n");
        }
        if result && m.ret.kind != 1 {
            out.push_str("let hr__ = ");
        }
        out.push_str(call);
        out.push('(');
        out.push_str(&args);
        out.push(')');
        if result {
            if hresult || bool_error {
                out.push_str("; hr__.ok()?;\n");
            } else if handle_error {
                out.push_str("; if hr__.is_invalid() {return Err(crate::core::HRESULT::from_thread());} return Ok(hr__);\n");
            } else {
                out.push_str(";\n");
            }
            if let Some(i) = retval {
                let s = &m.params[i].sig.args[0];
                if self.iface(&s.name) {
                    if self.nullable_param(item, m, &m.params[i]) {
                        out.push_str("if result__.is_null() { return Ok(None); }\n");
                    }
                    out.push_str(&self.path(&s.name));
                    out.push_str("::from_raw(result__)");
                    if self.nullable_param(item, m, &m.params[i]) {
                        out.push_str(".map(Some)");
                    }
                    out.push('\n');
                } else {
                    out.push_str("Ok(result__)\n");
                }
            } else if !handle_error {
                out.push_str("Ok(())\n");
            }
        }
        out.push_str("}\n");
    }
    fn emit_interface(&self, item: Item, n: &str, out: &mut String) {
        let f = &self.files[item.file];
        let base = f.base(item.id);
        let Some(bytes) = f.attribute(2, item.id, "GuidAttribute") else {
            out.push_str("#[repr(C)]\npub struct ");
            out.push_str(n);
            out.push_str("_Vtbl {\n");
            for id in f.range(2, item.id, 5, 6) {
                let m = f.method(id);
                out.push_str("pub ");
                out.push_str(&self.method_name(item, id));
                out.push_str(": unsafe extern \"system\" fn(");
                self.params_abi(f, &m, out, true);
                out.push_str(") -> ");
                out.push_str(&self.return_abi(&m.ret));
                out.push_str(",\n");
            }
            out.push_str("}\n#[repr(transparent)]\npub struct ");
            out.push_str(n);
            out.push_str("(pub *mut ::core::ffi::c_void);\nimpl ");
            out.push_str(n);
            out.push_str(" { pub fn as_raw(&self) -> *mut ::core::ffi::c_void {self.0} }\n");
            return;
        };
        out.push_str("pub const IID_");
        out.push_str(n);
        out.push_str(": crate::core::GUID = ");
        out.push_str(&guid(bytes));
        out.push_str(";\n#[repr(C)]\npub struct ");
        out.push_str(n);
        out.push_str("_Vtbl {\n");
        if !base.is_empty() {
            out.push_str("pub base__: ");
            out.push_str(&self.path(&base));
            out.push_str("_Vtbl,\n");
        }
        for id in f.range(2, item.id, 5, 6) {
            let m = f.method(id);
            out.push_str("pub ");
            out.push_str(&self.method_name(item, id));
            out.push_str(": unsafe extern \"system\" fn(");
            self.params_abi(f, &m, out, true);
            out.push_str(") -> ");
            out.push_str(&self.return_abi(&m.ret));
            out.push_str(",\n");
        }
        out.push_str("}\n#[repr(transparent)]\npub struct ");
        out.push_str(n);
        out.push_str("(::core::ptr::NonNull<::core::ffi::c_void>);\nimpl ");
        out.push_str(n);
        out.push_str(" {\npub const IID: crate::core::GUID = IID_");
        out.push_str(n);
        out.push_str(";\npub fn as_raw(&self) -> *mut ::core::ffi::c_void { self.0.as_ptr() }\npub fn into_raw(self) -> *mut ::core::ffi::c_void { let p = self.as_raw(); ::core::mem::forget(self); p }\npub unsafe fn from_raw(raw: *mut ::core::ffi::c_void) -> Result<Self, crate::core::HRESULT> { match ::core::ptr::NonNull::new(raw) { Some(p) => Ok(Self(p)), None => Err(crate::core::HRESULT(-2147467261)) } }\npub unsafe fn query(raw: *mut ::core::ffi::c_void) -> Result<Self, crate::core::HRESULT> { Self::from_raw(crate::core::query(raw, &Self::IID)?) }\npub fn vtable(&self) -> &");
        out.push_str(n);
        out.push_str("_Vtbl { unsafe { &**(self.as_raw() as *const *const ");
        out.push_str(n);
        out.push_str("_Vtbl) } }\n");
        let mut chain = base.clone();
        while !chain.is_empty() {
            out.push_str("pub fn as_");
            out.push_str(&ident(short(&chain)));
            out.push_str("(&self) -> &");
            out.push_str(&self.path(&chain));
            out.push_str(" { unsafe { &*(self as *const Self as *const ");
            out.push_str(&self.path(&chain));
            out.push_str(") } }\n");
            let bi = self.index[&chain];
            chain = self.files[bi.file].base(bi.id);
        }
        // Flatten inherited methods without a trait or Deref chain.
        let mut current = Some(item);
        let mut access = String::from("self.vtable().");
        let mut seen = BTreeSet::new();
        while let Some(ci) = current {
            let cf = &self.files[ci.file];
            for id in cf.range(2, ci.id, 5, 6) {
                let m = cf.method(id);
                let name = self.method_name(ci, id);
                if !seen.insert(name.clone()) {
                    continue;
                }
                let call = String::from("(") + &access + &name + ")";
                self.wrapper(ci, &m, &name, &call, true, out);
            }
            let cb = cf.base(ci.id);
            current = if cb.is_empty() {
                None
            } else {
                Some(self.index[&cb])
            };
            access.push_str("base__.");
        }
        out.push_str("}\nimpl Clone for ");
        out.push_str(n);
        out.push_str(" { fn clone(&self) -> Self { unsafe { crate::core::add_ref(self.as_raw()); Self(self.0) } } }\nimpl Drop for ");
        out.push_str(n);
        out.push_str(
            " { fn drop(&mut self) { unsafe { crate::core::release(self.as_raw()); } } }\n",
        );
        out.push_str("impl ::core::cmp::PartialEq for ");
        out.push_str(n);
        out.push_str(
            " { fn eq(&self, other: &Self) -> bool { self.as_raw() == other.as_raw() } }\n",
        );
        if f.attribute(2, item.id, "AgileAttribute").is_some() {
            out.push_str("unsafe impl Send for ");
            out.push_str(n);
            out.push_str(" {}\nunsafe impl Sync for ");
            out.push_str(n);
            out.push_str(" {}\n");
        }
    }

    // Metadata keeps overloaded native names and their vtable order. Rust needs unique
    // field/method names: the second overload is Name2, the third Name3, and so on.
    fn method_name(&self, item: Item, id: u32) -> String {
        let f = &self.files[item.file];
        let name = f.string(f.row(6, id)[3]);
        let first = f.range(2, item.owner, 5, 6).start;
        let index = (first..=id).filter(|mid| f.string(f.row(6, *mid)[3]) == name).count();
        if index == 1 { ident(name) } else { format!("{}{index}", ident(name)) }
    }
}
fn sig_deps(s: &Sig, deps: &mut Vec<String>) {
    if !s.name.is_empty() {
        deps.push(s.name.clone());
    }
    for a in &s.args {
        sig_deps(a, deps);
    }
}
fn rust_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        for e in c.escape_default() {
            out.push(e);
        }
    }
    out.push('"');
    out
}
fn guid(bytes: &[u8]) -> String {
    let mut out = String::from("crate::core::GUID { data1: ")
        + &metadata::u32_at(bytes, 2).to_string()
        + ", data2: "
        + &metadata::u16_at(bytes, 6).to_string()
        + ", data3: "
        + &metadata::u16_at(bytes, 8).to_string()
        + ", data4: [";
    for b in &bytes[10..18] {
        out.push_str(&b.to_string());
        out.push(',');
    }
    out.push_str("] }");
    out
}
pub fn inventory(mut args: std::env::Args) {
    let g = Generator::load(&args.next().expect("metadata dir"));
    let mut words = BTreeSet::new();
    for file in args {
        let s = std::fs::read_to_string(file).expect("source file");
        for word in s.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
            words.insert(word.to_string());
        }
    }
    let mut out = String::from(
        "# Candidate API roots: review namespace collisions and commented code before use.\n",
    );
    for (name, item) in &g.index {
        if !name.starts_with("Windows.Win32.") {
            continue;
        }
        let n = short(name);
        if words.contains(n) && item.owner != 0 {
            out.push_str(name);
            out.push('\n');
        }
    }
    write(&out);
}
pub fn run(mut args: std::env::Args, additive: bool) {
    let dir = args.next().expect("metadata directory");
    let list = args.next().expect("API list");
    let dest = args.next().expect("output file");
    let mut g = Generator::load(&dir);
    if additive {
        g.module = Some(args.next().expect("output module name"));
        g.existing = additive::items(&args.next().expect("existing main bindings"));
    }
    let text = std::fs::read_to_string(list).expect("read API list");
    let mut function_pointers = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(signature) = line.strip_prefix("fnptr ") {
            let words: Vec<&str> = signature.split_whitespace().collect();
            if words.len() < 2 { fail("fnptr requires a qualified name and return type"); }
            for ty in &words[1..] {
                if !["bool", "u32", "PCWSTR"].contains(ty) { g.require(ty); }
            }
            function_pointers.push(words);
        } else if let Some(name) = line.strip_prefix("implement ") {
            let (name, thread_safe) = if let Some((name, bounds)) = name.split_once(':') {
                assert_eq!(bounds.trim(), "Send + Sync", "unsupported callback bounds");
                (name.trim(), true)
            } else {
                (name, false)
            };
            g.require_callback(name, thread_safe);
        } else if let Some(pattern) = line.strip_prefix("constants ") {
            g.require_constants(pattern);
        } else if let Some(name) = line.strip_prefix("dynamic ") {
            g.dynamic.insert(name.to_string());
            g.require(name);
        } else if let Some(name) = line.strip_prefix("nullable ") {
            g.nullable.insert(name.to_string());
        } else if line.starts_with("winrt ") {
            continue;
        } else {
            g.require(line);
        }
    }
    g.require("Windows.Win32.System.Com.IUnknown");
    g.require("Windows.Win32.System.WinRT.IInspectable");
    g.require("Windows.Win32.Foundation.GetLastError");
    let selected = g.selected.clone();
    for name in selected {
        g.emit(&name);
    }
    for words in function_pointers {
        let ty = |name: &str| match name {
            "bool" | "u32" => name.to_string(),
            "PCWSTR" => String::from("crate::core::PCWSTR"),
            _ => g.abi(&Sig { kind: 17, name: name.into(), args: Vec::new(), len: 0 }, false),
        };
        let declaration = format!("pub type {} = unsafe extern \"system\" fn({}) -> {};\n",
            ident(short(words[0])), words[2..].iter().map(|name| ty(name)).collect::<Vec<_>>().join(","), ty(words[1]));
        g.output.entry(namespace(words[0]).into()).or_default().push_str(&declaration);
    }
    let mut out = if additive {
        String::from("// Generated by tools/winbind — do not edit.\n// Inputs and generation command: libs/windows/metadata/provenance.txt\n")
    } else {
        String::from("// Generated by tools/winbind — do not edit.\n// Regenerate (Windows): tools\\winbind\\target\\release\\makepad-winbind.exe generate libs/windows/metadata tools/winbind/apis.txt libs/windows/sys/src/lib.rs\n#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]\n#![cfg(windows)]\npub mod core;\n")
    };
    let mut open: Vec<String> = Vec::new();
    for (namespace, body) in &g.output {
        let mut parts = Vec::new();
        for p in namespace
            .strip_prefix("Windows.")
            .unwrap_or(namespace)
            .split('.')
        {
            parts.push(p.to_string());
        }
        let mut common = 0;
        while common < open.len() && common < parts.len() && open[common] == parts[common] {
            common += 1;
        }
        while open.len() > common {
            out.push_str("}\n");
            open.pop();
        }
        for p in &parts[common..] {
            out.push_str("pub mod ");
            out.push_str(&ident(p));
            out.push_str(" {\n");
            open.push(p.clone());
        }
        out.push_str(body);
    }
    while !open.is_empty() {
        out.push_str("}\n");
        open.pop();
    }
    if !additive {
        out.push_str(&winrt::run(&g, &text));
    }
    let destination = std::path::Path::new(&dest);
    std::fs::create_dir_all(destination.parent().expect("destination directory"))
        .expect("create destination");
    std::fs::write(destination, &out).expect("write bindings");
    if !additive {
        let runtime = std::fs::read_to_string("tools/winbind/core.rs").expect("read runtime support");
        std::fs::write(
            destination.with_file_name("core.rs"),
            String::from("// Generated by tools/winbind — do not edit.\n") + &runtime,
        )
        .expect("write runtime support");
    }
    write(
        &(g.selected.len().to_string()
            + " metadata items, "
            + &out.len().to_string()
            + " generated bytes\n"),
    );
}

fn ser_string(bytes: &[u8], mut at: usize) -> String {
    let len = metadata::compressed(bytes, &mut at) as usize;
    std::str::from_utf8(&bytes[at..at + len])
        .expect("attribute string")
        .to_string()
}

impl Generator {
    fn callback_retval(&self, f: &File, m: &Method) -> Option<usize> {
        if short(&m.ret.name) != "HRESULT"
            || f.attribute(6, m.id, "CanReturnMultipleSuccessValuesAttribute")
                .is_some()
            || f.attribute(6, m.id, "CanReturnErrorsAsSuccessAttribute")
                .is_some()
        {
            return None;
        }
        if let Some(p) = m.params.last() {
            if p.flags & 2 != 0
                && p.flags & 17 == 0
                && p.sig.kind == 15
                && f.attribute(8, p.id, "NativeArrayInfoAttribute").is_none()
            {
                return Some(m.params.len() - 1);
            }
        }
        None
    }
    fn callback_vtable(
        &self,
        item: Item,
        n: &str,
        out: &mut String,
        thunks: &mut String,
        method_count: &mut usize,
    ) -> String {
        let f = &self.files[item.file];
        *method_count += f.range(2, item.id, 5, 6).len();
        let name = f.type_name(item.id << 2);
        if short(&name) == "IUnknown" {
            return String::from("crate::core::IUnknown_Vtbl { QueryInterface: crate::core::object_query, AddRef: crate::core::object_add_ref, Release: crate::core::object_release }");
        }
        let base = f.base(item.id);
        if base.is_empty() {
            fail("implemented interface must derive from IUnknown");
        }
        let mut initializer = self.path(&name) + "_Vtbl { base__: "
            + &self.callback_vtable(self.index[&base], n, out, thunks, method_count)
            + ",\n";
        for id in f.range(2, item.id, 5, 6) {
            let m = f.method(id);
            let mn = ident(&m.name);
            let retval = self.callback_retval(f, &m);
            let preserve = f
                .attribute(6, id, "CanReturnMultipleSuccessValuesAttribute")
                .is_some()
                || f.attribute(6, id, "CanReturnErrorsAsSuccessAttribute")
                    .is_some();
            let result = short(&m.ret.name) == "HRESULT" && !preserve;
            let mut output = String::from("()");
            if let Some(i) = retval {
                let s = &m.params[i].sig.args[0];
                output = if self.iface(&s.name) {
                    if self.nullable_param(item, &m, &m.params[i]) {
                        String::from("Option<") + &self.path(&s.name) + ">"
                    } else {
                        self.path(&s.name)
                    }
                } else {
                    self.abi(s, false)
                };
            }
            out.push_str("fn ");
            out.push_str(&mn);
            out.push_str("(&self");
            let mut callargs = String::new();
            let mut prep = String::new();
            for i in 0..m.params.len() {
                let p = &m.params[i];
                if Some(i) == retval {
                    continue;
                }
                let pn = ident(&p.name);
                out.push_str(", ");
                out.push_str(&pn);
                out.push_str(": ");
                if !callargs.is_empty() {
                    callargs.push(',');
                }
                if self.iface(&p.sig.name) {
                    out.push_str("Option<&");
                    out.push_str(&self.path(&p.sig.name));
                    out.push('>');
                    prep.push_str("let ");
                    prep.push_str(&pn);
                    prep.push_str("_borrow = if ");
                    prep.push_str(&pn);
                    prep.push_str(
                        ".is_null() { None } else { Some(::core::mem::ManuallyDrop::new(",
                    );
                    prep.push_str(&self.path(&p.sig.name));
                    prep.push_str("::from_raw(");
                    prep.push_str(&pn);
                    prep.push_str(").expect(\"non-null callback interface\"))) };\n");
                    callargs.push_str("match &");
                    callargs.push_str(&pn);
                    callargs.push_str("_borrow { Some(p) => Some(&**p), None => None }");
                } else {
                    out.push_str(&self.param_abi(f, p));
                    callargs.push_str(&pn);
                }
            }
            out.push_str(") -> ");
            if result {
                out.push_str("Result<");
                out.push_str(&output);
                out.push_str(", crate::core::HRESULT>");
            } else {
                out.push_str(&self.return_abi(&m.ret));
            }
            out.push_str(";\n");
            let tn = String::from("thunk_") + n + "_" + &mn;
            initializer.push_str(&mn);
            initializer.push_str(": ");
            initializer.push_str(&tn);
            initializer.push_str(",\n");
            thunks.push_str("unsafe extern \"system\" fn ");
            thunks.push_str(&tn);
            thunks.push('(');
            self.params_abi(f, &m, thunks, true);
            thunks.push_str(") -> ");
            thunks.push_str(&self.return_abi(&m.ret));
            thunks.push_str(" {\n");
            if let Some(i) = retval {
                let pn = ident(&m.params[i].name);
                thunks.push_str("if ");
                thunks.push_str(&pn);
                thunks.push_str(".is_null() {return crate::core::HRESULT(-2147467261);}\n");
                // COM callers may read or release an out-param after a failure: start it null.
                thunks.push('*');
                thunks.push_str(&pn);
                thunks.push_str(" = ::core::mem::zeroed();\n");
            }
            thunks.push_str("let implementation = &*((* (this as *const crate::core::ObjectSlot)).implementation as *const Box<dyn ");
            thunks.push_str(n);
            thunks.push_str("Impl>);\n");
            thunks.push_str(&prep);
            if result {
                thunks.push_str("match ");
            }
            thunks.push_str("implementation.");
            thunks.push_str(&mn);
            thunks.push('(');
            thunks.push_str(&callargs);
            thunks.push(')');
            if result {
                thunks.push_str(" { Ok(value__) => { ");
                if let Some(i) = retval {
                    let s = &m.params[i].sig.args[0];
                    thunks.push('*');
                    thunks.push_str(&ident(&m.params[i].name));
                    thunks.push_str(" = value__");
                    if self.iface(&s.name) {
                        if self.nullable_param(item, &m, &m.params[i]) {
                            thunks.push_str(".map_or(::core::ptr::null_mut(), |v| v.into_raw())");
                        } else {
                            thunks.push_str(".into_raw()");
                        }
                    }
                    thunks.push(';');
                } else {
                    thunks.push_str("let _ = value__;");
                }
                thunks.push_str(" crate::core::HRESULT(0) }, Err(error) => error }\n");
            }
            thunks.push_str("}\n");
        }
        initializer.push('}');
        initializer
    }
    fn emit_callback(&self, item: Item, n: &str, out: &mut String) {
        out.push_str("/// Callback methods may receive raw ABI pointers; validate them before use.\n");
        out.push_str("pub trait ");
        out.push_str(n);
        out.push_str("Impl");
        if self.thread_safe_callbacks.contains(&self.files[item.file].type_name(item.id << 2)) {
            out.push_str(": Send + Sync");
        }
        out.push_str(" {\n");
        let mut thunks = String::new();
        let mut method_count = 0;
        let initializer = self.callback_vtable(item, n, out, &mut thunks, &mut method_count);
        out.push_str("}\n");
        out.push_str("const _: () = assert!(::core::mem::size_of::<");
        out.push_str(n);
        out.push_str("_Vtbl>() == ");
        out.push_str(&method_count.to_string());
        out.push_str(" * ::core::mem::size_of::<unsafe extern \"system\" fn()>());\n");
        out.push_str("static IMPLEMENT_VTABLE_");
        out.push_str(n);
        out.push_str(": ");
        out.push_str(n);
        out.push_str("_Vtbl = ");
        out.push_str(&initializer);
        out.push_str(";\n");
        out.push_str(&thunks);
        out.push_str("unsafe fn destroy_");
        out.push_str(n);
        out.push_str("(raw: *mut ::core::ffi::c_void) { drop(Box::from_raw(raw as *mut Box<dyn ");
        out.push_str(n);
        out.push_str("Impl>)); }\nimpl crate::core::ObjectBuilder { pub fn add_");
        out.push_str(n);
        out.push_str("(mut self, implementation: Box<dyn ");
        out.push_str(n);
        out.push_str("Impl>) -> Self { let iids = vec![");
        let mut current = self.files[item.file].type_name(item.id << 2);
        while !current.is_empty() && short(&current) != "IUnknown" {
            out.push_str(&self.path(&current));
            out.push_str("::IID,");
            let ci = self.index[&current];
            current = self.files[ci.file].base(ci.id);
        }
        out.push_str("]; unsafe { self.add(&IMPLEMENT_VTABLE_");
        out.push_str(n);
        out.push_str(" as *const ");
        out.push_str(n);
        out.push_str("_Vtbl as *const ::core::ffi::c_void, Box::into_raw(Box::new(implementation)) as *mut ::core::ffi::c_void, destroy_");
        out.push_str(n);
        out.push_str(", iids); } self } }\nimpl ");
        out.push_str(n);
        out.push_str(" { pub fn implement(implementation: Box<dyn ");
        out.push_str(n);
        out.push_str(
            "Impl>) -> Self { let object = crate::core::ObjectBuilder::new().add_",
        );
        out.push_str(n);
        out.push_str("(implementation); unsafe { Self::from_raw(object.finish().expect(\"COM object\")).expect(\"allocated COM interface\") } } }\n");
    }
}

fn named_integer(bytes: &[u8], name: &str) -> Option<u32> {
    let needle = name.as_bytes();
    let mut at = 4;
    while at + needle.len() < bytes.len() {
        if &bytes[at..at + needle.len()] == needle {
            let start = at + needle.len();
            return match bytes[at - 2] {
                6 | 7 => Some(metadata::u16_at(bytes, start) as u32),
                8 | 9 => Some(metadata::u32_at(bytes, start)),
                _ => None,
            };
        }
        at += 1;
    }
    None
}

impl Generator {
    fn associated_enum(&self, f: &File, p: &Param) -> Option<String> {
        let bytes = f.attribute(8, p.id, "AssociatedEnumAttribute")?;
        let n = ser_string(bytes, 2);
        let mut found = None;
        for (name, item) in &self.index {
            if item.table == 2 && short(name) == n {
                if found.is_some() {
                    return None;
                }
                found = Some(name.clone());
            }
        }
        found
    }
}
