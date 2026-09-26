//! Read the resources of a linked PE image (.exe/.dll), to verify what the
//! linker placed there.
use crate::tree::{parse, Resource};

pub struct PeResources {
    pub machine: u16,
    /// Name of the section holding the resource directory (normally `.rsrc`).
    pub section: String,
    pub resources: Vec<Resource>,
}

pub fn read_pe_resources(bytes: &[u8]) -> Result<PeResources, String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("image truncated at {at}"));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("image truncated at {at}"));
    if bytes.get(..2) != Some(b"MZ") {
        return Err("not a PE image (no MZ header)".into());
    }
    let pe = u32_at(0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("not a PE image (no PE signature)".into());
    }
    let machine = u16_at(pe + 4)?;
    let sections_n = u16_at(pe + 6)? as usize;
    let optional = pe + 24;
    let optional_size = u16_at(pe + 20)? as usize;
    let (directories, count_at) = match u16_at(optional)? {
        0x10b => (optional + 96, optional + 92),
        0x20b => (optional + 112, optional + 108),
        magic => return Err(format!("unknown optional header magic {magic:#x}")),
    };
    if u32_at(count_at)? < 3 {
        return Err("image has no resource data directory".into());
    }
    let rsrc_rva = u32_at(directories + 16)? as usize;
    let rsrc_size = u32_at(directories + 20)? as usize;
    if rsrc_rva == 0 || rsrc_size == 0 {
        return Err("image has no resources".into());
    }
    let mut sections = Vec::new();
    for i in 0..sections_n {
        let h = optional + optional_size + i * 40;
        let name = bytes.get(h..h + 8).ok_or("section table truncated")?;
        let name = String::from_utf8_lossy(name).trim_end_matches('\0').to_string();
        let virtual_size = u32_at(h + 8)? as usize;
        let rva = u32_at(h + 12)? as usize;
        let raw_size = u32_at(h + 16)? as usize;
        let raw = u32_at(h + 20)? as usize;
        sections.push((name, rva, virtual_size.max(raw_size), raw, raw_size));
    }
    let to_file = |rva: usize, len: usize| -> Result<&[u8], String> {
        let (_, start, _, raw, raw_size) = sections.iter()
            .find(|(_, start, size, _, _)| rva >= *start && rva + len <= start + size)
            .ok_or_else(|| format!("RVA {rva:#x}+{len} is in no section"))?;
        let at = rva - start;
        if at + len > *raw_size {
            return Err(format!("RVA {rva:#x}+{len} is beyond its section's file data"));
        }
        bytes.get(raw + at..raw + at + len).ok_or_else(|| format!("RVA {rva:#x} maps outside the file"))
    };
    let section = sections.iter().find(|(_, start, size, _, _)| rsrc_rva >= *start && rsrc_rva < start + size)
        .map(|s| s.0.clone()).ok_or("resource directory is in no section")?;
    let directory = to_file(rsrc_rva, rsrc_size)?;
    let resolve = |_entry: usize, rva: u32, size: u32| to_file(rva as usize, size as usize).map(|d| d.to_vec());
    Ok(PeResources { machine, section, resources: parse(directory, &resolve)? })
}
