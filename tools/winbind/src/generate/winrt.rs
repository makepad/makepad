// WinRT ABI projection. Interfaces have IInspectable as their ABI base;
// InterfaceImpl entries describe required interfaces, never vtable inheritance.
use super::*;
struct Projection<'a> {
    g: &'a Generator,
    aliases: BTreeMap<String, String>,
    types: Vec<(String, Sig)>,
    values: BTreeSet<String>,
}
fn constant_name(text: &str) -> String {
    let mut result = String::new();
    let mut lower = false;
    for c in text.chars() {
        if c.is_uppercase() && lower {
            result.push('_');
        }
        result.push(c.to_ascii_uppercase());
        lower = c.is_lowercase() || c.is_ascii_digit();
    }
    result
}
fn parse_type(text: &str) -> Sig {
    let kind = match text {
        "bool" => 2,
        "u8" => 5,
        "i32" => 8,
        "u32" => 9,
        "i64" => 10,
        "u64" => 11,
        _ => 0,
    };
    if kind != 0 {
        return Sig {
            kind,
            name: String::new(),
            args: Vec::new(),
            len: 0,
        };
    }
    if text == "IInspectable" {
        return Sig {
            kind: 28,
            name: String::new(),
            args: Vec::new(),
            len: 0,
        };
    }
    if text == "HSTRING" {
        return Sig {
            kind: 14,
            name: String::new(),
            args: Vec::new(),
            len: 0,
        };
    }
    let mut s = Sig {
        kind: 18,
        name: text.to_string(),
        args: Vec::new(),
        len: 0,
    };
    if let Some(start) = text.find('<') {
        s.kind = 21;
        s.name = text[..start].to_string();
        let mut depth = 0;
        let mut begin = start + 1;
        for (at, c) in text.char_indices() {
            if at <= start {
                continue;
            }
            match c {
                '<' => depth += 1,
                '>' if depth != 0 => depth -= 1,
                ',' | '>' if depth == 0 => {
                    s.args.push(parse_type(&text[begin..at]));
                    begin = at + 1;
                }
                _ => {}
            }
        }
    }
    s
}
fn substitute(s: &Sig, args: &[Sig]) -> Sig {
    if s.kind == 19 {
        return args
            .get(s.len as usize)
            .unwrap_or_else(|| fail("unbound WinRT type argument"))
            .clone();
    }
    let mut r = s.clone();
    r.args.clear();
    for a in &s.args {
        r.args.push(substitute(a, args));
    }
    r
}
fn hex(bytes: &[u8]) -> String {
    let mut s = String::new();
    for b in bytes {
        s.push("0123456789abcdef".as_bytes()[(b >> 4) as usize] as char);
        s.push("0123456789abcdef".as_bytes()[(b & 15) as usize] as char);
    }
    s
}
fn guid_text(bytes: &[u8; 16]) -> String {
    String::from("{")
        + &hex(&bytes[..4])
        + "-"
        + &hex(&bytes[4..6])
        + "-"
        + &hex(&bytes[6..8])
        + "-"
        + &hex(&bytes[8..10])
        + "-"
        + &hex(&bytes[10..])
        + "}"
}
fn guid_value(bytes: &[u8; 16]) -> String {
    let mut b = Vec::new();
    b.extend_from_slice(&[1, 0]);
    b.extend_from_slice(&u32::from_be_bytes(bytes[..4].try_into().expect("guid")).to_le_bytes());
    b.extend_from_slice(&u16::from_be_bytes(bytes[4..6].try_into().expect("guid")).to_le_bytes());
    b.extend_from_slice(&u16::from_be_bytes(bytes[6..8].try_into().expect("guid")).to_le_bytes());
    b.extend_from_slice(&bytes[8..]);
    guid(&b)
}
// UUID v5, RFC 4122. Used only while generating concrete parameterized IIDs.
fn uuid(signature: &str) -> [u8; 16] {
    let mut data = Vec::new();
    data.extend_from_slice(&[
        0x11, 0xf4, 0x7a, 0xd5, 0x7b, 0x73, 0x42, 0xc0, 0xab, 0xae, 0x87, 0x8b, 0x1e, 0x16, 0xad,
        0xee,
    ]);
    data.extend_from_slice(signature.as_bytes());
    let bits = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());
    let mut h = [
        0x67452301u32,
        0xefcdab89,
        0x98badcfe,
        0x10325476,
        0xc3d2e1f0,
    ];
    for block in data.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().expect("SHA1 word"));
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | (!b & d), 0x5a827999)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ed9eba1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8f1bbcdc)
            } else {
                (b ^ c ^ d, 0xca62c1d6)
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        let values = [a, b, c, d, e];
        for i in 0..values.len() {
            h[i] = h[i].wrapping_add(values[i]);
        }
    }
    let mut id = [0; 16];
    for i in 0..4 {
        id[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    id[6] = (id[6] & 15) | 0x50;
    id[8] = (id[8] & 63) | 0x80;
    id
}
impl Projection<'_> {
    fn item(&self, s: &Sig) -> Item {
        let item =
            *self.g.index.get(&s.name).unwrap_or_else(|| {
                fail(&(String::from("WinRT metadata type missing: ") + &s.name))
            });
        if item.table != 2 {
            fail(&(String::from("WinRT type is not a TypeDef: ") + &s.name));
        }
        item
    }
    fn default_interface(&self, s: &Sig) -> Sig {
        let i = self.item(s);
        let f = &self.g.files[i.file];
        if f.row(2, i.id)[0] & 0x20 != 0 || self.g.delegate(&s.name) {
            return s.clone();
        }
        for index in 0..f.tables[9].len() {
            let row = &f.tables[9][index];
            if row[0] == i.id
                && f.attribute(9, index as u32 + 1, "DefaultAttribute")
                    .is_some()
            {
                let (table, id) = metadata::decode(0, row[1]);
                return if table == 27 {
                    f.signature(f.blob(f.row(27, id)[0]), &mut 0)
                } else {
                    parse_type(&f.type_name(row[1]))
                };
            }
        }
        fail(&(String::from("WinRT class lacks default interface: ") + &s.name))
    }
    fn guid(&self, s: &Sig) -> [u8; 16] {
        if s.kind == 21 {
            return uuid(&self.signature(s));
        }
        let i = self.item(s);
        let f = &self.g.files[i.file];
        let b = f
            .attribute(2, i.id, "GuidAttribute")
            .unwrap_or_else(|| fail(&(String::from("WinRT IID missing: ") + &s.name)));
        let mut id = [0; 16];
        id[..4].copy_from_slice(&metadata::u32_at(b, 2).to_be_bytes());
        id[4..6].copy_from_slice(&metadata::u16_at(b, 6).to_be_bytes());
        id[6..8].copy_from_slice(&metadata::u16_at(b, 8).to_be_bytes());
        id[8..].copy_from_slice(&b[10..18]);
        id
    }
    fn signature(&self, s: &Sig) -> String {
        let basic = match s.kind {
            2 => "b1",
            3 => "c2",
            4 => "i1",
            5 => "u1",
            6 => "i2",
            7 => "u2",
            8 => "i4",
            9 => "u4",
            10 => "i8",
            11 => "u8",
            12 => "f4",
            13 => "f8",
            14 => "string",
            28 => "cinterface(IInspectable)",
            _ => "",
        };
        if !basic.is_empty() {
            return basic.to_string();
        }
        if s.name == "System.Guid" {
            return String::from("g16");
        }
        if s.kind == 21 {
            let mut base = s.clone();
            base.kind = 18;
            base.args.clear();
            let mut text = String::from("pinterface(") + &guid_text(&self.guid(&base));
            for a in &s.args {
                text.push(';');
                text.push_str(&self.signature(a));
            }
            text.push(')');
            return text;
        }
        let i = self.item(s);
        let f = &self.g.files[i.file];
        let base = f.type_name(f.row(2, i.id)[3]);
        if base == "System.Object" {
            return String::from("rc(")
                + &s.name
                + ";"
                + &self.signature(&self.default_interface(s))
                + ")";
        }
        if base == "System.Enum" {
            return String::from("enum(")
                + &s.name
                + ";"
                + &self.signature(&f.field_sig(f.row(2, i.id)[4]))
                + ")";
        }
        if base == "System.ValueType" {
            let mut text = String::from("struct(") + &s.name;
            for field in f.range(2, i.id, 4, 4) {
                text.push(';');
                text.push_str(&self.signature(&f.field_sig(field)));
            }
            text.push(')');
            return text;
        }
        let id = guid_text(&self.guid(s));
        if base == "System.MulticastDelegate" {
            String::from("delegate(") + &id + ")"
        } else {
            id
        }
    }
    fn value_type(&self, s: &Sig) -> bool {
        if s.name == "System.Guid" {
            return true;
        }
        if s.name.is_empty() {
            return false;
        }
        let i = self.item(s);
        let f = &self.g.files[i.file];
        let base = f.type_name(f.row(2, i.id)[3]);
        base == "System.ValueType" || base == "System.Enum"
    }
    fn collect_values(&mut self, s: &Sig) {
        if !s.name.is_empty()
            && s.name != "System.Guid"
            && self.value_type(s)
            && self.values.insert(s.name.clone())
        {
            let i = self.item(s);
            let f = &self.g.files[i.file];
            for id in f.range(2, i.id, 4, 4) {
                self.collect_values(&f.field_sig(id));
            }
        }
        for a in &s.args {
            self.collect_values(a);
        }
    }
    fn abi(&self, s: &Sig) -> String {
        match s.kind {
            1 => String::from("()"),
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
            16 => String::from("*mut ") + &self.abi(&s.args[0]),
            17 | 18 | 21 => {
                if s.name == "System.Guid" {
                    String::from("crate::core::GUID")
                } else if self.value_type(s) {
                    ident(short(&s.name))
                } else {
                    String::from("*mut ::core::ffi::c_void")
                }
            }
            _ => fail(&(String::from("unsupported WinRT ABI: ") + &s.describe())),
        }
    }
    fn projected(&self, s: &Sig) -> String {
        if s.kind == 14 {
            return String::from("crate::core::HSTRING");
        }
        if let Some(name) = self.aliases.get(&s.describe()) {
            return name.clone();
        }
        if s.kind == 28 {
            return String::from("crate::Win32::System::WinRT::IInspectable");
        }
        self.abi(s)
    }
    fn object(&self, s: &Sig) -> bool {
        s.kind == 28 || self.aliases.contains_key(&s.describe())
    }
    fn emit(&self, n: &str, s: &Sig, out: &mut String) {
        let i = self.item(s);
        let f = &self.g.files[i.file];
        let delegate = self.g.delegate(&s.name);
        let base = if delegate {
            "crate::core::IUnknown"
        } else {
            "crate::Win32::System::WinRT::IInspectable"
        };
        out.push_str("pub const IID_");
        out.push_str(n);
        out.push_str(": crate::core::GUID = ");
        out.push_str(&guid_value(&self.guid(s)));
        out.push_str(";\n#[repr(C)]\npub struct ");
        out.push_str(n);
        out.push_str("_Vtbl { pub base__: ");
        out.push_str(base);
        out.push_str("_Vtbl,\n");
        let mut wrappers = String::new();
        let mut callback = String::new();
        let mut thunks = String::new();
        let mut initializer = String::new();
        if delegate {
            callback.push_str("pub trait ");
            callback.push_str(n);
            callback.push_str("Impl: Send + Sync {\n");
            initializer=String::from("static VTABLE_")+n+": "+n+"_Vtbl = "+n+"_Vtbl { base__: crate::core::IUnknown_Vtbl { QueryInterface: crate::core::object_query, AddRef: crate::core::object_add_ref, Release: crate::core::object_release },\n";
        }
        for mid in f.range(2, i.id, 5, 6) {
            let mut m = f.method(mid);
            if m.name == ".ctor" {
                continue;
            }
            m.ret = substitute(&m.ret, &s.args);
            for p in &mut m.params {
                p.sig = substitute(&p.sig, &s.args);
            }
            let mn = ident(&if let Some(a) = f.attribute(6, mid, "OverloadAttribute") {
                ser_string(a, 2)
            } else {
                m.name.clone()
            });
            let mut abi = String::from("this: *mut ::core::ffi::c_void");
            let mut params = String::from("&self");
            let mut args = String::from("self.as_raw()");
            let mut callback_params = String::from("&self");
            let mut callback_args = String::new();
            let mut borrow = String::new();
            for p in &m.params {
                let pn = ident(&p.name);
                let t = &p.sig;
                abi.push(',');
                params.push(',');
                params.push_str(&pn);
                params.push_str(": ");
                args.push(',');
                callback_params.push(',');
                callback_params.push_str(&pn);
                callback_params.push_str(": ");
                if !callback_args.is_empty() {
                    callback_args.push(',');
                }
                if t.kind == 29 {
                    let element = self.abi(&t.args[0]);
                    let output = p.flags & 2 != 0;
                    abi.push_str(&pn);
                    abi.push_str("_len: u32,");
                    abi.push_str(&pn);
                    abi.push_str(if output { ": *mut " } else { ": *const " });
                    abi.push_str(&element);
                    params.push_str(if output { "&mut [" } else { "&[" });
                    params.push_str(&element);
                    params.push(']');
                    args.push_str(&pn);
                    args.push_str(".len().try_into().expect(\"WinRT array length\"),");
                    args.push_str(&pn);
                    args.push_str(if output { ".as_mut_ptr()" } else { ".as_ptr()" });
                    if delegate {
                        fail("WinRT delegate array projection is not supported");
                    }
                } else {
                    abi.push_str(&pn);
                    abi.push_str(": ");
                    abi.push_str(&self.abi(t));
                    if self.object(t) || t.kind == 14 {
                        params.push('&');
                        params.push_str(&self.projected(t));
                        args.push_str(&pn);
                        args.push_str(".as_raw()");
                    } else {
                        params.push_str(&self.abi(t));
                        args.push_str(&pn);
                    }
                    if self.object(t) {
                        let pt = self.projected(t);
                        callback_params.push_str("Option<&");
                        callback_params.push_str(&pt);
                        callback_params.push('>');
                        borrow.push_str("let ");
                        borrow.push_str(&pn);
                        borrow.push_str("_borrow = if ");
                        borrow.push_str(&pn);
                        borrow.push_str(
                            ".is_null() {None} else {Some(::core::mem::ManuallyDrop::new(",
                        );
                        borrow.push_str(&pt);
                        borrow.push_str("::from_raw(");
                        borrow.push_str(&pn);
                        borrow.push_str(").expect(\"callback interface\")))};\n");
                        callback_args.push_str("match &");
                        callback_args.push_str(&pn);
                        callback_args.push_str("_borrow {Some(v)=>Some(&**v),None=>None}");
                    } else {
                        callback_params.push_str(&self.abi(t));
                        callback_args.push_str(&pn);
                    }
                }
            }
            if m.ret.kind != 1 {
                abi.push_str(", result__: *mut ");
                abi.push_str(&self.abi(&m.ret));
                args.push_str(",&mut result__");
            }
            out.push_str("pub ");
            out.push_str(&mn);
            out.push_str(": unsafe extern \"system\" fn(");
            out.push_str(&abi);
            out.push_str(") -> crate::core::HRESULT,\n");
            wrappers.push_str("#[inline]\npub unsafe fn ");
            wrappers.push_str(&mn);
            wrappers.push('(');
            wrappers.push_str(&params);
            wrappers.push_str(") -> Result<");
            wrappers.push_str(&self.projected(&m.ret));
            wrappers.push_str(",crate::core::HRESULT> {\n");
            if m.ret.kind != 1 {
                wrappers.push_str("let mut result__ = ::core::mem::zeroed();\n");
            }
            wrappers.push_str("(self.vtable().");
            wrappers.push_str(&mn);
            wrappers.push_str(")(");
            wrappers.push_str(&args);
            wrappers.push_str(").ok()?;\n");
            if self.object(&m.ret) {
                wrappers.push_str(&self.projected(&m.ret));
                wrappers.push_str("::from_raw(result__)\n");
            } else if m.ret.kind == 14 {
                wrappers.push_str("Ok(crate::core::HSTRING::from_raw(result__))\n");
            } else if m.ret.kind == 1 {
                wrappers.push_str("Ok(())\n");
            } else {
                wrappers.push_str("Ok(result__)\n");
            }
            wrappers.push_str("}\n");
            if delegate {
                if m.ret.kind != 1 {
                    fail("WinRT delegate return projection is not supported");
                }
                callback.push_str("fn ");
                callback.push_str(&mn);
                callback.push('(');
                callback.push_str(&callback_params);
                callback.push_str(") -> Result<(),crate::core::HRESULT>;\n");
                let tn = String::from("thunk_") + n + "_" + &mn;
                initializer.push_str(&mn);
                initializer.push_str(": ");
                initializer.push_str(&tn);
                initializer.push(',');
                thunks.push_str("unsafe extern \"system\" fn ");
                thunks.push_str(&tn);
                thunks.push('(');
                thunks.push_str(&abi);
                thunks.push_str(") -> crate::core::HRESULT {\n");
                thunks.push_str(&borrow);
                thunks.push_str("let implementation = &*((* (this as *const crate::core::ObjectSlot)).implementation as *const Box<dyn ");
                thunks.push_str(n);
                thunks.push_str("Impl>);\nmatch implementation.");
                thunks.push_str(&mn);
                thunks.push('(');
                thunks.push_str(&callback_args);
                thunks.push_str(") {Ok(())=>crate::core::HRESULT(0),Err(e)=>e}\n}\n");
            }
        }
        out.push_str("}\n");
        out.push_str(&HANDLE.replace("$N", n).replace("$BASE", base));
        out.push_str(&wrappers);
        out.push_str("}\n");
        out.push_str(&OWNERSHIP.replace("$N", n));
        if delegate {
            callback.push_str("}\n");
            initializer.push_str("};\n");
            out.push_str(&callback);
            out.push_str(&initializer);
            out.push_str(&thunks);
            out.push_str(&CALLBACK.replace("$N", n));
        }
    }
}
const HANDLE:&str="#[repr(transparent)]\npub struct $N(::core::ptr::NonNull<::core::ffi::c_void>);\nimpl $N { pub const IID: crate::core::GUID = IID_$N;\npub fn as_raw(&self)->*mut ::core::ffi::c_void {self.0.as_ptr()}\npub fn into_raw(self)->*mut ::core::ffi::c_void {let raw=self.as_raw();::core::mem::forget(self);raw}\npub unsafe fn from_raw(raw:*mut ::core::ffi::c_void)->Result<Self,crate::core::HRESULT>{match ::core::ptr::NonNull::new(raw){Some(p)=>Ok(Self(p)),None=>Err(crate::core::HRESULT(-2147467261))}}\npub unsafe fn query(raw:*mut ::core::ffi::c_void)->Result<Self,crate::core::HRESULT>{Self::from_raw(crate::core::query(raw,&Self::IID)?)}\npub unsafe fn factory(class:&str)->Result<Self,crate::core::HRESULT>{Self::from_raw(crate::core::activation_factory(class,&Self::IID)?)}\npub unsafe fn activate(class:&str)->Result<Self,crate::core::HRESULT>{let instance=crate::core::activate(class)?;Self::query(instance.as_raw())}\npub fn as_base(&self)->&$BASE {unsafe{&*(self as *const Self as *const $BASE)}}\npub fn vtable(&self)->&$N_Vtbl{unsafe{&**(self.as_raw() as *const *const $N_Vtbl)}}\n";
const OWNERSHIP:&str="impl Clone for $N {fn clone(&self)->Self{unsafe{crate::core::add_ref(self.as_raw());Self(self.0)}}}\nimpl Drop for $N{fn drop(&mut self){unsafe{crate::core::release(self.as_raw());}}}\n";
const CALLBACK:&str="unsafe fn destroy_$N(raw:*mut ::core::ffi::c_void){drop(Box::from_raw(raw as *mut Box<dyn $NImpl>));}\nimpl crate::core::ObjectBuilder {pub fn add_$N(&mut self,implementation:Box<dyn $NImpl>){let mut iids=Vec::new();iids.push(IID_$N); iids.push(crate::Win32::System::Com::IAgileObject::IID); unsafe{self.add(&VTABLE_$N as *const $N_Vtbl as *const ::core::ffi::c_void,Box::into_raw(Box::new(implementation)) as *mut ::core::ffi::c_void,destroy_$N,iids);}}}\nimpl $N {pub fn implement(implementation:Box<dyn $NImpl>)->Self{let mut object=crate::core::ObjectBuilder::new();object.add_$N(implementation);unsafe{Self::from_raw(object.finish().expect(\"delegate\")).expect(\"delegate pointer\")}}}\n";
pub(super) fn run(g: &Generator, list: &str) -> String {
    let mut p = Projection {
        g,
        aliases: BTreeMap::new(),
        types: Vec::new(),
        values: BTreeSet::new(),
    };
    for line in list.lines() {
        if let Some(line) = line.trim().strip_prefix("winrt ") {
            let (alias, text) = line.split_once(' ').expect("winrt Alias Type");
            let ty = parse_type(text.trim());
            let interface = p.default_interface(&ty);
            p.aliases.insert(ty.describe(), alias.to_string());
            p.aliases.insert(interface.describe(), alias.to_string());
            let mut present = false;
            for (name, existing) in &p.types {
                if name == alias {
                    if existing.describe() != interface.describe() {
                        fail("WinRT alias names different interfaces");
                    }
                    present = true;
                }
            }
            if !present {
                p.types.push((alias.to_string(), interface));
            }
        }
    }
    for (_, s) in p.types.clone() {
        let i = p.item(&s);
        let f = &g.files[i.file];
        for id in f.range(2, i.id, 5, 6) {
            let m = f.method(id);
            if m.name == ".ctor" {
                continue;
            }
            p.collect_values(&substitute(&m.ret, &s.args));
            for arg in m.params {
                p.collect_values(&substitute(&arg.sig, &s.args));
            }
        }
    }
    let mut out = String::from("pub mod WinRT {\n");
    for name in &p.values {
        let s = parse_type(name);
        let i = p.item(&s);
        let f = &g.files[i.file];
        let n = ident(short(name));
        let base = f.type_name(f.row(2, i.id)[3]);
        if base == "System.Enum" {
            out.push_str("pub type ");
            out.push_str(&n);
            out.push_str(" = ");
            out.push_str(&p.abi(&f.field_sig(f.row(2, i.id)[4])));
            out.push_str(";\n");
            for id in f.range(2, i.id, 4, 4) {
                if f.row(4, id)[0] & 0x40 == 0 {
                    continue;
                }
                for row in &f.tables[11] {
                    if metadata::decode(1, row[1]) != (4, id) {
                        continue;
                    }
                    let bytes = f.blob(row[2]);
                    let value = metadata::u32_at(bytes, 0);
                    out.push_str("pub const ");
                    out.push_str(&constant_name(&n));
                    out.push('_');
                    out.push_str(&constant_name(f.string(f.row(4, id)[1])));
                    out.push_str(": ");
                    out.push_str(&n);
                    out.push_str(" = ");
                    out.push_str(&value.to_string());
                    out.push_str("u32 as ");
                    out.push_str(&n);
                    out.push_str(";\n");
                }
            }
        } else {
            out.push_str("#[repr(C)]\n#[derive(Clone,Copy)]\npub struct ");
            out.push_str(&n);
            out.push_str(" {\n");
            for id in f.range(2, i.id, 4, 4) {
                out.push_str("pub ");
                out.push_str(&ident(f.string(f.row(4, id)[1])));
                out.push_str(": ");
                out.push_str(&p.abi(&f.field_sig(id)));
                out.push(',');
            }
            out.push_str("}\n");
        }
    }
    for (name, s) in &p.types {
        p.emit(name, s, &mut out);
    }
    out.push_str("}\n");
    out
}
