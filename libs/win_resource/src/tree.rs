//! The resource directory: type → name → language tables, then the data
//! entries, laid out the way cvtres does. Shared by the COFF writer and the
//! COFF/PE readers.

pub const RT_ICON: u16 = 3;
pub const RT_GROUP_ICON: u16 = 14;
pub const RT_VERSION: u16 = 16;
pub const RT_MANIFEST: u16 = 24;
pub const LANG_EN_US: u16 = 0x0409;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResId {
    Id(u16),
    Name(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resource {
    pub kind: ResId,
    pub name: ResId,
    pub lang: u16,
    pub data: Vec<u8>,
}

impl Resource {
    pub fn new(kind: u16, name: u16, data: Vec<u8>) -> Resource {
        Resource { kind: ResId::Id(kind), name: ResId::Id(name), lang: LANG_EN_US, data }
    }
}

pub(crate) struct Layout {
    /// Directory tables and data entries; each data entry's first field (the
    /// data RVA) is left zero for the caller to relocate.
    pub directory: Vec<u8>,
    /// Offset of each resource's data entry in `directory`, in resource order.
    pub entries: Vec<usize>,
    /// All payloads, each 8-byte aligned.
    pub data: Vec<u8>,
    /// Offset of each resource's payload in `data`, in resource order.
    pub data_offsets: Vec<usize>,
}

fn id(r: &ResId) -> Result<u16, String> {
    match r {
        ResId::Id(id) => Ok(*id),
        ResId::Name(name) => Err(format!("named resource {name:?} is not supported by the writer")),
    }
}

fn table(out: &mut Vec<u8>, entries: &[(u16, u32)]) {
    out.extend_from_slice(&[0; 12]);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (id, offset) in entries {
        out.extend_from_slice(&(*id as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
    }
}

const SUBDIRECTORY: u32 = 0x8000_0000;

pub(crate) fn layout(resources: &[Resource]) -> Result<Layout, String> {
    let mut keys = Vec::with_capacity(resources.len());
    for (i, r) in resources.iter().enumerate() {
        keys.push((id(&r.kind)?, id(&r.name)?, r.lang, i));
    }
    keys.sort();
    if let Some(w) = keys.windows(2).find(|w| w[0].0 == w[1].0 && w[0].1 == w[1].1 && w[0].2 == w[1].2) {
        return Err(format!("duplicate resource type {} name {} language {:#06x}", w[0].0, w[0].1, w[0].2));
    }
    // types[t] = (type id, names[n] = (name id, leaves[(lang, resource index)]))
    let mut types: Vec<(u16, Vec<(u16, Vec<(u16, usize)>)>)> = Vec::new();
    for (kind, name, lang, index) in keys {
        if types.last().map(|t| t.0) != Some(kind) { types.push((kind, Vec::new())); }
        let names = &mut types.last_mut().unwrap().1;
        if names.last().map(|n| n.0) != Some(name) { names.push((name, Vec::new())); }
        names.last_mut().unwrap().1.push((lang, index));
    }

    let mut offset = 16 + 8 * types.len();
    let mut type_tables = Vec::new();
    for (_, names) in &types {
        type_tables.push(offset);
        offset += 16 + 8 * names.len();
    }
    let mut name_tables = Vec::new();
    for (_, names) in &types {
        for (_, leaves) in names {
            name_tables.push(offset);
            offset += 16 + 8 * leaves.len();
        }
    }
    let mut leaf_offsets = Vec::new();
    for (_, names) in &types {
        for (_, leaves) in names {
            for _ in leaves {
                leaf_offsets.push(offset);
                offset += 16;
            }
        }
    }

    let mut directory = Vec::with_capacity(offset);
    table(&mut directory, &types.iter().zip(&type_tables).map(|((kind, _), &o)| (*kind, o as u32 | SUBDIRECTORY)).collect::<Vec<_>>());
    let mut name_table = name_tables.iter();
    for (_, names) in &types {
        table(&mut directory, &names.iter().map(|(name, _)| (*name, *name_table.next().unwrap() as u32 | SUBDIRECTORY)).collect::<Vec<_>>());
    }
    let mut leaf = leaf_offsets.iter();
    for (_, names) in &types {
        for (_, leaves) in names {
            table(&mut directory, &leaves.iter().map(|(lang, _)| (*lang, *leaf.next().unwrap() as u32)).collect::<Vec<_>>());
        }
    }
    let mut entries = vec![0; resources.len()];
    let mut data_offsets = vec![0; resources.len()];
    let mut data = Vec::new();
    let mut leaf = leaf_offsets.iter();
    for (_, names) in &types {
        for (_, leaves) in names {
            for (_, index) in leaves {
                let at = *leaf.next().unwrap();
                debug_assert_eq!(at, directory.len());
                entries[*index] = at;
                while data.len() % 8 != 0 { data.push(0); }
                data_offsets[*index] = data.len();
                data.extend_from_slice(&resources[*index].data);
                directory.extend_from_slice(&0u32.to_le_bytes());
                directory.extend_from_slice(&(resources[*index].data.len() as u32).to_le_bytes());
                directory.extend_from_slice(&[0; 8]);
            }
        }
    }
    debug_assert_eq!(directory.len(), offset);
    Ok(Layout { directory, entries, data, data_offsets })
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("resource directory truncated at {at}"))
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("resource directory truncated at {at}"))
}

fn read_table(dir: &[u8], at: usize) -> Result<Vec<(ResId, u32)>, String> {
    let count = u16_at(dir, at + 12)? as usize + u16_at(dir, at + 14)? as usize;
    (0..count).map(|i| {
        let e = at + 16 + 8 * i;
        let raw = u32_at(dir, e)?;
        let id = if raw & SUBDIRECTORY != 0 {
            let name = (raw & !SUBDIRECTORY) as usize;
            let len = u16_at(dir, name)? as usize;
            let units: Result<Vec<u16>, String> = (0..len).map(|k| u16_at(dir, name + 2 + 2 * k)).collect();
            ResId::Name(String::from_utf16_lossy(&units?))
        } else {
            ResId::Id(raw as u16)
        };
        Ok((id, u32_at(dir, e + 4)?))
    }).collect()
}

/// Walk the three directory levels. `resolve(entry_offset, data_rva, size)`
/// returns a data entry's payload; how the RVA maps to bytes depends on
/// whether the directory came from an object (relocations) or an image.
pub(crate) fn parse(dir: &[u8], resolve: &dyn Fn(usize, u32, u32) -> Result<Vec<u8>, String>) -> Result<Vec<Resource>, String> {
    let sub = |o: u32, level: &str| if o & SUBDIRECTORY != 0 { Ok((o & !SUBDIRECTORY) as usize) } else { Err(format!("{level} entry is not a directory")) };
    let mut out = Vec::new();
    for (kind, names) in read_table(dir, 0)? {
        for (name, langs) in read_table(dir, sub(names, "type")?)? {
            for (lang, entry) in read_table(dir, sub(langs, "name")?)? {
                if entry & SUBDIRECTORY != 0 {
                    return Err("language entry points to a directory".into());
                }
                let entry = entry as usize;
                let data = resolve(entry, u32_at(dir, entry)?, u32_at(dir, entry + 4)?)?;
                let lang = match lang { ResId::Id(l) => l, ResId::Name(_) => return Err("named language".into()) };
                out.push(Resource { kind: kind.clone(), name: name.clone(), lang, data });
            }
        }
    }
    Ok(out)
}
