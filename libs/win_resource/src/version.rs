//! `RT_VERSION` (VS_VERSIONINFO): the file description Task Manager and the
//! taskbar show for the process, and Explorer's Details tab.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VersionInfo {
    pub file_description: String,
    pub product_name: String,
    pub company_name: String,
    pub internal_name: String,
    pub original_filename: String,
    pub legal_copyright: String,
    /// FileVersion/ProductVersion strings; the numeric fields use `version`.
    pub version_text: String,
    pub version: [u16; 4],
}

impl VersionInfo {
    /// `major.minor.patch[.build]` (a pre-release suffix is ignored).
    pub fn parse_version(text: &str) -> [u16; 4] {
        let mut out = [0u16; 4];
        let core = text.split(|c| c == '-' || c == '+').next().unwrap_or("");
        for (slot, part) in out.iter_mut().zip(core.split('.')) {
            *slot = part.parse().unwrap_or(0);
        }
        out
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect()
}

fn pad4(b: &mut Vec<u8>) {
    while b.len() % 4 != 0 { b.push(0); }
}

/// One node: wLength, wValueLength, wType, key, padding, value, then the
/// children each starting on a 32-bit boundary. wLength excludes padding
/// after the node; the parent counts it.
fn node(key: &str, text: bool, value: &[u8], value_length: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend_from_slice(&value_length.to_le_bytes());
    b.extend_from_slice(&(text as u16).to_le_bytes());
    b.extend_from_slice(&utf16z(key));
    pad4(&mut b);
    b.extend_from_slice(value);
    for child in children {
        pad4(&mut b);
        b.extend_from_slice(child);
    }
    let len = b.len() as u16;
    b[..2].copy_from_slice(&len.to_le_bytes());
    b
}

pub fn version_resource(info: &VersionInfo) -> Vec<u8> {
    let v = info.version;
    let ms = ((v[0] as u32) << 16) | v[1] as u32;
    let ls = ((v[2] as u32) << 16) | v[3] as u32;
    let mut fixed = Vec::with_capacity(52);
    for x in [0xfeef_04bdu32, 0x0001_0000, ms, ls, ms, ls, 0x3f, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend_from_slice(&x.to_le_bytes());
    }
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", &info.company_name),
        ("FileDescription", &info.file_description),
        ("FileVersion", &info.version_text),
        ("InternalName", &info.internal_name),
        ("LegalCopyright", &info.legal_copyright),
        ("OriginalFilename", &info.original_filename),
        ("ProductName", &info.product_name),
        ("ProductVersion", &info.version_text),
    ].into_iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| {
        let value = utf16z(v);
        node(k, true, &value, (value.len() / 2) as u16, &[])
    }).collect();
    // U.S. English, Unicode: matches the resource language.
    let table = node("040904B0", true, &[], 0, &strings);
    let string_info = node("StringFileInfo", true, &[], 0, &[table]);
    let translation = node("Translation", false, &[0x09, 0x04, 0xb0, 0x04], 4, &[]);
    let var_info = node("VarFileInfo", true, &[], 0, &[translation]);
    node("VS_VERSION_INFO", false, &fixed, 52, &[string_info, var_info])
}

/// Read the numeric file version and the string table back.
pub fn parse_version_resource(data: &[u8]) -> Result<([u16; 4], Vec<(String, String)>), String> {
    struct Node { key: String, value: Vec<u8>, children: Vec<Node> }
    fn read(data: &[u8], at: usize, depth: usize) -> Result<(Node, usize), String> {
        let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or("version resource truncated");
        let len = u16_at(at)? as usize;
        let value_len = u16_at(at + 2)? as usize;
        let text = u16_at(at + 4)? == 1;
        let end = at + len;
        if len < 6 || end > data.len() || depth > 4 {
            return Err("malformed version node".into());
        }
        let mut o = at + 6;
        let mut key = Vec::new();
        loop {
            let u = u16_at(o)?;
            o += 2;
            if u == 0 { break; }
            key.push(u);
        }
        o = (o + 3) & !3;
        let value_bytes = if text { value_len * 2 } else { value_len };
        let value = data.get(o..(o + value_bytes).min(end)).ok_or("version value truncated")?.to_vec();
        o = (o + value_bytes + 3) & !3;
        let mut children = Vec::new();
        while o < end {
            let (child, next) = read(data, o, depth + 1)?;
            children.push(child);
            o = (next + 3) & !3;
        }
        Ok((Node { key: String::from_utf16_lossy(&key), value, children }, end))
    }
    let (root, _) = read(data, 0, 0)?;
    if root.key != "VS_VERSION_INFO" || root.value.len() != 52 {
        return Err("not a VS_VERSION_INFO".into());
    }
    let word = |i: usize| u32::from_le_bytes(root.value[i * 4..i * 4 + 4].try_into().unwrap());
    let (ms, ls) = (word(2), word(3));
    let version = [(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16];
    let mut strings = Vec::new();
    for info in root.children.iter().filter(|c| c.key == "StringFileInfo") {
        for table in &info.children {
            for s in &table.children {
                let units: Vec<u16> = s.value.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
                let text = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
                strings.push((s.key.clone(), text));
            }
        }
    }
    Ok((version, strings))
}
