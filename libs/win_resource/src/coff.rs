//! A COFF object holding `.rsrc$01` (directory) and `.rsrc$02` (payloads), in
//! the shape cvtres/llvm-cvtres produce: each data entry's RVA field carries an
//! ADDR32NB relocation to a static `$Rxxxxxx` symbol at its payload. GNU ld
//! and lld in MinGW mode merge any number of these; link.exe and lld-link in
//! MSVC mode take at most one (and none next to `.res` inputs), which is why
//! MSVC targets get a `.res` file instead.
use crate::tree::{layout, parse, Resource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    X64,
    X86,
    Arm64,
    Arm,
}

impl Machine {
    /// From `CARGO_CFG_TARGET_ARCH` / `std::env::consts::ARCH`.
    pub fn from_target_arch(arch: &str) -> Option<Machine> {
        match arch {
            "x86_64" => Some(Machine::X64),
            "x86" | "i386" | "i586" | "i686" => Some(Machine::X86),
            "aarch64" => Some(Machine::Arm64),
            "arm" => Some(Machine::Arm),
            _ => None,
        }
    }
    pub fn code(self) -> u16 {
        match self {
            Machine::X64 => 0x8664,
            Machine::X86 => 0x014c,
            Machine::Arm64 => 0xaa64,
            Machine::Arm => 0x01c4,
        }
    }
    pub fn from_code(code: u16) -> Option<Machine> {
        [Machine::X64, Machine::X86, Machine::Arm64, Machine::Arm].into_iter().find(|m| m.code() == code)
    }
    fn addr32nb(self) -> u16 {
        match self {
            Machine::X64 => 0x0003,
            Machine::X86 => 0x0007,
            Machine::Arm64 | Machine::Arm => 0x0002,
        }
    }
}

const FILE_HEADER: usize = 20;
const SECTION_HEADER: usize = 40;
const RELOCATION: usize = 10;
const SYMBOL: usize = 18;
const SCN_DATA_READ: u32 = 0x4000_0040;
const SYM_ABSOLUTE: u16 = 0xffff;
const CLASS_STATIC: u8 = 3;
const FILE_32BIT_MACHINE: u16 = 0x0100;

fn symbol(out: &mut Vec<u8>, name: &[u8], value: u32, section: u16, aux: u8) {
    let mut short = [0u8; 8];
    short[..name.len()].copy_from_slice(name);
    out.extend_from_slice(&short);
    out.extend_from_slice(&value.to_le_bytes());
    out.extend_from_slice(&section.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&[CLASS_STATIC, aux]);
}

fn section_aux(out: &mut Vec<u8>, length: usize, relocations: usize, number: u16) {
    out.extend_from_slice(&(length as u32).to_le_bytes());
    out.extend_from_slice(&(relocations as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&number.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
}

fn section_header(out: &mut Vec<u8>, name: &[u8; 8], size: usize, raw: usize, relocs_at: usize, relocs: usize) {
    out.extend_from_slice(name);
    out.extend_from_slice(&[0; 8]); // VirtualSize, VirtualAddress
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(raw as u32).to_le_bytes());
    out.extend_from_slice(&(relocs_at as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(relocs as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&SCN_DATA_READ.to_le_bytes());
}

pub fn write_object(machine: Machine, resources: &[Resource]) -> Result<Vec<u8>, String> {
    if resources.is_empty() {
        return Err("no resources to write".into());
    }
    let l = layout(resources)?;
    let n = resources.len();
    // Relocation counts are 16-bit in the section header.
    if n > 0xffff {
        return Err(format!("{n} resources do not fit one object"));
    }
    let dir_at = FILE_HEADER + 2 * SECTION_HEADER;
    let relocs_at = dir_at + l.directory.len();
    let data_at = (relocs_at + n * RELOCATION + 3) & !3;
    let symbols_at = (data_at + l.data.len() + 3) & !3;
    let symbol_count = 5 + n;

    let mut out = Vec::with_capacity(symbols_at + symbol_count * SYMBOL + 4);
    out.extend_from_slice(&machine.code().to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // no timestamp: reproducible output
    out.extend_from_slice(&(symbols_at as u32).to_le_bytes());
    out.extend_from_slice(&(symbol_count as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    let characteristics = if matches!(machine, Machine::X86 | Machine::Arm) { FILE_32BIT_MACHINE } else { 0 };
    out.extend_from_slice(&characteristics.to_le_bytes());
    section_header(&mut out, b".rsrc$01", l.directory.len(), dir_at, relocs_at, n);
    section_header(&mut out, b".rsrc$02", l.data.len(), data_at, 0, 0);
    out.extend_from_slice(&l.directory);
    for i in 0..n {
        out.extend_from_slice(&(l.entries[i] as u32).to_le_bytes());
        out.extend_from_slice(&((5 + i) as u32).to_le_bytes());
        out.extend_from_slice(&machine.addr32nb().to_le_bytes());
    }
    out.resize(data_at, 0);
    out.extend_from_slice(&l.data);
    out.resize(symbols_at, 0);
    // @feat.00 bit 0: safe for /SAFESEH (there is no code here).
    symbol(&mut out, b"@feat.00", if machine == Machine::X86 { 1 } else { 0 }, SYM_ABSOLUTE, 0);
    symbol(&mut out, b".rsrc$01", 0, 1, 1);
    section_aux(&mut out, l.directory.len(), n, 1);
    symbol(&mut out, b".rsrc$02", 0, 2, 1);
    section_aux(&mut out, l.data.len(), 0, 2);
    for i in 0..n {
        symbol(&mut out, format!("$R{i:06X}").as_bytes(), l.data_offsets[i] as u32, 2, 0);
    }
    out.extend_from_slice(&4u32.to_le_bytes()); // empty string table
    Ok(out)
}

struct Section<'a> {
    name: [u8; 8],
    data: &'a [u8],
    relocs: &'a [u8],
}

/// Read back the resources of an object: `.rsrc$01` with relocations into
/// `.rsrc$02` (as written above), or a single `.rsrc` section.
pub fn read_object(bytes: &[u8]) -> Result<(Machine, Vec<Resource>), String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("object truncated at {at}"));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("object truncated at {at}"));
    let machine = Machine::from_code(u16_at(0)?).ok_or("unknown COFF machine")?;
    let sections_n = u16_at(2)? as usize;
    let symbols_at = u32_at(8)? as usize;
    let symbols_n = u32_at(12)? as usize;
    let first_section = FILE_HEADER + u16_at(16)? as usize;
    let slice = |at: usize, len: usize| bytes.get(at..at + len).ok_or_else(|| format!("object range {at}+{len} out of bounds"));
    let mut sections = Vec::new();
    for i in 0..sections_n {
        let h = first_section + i * SECTION_HEADER;
        let name: [u8; 8] = slice(h, 8)?.try_into().unwrap();
        let data = slice(u32_at(h + 20)? as usize, u32_at(h + 16)? as usize)?;
        let relocs = slice(u32_at(h + 24)? as usize, u16_at(h + 32)? as usize * RELOCATION)?;
        sections.push(Section { name, data, relocs });
    }
    let dir_index = sections.iter().position(|s| &s.name == b".rsrc$01" || &s.name == b".rsrc\0\0\0")
        .ok_or("object has no resource section")?;
    let dir = &sections[dir_index];
    let resolve = |entry: usize, inplace: u32, size: u32| -> Result<Vec<u8>, String> {
        let reloc = dir.relocs.chunks_exact(RELOCATION)
            .find(|r| u32::from_le_bytes(r[0..4].try_into().unwrap()) as usize == entry)
            .ok_or_else(|| format!("data entry at {entry} has no relocation"))?;
        let sym = u32::from_le_bytes(reloc[4..8].try_into().unwrap()) as usize;
        if sym >= symbols_n {
            return Err(format!("relocation to symbol {sym} of {symbols_n}"));
        }
        let s = symbols_at + sym * SYMBOL;
        let value = u32_at(s + 8)? as usize;
        let section = u16_at(s + 12)? as usize;
        let target = sections.get(section.wrapping_sub(1)).ok_or("relocation symbol is not in a section")?;
        let at = value + inplace as usize;
        target.data.get(at..at + size as usize).map(|d| d.to_vec()).ok_or_else(|| format!("resource data {at}+{size} outside its section"))
    };
    Ok((machine, parse(dir.data, &resolve)?))
}
