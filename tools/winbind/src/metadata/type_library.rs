//! MSFT COM type libraries, normalized into the same ECMA-335 tables used by winmd.
//! Member records carry native vtable offsets, invocation kinds and PARAMFLAG directions.
//! A library's marshaling aliases for OS types resolve to the native Win32 metadata types;
//! their wire representations must never become native handles or duplicate COM interfaces.
use super::{fail, File};
use std::collections::BTreeMap;

const NS: &str = "Windows.TypeLibrary";

struct Reader<'a> {
    data: &'a [u8],
    segments: Vec<&'a [u8]>,
    types: BTreeMap<u32, (String, u32)>,
    pointer_size: u32,
}

fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data.get(at..at.checked_add(4).expect("type library offset overflow"))
        .unwrap_or_else(|| fail("truncated MSFT type library")).try_into().unwrap())
}
fn half(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data.get(at..at.checked_add(2).expect("type library offset overflow"))
        .unwrap_or_else(|| fail("truncated MSFT type library")).try_into().unwrap())
}
fn compact(value: u32, out: &mut Vec<u8>) {
    if value < 0x80 { out.push(value as u8); }
    else if value < 0x4000 { out.extend_from_slice(&((value as u16) | 0x8000).to_be_bytes()); }
    else if value < 0x20000000 { out.extend_from_slice(&(value | 0xc0000000).to_be_bytes()); }
    else { fail("metadata compressed integer overflow"); }
}

// These are ABI mappings, not system web view interfaces. The same names occur in any
// MIDL library importing objidl/oaidl/wtypes. All definitions still come from winmd.
fn native(name: &str) -> Option<&'static str> {
    Some(match name {
        "GUID" => "System.Guid",
        "IUnknown" => "Windows.Win32.System.Com.IUnknown",
        "IStream" => "Windows.Win32.System.Com.IStream",
        "ISequentialStream" => "Windows.Win32.System.Com.ISequentialStream",
        "IDataObject" => "Windows.Win32.System.Com.IDataObject",
        "IEnumFORMATETC" => "Windows.Win32.System.Com.IEnumFORMATETC",
        "IAdviseSink" => "Windows.Win32.System.Com.IAdviseSink",
        "IMoniker" => "Windows.Win32.System.Com.IMoniker",
        "IPersist" => "Windows.Win32.System.Com.IPersist",
        "IPersistStream" => "Windows.Win32.System.Com.IPersistStream",
        "IBindCtx" => "Windows.Win32.System.Com.IBindCtx",
        "IRunningObjectTable" => "Windows.Win32.System.Com.IRunningObjectTable",
        "IEnumMoniker" => "Windows.Win32.System.Com.IEnumMoniker",
        "IEnumString" => "Windows.Win32.System.Com.IEnumString",
        "IEnumSTATDATA" => "Windows.Win32.System.Com.IEnumSTATDATA",
        "wireHWND" => "Windows.Win32.Foundation.HWND",
        "wireHICON" => "Windows.Win32.UI.WindowsAndMessaging.HICON",
        "wireCLIPFORMAT" => "Windows.Win32.System.Ole.CLIPBOARD_FORMAT",
        "tagRECT" => "Windows.Win32.Foundation.RECT",
        "tagPOINT" => "Windows.Win32.Foundation.POINT",
        "_FILETIME" => "Windows.Win32.Foundation.FILETIME",
        "EventRegistrationToken" => "Windows.WinRT.EventRegistrationToken",
        _ => return None,
    })
}

struct Model {
    file: File,
    references: BTreeMap<String, u32>,
}
impl Model {
    fn new(path: &str) -> Self {
        Self { file: File {
            path: path.to_string(), tables: vec![Vec::new(); 64], strings: vec![0],
            blobs: vec![0], attrs: BTreeMap::new(), nested: BTreeMap::new(),
        }, references: BTreeMap::new() }
    }
    fn row(&mut self, table: usize, row: Vec<u32>) -> u32 {
        self.file.tables[table].push(row);
        self.file.tables[table].len() as u32
    }
    fn string(&mut self, text: &str) -> u32 {
        let offset = self.file.strings.len() as u32;
        self.file.strings.extend_from_slice(text.as_bytes()); self.file.strings.push(0);
        offset
    }
    fn blob(&mut self, bytes: &[u8]) -> u32 {
        let offset = self.file.blobs.len() as u32;
        compact(bytes.len().try_into().expect("metadata blob length"), &mut self.file.blobs);
        self.file.blobs.extend_from_slice(bytes);
        offset
    }
    fn reference(&mut self, name: &str) -> u32 {
        if let Some(token) = self.references.get(name) { return *token; }
        let (ns, short) = name.rsplit_once('.').expect("qualified type name");
        let ns = self.string(ns); let short = self.string(short);
        let token = (self.row(1, vec![0, short, ns]) << 2) | 1;
        self.references.insert(name.to_string(), token);
        token
    }
    fn named(&mut self, name: &str) -> Vec<u8> {
        let mut sig = vec![17]; compact(self.reference(name), &mut sig); sig
    }
    fn attr(&mut self, table: usize, id: u32, name: &str, value: &[u8]) {
        let token = self.reference(&format!("Windows.Win32.Foundation.Metadata.{name}"));
        let ctor_name = self.string(".ctor");
        let ctor = self.row(10, vec![((token >> 2) << 3) | 1, ctor_name, 0]);
        let mut bytes = vec![1, 0]; bytes.extend_from_slice(value); bytes.extend_from_slice(&[0, 0]);
        let blob = self.blob(&bytes);
        let tag = match table { 2 => 3, 4 => 1, 6 => 0, 8 => 4, _ => unreachable!() };
        let aid = self.row(12, vec![(id << 5) | tag, (ctor << 3) | 3, blob]);
        self.file.attrs.entry((table, id)).or_default().push(aid);
    }
    fn field(&mut self, name: &str, sig: Vec<u8>, constant: Option<i32>) {
        let name = self.string(name); let mut bytes = vec![6]; bytes.extend_from_slice(&sig);
        let sig = self.blob(&bytes);
        let id = self.row(4, vec![if constant.is_some() {0x56} else {6}, name, sig]);
        if let Some(value) = constant {
            let value = self.blob(&value.to_le_bytes());
            self.row(11, vec![8, id << 2, value]);
        }
    }
}

impl Reader<'_> {
    fn name(&self, offset: u32) -> &str {
        let at = offset as usize;
        let len = (word(self.segments[7], at + 8) & 0xff) as usize;
        std::str::from_utf8(self.segments[7].get(at + 12..at + 12 + len)
            .unwrap_or_else(|| fail("invalid MSFT name offset")))
            .unwrap_or_else(|_| fail("non-UTF8 MSFT identifier"))
    }
    fn named(&self, offset: u32) -> &(String, u32) {
        self.types.get(&offset).unwrap_or_else(|| fail("unknown MSFT type reference"))
    }
    fn signature(&self, value: u32, model: &mut Model, visiting: &mut Vec<u32>) -> Vec<u8> {
        let (vt, argument) = if value & 0x80000000 != 0 { (value & 0xffff, 0) } else {
            if visiting.contains(&value) { fail("cyclic MSFT type descriptor"); }
            visiting.push(value);
            (word(self.segments[9], value as usize) & 0xffff, word(self.segments[9], value as usize + 4))
        };
        let sig = match vt {
            2 => vec![6], 3 | 22 => vec![8], 4 => vec![12], 5 => vec![13],
            16 => vec![4], 17 => vec![5], 18 => vec![7], 19 | 23 => vec![9],
            20 => vec![10], 21 => vec![11], 24 => vec![1],
            8 => model.named("Windows.Win32.Foundation.BSTR"),
            11 => model.named("Windows.Win32.Foundation.VARIANT_BOOL"),
            12 => model.named("Windows.Win32.System.Variant.VARIANT"),
            13 => model.named("Windows.Win32.System.Com.IUnknown"),
            25 => model.named("Windows.Win32.Foundation.HRESULT"),
            30 => model.named("Windows.Win32.Foundation.PSTR"),
            31 => model.named("Windows.Win32.Foundation.PWSTR"),
            26 => {
                let inner = self.signature(argument, model, visiting);
                // A COM interface's named signature already denotes one native pointer,
                // unlike a struct/enum. Remove exactly the first TLB VT_PTR around it.
                let interface = if argument & 0x80000000 == 0 && word(self.segments[9], argument as usize) & 0xffff == 29 {
                    self.named(word(self.segments[9], argument as usize + 4)).1 == 3
                } else { false };
                if interface { inner } else { let mut sig = vec![15]; sig.extend(inner); sig }
            }
            28 => {
                let array = self.segments[10]; let at = argument as usize;
                let element = self.signature(word(array, at), model, visiting);
                if half(array, at + 4) != 1 || word(array, at + 12) != 0 {
                    fail("only zero-based one-dimensional MSFT arrays are supported");
                }
                let mut sig = vec![20]; sig.extend(element);
                compact(1, &mut sig); compact(1, &mut sig);
                compact(word(array, at + 8), &mut sig); compact(0, &mut sig); sig
            }
            29 => model.named(&self.named(argument).0),
            _ => fail(&format!("unsupported MSFT VARTYPE {vt}")),
        };
        if value & 0x80000000 == 0 { visiting.pop(); }
        sig
    }
    fn sig(&self, value: u32, model: &mut Model) -> Vec<u8> {
        self.signature(value, model, &mut Vec::new())
    }
    fn constant(&self, value: u32) -> i32 {
        if value & 0x80000000 != 0 {
            if (value >> 26) & 31 != 3 { fail("unsupported packed MSFT enum constant"); }
            // Small constants occupy 26 bits; negative/larger values use custom data.
            (value & 0x03ffffff) as i32
        } else {
            let data = self.segments[11];
            if half(data, value as usize) != 3 { fail("unsupported MSFT enum constant"); }
            word(data, value as usize + 2) as i32
        }
    }
}

pub(super) fn read(path: &str, data: &[u8]) -> File {
    if word(data, 4) != 0x10002 { fail("unsupported MSFT type library version"); }
    let count = word(data, 32) as usize;
    let pointer_size = match word(data, 20) & 0xf { 1 => 4, 3 => 8, _ => fail("unsupported MSFT system kind") };
    let directory = 84usize.checked_add(count.checked_mul(4).expect("MSFT type count overflow"))
        .expect("MSFT directory overflow") + if word(data, 20) & 0x100 != 0 {4} else {0};
    let mut segments = Vec::new();
    for i in 0..15 {
        let at = word(data, directory + 16 * i) as usize;
        let len = word(data, directory + 16 * i + 4) as usize;
        segments.push(if len == 0 { &data[0..0] } else {
            data.get(at..at.checked_add(len).expect("MSFT segment overflow"))
                .unwrap_or_else(|| fail("MSFT segment outside input"))
        });
    }
    let mut reader = Reader { data, segments, types: BTreeMap::new(), pointer_size };
    for i in 0..count {
        let offset = word(data, 84 + 4 * i); let at = offset as usize;
        let name = reader.name(word(reader.segments[0], at + 52));
        let full = native(name).map(str::to_string).unwrap_or_else(|| format!("{NS}.{name}"));
        let kind = word(reader.segments[0], at) & 15;
        reader.types.insert(offset, (full, kind));
    }
    let mut model = Model::new(path);
    for i in 0..count {
        let offset = word(data, 84 + 4 * i); let at = offset as usize;
        let (full, kind) = reader.named(offset);
        if !full.starts_with(&format!("{NS}.")) { continue; }
        let info = reader.segments[0];
        let name = model.string(full.rsplit('.').next().unwrap()); let ns = model.string(NS);
        let parent = match kind {
            0 => model.reference("System.Enum"),
            1 | 6 | 7 => model.reference("System.ValueType"),
            3 | 5 => 0,
            _ => fail(&format!("unsupported MSFT type kind {kind}")),
        };
        let flags = match kind {3 => 0x20, 7 => 0x10, _ => 0x8};
        let id = model.row(2, vec![flags, name, ns, parent,
            model.file.tables[4].len() as u32 + 1, model.file.tables[6].len() as u32 + 1]);
        if matches!(*kind, 1 | 7) {
            let alignment = (word(info, at) >> 11) & 31;
            if alignment == 0 || !alignment.is_power_of_two() { fail("invalid MSFT record alignment"); }
            model.row(15, vec![alignment, word(info, at + 80), id]);
        }
        let guid = word(info, at + 44);
        if guid != u32::MAX {
            let value = reader.segments[5].get(guid as usize..guid as usize + 16)
                .unwrap_or_else(|| fail("invalid MSFT GUID offset"));
            model.attr(2, id, "GuidAttribute", value);
        }
        if *kind == 6 {
            model.attr(2, id, "AliasAttribute", &[]);
            let sig = reader.sig(word(info, at + 84), &mut model);
            model.field("value", sig, None);
            continue;
        }
        if *kind == 5 { continue; } // A coclass is its CLSID, not an invented interface.
        let inherited = half(info, at + 76);
        let base_slots = if *kind == 3 && inherited != 0 {
            if inherited != 1 { fail("multiple MSFT interface bases"); }
            let base_offset = word(info, at + 84);
            let base = model.reference(&reader.named(base_offset).0);
            model.row(9, vec![id, base]);
            half(info, base_offset as usize + 78) as u32 / pointer_size
        } else { 0 };
        let nf = word(info, at + 24) as usize & 0xffff;
        let nv = (word(info, at + 24) >> 16) as usize;
        let member = word(info, at + 4) as usize;
        let length = word(reader.data, member) as usize;
        let records = reader.data.get(member + 4..member + 4 + length)
            .unwrap_or_else(|| fail("MSFT members outside input"));
        let tail = member + 4 + length;
        let mut methods = Vec::new();
        if *kind == 0 { model.field("value__", vec![8], None); }
        for j in 0..nf + nv {
            let record_at = word(data, tail + 8 * (nf + nv) + 4 * j) as usize;
            let size = (word(records, record_at) & 0xffff) as usize;
            let record = records.get(record_at..record_at + size)
                .unwrap_or_else(|| fail("invalid MSFT member record"));
            let name = reader.name(word(data, tail + 4 * (nf + nv) + 4 * j));
            if j < nf {
                if half(record, 12) as u32 % reader.pointer_size != 0 {
                    fail("unaligned MSFT vtable slot");
                }
                if (word(record, 16) >> 8) & 15 != 4 {
                    fail("MSFT COM methods must use the system calling convention");
                }
                methods.push((half(record, 12) as u32 / reader.pointer_size, name, record));
            } else if *kind == 0 {
                let sig = model.named(full);
                model.field(name, sig, Some(reader.constant(word(record, 16))));
            } else {
                let sig = reader.sig(word(record, 4), &mut model);
                model.field(name, sig, None);
            }
        }
        methods.sort_by_key(|m| m.0);
        for (index, (slot, name, record)) in methods.into_iter().enumerate() {
            if slot != base_slots + index as u32 { fail("noncontiguous MSFT vtable slots"); }
            let invocation = (word(record, 16) >> 3) & 15;
            let name = match invocation {
                1 => name.to_string(), 2 => format!("get_{name}"),
                4 | 8 => format!("put_{name}"), _ => fail("unsupported MSFT invocation kind"),
            };
            let argc = half(record, 20) as usize;
            let start = record.len().checked_sub(argc * 12).expect("MSFT parameter record length");
            if start < 24 { fail("truncated MSFT function record"); }
            let mut sig = vec![0x20]; compact(argc as u32, &mut sig);
            sig.extend(reader.sig(word(record, 4), &mut model));
            let first_param = model.file.tables[8].len() as u32 + 1;
            for p in 0..argc {
                let at = start + p * 12; let ty = reader.sig(word(record, at), &mut model);
                sig.extend_from_slice(&ty);
                let flags = word(record, at + 8);
                let name_offset = word(record, at + 4);
                let name = if name_offset == u32::MAX {format!("arg{p}")} else {reader.name(name_offset).to_string()};
                let name = model.string(&name);
                let pid = model.row(8, vec![flags, p as u32 + 1, name]);
                let vt = word(record, at);
                if flags & 3 == 1 && vt & 0x80000000 != 0 && [30,31].contains(&(vt & 0xffff)) {
                    model.attr(8, pid, "ConstAttribute", &[]);
                }
            }
            let name = model.string(&name); let sig = model.blob(&sig);
            model.row(6, vec![0, 0, 0x46, name, sig, first_param]);
        }
        if *kind == 3 && half(info, at + 78) as u32 != (base_slots + nf as u32) * pointer_size {
            fail("MSFT vtable size disagrees with method records");
        }
    }
    model.file
}
