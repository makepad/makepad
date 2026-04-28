use makepad_profiler::LoadedImage;
use std::cmp::Ordering;
use std::mem::{size_of, MaybeUninit};
use std::path::Path;

const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_SYMTAB: u32 = 0x2;
const LC_SEGMENT_64: u32 = 0x19;
const N_STAB: u8 = 0xe0;
const N_TYPE: u8 = 0x0e;
const N_SECT: u8 = 0x0e;

const CPU_ARCH_ABI64: i32 = 0x0100_0000;
#[cfg(target_arch = "x86_64")]
const CPU_TYPE_X86: i32 = 7;
#[cfg(target_arch = "aarch64")]
const CPU_TYPE_ARM: i32 = 12;
#[cfg(target_arch = "x86_64")]
const CPU_TYPE_X86_64: i32 = CPU_TYPE_X86 | CPU_ARCH_ABI64;
#[cfg(target_arch = "aarch64")]
const CPU_TYPE_ARM64: i32 = CPU_TYPE_ARM | CPU_ARCH_ABI64;

#[repr(C)]
#[derive(Clone, Copy)]
struct MachHeader64 {
    magic: u32,
    cputype: i32,
    cpusubtype: i32,
    filetype: u32,
    ncmds: u32,
    sizeofcmds: u32,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LoadCommand {
    cmd: u32,
    cmdsize: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SegmentCommand64 {
    cmd: u32,
    cmdsize: u32,
    segname: [u8; 16],
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    maxprot: i32,
    initprot: i32,
    nsects: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Section64 {
    sectname: [u8; 16],
    segname: [u8; 16],
    addr: u64,
    size: u64,
    offset: u32,
    align: u32,
    reloff: u32,
    nreloc: u32,
    flags: u32,
    reserved1: u32,
    reserved2: u32,
    reserved3: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SymtabCommand {
    cmd: u32,
    cmdsize: u32,
    symoff: u32,
    nsyms: u32,
    stroff: u32,
    strsize: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Nlist64 {
    n_strx: u32,
    n_type: u8,
    n_sect: u8,
    n_desc: u16,
    n_value: u64,
}

#[derive(Clone)]
struct ParsedSymbol {
    runtime_address: u64,
    name: String,
}

#[derive(Clone)]
struct ImageSymbols {
    image_index: usize,
    base_address: u64,
    end_address: u64,
    display_name: String,
    symbols: Vec<ParsedSymbol>,
}

#[derive(Clone)]
pub struct ResolvedFrame {
    pub image_index: Option<usize>,
    pub symbol_name: String,
    pub symbol_start: u64,
}

pub struct SymbolResolver {
    images: Vec<ImageSymbols>,
    warnings: Vec<String>,
}

impl SymbolResolver {
    pub fn from_images(images: &[LoadedImage]) -> Self {
        let mut parsed_images = Vec::new();
        let mut warnings = Vec::new();

        for (image_index, image) in images.iter().enumerate() {
            if image.path.is_empty() {
                continue;
            }
            match parse_image_symbols(image_index, image) {
                Ok(parsed) => parsed_images.push(parsed),
                Err(err) => warnings.push(format!("{}: {}", image.path, err)),
            }
        }

        parsed_images.sort_by_key(|image| image.base_address);
        Self {
            images: parsed_images,
            warnings,
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn resolve(&self, address: u64) -> ResolvedFrame {
        for image in self.images.iter().rev() {
            if address < image.base_address || address >= image.end_address {
                continue;
            }
            if let Some(symbol) = find_symbol(&image.symbols, address) {
                return ResolvedFrame {
                    image_index: Some(image.image_index),
                    symbol_name: symbol.name.clone(),
                    symbol_start: symbol.runtime_address,
                };
            }
            return ResolvedFrame {
                image_index: Some(image.image_index),
                symbol_name: format!(
                    "<{}+0x{:x}>",
                    image.display_name,
                    address.saturating_sub(image.base_address)
                ),
                symbol_start: image.base_address,
            };
        }

        ResolvedFrame {
            image_index: None,
            symbol_name: format!("<0x{:x}>", address),
            symbol_start: address,
        }
    }
}

fn parse_image_symbols(image_index: usize, image: &LoadedImage) -> Result<ImageSymbols, String> {
    let file = std::fs::read(&image.path).map_err(|err| format!("failed to read file: {}", err))?;
    let macho = select_macho_slice(&file)?;
    let header = read_struct::<MachHeader64>(macho, 0)?;
    if header.magic != MH_MAGIC_64 {
        return Err(format!("unsupported Mach-O magic 0x{:08x}", header.magic));
    }
    if header.cputype != target_cpu_type() {
        return Err(format!(
            "unexpected CPU type {}, expected {}",
            header.cputype,
            target_cpu_type()
        ));
    }

    let mut load_offset = size_of::<MachHeader64>();
    let mut section_ordinal = 1u8;
    let mut text_sections = Vec::<u8>::new();
    let mut preferred_load_address = None;
    let mut vm_min = u64::MAX;
    let mut vm_max = 0u64;
    let mut symtab = None;

    for _ in 0..header.ncmds {
        let command = read_struct::<LoadCommand>(macho, load_offset)?;
        if command.cmdsize < size_of::<LoadCommand>() as u32 {
            return Err("load command size was too small".to_string());
        }

        match command.cmd {
            LC_SEGMENT_64 => {
                let segment = read_struct::<SegmentCommand64>(macho, load_offset)?;
                if segment.fileoff == 0 && segment.filesize != 0 {
                    preferred_load_address.get_or_insert(segment.vmaddr);
                }
                vm_min = vm_min.min(segment.vmaddr);
                vm_max = vm_max.max(segment.vmaddr.saturating_add(segment.vmsize));

                let mut section_offset = load_offset + size_of::<SegmentCommand64>();
                for _ in 0..segment.nsects {
                    let section = read_struct::<Section64>(macho, section_offset)?;
                    if c_string_16(&section.sectname) == "__text" {
                        text_sections.push(section_ordinal);
                    }
                    section_ordinal = section_ordinal.saturating_add(1);
                    section_offset += size_of::<Section64>();
                }
            }
            LC_SYMTAB => {
                symtab = Some(read_struct::<SymtabCommand>(macho, load_offset)?);
            }
            _ => {}
        }

        load_offset += command.cmdsize as usize;
    }

    let preferred_load_address = preferred_load_address.unwrap_or(vm_min);
    if preferred_load_address == u64::MAX || vm_max <= preferred_load_address {
        return Err("could not determine image address range".to_string());
    }

    let symtab = symtab.ok_or_else(|| "image has no LC_SYMTAB".to_string())?;
    let string_table = get_range(macho, symtab.stroff as usize, symtab.strsize as usize)?;
    let mut symbols = Vec::new();
    let restrict_to_text = !text_sections.is_empty();

    for symbol_index in 0..symtab.nsyms as usize {
        let symbol_offset = symtab.symoff as usize + symbol_index * size_of::<Nlist64>();
        let symbol = read_struct::<Nlist64>(macho, symbol_offset)?;
        if symbol.n_strx == 0 || (symbol.n_type & N_STAB) != 0 {
            continue;
        }
        if (symbol.n_type & N_TYPE) != N_SECT {
            continue;
        }
        if restrict_to_text && !text_sections.contains(&symbol.n_sect) {
            continue;
        }

        let Some(relative_address) = symbol.n_value.checked_sub(preferred_load_address) else {
            continue;
        };
        let name = read_string_table_entry(string_table, symbol.n_strx as usize)?;
        if name.is_empty() {
            continue;
        }

        symbols.push(ParsedSymbol {
            runtime_address: image.load_address.saturating_add(relative_address),
            name: normalize_symbol_name(&name),
        });
    }

    symbols.sort_by_key(|symbol| symbol.runtime_address);
    symbols.dedup_by(|a, b| {
        a.runtime_address == b.runtime_address && a.name.cmp(&b.name) != Ordering::Greater
    });

    Ok(ImageSymbols {
        image_index,
        base_address: image.load_address,
        end_address: image
            .load_address
            .saturating_add(vm_max.saturating_sub(preferred_load_address)),
        display_name: image_display_name(&image.path),
        symbols,
    })
}

fn select_macho_slice(file: &[u8]) -> Result<&[u8], String> {
    if file.len() < 4 {
        return Err("file is too small".to_string());
    }

    let le_magic = u32::from_le_bytes(file[0..4].try_into().unwrap());
    if le_magic == MH_MAGIC_64 {
        return Ok(file);
    }

    let be_magic = u32::from_be_bytes(file[0..4].try_into().unwrap());
    match be_magic {
        FAT_MAGIC => {
            let arch_count = read_be_u32(file, 4)? as usize;
            let mut offset = 8usize;
            for _ in 0..arch_count {
                let cputype = read_be_i32(file, offset)?;
                let slice_offset = read_be_u32(file, offset + 8)? as usize;
                let slice_size = read_be_u32(file, offset + 12)? as usize;
                if cputype == target_cpu_type() {
                    return get_range(file, slice_offset, slice_size);
                }
                offset += 20;
            }
            Err("fat binary did not contain a matching architecture slice".to_string())
        }
        FAT_MAGIC_64 => {
            let arch_count = read_be_u32(file, 4)? as usize;
            let mut offset = 8usize;
            for _ in 0..arch_count {
                let cputype = read_be_i32(file, offset)?;
                let slice_offset = read_be_u64(file, offset + 8)? as usize;
                let slice_size = read_be_u64(file, offset + 16)? as usize;
                if cputype == target_cpu_type() {
                    return get_range(file, slice_offset, slice_size);
                }
                offset += 32;
            }
            Err("fat64 binary did not contain a matching architecture slice".to_string())
        }
        _ => Err("file is not a supported Mach-O image".to_string()),
    }
}

fn read_struct<T: Copy>(bytes: &[u8], offset: usize) -> Result<T, String> {
    let size = size_of::<T>();
    let range = get_range(bytes, offset, size)?;
    let mut value = MaybeUninit::<T>::uninit();
    unsafe {
        std::ptr::copy_nonoverlapping(range.as_ptr(), value.as_mut_ptr().cast::<u8>(), size);
        Ok(value.assume_init())
    }
}

fn get_range(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], String> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| "slice range overflow".to_string())?;
    bytes
        .get(offset..end)
        .ok_or_else(|| "slice range fell outside the file".to_string())
}

fn read_string_table_entry(string_table: &[u8], offset: usize) -> Result<String, String> {
    let bytes = string_table
        .get(offset..)
        .ok_or_else(|| "string table offset out of bounds".to_string())?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| "unterminated string table entry".to_string())?;
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

fn c_string_16(bytes: &[u8; 16]) -> &str {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).unwrap_or("")
}

fn image_display_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn normalize_symbol_name(name: &str) -> String {
    let stripped = name.strip_prefix('_').unwrap_or(name);
    demangle_legacy_rust_symbol(stripped).unwrap_or_else(|| stripped.to_string())
}

fn demangle_legacy_rust_symbol(name: &str) -> Option<String> {
    if !name.starts_with("ZN") || !name.ends_with('E') {
        return None;
    }

    let mut cursor = &name[2..name.len() - 1];
    let mut components = Vec::new();

    while !cursor.is_empty() {
        let digits_end = cursor
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(cursor.len());
        if digits_end == 0 {
            return None;
        }
        let len = cursor[..digits_end].parse::<usize>().ok()?;
        cursor = &cursor[digits_end..];
        if cursor.len() < len {
            return None;
        }

        let component = &cursor[..len];
        cursor = &cursor[len..];
        if component.starts_with('h')
            && component.len() >= 8
            && component[1..].chars().all(|ch| ch.is_ascii_hexdigit())
            && cursor.is_empty()
        {
            break;
        }

        components.push(decode_legacy_rust_component(component));
    }

    if components.is_empty() {
        None
    } else {
        Some(components.join("::"))
    }
}

fn decode_legacy_rust_component(component: &str) -> String {
    let mut decoded = String::new();
    let mut cursor = component.strip_prefix("_$").map(|rest| &component[1..]).unwrap_or(component);

    while let Some(start) = cursor.find('$') {
        decoded.push_str(&cursor[..start]);
        cursor = &cursor[start + 1..];

        let Some(end) = cursor.find('$') else {
            decoded.push('$');
            decoded.push_str(cursor);
            return decoded;
        };
        let escape = &cursor[..end];
        cursor = &cursor[end + 1..];

        match escape {
            "SP" => decoded.push('@'),
            "BP" => decoded.push('*'),
            "RF" => decoded.push('&'),
            "LT" => decoded.push('<'),
            "GT" => decoded.push('>'),
            "LP" => decoded.push('('),
            "RP" => decoded.push(')'),
            "C" => decoded.push(','),
            "u20" => decoded.push(' '),
            "u27" => decoded.push('\''),
            "u3d" => decoded.push('='),
            "u5b" => decoded.push('['),
            "u5d" => decoded.push(']'),
            "u7b" => decoded.push('{'),
            "u7d" => decoded.push('}'),
            "u7e" => decoded.push('~'),
            _ if escape.starts_with('u') => {
                if let Ok(value) = u32::from_str_radix(&escape[1..], 16) {
                    if let Some(ch) = char::from_u32(value) {
                        decoded.push(ch);
                    } else {
                        decoded.push('$');
                        decoded.push_str(escape);
                        decoded.push('$');
                    }
                } else {
                    decoded.push('$');
                    decoded.push_str(escape);
                    decoded.push('$');
                }
            }
            _ => {
                decoded.push('$');
                decoded.push_str(escape);
                decoded.push('$');
            }
        }
    }

    decoded.push_str(cursor);
    decoded
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let range = get_range(bytes, offset, 4)?;
    Ok(u32::from_be_bytes(range.try_into().unwrap()))
}

fn read_be_i32(bytes: &[u8], offset: usize) -> Result<i32, String> {
    let range = get_range(bytes, offset, 4)?;
    Ok(i32::from_be_bytes(range.try_into().unwrap()))
}

fn read_be_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let range = get_range(bytes, offset, 8)?;
    Ok(u64::from_be_bytes(range.try_into().unwrap()))
}

fn find_symbol<'a>(symbols: &'a [ParsedSymbol], address: u64) -> Option<&'a ParsedSymbol> {
    if symbols.is_empty() {
        return None;
    }
    let partition = symbols.partition_point(|symbol| symbol.runtime_address <= address);
    if partition == 0 {
        None
    } else {
        symbols.get(partition - 1)
    }
}

fn target_cpu_type() -> i32 {
    #[cfg(target_arch = "aarch64")]
    {
        CPU_TYPE_ARM64
    }
    #[cfg(target_arch = "x86_64")]
    {
        CPU_TYPE_X86_64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_symbol_name_demangles_legacy_rust_names() {
        assert_eq!(
            normalize_symbol_name("_ZN16makepad_profiler9busy_leaf17h71e73e8193f697a2E"),
            "makepad_profiler::busy_leaf"
        );
        assert_eq!(
            normalize_symbol_name("_ZN16makepad_profiler10run_record28_$u7b$$u7b$closure$u7d$$u7d$17h3fd7b32b1463c858E"),
            "makepad_profiler::run_record::{{closure}}"
        );
    }
}
