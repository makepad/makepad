//! Import libraries for building with Rust's GNU toolchain on Windows.
//!
//! On `x86_64-pc-windows-gnu`, rustc makes the import library of every
//! `raw-dylib` DLL (all of Makepad's Windows bindings are raw-dylib, through
//! `windows_link::link!`) by running `dlltool` on a .def file it writes.
//! GNU dlltool needs a GNU assembler, which Rust's distribution does not
//! ship, so the Builder answers as `dlltool` itself (a copy of
//! makepad-builder.exe by that name on the build's PATH): it writes the
//! library as an archive of short import objects, the format Microsoft's own
//! .lib files use, which Rust's LLD links (apps build with
//! `-C linker=rust-lld -C linker-flavor=ld.lld`). Nothing else runs and no
//! source changes.

use std::fs;
use std::path::{Path, PathBuf};

/// `dlltool -d <file.def> -D <name.dll> -l <out.lib> [-m ...] [-f ...]
/// [--no-leading-underscore] [--temp-prefix ...]`: the arguments rustc
/// passes. The .def is `LIBRARY`/`EXPORTS` with one name per line (an
/// `internal==external` pair imports `external`).
pub fn dlltool(args: &[String]) -> Result<(), String> {
    let (mut def, mut dll, mut out) = (None, None, None);
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).cloned();
        match args[i].as_str() {
            "-d" | "--input-def" => def = value,
            "-D" | "--dllname" => dll = value,
            "-l" | "--output-lib" => out = value,
            "-m" | "--machine" => {
                if value.as_deref() != Some("i386:x86-64") {
                    return Err(format!("only x86-64 import libraries, not {}", value.unwrap_or_default()));
                }
            }
            // Options with a value that change nothing here.
            "-f" | "--as-flags" | "--temp-prefix" | "-t" => {}
            // Flags without one.
            _ => {
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let def = def.ok_or("-d names no .def file")?;
    let out = out.ok_or("-l names no output library")?;
    let text = fs::read_to_string(&def).map_err(|e| format!("{def}: {e}"))?;
    let mut library = None;
    let mut names = Vec::new();
    let mut in_exports = false;
    for line in text.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or("");
        if first.eq_ignore_ascii_case("LIBRARY") {
            library = words.next().map(|w| w.trim_matches('"').to_owned());
            in_exports = false;
        } else if first.eq_ignore_ascii_case("EXPORTS") {
            in_exports = true;
        } else if in_exports {
            let name = first.split("==").last().unwrap_or(first).trim_matches('"');
            if !name.is_empty() {
                names.push(name.to_owned());
            }
        }
    }
    let dll = dll.or(library).ok_or("no DLL name")?;
    fs::write(&out, archive(&dll, &names)).map_err(|e| format!("{out}: {e}"))
}

/// An archive of short import objects (`IMPORT_OBJECT_HEADER` + name + DLL),
/// one per name, with a GNU symbol table naming `name` and `__imp_name`.
pub fn archive(dll: &str, names: &[String]) -> Vec<u8> {
    let members: Vec<Vec<u8>> = names
        .iter()
        .map(|name| {
            let mut data = name.as_bytes().to_vec();
            data.push(0);
            data.extend_from_slice(dll.as_bytes());
            data.push(0);
            let mut object = Vec::with_capacity(20 + data.len());
            object.extend_from_slice(&0u16.to_le_bytes()); // Sig1: IMAGE_FILE_MACHINE_UNKNOWN
            object.extend_from_slice(&0xffffu16.to_le_bytes()); // Sig2
            object.extend_from_slice(&0u16.to_le_bytes()); // Version
            object.extend_from_slice(&0x8664u16.to_le_bytes()); // Machine: AMD64
            object.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp
            object.extend_from_slice(&(data.len() as u32).to_le_bytes()); // SizeOfData
            object.extend_from_slice(&0u16.to_le_bytes()); // Ordinal/Hint
            object.extend_from_slice(&(1u16 << 2).to_le_bytes()); // IMPORT_OBJECT_CODE, IMPORT_OBJECT_NAME
            object.extend_from_slice(&data);
            object
        })
        .collect();
    let symbols: Vec<String> = names.iter().flat_map(|n| [n.clone(), format!("__imp_{n}")]).collect();
    let strings: Vec<u8> = symbols.iter().flat_map(|s| s.bytes().chain([0])).collect();
    let padded = |n: usize| n + (n & 1);
    let table_size = 4 + 4 * symbols.len() + strings.len();
    let mut offsets = Vec::with_capacity(symbols.len());
    let mut position = 8 + 60 + padded(table_size);
    for member in &members {
        offsets.push(position as u32);
        offsets.push(position as u32);
        position += 60 + padded(member.len());
    }
    let mut out = b"!<arch>\n".to_vec();
    let add = |out: &mut Vec<u8>, name: &str, body: &[u8]| {
        let header = format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", name, 0, 0, 0, 644, body.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(body);
        if body.len() & 1 == 1 {
            out.push(b'\n');
        }
    };
    let mut table = (symbols.len() as u32).to_be_bytes().to_vec();
    for o in offsets {
        table.extend_from_slice(&o.to_be_bytes());
    }
    table.extend_from_slice(&strings);
    add(&mut out, "/", &table);
    let stem: String = dll.trim_end_matches(".dll").chars().take(15).collect();
    for member in &members {
        add(&mut out, &format!("{stem}/"), member);
    }
    out
}


/// Import libraries (`lib<name>.a`) for the plain `#[link(name = "...")]`
/// DLLs the sources under `sources` name and Rust's MinGW runtime has none
/// for (dwmapi, d3d11, ...), from the DLLs in System32, written into `dir`
/// (the toolchain's self-contained library directory, which rustc searches).
pub fn prepare(dir: &Path, sources: &Path) -> Result<(), String> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32");
    // Reading every source file of a snapshot takes seconds (thousands of
    // files, each scanned by the virus checker), so its list of DLLs is
    // kept in the snapshot; an edit that names a new DLL is rare, and its
    // link error names the library to add.
    let record = sources.join(".builder-linked-dlls");
    let names = match fs::read_to_string(&record) {
        Ok(text) => text.lines().map(str::to_owned).filter(|n| !n.is_empty()).collect(),
        Err(_) => {
            let names = linked_names(sources);
            let _ = fs::write(&record, names.join("\n"));
            names
        }
    };
    for name in names {
        let lib = dir.join(format!("lib{name}.a"));
        if lib.is_file() {
            continue;
        }
        let Ok(image) = fs::read(system.join(format!("{name}.dll"))) else { continue };
        let names: Vec<String> = exports(&image)?.into_iter().filter(|n| n != "DllMain").collect();
        let next = dir.join(format!("lib{name}.a.new"));
        fs::write(&next, archive(&format!("{name}.dll"), &names)).map_err(|e| format!("Import library {name}: {e}"))?;
        fs::rename(&next, &lib).map_err(|e| format!("Import library {name}: {e}"))?;
    }
    Ok(())
}

/// The names in `link(name = "...")` attributes of `*.rs` files under
/// `sources`, lower-cased.
fn linked_names(sources: &Path) -> Vec<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut stack = vec![sources.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if path.is_dir() {
                if file != "target" && !file.starts_with('.') {
                    stack.push(path);
                }
            } else if file.ends_with(".rs") {
                let Ok(text) = fs::read_to_string(&path) else { continue };
                for piece in text.split("link(name = \"").skip(1) {
                    let name: String = piece.chars().take_while(|c| *c != '"').collect();
                    let name = name.to_ascii_lowercase();
                    if !name.is_empty() && name.len() < 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                        found.insert(name);
                    }
                }
            }
        }
    }
    found.into_iter().collect()
}

/// The exported names of a PE image (64-bit or 32-bit headers).
pub fn exports(image: &[u8]) -> Result<Vec<String>, String> {
    let u16_at = |o: usize| image.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or("truncated image");
    let u32_at = |o: usize| image.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or("truncated image");
    let pe = u32_at(0x3c)? as usize;
    if image.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("not a PE image".into());
    }
    let sections = u16_at(pe + 6)? as usize;
    let optional = pe + 24;
    let optional_size = u16_at(pe + 20)? as usize;
    let directories = optional + if u16_at(optional)? == 0x20b { 112 } else { 96 };
    let export_rva = u32_at(directories)?;
    if export_rva == 0 {
        return Ok(Vec::new());
    }
    let mut table = Vec::new();
    for i in 0..sections {
        let s = optional + optional_size + i * 40;
        let (virtual_size, address, raw_size, raw) = (u32_at(s + 8)?, u32_at(s + 12)?, u32_at(s + 16)?, u32_at(s + 20)?);
        table.push((address, virtual_size.max(raw_size), raw));
    }
    let offset = |rva: u32| {
        table
            .iter()
            .find(|(address, size, _)| rva >= *address && rva < address + size)
            .map(|(address, _, raw)| (rva - address + raw) as usize)
            .ok_or("address outside the image")
    };
    let directory = offset(export_rva)?;
    let count = u32_at(directory + 24)? as usize;
    let names = offset(u32_at(directory + 32)?)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = offset(u32_at(names + 4 * i)?)?;
        let end = image[start..].iter().position(|b| *b == 0).ok_or("unterminated export name")?;
        out.push(String::from_utf8_lossy(&image[start..start + end]).into_owned());
    }
    Ok(out)
}

