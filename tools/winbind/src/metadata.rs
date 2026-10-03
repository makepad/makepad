//! ECMA-335 Partition II reader. All indexes are checked against their table/heaps.
use std::collections::BTreeMap;

// Column tags: 2/4 fixed width, 0x70 strings, 0x71 GUID, 0x72 blob,
// 0x100 + table, 0x200 + coded-index kind (II.24.2.6).
const SCHEMAS: &[&[u16]] = &[
    &[2, 0x70, 0x71, 0x71, 0x71],
    &[0x20b, 0x70, 0x70],
    &[4, 0x70, 0x70, 0x200, 0x104, 0x106],
    &[0x104],
    &[2, 0x70, 0x72],
    &[0x106],
    &[4, 2, 2, 0x70, 0x72, 0x108],
    &[0x108],
    &[2, 2, 0x70],
    &[0x102, 0x200],
    &[0x205, 0x70, 0x72],
    &[2, 0x201, 0x72],
    &[0x202, 0x20a, 0x72],
    &[0x203, 0x72],
    &[2, 0x204, 0x72],
    &[2, 4, 0x102],
    &[4, 0x104],
    &[0x72],
    &[0x102, 0x114],
    &[0x114],
    &[2, 0x70, 0x200],
    &[0x102, 0x117],
    &[0x117],
    &[2, 0x70, 0x72],
    &[2, 0x106, 0x206],
    &[0x102, 0x207, 0x207],
    &[0x70],
    &[0x72],
    &[2, 0x208, 0x70, 0x11a],
    &[4, 0x104],
    &[4, 4],
    &[4],
    &[4, 2, 2, 2, 2, 4, 0x72, 0x70, 0x70],
    &[4],
    &[4, 4, 4],
    &[2, 2, 2, 2, 4, 0x72, 0x70, 0x70, 0x72],
    &[4, 0x123],
    &[4, 4, 4, 0x123],
    &[4, 0x70, 0x72],
    &[4, 4, 0x70, 0x70, 0x209],
    &[4, 4, 0x70, 0x209],
    &[0x102, 0x102],
    &[2, 2, 0x20c, 0x70],
    &[0x207, 0x72],
    &[0x12a, 0x200],
];
const CODED_TABLES: &[&[usize]] = &[
    &[2, 1, 27],
    &[4, 8, 23],
    &[
        6, 4, 1, 2, 8, 9, 10, 0, 14, 23, 20, 17, 26, 27, 32, 35, 38, 39, 40, 42, 44, 43,
    ],
    &[4, 8],
    &[2, 6, 32],
    &[2, 1, 26, 6, 27],
    &[20, 23],
    &[6, 10],
    &[4, 6],
    &[38, 35, 39],
    &[usize::MAX, usize::MAX, 6, 10, usize::MAX],
    &[0, 26, 35, 1],
    &[2, 6],
];
const CODED_BITS: &[u32] = &[2, 2, 5, 1, 2, 3, 1, 1, 1, 2, 3, 2, 1];

pub struct File {
    pub path: String,
    pub tables: Vec<Vec<Vec<u32>>>,
    strings: Vec<u8>,
    blobs: Vec<u8>,
    pub attrs: BTreeMap<(usize, u32), Vec<u32>>,
    pub nested: BTreeMap<u32, u32>,
}

pub fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().expect("truncated u16"))
}
pub fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("truncated u32"))
}
pub fn compressed(data: &[u8], at: &mut usize) -> u32 {
    let first = data[*at];
    *at += 1;
    if first & 0x80 == 0 {
        return first as u32;
    }
    if first & 0xc0 == 0x80 {
        let value = (((first & 0x3f) as u32) << 8) | data[*at] as u32;
        *at += 1;
        return value;
    }
    if first & 0xe0 != 0xc0 {
        fail("invalid compressed integer");
    }
    let value = (((first & 0x1f) as u32) << 24)
        | ((data[*at] as u32) << 16)
        | ((data[*at + 1] as u32) << 8)
        | data[*at + 2] as u32;
    *at += 3;
    value
}
pub fn fail(message: &str) -> ! {
    use std::io::Write;
    let _ = std::io::stderr().write_all(message.as_bytes());
    let _ = std::io::stderr().write_all(b"\n");
    std::process::exit(1)
}
fn rva(data: &[u8], pe: usize, address: u32) -> usize {
    let count = u16_at(data, pe + 6) as usize;
    let sections = pe + 24 + u16_at(data, pe + 20) as usize;
    for i in 0..count {
        let s = sections + i * 40;
        let start = u32_at(data, s + 12);
        let size = u32_at(data, s + 8).max(u32_at(data, s + 16));
        if address >= start && address - start < size {
            return (u32_at(data, s + 20) + address - start) as usize;
        }
    }
    fail("RVA outside PE sections")
}
impl File {
    pub fn read(path: &str) -> Self {
        let data = std::fs::read(path).expect("read metadata file");
        if &data[0..2] != b"MZ" {
            fail("metadata is not a PE file");
        }
        let pe = u32_at(&data, 0x3c) as usize;
        if &data[pe..pe + 4] != b"PE\0\0" {
            fail("bad PE signature");
        }
        let opt = pe + 24;
        let dir = match u16_at(&data, opt) {
            0x10b => opt + 96,
            0x20b => opt + 112,
            _ => fail("unknown PE optional header"),
        };
        let cli = rva(&data, pe, u32_at(&data, dir + 14 * 8));
        let root = rva(&data, pe, u32_at(&data, cli + 8));
        if &data[root..root + 4] != b"BSJB" {
            fail("bad metadata root");
        }
        let mut at = root + 16 + u32_at(&data, root + 12) as usize;
        at = (at + 3) & !3;
        let streams = u16_at(&data, at + 2);
        at += 4;
        let mut strings = Vec::new();
        let mut blobs = Vec::new();
        let mut tables = Vec::new();
        for _ in 0..streams {
            let offset = u32_at(&data, at) as usize;
            let size = u32_at(&data, at + 4) as usize;
            at += 8;
            let start = at;
            while data[at] != 0 {
                at += 1;
            }
            let name = std::str::from_utf8(&data[start..at]).expect("stream name");
            at = (at + 4) & !3;
            match name {
                "#Strings" => strings = data[root + offset..root + offset + size].to_vec(),
                "#Blob" => blobs = data[root + offset..root + offset + size].to_vec(),
                "#~" | "#-" => tables = data[root + offset..root + offset + size].to_vec(),
                _ => (),
            }
        }
        let heap_flags = tables[6];
        let valid = u64::from_le_bytes(tables[8..16].try_into().expect("valid mask"));
        let mut counts = [0u32; 64];
        at = 24;
        for i in 0..64 {
            if valid & (1u64 << i) != 0 {
                counts[i] = u32_at(&tables, at);
                at += 4;
            }
        }
        let mut rows = Vec::new();
        for i in 0..64 {
            let mut table = Vec::new();
            if counts[i] != 0 && i >= SCHEMAS.len() {
                fail("unsupported metadata table");
            }
            for _ in 0..counts[i] {
                let mut row = Vec::new();
                for column in SCHEMAS[i] {
                    let width = match *column {
                        2 => 2,
                        4 => 4,
                        0x70 => {
                            if heap_flags & 1 != 0 {
                                4
                            } else {
                                2
                            }
                        }
                        0x71 => {
                            if heap_flags & 2 != 0 {
                                4
                            } else {
                                2
                            }
                        }
                        0x72 => {
                            if heap_flags & 4 != 0 {
                                4
                            } else {
                                2
                            }
                        }
                        c if c >= 0x200 => {
                            let kind = (c - 0x200) as usize;
                            let mut max = 0;
                            for t in CODED_TABLES[kind] {
                                if *t != usize::MAX {
                                    max = max.max(counts[*t]);
                                }
                            }
                            if max < (1 << (16 - CODED_BITS[kind])) {
                                2
                            } else {
                                4
                            }
                        }
                        c => {
                            if counts[(c - 0x100) as usize] < 65536 {
                                2
                            } else {
                                4
                            }
                        }
                    };
                    row.push(if width == 2 {
                        u16_at(&tables, at) as u32
                    } else {
                        u32_at(&tables, at)
                    });
                    at += width;
                }
                table.push(row);
            }
            rows.push(table);
        }
        let mut attrs: BTreeMap<(usize, u32), Vec<u32>> = BTreeMap::new();
        for i in 0..rows[12].len() {
            let row = &rows[12][i];
            let parent = decode(2, row[0]);
            attrs.entry(parent).or_default().push(i as u32 + 1);
        }
        let mut nested = BTreeMap::new();
        for row in &rows[41] {
            nested.insert(row[0], row[1]);
        }
        Self {
            path: path.to_string(),
            tables: rows,
            strings,
            blobs,
            attrs,
            nested,
        }
    }
    pub fn row(&self, table: usize, row: u32) -> &[u32] {
        if row == 0 || row as usize > self.tables[table].len() {
            fail(
                &(String::from("invalid metadata row: ")
                    + &self.path
                    + " table "
                    + &table.to_string()
                    + " row "
                    + &row.to_string()),
            );
        }
        &self.tables[table][row as usize - 1]
    }
    pub fn string(&self, index: u32) -> &str {
        let start = index as usize;
        let mut end = start;
        while self.strings[end] != 0 {
            end += 1;
        }
        std::str::from_utf8(&self.strings[start..end]).expect("invalid UTF-8 metadata string")
    }
    pub fn blob(&self, index: u32) -> &[u8] {
        if index == 0 {
            return &[];
        }
        let mut at = index as usize;
        let size = compressed(&self.blobs, &mut at) as usize;
        &self.blobs[at..at + size]
    }
    pub fn type_name(&self, token: u32) -> String {
        let (table, id) = decode(0, token);
        if id == 0 {
            return String::new();
        }
        if table == 27 {
            return self.signature(self.blob(self.row(27, id)[0]), &mut 0).name;
        }
        let row = self.row(table, id);
        let mut name = if table == 2 {
            if let Some(parent) = self.nested.get(&id) {
                return nested_name(&self.type_name(*parent << 2), self.string(row[1]));
            } else {
                let mut p = self.string(row[2]).to_string();
                if !p.is_empty() {
                    p.push('.');
                }
                p
            }
        } else {
            let (scope, parent) = decode(11, row[0]);
            if scope == 1 {
                return nested_name(&self.type_name((parent << 2) | 1), self.string(row[1]));
            }
            let mut p = self.string(row[2]).to_string();
            if !p.is_empty() {
                p.push('.');
            }
            p
        };
        name.push_str(self.string(row[1]));
        name
    }
    pub fn range(&self, table: usize, row: u32, col: usize, target: usize) -> std::ops::Range<u32> {
        let start = self.row(table, row)[col];
        let end = if row as usize == self.tables[table].len() {
            self.tables[target].len() as u32 + 1
        } else {
            self.row(table, row + 1)[col]
        };
        start..end
    }
    pub fn attr_name(&self, id: u32) -> String {
        let row = self.row(12, id);
        let (table, index) = decode(10, row[1]);
        if table == 10 {
            let (parent, pid) = decode(5, self.row(10, index)[0]);
            if parent == 1 {
                return self.type_name((pid << 2) | 1);
            }
            if parent == 2 {
                return self.type_name(pid << 2);
            }
        }
        if table == 6 {
            for i in 1..=self.tables[2].len() as u32 {
                if self.range(2, i, 5, 6).contains(&index) {
                    return self.type_name(i << 2);
                }
            }
        }
        fail("unsupported attribute constructor")
    }
    pub fn attribute(&self, table: usize, id: u32, name: &str) -> Option<&[u8]> {
        if let Some(attrs) = self.attrs.get(&(table, id)) {
            for a in attrs {
                let full = self.attr_name(*a);
                if full.rsplit('.').next() == Some(name) {
                    return Some(self.blob(self.row(12, *a)[2]));
                }
            }
        }
        None
    }
    pub fn signature(&self, data: &[u8], at: &mut usize) -> Sig {
        let kind = data[*at];
        *at += 1;
        let mut s = Sig {
            kind,
            name: String::new(),
            args: Vec::new(),
            len: 0,
        };
        match kind {
            0x11 | 0x12 => {
                s.name = self.type_name(compressed(data, at));
            }
            0x0f | 0x10 | 0x1d => {
                s.args.push(self.signature(data, at));
            }
            0x13 | 0x1e => {
                s.len = compressed(data, at);
            }
            0x14 => {
                s.args.push(self.signature(data, at));
                let rank = compressed(data, at);
                let sizes = compressed(data, at);
                if rank != 1 || sizes != 1 {
                    fail("unsupported multidimensional field array");
                }
                s.len = compressed(data, at);
                let lows = compressed(data, at);
                for _ in 0..lows {
                    let _ = compressed(data, at);
                }
            }
            0x15 => {
                s.kind = data[*at];
                *at += 1;
                s.name = self.type_name(compressed(data, at));
                let count = compressed(data, at);
                for _ in 0..count {
                    s.args.push(self.signature(data, at));
                }
                s.kind = 0x15;
            }
            0x1f | 0x20 => {
                let _modifier = compressed(data, at);
                s = self.signature(data, at);
            }
            0x01..=0x0e | 0x16 | 0x18 | 0x19 | 0x1c => (),
            _ => fail(
                &(String::from("unsupported signature element ")
                    + &kind.to_string()
                    + " in "
                    + &self.path),
            ),
        }
        s
    }
    pub fn field_sig(&self, id: u32) -> Sig {
        self.signature(self.blob(self.row(4, id)[2]), &mut 1)
    }
    pub fn method(&self, id: u32) -> Method {
        let row = self.row(6, id);
        let data = self.blob(row[4]);
        let mut at = 1;
        if data[0] & 0x10 != 0 {
            let _ = compressed(data, &mut at);
        }
        let count = compressed(data, &mut at);
        let ret = self.signature(data, &mut at);
        let mut params = Vec::new();
        for i in 0..count {
            let sig = self.signature(data, &mut at);
            let mut p = Param {
                id: 0,
                name: String::from("arg") + &i.to_string(),
                flags: 0,
                sig,
            };
            for j in self.range(6, id, 5, 8) {
                let r = self.row(8, j);
                if r[1] == i + 1 {
                    p.id = j;
                    p.flags = r[0];
                    p.name = self.string(r[2]).to_string();
                }
            }
            params.push(p);
        }
        Method {
            id,
            name: self.string(row[3]).to_string(),
            ret,
            params,
        }
    }
}
pub fn decode(kind: usize, index: u32) -> (usize, u32) {
    let bits = CODED_BITS[kind];
    (
        CODED_TABLES[kind][(index & ((1 << bits) - 1)) as usize],
        index >> bits,
    )
}
#[derive(Clone)]
pub struct Sig {
    pub kind: u8,
    pub name: String,
    pub args: Vec<Sig>,
    pub len: u32,
}
pub struct Param {
    pub id: u32,
    pub name: String,
    pub flags: u32,
    pub sig: Sig,
}
pub struct Method {
    pub id: u32,
    pub name: String,
    pub ret: Sig,
    pub params: Vec<Param>,
}
impl Sig {
    pub fn describe(&self) -> String {
        let mut s = match self.kind {
            1 => String::from("void"),
            2 => String::from("bool"),
            3 => String::from("char"),
            4 => String::from("i8"),
            5 => String::from("u8"),
            6 => String::from("i16"),
            7 => String::from("u16"),
            8 => String::from("i32"),
            9 => String::from("u32"),
            10 => String::from("i64"),
            11 => String::from("u64"),
            12 => String::from("f32"),
            13 => String::from("f64"),
            14 => String::from("HSTRING"),
            15 => String::from("ptr"),
            16 => String::from("ref"),
            17 | 18 | 21 => self.name.clone(),
            19 => String::from("T") + &self.len.to_string(),
            20 | 29 => String::from("array"),
            24 => String::from("isize"),
            25 => String::from("usize"),
            28 => String::from("IInspectable"),
            _ => String::from("?"),
        };
        if !self.args.is_empty() {
            s.push('<');
            for i in 0..self.args.len() {
                let a = &self.args[i];
                if i != 0 {
                    s.push(',');
                }
                s.push_str(&a.describe());
            }
            s.push('>');
        }
        s
    }
}
impl File {
    pub fn base(&self, id: u32) -> String {
        for row in &self.tables[9] {
            if row[0] == id {
                return self.type_name(row[1]);
            }
        }
        String::new()
    }
}

fn nested_name(parent: &str, name: &str) -> String {
    if let Some(rest) = name.strip_prefix("_Anonymous") {
        let number = rest.split('_').next().unwrap_or("");
        let index = if number.is_empty() {
            0
        } else {
            number.parse::<u32>().expect("anonymous nested type number") - 1
        };
        return String::from(parent) + "_" + &index.to_string();
    }
    if name.starts_with('_') && (name.ends_with("_e__Union") || name.ends_with("_e__Struct")) {
        return String::from(parent) + "_0";
    }
    String::from(parent) + "_" + name
}
