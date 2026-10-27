//! Object-file containers for natbin: ELF64 LE and Mach-O 64 (thin, universal, relocatable
//! objects, `ar` archives). Only what the symbol table, the code bytes and the debug sections need.

use crate::inspect::symbols::{cstr, u16_at, u32_at, u64_at};
use relax_backend::asm::Arch;

/// A section of an object file.
#[derive(Clone, Debug, Default)]
pub(super) struct Sec {
    pub name: String,
    pub seg: String,
    pub addr: u64,
    pub off: usize,
    pub size: usize,
    /// ELF sh_flags / Mach-O flags
    pub flags: u64,
    /// ELF sh_type (8 = NOBITS)
    pub ty: u32,
    pub reloff: usize,
    pub nreloc: usize,
}

/// A Mach-O nlist entry.
pub(super) struct Nlist {
    pub name: String,
    pub ty: u8,
    pub sect: u8,
    pub value: u64,
}

/// An ELF function symbol: (address, size, raw name, section index).
pub(super) struct ElfFn {
    pub addr: u64,
    pub size: u64,
    pub name: String,
}

pub(super) struct Elf {
    pub arch: Arch,
    pub secs: Vec<Sec>,
    pub fns: Vec<ElfFn>,
}

pub(super) struct Macho {
    pub arch: Arch,
    pub secs: Vec<Sec>,
    pub syms: Vec<Nlist>,
}

pub(super) fn is_elf(b: &[u8]) -> bool {
    b.starts_with(b"\x7fELF") && b.get(4) == Some(&2) && b.get(5) == Some(&1)
}

pub(super) fn is_macho(b: &[u8]) -> bool {
    b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
}

pub(super) fn is_fat(b: &[u8]) -> bool {
    b.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) || b.starts_with(&[0xca, 0xfe, 0xba, 0xbf])
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn be64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

pub(super) fn arch_of_cpu(cpu: u32) -> Option<Arch> {
    match cpu {
        0x0100_0007 => Some(Arch::X64),
        0x0100_000c => Some(Arch::Arm64),
        _ => None,
    }
}

/// The (offset, size) of the universal binary's slice for `want`.
pub(super) fn fat_slice(b: &[u8], want: Arch) -> Option<(usize, usize)> {
    let big = b[3] == 0xbf;
    let n = be32(b, 4)? as usize;
    let ent = if big { 32 } else { 20 };
    for k in 0..n {
        let at = 8 + k * ent;
        if arch_of_cpu(be32(b, at)?) != Some(want) {
            continue;
        }
        return if big { Some((be64(b, at + 8)? as usize, be64(b, at + 16)? as usize)) } else { Some((be32(b, at + 8)? as usize, be32(b, at + 12)? as usize)) };
    }
    None
}

pub(super) fn parse_elf(b: &[u8]) -> Option<Elf> {
    let arch = match u16_at(b, 0x12)? {
        62 => Arch::X64,
        183 => Arch::Arm64,
        _ => return None,
    };
    let shoff = u64_at(b, 0x28)? as usize;
    let shentsize = u16_at(b, 0x3a)? as usize;
    let shnum = u16_at(b, 0x3c)? as usize;
    let shstrndx = u16_at(b, 0x3e)? as usize;
    let raw = |i: usize| -> Option<(u32, u32, u64, u64, usize, usize, u32)> {
        let o = shoff + i * shentsize;
        // (name, type, flags, addr, offset, size, link)
        Some((u32_at(b, o)?, u32_at(b, o + 4)?, u64_at(b, o + 8)?, u64_at(b, o + 0x10)?, u64_at(b, o + 0x18)? as usize, u64_at(b, o + 0x20)? as usize, u32_at(b, o + 0x28)?))
    };
    let strtab = raw(shstrndx)?.4;
    let mut secs = Vec::with_capacity(shnum);
    let mut kinds = Vec::with_capacity(shnum);
    for i in 0..shnum {
        let (name, ty, flags, addr, off, size, link) = raw(i)?;
        secs.push(Sec { name: cstr(b, strtab + name as usize).unwrap_or("").to_string(), addr, off, size, flags, ty, ..Default::default() });
        kinds.push((ty, link, off, size));
    }
    // SHT_SYMTAB (2) when present, else SHT_DYNSYM (11)
    let mut tables: Vec<usize> = (0..shnum).filter(|&i| kinds[i].0 == 2).collect();
    if tables.is_empty() {
        tables = (0..shnum).filter(|&i| kinds[i].0 == 11).collect();
    }
    let mut fns = Vec::new();
    for t in tables {
        let (_, link, off, size) = kinds[t];
        let stroff = kinds.get(link as usize)?.2;
        for k in 0..size / 24 {
            let o = off + k * 24;
            let info = *b.get(o + 4)?;
            let shndx = u16_at(b, o + 6)? as usize;
            let sz = u64_at(b, o + 16)?;
            if info & 0xf != 2 || shndx == 0 || shndx >= shnum || sz == 0 {
                continue; // STT_FUNC, defined, sized
            }
            // SHF_EXECINSTR in a section with file contents
            if secs[shndx].flags & 4 == 0 || secs[shndx].ty == 8 {
                continue;
            }
            let value = u64_at(b, o + 8)?;
            if let Some(name) = cstr(b, stroff + u32_at(b, o)? as usize) {
                fns.push(ElfFn { addr: value, size: sz, name: name.to_string() });
            }
        }
    }
    Some(Elf { arch, secs, fns })
}

pub(super) fn parse_macho(b: &[u8]) -> Option<Macho> {
    let arch = arch_of_cpu(u32_at(b, 4)?)?;
    let ncmds = u32_at(b, 16)? as usize;
    let mut p = 32usize;
    let mut secs = Vec::new();
    let mut syms = Vec::new();
    for _ in 0..ncmds {
        let cmd = u32_at(b, p)?;
        let size = u32_at(b, p + 4)? as usize;
        if cmd == 0x19 {
            // LC_SEGMENT_64: 16 name, vmaddr, vmsize, fileoff, filesize, maxprot, initprot, nsects, flags
            let nsects = u32_at(b, p + 64)? as usize;
            for k in 0..nsects {
                let o = p + 72 + k * 80;
                let name = |at: usize| -> String { String::from_utf8_lossy(&b[at..at + 16]).trim_end_matches('\0').to_string() };
                if b.len() < o + 80 {
                    return None;
                }
                secs.push(Sec {
                    name: name(o),
                    seg: name(o + 16),
                    addr: u64_at(b, o + 32)?,
                    size: u64_at(b, o + 40)? as usize,
                    off: u32_at(b, o + 48)? as usize,
                    reloff: u32_at(b, o + 56)? as usize,
                    nreloc: u32_at(b, o + 60)? as usize,
                    flags: u32_at(b, o + 64)? as u64,
                    ty: 0,
                });
            }
        } else if cmd == 2 {
            // LC_SYMTAB: symoff, nsyms, stroff, strsize
            let (symoff, nsyms, stroff) = (u32_at(b, p + 8)? as usize, u32_at(b, p + 12)? as usize, u32_at(b, p + 16)? as usize);
            syms.reserve(nsyms);
            for k in 0..nsyms {
                // nlist_64: strx u32, type u8, sect u8, desc u16, value u64
                let o = symoff + k * 16;
                let name = cstr(b, stroff + u32_at(b, o)? as usize).unwrap_or("").to_string();
                syms.push(Nlist { name, ty: *b.get(o + 4)?, sect: *b.get(o + 5)?, value: u64_at(b, o + 8)? });
            }
        }
        p += size;
    }
    Some(Macho { arch, secs, syms })
}

/// The member `member` of an `ar` archive (BSD `#1/N` and GNU `name/` member names).
pub(super) fn ar_member(b: &[u8], member: &str) -> Option<Vec<u8>> {
    if !b.starts_with(b"!<arch>\n") {
        return None;
    }
    let mut p = 8usize;
    while p + 60 <= b.len() {
        let h = &b[p..p + 60];
        let size: usize = std::str::from_utf8(&h[48..58]).ok()?.trim().parse().ok()?;
        let raw_name = std::str::from_utf8(&h[0..16]).ok()?.trim_end();
        let mut data = p + 60;
        let mut len = size;
        let name = if let Some(n) = raw_name.strip_prefix("#1/") {
            let n: usize = n.trim().parse().ok()?;
            let s = String::from_utf8_lossy(b.get(data..data + n)?).trim_end_matches('\0').to_string();
            data += n;
            len = len.checked_sub(n)?;
            s
        } else {
            raw_name.trim_end_matches('/').to_string()
        };
        if name == member {
            return Some(b.get(data..data + len)?.to_vec());
        }
        p += 60 + size + (size & 1);
    }
    None
}

/// Applies the Mach-O relocations of the `__DWARF` sections of a relocatable object in place,
/// leaving the object's own address space: the debug sections of a `.o` store addends against
/// symbols (`extern`) and section-relative values (already object addresses).
pub(super) fn apply_dwarf_relocs(data: &mut [u8], m: &Macho) {
    for s in m.secs.iter().filter(|s| s.seg == "__DWARF" && s.nreloc > 0) {
        for k in 0..s.nreloc {
            let o = s.reloff + k * 8;
            let (Some(addr), Some(info)) = (u32_at(data, o), u32_at(data, o + 4)) else { return };
            if addr & 0x8000_0000 != 0 {
                continue; // scattered
            }
            let symnum = (info & 0xff_ffff) as usize;
            let pcrel = (info >> 24) & 1 != 0;
            let len = 1usize << ((info >> 25) & 3);
            let ext = (info >> 31) & 1 != 0;
            if pcrel || !ext || (len != 8 && len != 4) {
                continue;
            }
            let Some(sym) = m.syms.get(symnum) else { continue };
            let at = s.off + addr as usize;
            if at + len > data.len() {
                continue;
            }
            if len == 8 {
                let v = u64::from_le_bytes(data[at..at + 8].try_into().unwrap()).wrapping_add(sym.value);
                data[at..at + 8].copy_from_slice(&v.to_le_bytes());
            } else {
                let v = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()).wrapping_add(sym.value as u32);
                data[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}
