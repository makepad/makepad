use std::{fs, path::Path};

const WASM_HEADER: &[u8; 8] = b"\0asm\x01\0\0\0";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WasmSectionSizes {
    pub raw_bytes: usize,
    pub code_bytes: usize,
    pub data_bytes: usize,
    pub custom_bytes: usize,
    pub name_bytes: usize,
}

impl WasmSectionSizes {
    pub fn has_name_section(&self) -> bool {
        self.name_bytes != 0
    }
}

#[derive(Clone, Debug)]
struct Section<'a> {
    id: u8,
    encoded: &'a [u8],
    #[cfg_attr(not(test), allow(dead_code))]
    payload: &'a [u8],
    custom_name: Option<&'a str>,
}

fn read_var_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let mut value = 0u32;
    for shift in (0..=28).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| "truncated unsigned LEB128".to_string())?;
        *cursor += 1;
        if shift == 28 && byte & 0xf0 != 0 {
            return Err("overflowing unsigned LEB128".to_string());
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("overflowing unsigned LEB128".to_string())
}

fn parse_sections(bytes: &[u8]) -> Result<Vec<Section<'_>>, String> {
    if bytes.get(..WASM_HEADER.len()) != Some(WASM_HEADER.as_slice()) {
        return Err("invalid wasm header".to_string());
    }

    let mut sections = Vec::new();
    let mut cursor = WASM_HEADER.len();
    while cursor < bytes.len() {
        let start = cursor;
        let id = bytes[cursor];
        cursor += 1;
        let payload_len = read_var_u32(bytes, &mut cursor)? as usize;
        let payload_start = cursor;
        let end = payload_start
            .checked_add(payload_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "truncated wasm section".to_string())?;
        let payload = &bytes[payload_start..end];
        let custom_name = if id == 0 {
            let mut name_cursor = 0;
            let name_len = read_var_u32(payload, &mut name_cursor)? as usize;
            let name_end = name_cursor
                .checked_add(name_len)
                .filter(|name_end| *name_end <= payload.len())
                .ok_or_else(|| "truncated wasm custom section name".to_string())?;
            Some(
                std::str::from_utf8(&payload[name_cursor..name_end])
                    .map_err(|_| "non-UTF-8 wasm custom section name".to_string())?,
            )
        } else {
            None
        };
        sections.push(Section {
            id,
            encoded: &bytes[start..end],
            payload,
            custom_name,
        });
        cursor = end;
    }
    Ok(sections)
}

pub fn wasm_section_sizes(bytes: &[u8]) -> Result<WasmSectionSizes, String> {
    let sections = parse_sections(bytes)?;
    let mut sizes = WasmSectionSizes {
        raw_bytes: bytes.len(),
        ..WasmSectionSizes::default()
    };
    for section in sections {
        match section.id {
            0 => {
                sizes.custom_bytes += section.encoded.len();
                if section.custom_name == Some("name") {
                    sizes.name_bytes += section.encoded.len();
                }
            }
            10 => sizes.code_bytes += section.encoded.len(),
            11 => sizes.data_bytes += section.encoded.len(),
            _ => {}
        }
    }
    Ok(sizes)
}

fn artifact_size(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn brotli_path(path: &Path) -> Option<std::path::PathBuf> {
    Some(path.parent()?.join(format!("{}.br", path.file_name()?.to_string_lossy())))
}

fn display_size(size: Option<u64>) -> String {
    size.map(|size| size.to_string())
        .unwrap_or_else(|| "-".to_string())
}

pub fn print_package_size_report(
    app: &str,
    config_id: &str,
    wasm_path: &Path,
) -> Result<(), String> {
    let wasm = fs::read(wasm_path)
        .map_err(|err| format!("Cannot read wasm size-report input {:?}: {err}", wasm_path))?;
    let sizes = wasm_section_sizes(&wasm)
        .map_err(|err| format!("Cannot parse wasm size-report input {:?}: {err}", wasm_path))?;
    let wasm_brotli = brotli_path(wasm_path).and_then(|path| artifact_size(&path));
    let parent = wasm_path
        .parent()
        .ok_or_else(|| format!("Wasm size-report input has no parent: {:?}", wasm_path))?;
    let data_path = parent.join(format!("{app}.data.bin"));
    let secondary_path = parent.join(format!("{app}.secondary.wasm"));
    let data_bytes = artifact_size(&data_path);
    let secondary_bytes = artifact_size(&secondary_path);

    let compressed_parts = [
        wasm_brotli,
        brotli_path(&data_path).and_then(|path| artifact_size(&path)),
        brotli_path(&secondary_path).and_then(|path| artifact_size(&path)),
    ];
    let compressed_total = if compressed_parts.iter().any(Option::is_some) {
        Some(compressed_parts.into_iter().flatten().sum::<u64>())
    } else {
        None
    };
    let custom_name = format!(
        "{}/{}",
        sizes.custom_bytes,
        if sizes.has_name_section() { "yes" } else { "no" }
    );

    println!("Wasm verification size report (exact bytes):");
    println!("| App | Git/config ID | Raw WASM | Brotli WASM | Code | Data | Custom/name | Primary | .data.bin | Secondary | Compressed total |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    println!(
        "| {app} | {config_id} | {} | {} | {} | {} | {custom_name} | {} | {} | {} | {} |",
        sizes.raw_bytes,
        display_size(wasm_brotli),
        sizes.code_bytes,
        sizes.data_bytes,
        sizes.raw_bytes,
        display_size(data_bytes),
        display_size(secondary_bytes),
        display_size(compressed_total),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_wasm_strip::wasm_strip_custom_sections;

    fn section(id: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() < 128);
        let mut out = vec![id, payload.len() as u8];
        out.extend_from_slice(payload);
        out
    }

    fn fixture() -> Vec<u8> {
        let mut wasm = WASM_HEADER.to_vec();
        wasm.extend(section(1, &[0]));
        wasm.extend(section(10, &[1, 2, 3]));
        wasm.extend(section(0, &[4, b'n', b'a', b'm', b'e', 9, 8]));
        wasm.extend(section(11, &[4, 5, 6, 7]));
        wasm.extend(section(0, &[1, b'x', 1]));
        wasm
    }

    #[test]
    fn parses_fixture_section_sizes_and_name_presence() {
        let wasm = fixture();
        assert_eq!(
            wasm_section_sizes(&wasm).unwrap(),
            WasmSectionSizes {
                raw_bytes: wasm.len(),
                code_bytes: 5,
                data_bytes: 6,
                custom_bytes: 14,
                name_bytes: 9,
            }
        );
    }

    #[test]
    fn custom_strip_removes_name_and_preserves_code_and_data_payloads() {
        let before = fixture();
        let after = wasm_strip_custom_sections(&before).unwrap();
        let before_sections = parse_sections(&before).unwrap();
        let after_sections = parse_sections(&after).unwrap();

        assert!(wasm_section_sizes(&before).unwrap().has_name_section());
        assert!(!wasm_section_sizes(&after).unwrap().has_name_section());
        for id in [10, 11] {
            let before_payloads = before_sections
                .iter()
                .filter(|section| section.id == id)
                .map(|section| section.payload)
                .collect::<Vec<_>>();
            let after_payloads = after_sections
                .iter()
                .filter(|section| section.id == id)
                .map(|section| section.payload)
                .collect::<Vec<_>>();
            assert_eq!(before_payloads, after_payloads);
        }
    }

    #[test]
    fn rejects_truncated_sections() {
        let mut wasm = WASM_HEADER.to_vec();
        wasm.extend([10, 4, 1]);
        assert!(wasm_section_sizes(&wasm).is_err());
    }
}
