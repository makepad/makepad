//! Reads short import members of a COFF archive. An import library can point to
//! several DLLs, so the runtime needs the DLL and export recorded for each symbol.

/// How the DLL exports one import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Export {
    Name(String),
    Ordinal(u16),
}

/// A short import member records the linker's symbol, its DLL and the DLL's export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Import {
    pub symbol: String,
    pub dll: String,
    pub export: Export,
    pub data: bool,
}

fn slice(bytes: &[u8], start: usize, len: usize) -> Option<&[u8]> {
    bytes.get(start..start.checked_add(len)?)
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let word = slice(bytes, offset, 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(word))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let word = slice(bytes, offset, 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(word))
}

fn field(header: &[u8], offset: usize, width: usize) -> Option<&str> {
    let text = std::str::from_utf8(slice(header, offset, width)?).ok()?;
    Some(text.trim_end_matches(' '))
}

fn import_name<'a>(names: &mut impl Iterator<Item = &'a [u8]>) -> Option<&'a str> {
    let name = names.next()?.strip_suffix(b"\0")?;
    if name.is_empty() {
        return None;
    }
    std::str::from_utf8(name).ok()
}

fn read_import(member: &[u8]) -> Option<Import> {
    let (header, payload) = member.split_at_checked(20)?;
    let size = usize::try_from(u32_at(header, 12)?).ok()?;
    let flags = u16_at(header, 18)?;
    let kind = flags & 3;
    if kind == 3 {
        return None;
    }
    let mut names = slice(payload, 0, size)?.split_inclusive(|&b| b == 0);
    let symbol = import_name(&mut names)?;
    let dll = import_name(&mut names)?;
    let name_type = (flags >> 2) & 7;
    let export = match name_type {
        0 => Export::Ordinal(u16_at(header, 16)?),
        1 => Export::Name(symbol.to_owned()),
        2 | 3 => {
            let name = symbol.strip_prefix(['?', '@', '_']).unwrap_or(symbol);
            let name = if name_type == 3 {
                name.split('@').next()?
            } else {
                name
            };
            Export::Name(name.to_owned())
        }
        4 => Export::Name(import_name(&mut names)?.to_owned()),
        _ => return None,
    };
    Some(Import {
        symbol: symbol.to_owned(),
        dll: dll.to_owned(),
        export,
        data: kind == 1,
    })
}

/// Reads the imports in archive order and counts malformed short import members.
/// Other members are skipped. A damaged archive returns the offset and reason.
pub(crate) fn parse(bytes: &[u8]) -> Result<(Vec<Import>, usize), String> {
    let mut remaining = bytes
        .strip_prefix(b"!<arch>\n")
        .ok_or("offset 0: invalid archive signature")?;
    let mut imports = Vec::new();
    let mut malformed = 0;
    while !remaining.is_empty() {
        let offset = bytes.len() - remaining.len();
        let (header, rest) = remaining
            .split_at_checked(60)
            .ok_or_else(|| format!("offset {offset}: truncated member header"))?;
        if slice(header, 58, 2) != Some(b"`\n") {
            return Err(format!("offset {offset}: invalid member header terminator"));
        }
        let size = field(header, 48, 10)
            .map(|s| s.trim_start_matches(' '))
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| format!("offset {offset}: invalid decimal member size"))?;
        let (member, tail) = usize::try_from(size)
            .ok()
            .and_then(|size| rest.split_at_checked(size))
            .ok_or_else(|| format!("offset {offset}: member data extends past end of archive"))?;
        if member.starts_with(b"\0\0\xff\xff")
            && !matches!(field(header, 0, 16), Some("/" | "//"))
        {
            match read_import(member) {
                Some(import) => imports.push(import),
                None => malformed += 1,
            }
        }
        // An odd last member may end at EOF without its alignment byte.
        remaining = tail.get(member.len() % 2..).unwrap_or(&[]);
    }
    Ok((imports, malformed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, size: usize) -> Vec<u8> {
        format!("{name:<16}{:<12}{:<6}{:<6}{:<8}{size:<10}`\n", 0, 0, 0, 0).into_bytes()
    }

    fn member(name: &str, data: &[u8]) -> Vec<u8> {
        let mut bytes = header(name, data.len());
        bytes.extend_from_slice(data);
        if data.len() % 2 == 1 {
            bytes.push(b'\n');
        }
        bytes
    }

    fn short_import(
        symbol: &str,
        dll: &str,
        ordinal_or_hint: u16,
        kind: u16,
        name_type: u16,
        export_as: Option<&str>,
    ) -> Vec<u8> {
        let mut names = Vec::new();
        for name in [Some(symbol), Some(dll), export_as].into_iter().flatten() {
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        let mut bytes = vec![0, 0, 0xff, 0xff];
        bytes.extend(0u16.to_le_bytes()); // version
        bytes.extend(0x8664u16.to_le_bytes()); // machine
        bytes.extend(0u32.to_le_bytes()); // timestamp
        bytes.extend((names.len() as u32).to_le_bytes());
        bytes.extend(ordinal_or_hint.to_le_bytes());
        bytes.extend((kind | (name_type << 2)).to_le_bytes());
        bytes.extend(names);
        bytes
    }

    fn named(name: &str) -> Export {
        Export::Name(name.to_owned())
    }

    fn import(symbol: &str, dll: &str, export: Export, data: bool) -> Import {
        Import {
            symbol: symbol.to_owned(),
            dll: dll.to_owned(),
            export,
            data,
        }
    }

    #[test]
    fn reads_imports_in_archive_order() {
        let mut bytes = b"!<arch>\n".to_vec();
        bytes.extend(member("/", &short_import("hidden", "x.dll", 0, 0, 1, None)));
        bytes.extend(member("//", b"\0\0\xff\xff"));
        bytes.extend(member("object/", b"obj"));

        let cases = [
            (import("f", "a.dll", named("f"), false), 0, 1, None),
            (import("_f", "b.dll", named("f"), false), 0, 2, None),
            (import("?_f", "b.dll", named("_f"), false), 0, 2, None),
            (import("_g@8", "a.dll", named("g"), false), 0, 3, None),
            (import("@h@8@4", "b.dll", named("h"), false), 0, 3, None),
            (import("ord", "a.dll", Export::Ordinal(0x1234), false), 0, 0, None),
            (import("alias", "a.dll", named("actual"), false), 0, 4, Some("actual")),
            (import("var", "b.dll", named("var"), true), 1, 1, None),
            (import("c", "a.dll", named("c"), false), 2, 1, None),
        ];
        let mut expected = Vec::new();
        for (want, kind, name_type, export_as) in cases {
            let data = short_import(&want.symbol, &want.dll, 0x1234, kind, name_type, export_as);
            bytes.extend(member("/0", &data));
            expected.push(want);
        }
        assert_eq!(
            parse(&bytes),
            Ok((expected, 0)),
            "imports, DLLs, exports and data flags in member order"
        );
    }

    #[test]
    fn truncations_end_only_at_member_boundaries() {
        let mut bytes = b"!<arch>\n".to_vec();
        let empty = bytes.len();
        bytes.extend(member("object/", b"obj"));
        let first = bytes.len();
        bytes.extend(member("f/", &short_import("f", "xy", 0, 0, 1, None)));
        let full = bytes.len();
        // Both members are odd-sized, so EOF can fall on either side of each pad.
        let complete = [empty, first - 1, first, full - 1, full];
        for n in 0..=full {
            let result = parse(&bytes[..n]);
            assert_eq!(
                result.is_ok(),
                complete.contains(&n),
                "archive completeness at length {n}: {result:?}"
            );
        }
    }

    #[test]
    fn counts_malformed_imports_and_continues() {
        let before = short_import("first", "x.dll", 0, 0, 1, None);
        let after = short_import("last", "y.dll", 0, 0, 1, None);
        let mut oversized = before.clone();
        oversized[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut missing_nul = before.clone();
        *missing_nul.last_mut().expect("fixture ends with DLL terminator") = b'x';
        missing_nul.push(0); // Outside SizeOfData, still inside the member.
        let mut invalid_utf8 = before.clone();
        invalid_utf8[20] = 0xff;

        let malformed = [
            b"\0\0\xff\xff".to_vec(),
            oversized,
            missing_nul,
            invalid_utf8,
            short_import("", "x.dll", 0, 0, 1, None),
            short_import("f", "", 0, 0, 1, None),
            short_import("f", "x.dll", 0, 0, 4, None),
            short_import("f", "x.dll", 0, 0, 4, Some("")),
            short_import("f", "x.dll", 0, 3, 1, None),
            short_import("f", "x.dll", 0, 0, 5, None),
        ];
        let count = malformed.len();
        let mut bytes = b"!<arch>\n".to_vec();
        bytes.extend(member("first/", &before));
        for bad in malformed {
            bytes.extend(member("bad/", &bad));
        }
        bytes.extend(member("last/", &after));
        let expected = vec![
            import("first", "x.dll", named("first"), false),
            import("last", "y.dll", named("last"), false),
        ];
        assert_eq!(
            parse(&bytes),
            Ok((expected, count)),
            "count each malformed import and preserve the good imports on either side"
        );
    }

    #[test]
    fn reports_offset_and_reason_for_archive_damage() {
        let error = parse(b"!<wrong>").expect_err("reject a wrong archive signature");
        assert!(
            error.contains("offset 0") && error.contains("signature"),
            "locate and identify the bad signature: {error}"
        );

        let mut prefix = b"!<arch>\n".to_vec();
        prefix.extend(member("object/", b"obj"));
        let offset = prefix.len();
        let mut bad_size = header("bad/", 0);
        bad_size[48] = b'x';
        let mut bad_end = header("bad/", 0);
        bad_end[58] = b'!';
        for (bad, reason) in [
            (bad_size, "decimal member size"),
            (bad_end, "header terminator"),
            (header("bad/", 1), "member data"),
            (vec![b'x'], "truncated member header"),
        ] {
            let mut bytes = prefix.clone();
            bytes.extend(bad);
            let error = parse(&bytes).expect_err(reason);
            assert!(
                error.contains(&format!("offset {offset}")) && error.contains(reason),
                "locate and identify {reason}: {error}"
            );
        }
    }
}
