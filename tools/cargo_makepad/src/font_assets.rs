use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

const MANIFEST_SECTION: &str = "makepad.font-assets.v1";
const ELF_MANIFEST_SECTION: &str = ".makepad.font-assets.v1";
const MACHO_MANIFEST_SECTION: &str = "__mp_font_v1";
const WASM_HEADER: &[u8; 8] = b"\0asm\x01\0\0\0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontAssetManifest {
    set: String,
    assets: BTreeSet<String>,
}

impl FontAssetManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| format!("{MANIFEST_SECTION} is not valid UTF-8"))?;
        let mut format = None;
        let mut set = None;
        let mut assets = BTreeSet::new();

        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("invalid {MANIFEST_SECTION} line {line:?}"))?;
            match key {
                "format" => {
                    if format.replace(value).is_some() {
                        return Err(format!("duplicate format in {MANIFEST_SECTION}"));
                    }
                }
                "set" => {
                    if value.is_empty() || set.replace(value.to_string()).is_some() {
                        return Err(format!("invalid or duplicate set in {MANIFEST_SECTION}"));
                    }
                }
                "asset" => {
                    validate_logical_font_path(value)?;
                    if !assets.insert(value.to_string()) {
                        return Err(format!(
                            "duplicate font asset {value:?} in {MANIFEST_SECTION}"
                        ));
                    }
                }
                _ => return Err(format!("unknown {MANIFEST_SECTION} key {key:?}")),
            }
        }

        if format != Some(MANIFEST_SECTION) {
            return Err(format!(
                "invalid or missing format in {MANIFEST_SECTION}"
            ));
        }
        let set = set.ok_or_else(|| format!("missing set in {MANIFEST_SECTION}"))?;
        if assets.is_empty() {
            return Err(format!("no font assets in {MANIFEST_SECTION}"));
        }
        Ok(Self { set, assets })
    }

    pub fn from_wasm_file(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|error| {
            format!("cannot read wasm font manifest from {path:?}: {error}")
        })?;
        let payload = wasm_custom_section(&bytes, MANIFEST_SECTION)?;
        Self::parse(payload).map_err(|error| format!("{path:?}: {error}"))
    }

    pub fn from_native_file(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|error| {
            format!("cannot read native font manifest from {path:?}: {error}")
        })?;
        let payload = if bytes.starts_with(b"\x7fELF") {
            elf_section(&bytes, ELF_MANIFEST_SECTION)?
        } else if is_macho(&bytes) {
            macho_section(&bytes, MACHO_MANIFEST_SECTION)?
        } else {
            return Err(format!(
                "cannot read {MANIFEST_SECTION} from unsupported native artifact {path:?}"
            ));
        };
        Self::parse(payload).map_err(|error| format!("{path:?}: {error}"))
    }

    pub fn set(&self) -> &str {
        &self.set
    }

    pub fn assets(&self) -> impl Iterator<Item = &str> {
        self.assets.iter().map(String::as_str)
    }

    pub fn allows(&self, logical_path: &str) -> bool {
        self.assets.contains(logical_path)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct FontInventory {
    pub set: String,
    pub fonts: BTreeMap<String, u64>,
}

impl FontInventory {
    pub fn print(&self) {
        let total = self.fonts.values().sum::<u64>();
        let fonts = self
            .fonts
            .iter()
            .map(|(path, bytes)| format!("{path} ({bytes} bytes)"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "Packaged fonts ({}): {} [total {} bytes]",
            self.set, fonts, total
        );
    }
}

pub struct FontPackage<'a> {
    manifest: &'a FontAssetManifest,
    packaged: BTreeMap<String, u64>,
}

impl<'a> FontPackage<'a> {
    pub fn new(manifest: &'a FontAssetManifest) -> Self {
        Self {
            manifest,
            packaged: BTreeMap::new(),
        }
    }

    pub fn copy_tree_filtered<Skip, AfterCopy>(
        &mut self,
        source_dir: &Path,
        dest_dir: &Path,
        logical_prefix: &str,
        skip: Skip,
        mut after_copy: AfterCopy,
    ) -> Result<(), String>
    where
        Skip: Fn(&Path) -> bool,
        AfterCopy: FnMut(&Path) -> Result<(), String>,
    {
        if !source_dir.is_dir() {
            return Ok(());
        }
        fs::create_dir_all(dest_dir)
            .map_err(|error| format!("cannot create resource directory {dest_dir:?}: {error}"))?;
        self.copy_tree_recursive(
            source_dir,
            source_dir,
            dest_dir,
            logical_prefix,
            &skip,
            &mut after_copy,
        )
    }

    fn copy_tree_recursive<Skip, AfterCopy>(
        &mut self,
        root: &Path,
        source_dir: &Path,
        dest_root: &Path,
        logical_prefix: &str,
        skip: &Skip,
        after_copy: &mut AfterCopy,
    ) -> Result<(), String>
    where
        Skip: Fn(&Path) -> bool,
        AfterCopy: FnMut(&Path) -> Result<(), String>,
    {
        let mut entries = fs::read_dir(source_dir)
            .map_err(|error| format!("cannot read resource directory {source_dir:?}: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot read resource entry in {source_dir:?}: {error}"))?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let source_path = entry.path();
            let relative = source_path.strip_prefix(root).map_err(|error| {
                format!("cannot make {source_path:?} relative to {root:?}: {error}")
            })?;
            if skip(relative) {
                continue;
            }
            let dest_path = dest_root.join(relative);
            if source_path.is_dir() {
                fs::create_dir_all(&dest_path).map_err(|error| {
                    format!("cannot create resource directory {dest_path:?}: {error}")
                })?;
                self.copy_tree_recursive(
                    root,
                    &source_path,
                    dest_root,
                    logical_prefix,
                    skip,
                    after_copy,
                )?;
                continue;
            }
            if !source_path.is_file() {
                continue;
            }

            let relative = slash_path(relative)?;
            let logical_path = format!("{logical_prefix}/{relative}");
            if is_font_path(&source_path) {
                if !self.manifest.allows(&logical_path) {
                    remove_file_if_present(&dest_path)?;
                    remove_file_if_present(&brotli_path(&dest_path))?;
                    continue;
                }
                let bytes = source_path
                    .metadata()
                    .map_err(|error| format!("cannot stat font {source_path:?}: {error}"))?
                    .len();
                if self.packaged.insert(logical_path.clone(), bytes).is_some() {
                    return Err(format!(
                        "font asset {logical_path:?} resolves to more than one source file"
                    ));
                }
            }

            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("cannot create resource directory {parent:?}: {error}")
                })?;
            }
            fs::copy(&source_path, &dest_path).map_err(|error| {
                format!("cannot copy resource {source_path:?} to {dest_path:?}: {error}")
            })?;
            after_copy(&dest_path)?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<FontInventory, String> {
        let missing = self
            .manifest
            .assets()
            .filter(|path| !self.packaged.contains_key(*path))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "font assets declared by {} ({}) are missing on disk: {}",
                MANIFEST_SECTION,
                self.manifest.set(),
                missing.join(", ")
            ));
        }
        Ok(FontInventory {
            set: self.manifest.set.clone(),
            fonts: self.packaged,
        })
    }
}

pub fn remove_existing_fonts(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(root)
        .map_err(|error| format!("cannot inspect package directory {root:?}: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot inspect package entry in {root:?}: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            remove_existing_fonts(&path)?;
        } else if is_font_path(&path) || is_brotli_font_path(&path) {
            fs::remove_file(&path)
                .map_err(|error| format!("cannot remove stale packaged font {path:?}: {error}"))?;
        }
    }
    Ok(())
}

pub fn no_skip(_: &Path) -> bool {
    false
}

pub fn is_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| matches!(extension.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
        .unwrap_or(false)
}

fn is_brotli_font_path(path: &Path) -> bool {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("br"))
        .unwrap_or(false)
    {
        return false;
    }
    path.file_stem().map(Path::new).map(is_font_path).unwrap_or(false)
}

fn brotli_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".br");
    path.with_file_name(name)
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove stale packaged font {path:?}: {error}")),
    }
}

fn slash_path(path: &Path) -> Result<String, String> {
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => out.push(value.to_string_lossy().into_owned()),
            _ => return Err(format!("resource path {path:?} is not relative and normalized")),
        }
    }
    Ok(out.join("/"))
}

fn validate_logical_font_path(path: &str) -> Result<(), String> {
    if path.contains('\\') || path.starts_with('/') || path.ends_with('/') {
        return Err(format!("font asset path {path:?} is not a normalized logical path"));
    }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() < 3
        || parts.iter().any(|part| part.is_empty() || *part == "." || *part == "..")
        || !matches!(parts[1], "resources" | "fonts")
        || !is_font_path(Path::new(path))
    {
        return Err(format!("invalid logical font asset path {path:?}"));
    }
    Ok(())
}

fn read_leb_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| "truncated unsigned LEB128".to_string())?;
        *cursor += 1;
        value |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("overflowing unsigned LEB128".to_string())
}

fn wasm_custom_section<'a>(bytes: &'a [u8], wanted: &str) -> Result<&'a [u8], String> {
    if bytes.get(..WASM_HEADER.len()) != Some(WASM_HEADER.as_slice()) {
        return Err("invalid wasm header while reading font manifest".to_string());
    }
    let mut cursor = WASM_HEADER.len();
    let mut found = None;
    while cursor < bytes.len() {
        let id = bytes[cursor];
        cursor += 1;
        let payload_len = read_leb_u32(bytes, &mut cursor)? as usize;
        let end = cursor
            .checked_add(payload_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "truncated wasm section while reading font manifest".to_string())?;
        if id == 0 {
            let mut section_cursor = cursor;
            let name_len = read_leb_u32(bytes, &mut section_cursor)? as usize;
            let name_end = section_cursor
                .checked_add(name_len)
                .filter(|name_end| *name_end <= end)
                .ok_or_else(|| "truncated wasm custom section name".to_string())?;
            let name = std::str::from_utf8(&bytes[section_cursor..name_end])
                .map_err(|_| "non-UTF-8 wasm custom section name".to_string())?;
            if name == wanted {
                if found.replace(&bytes[name_end..end]).is_some() {
                    return Err(format!("duplicate wasm custom section {wanted:?}"));
                }
            }
        }
        cursor = end;
    }
    found.ok_or_else(|| format!("wasm is missing required custom section {wanted:?}"))
}

fn byte_order(bytes: &[u8], offset: usize, width: usize, little: bool) -> Result<u64, String> {
    let slice = bytes
        .get(offset..offset + width)
        .ok_or_else(|| "truncated native object while reading font manifest".to_string())?;
    let mut value = 0u64;
    if little {
        for (index, byte) in slice.iter().enumerate() {
            value |= (*byte as u64) << (index * 8);
        }
    } else {
        for byte in slice {
            value = (value << 8) | *byte as u64;
        }
    }
    Ok(value)
}

fn object_slice(bytes: &[u8], offset: u64, size: u64) -> Result<&[u8], String> {
    let start = usize::try_from(offset).map_err(|_| "native section offset overflow".to_string())?;
    let len = usize::try_from(size).map_err(|_| "native section size overflow".to_string())?;
    bytes
        .get(start..start.saturating_add(len))
        .ok_or_else(|| "truncated native font manifest section".to_string())
}

fn c_name(bytes: &[u8]) -> Result<&str, String> {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end])
        .map_err(|_| "non-UTF-8 native section name".to_string())
}

fn elf_section<'a>(bytes: &'a [u8], wanted: &str) -> Result<&'a [u8], String> {
    let class = *bytes.get(4).ok_or_else(|| "truncated ELF header".to_string())?;
    let little = match bytes.get(5) {
        Some(1) => true,
        Some(2) => false,
        _ => return Err("unsupported ELF byte order".to_string()),
    };
    let (shoff_at, shentsize_at, shnum_at, shstrndx_at, sh_name_at, sh_offset_at, sh_size_at, word) =
        match class {
            1 => (0x20, 0x2e, 0x30, 0x32, 0, 0x10, 0x14, 4),
            2 => (0x28, 0x3a, 0x3c, 0x3e, 0, 0x18, 0x20, 8),
            _ => return Err("unsupported ELF class".to_string()),
        };
    let shoff = byte_order(bytes, shoff_at, word, little)? as usize;
    let shentsize = byte_order(bytes, shentsize_at, 2, little)? as usize;
    let shnum = byte_order(bytes, shnum_at, 2, little)? as usize;
    let shstrndx = byte_order(bytes, shstrndx_at, 2, little)? as usize;
    if shnum == 0 || shstrndx >= shnum || shentsize == 0 {
        return Err("unsupported extended or invalid ELF section table".to_string());
    }
    let section_header = |index: usize| -> Result<&[u8], String> {
        let start = shoff
            .checked_add(index.saturating_mul(shentsize))
            .ok_or_else(|| "ELF section table overflow".to_string())?;
        bytes
            .get(start..start + shentsize)
            .ok_or_else(|| "truncated ELF section table".to_string())
    };
    let strings_header = section_header(shstrndx)?;
    let strings = object_slice(
        bytes,
        byte_order(strings_header, sh_offset_at, word, little)?,
        byte_order(strings_header, sh_size_at, word, little)?,
    )?;
    let mut found = None;
    for index in 0..shnum {
        let header = section_header(index)?;
        let name_offset = byte_order(header, sh_name_at, 4, little)? as usize;
        let name = c_name(strings.get(name_offset..).ok_or_else(|| {
            "invalid ELF section-name offset while reading font manifest".to_string()
        })?)?;
        if name == wanted {
            let payload = object_slice(
                bytes,
                byte_order(header, sh_offset_at, word, little)?,
                byte_order(header, sh_size_at, word, little)?,
            )?;
            if found.replace(payload).is_some() {
                return Err(format!("duplicate ELF section {wanted:?}"));
            }
        }
    }
    found.ok_or_else(|| format!("native artifact is missing required section {wanted:?}"))
}

fn is_macho(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some(b"\xcf\xfa\xed\xfe" | b"\xfe\xed\xfa\xcf" | b"\xce\xfa\xed\xfe" | b"\xfe\xed\xfa\xce"))
}

fn macho_section<'a>(bytes: &'a [u8], wanted: &str) -> Result<&'a [u8], String> {
    let magic = bytes.get(..4).ok_or_else(|| "truncated Mach-O header".to_string())?;
    let (is_64, little) = match magic {
        b"\xcf\xfa\xed\xfe" => (true, true),
        b"\xfe\xed\xfa\xcf" => (true, false),
        b"\xce\xfa\xed\xfe" => (false, true),
        b"\xfe\xed\xfa\xce" => (false, false),
        _ => return Err("unsupported Mach-O header".to_string()),
    };
    let header_size = if is_64 { 32 } else { 28 };
    let ncmds = byte_order(bytes, 16, 4, little)? as usize;
    let segment_command = if is_64 { 0x19 } else { 0x1 };
    let segment_size = if is_64 { 72 } else { 56 };
    let section_size = if is_64 { 80 } else { 68 };
    let mut cursor = header_size;
    let mut found = None;
    for _ in 0..ncmds {
        let command = byte_order(bytes, cursor, 4, little)? as u32;
        let command_size = byte_order(bytes, cursor + 4, 4, little)? as usize;
        let command_end = cursor
            .checked_add(command_size)
            .filter(|end| *end <= bytes.len() && command_size >= 8)
            .ok_or_else(|| "truncated Mach-O load command".to_string())?;
        if command == segment_command {
            let section_count_at = if is_64 { cursor + 64 } else { cursor + 48 };
            let section_count = byte_order(bytes, section_count_at, 4, little)? as usize;
            for index in 0..section_count {
                let section = cursor + segment_size + index * section_size;
                let section_end = section + section_size;
                if section_end > command_end {
                    return Err("truncated Mach-O section table".to_string());
                }
                let name = c_name(&bytes[section..section + 16])?;
                if name == wanted {
                    let (size_at, offset_at, size_width) = if is_64 {
                        (section + 40, section + 48, 8)
                    } else {
                        (section + 36, section + 40, 4)
                    };
                    let payload = object_slice(
                        bytes,
                        byte_order(bytes, offset_at, 4, little)?,
                        byte_order(bytes, size_at, size_width, little)?,
                    )?;
                    if found.replace(payload).is_some() {
                        return Err(format!("duplicate Mach-O section {wanted:?}"));
                    }
                }
            }
        }
        cursor = command_end;
    }
    found.ok_or_else(|| format!("native artifact is missing required section {wanted:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const LATIN: &[u8] = include_bytes!("../../../platform/tests/fixtures/font-assets-latin-v1.txt");
    const INTERNATIONAL: &[u8] =
        include_bytes!("../../../platform/tests/fixtures/font-assets-international-v1.txt");

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cargo-makepad-font-assets-{}-{name}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn push_leb(mut value: usize, out: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn fixture_wasm(manifest: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        push_leb(MANIFEST_SECTION.len(), &mut payload);
        payload.extend_from_slice(MANIFEST_SECTION.as_bytes());
        payload.extend_from_slice(manifest);
        let mut wasm = WASM_HEADER.to_vec();
        wasm.push(0);
        push_leb(payload.len(), &mut wasm);
        wasm.extend_from_slice(&payload);
        wasm
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn fixture_elf64(manifest: &[u8]) -> Vec<u8> {
        const HEADER: usize = 64;
        const SECTION: usize = 64;
        let strings = b"\0.shstrtab\0.makepad.font-assets.v1\0";
        let strings_offset = HEADER + SECTION * 3;
        let manifest_offset = strings_offset + strings.len();
        let mut elf = vec![0; manifest_offset + manifest.len()];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 1;
        write_u64(&mut elf, 0x28, HEADER as u64);
        write_u16(&mut elf, 0x3a, SECTION as u16);
        write_u16(&mut elf, 0x3c, 3);
        write_u16(&mut elf, 0x3e, 1);

        let strings_header = HEADER + SECTION;
        write_u32(&mut elf, strings_header, 1);
        write_u64(&mut elf, strings_header + 0x18, strings_offset as u64);
        write_u64(&mut elf, strings_header + 0x20, strings.len() as u64);
        let manifest_header = HEADER + SECTION * 2;
        write_u32(&mut elf, manifest_header, 11);
        write_u64(&mut elf, manifest_header + 0x18, manifest_offset as u64);
        write_u64(&mut elf, manifest_header + 0x20, manifest.len() as u64);
        elf[strings_offset..manifest_offset].copy_from_slice(strings);
        elf[manifest_offset..].copy_from_slice(manifest);
        elf
    }

    fn fixture_macho64(manifest: &[u8]) -> Vec<u8> {
        const HEADER: usize = 32;
        const SEGMENT: usize = 72;
        const SECTION: usize = 80;
        let command_size = SEGMENT + SECTION;
        let manifest_offset = HEADER + command_size;
        let mut macho = vec![0; manifest_offset + manifest.len()];
        macho[..4].copy_from_slice(b"\xcf\xfa\xed\xfe");
        write_u32(&mut macho, 16, 1);
        write_u32(&mut macho, 20, command_size as u32);
        write_u32(&mut macho, HEADER, 0x19);
        write_u32(&mut macho, HEADER + 4, command_size as u32);
        macho[HEADER + 8..HEADER + 14].copy_from_slice(b"__DATA");
        write_u32(&mut macho, HEADER + 64, 1);
        let section = HEADER + SEGMENT;
        macho[section..section + MACHO_MANIFEST_SECTION.len()]
            .copy_from_slice(MACHO_MANIFEST_SECTION.as_bytes());
        macho[section + 16..section + 22].copy_from_slice(b"__DATA");
        write_u64(&mut macho, section + 40, manifest.len() as u64);
        write_u32(&mut macho, section + 48, manifest_offset as u32);
        macho[manifest_offset..].copy_from_slice(manifest);
        macho
    }

    #[test]
    fn extracts_manifest_from_fixture_wasm_custom_section() {
        let wasm = fixture_wasm(LATIN);
        let manifest = FontAssetManifest::parse(
            wasm_custom_section(&wasm, MANIFEST_SECTION).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.set(), "Latin");
        assert!(manifest.allows("makepad_widgets/resources/IBMPlexSans-Text.ttf"));
        assert!(!manifest.allows("makepad_widgets/resources/LXGWWenKaiRegular.ttf"));
    }

    #[test]
    fn extracts_native_manifest_from_elf_and_macho_sections() {
        let elf = fixture_elf64(INTERNATIONAL);
        let macho = fixture_macho64(INTERNATIONAL);
        for payload in [
            elf_section(&elf, ELF_MANIFEST_SECTION).unwrap(),
            macho_section(&macho, MACHO_MANIFEST_SECTION).unwrap(),
        ] {
            let manifest = FontAssetManifest::parse(payload).unwrap();
            assert_eq!(manifest.set(), "International");
            assert!(manifest.allows("makepad_widgets/resources/NotoColorEmoji.ttf"));
        }
    }

    #[test]
    fn latin_and_international_allowlists_are_exact() {
        let latin = FontAssetManifest::parse(LATIN).unwrap();
        let international = FontAssetManifest::parse(INTERNATIONAL).unwrap();
        let table = [
            ("makepad_widgets/resources/IBMPlexSans-Text.ttf", true, true),
            ("makepad_widgets/resources/LXGWWenKaiRegular.ttf", false, true),
            ("makepad_widgets/resources/NotoColorEmoji.ttf", false, true),
            ("other/resources/IBMPlexSans-Text.ttf", false, false),
            ("makepad_widgets/resources/readme.txt", false, false),
        ];
        for (path, latin_expected, international_expected) in table {
            assert_eq!(latin.allows(path), latin_expected, "Latin {path}");
            assert_eq!(
                international.allows(path),
                international_expected,
                "International {path}"
            );
        }
    }

    #[test]
    fn manifest_rejects_duplicate_or_non_normalized_assets() {
        let duplicate = b"format=makepad.font-assets.v1\nset=Latin\nasset=app/resources/font.ttf\nasset=app/resources/font.ttf\n";
        assert!(FontAssetManifest::parse(duplicate)
            .unwrap_err()
            .contains("duplicate font asset"));
        let traversal =
            b"format=makepad.font-assets.v1\nset=Latin\nasset=app/resources/../font.ttf\n";
        assert!(FontAssetManifest::parse(traversal)
            .unwrap_err()
            .contains("invalid logical font asset path"));
    }

    #[test]
    fn missing_declared_font_is_a_packaging_error() {
        let manifest = FontAssetManifest::parse(
            b"format=makepad.font-assets.v1\nset=Latin\nasset=app/resources/missing.ttf\n",
        )
        .unwrap();
        let error = FontPackage::new(&manifest).finish().unwrap_err();
        assert!(error.contains("app/resources/missing.ttf"));
        assert!(error.contains("missing on disk"));
    }

    #[test]
    fn packager_filters_fonts_by_exact_logical_path_and_removes_stale_files() {
        let manifest = FontAssetManifest::parse(
            b"format=makepad.font-assets.v1\nset=Latin\nasset=app/resources/shared.ttf\n",
        )
        .unwrap();
        let root = temp_dir("exact-allowlist");
        let app_source = root.join("app-source");
        let dep_source = root.join("dep-source");
        let dest = root.join("package");
        fs::create_dir_all(&app_source).unwrap();
        fs::create_dir_all(&dep_source).unwrap();
        fs::create_dir_all(dest.join("app/resources")).unwrap();
        fs::write(app_source.join("shared.ttf"), b"selected").unwrap();
        fs::write(app_source.join("data.bin"), b"data").unwrap();
        fs::write(dep_source.join("shared.ttf"), b"same basename, other crate").unwrap();
        fs::write(dest.join("app/resources/stale.otf"), b"stale").unwrap();
        fs::write(dest.join("app/resources/stale.otf.br"), b"stale br").unwrap();

        remove_existing_fonts(&dest).unwrap();
        let mut package = FontPackage::new(&manifest);
        package
            .copy_tree_filtered(
                &app_source,
                &dest.join("app/resources"),
                "app/resources",
                no_skip,
                |_| Ok(()),
            )
            .unwrap();
        package
            .copy_tree_filtered(
                &dep_source,
                &dest.join("dependency/resources"),
                "dependency/resources",
                no_skip,
                |_| Ok(()),
            )
            .unwrap();
        let inventory = package.finish().unwrap();

        assert_eq!(inventory.fonts.len(), 1);
        assert!(dest.join("app/resources/shared.ttf").is_file());
        assert!(dest.join("app/resources/data.bin").is_file());
        assert!(!dest.join("dependency/resources/shared.ttf").exists());
        assert!(!dest.join("app/resources/stale.otf").exists());
        assert!(!dest.join("app/resources/stale.otf.br").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plain_and_brotli_packages_have_the_same_font_inventory() {
        let manifest = FontAssetManifest::parse(
            b"format=makepad.font-assets.v1\nset=Latin\nasset=app/resources/latin.ttf\n",
        )
        .unwrap();
        let root = temp_dir("brotli-equivalence");
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("latin.ttf"), b"latin").unwrap();
        fs::write(source.join("international.ttf"), b"international").unwrap();

        let mut inventories = Vec::new();
        for encoded in [false, true] {
            let dest = root.join(if encoded { "production" } else { "plain" });
            let mut package = FontPackage::new(&manifest);
            package
                .copy_tree_filtered(&source, &dest, "app/resources", no_skip, |path| {
                    if encoded {
                        fs::write(brotli_path(path), b"encoded").map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })
                .unwrap();
            inventories.push(package.finish().unwrap().fonts);
            assert!(!dest.join("international.ttf").exists());
        }
        assert_eq!(inventories[0], inventories[1]);
        let _ = fs::remove_dir_all(root);
    }
}
